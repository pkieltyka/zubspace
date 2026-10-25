use crate::DOOR_MODE_LOCAL_INCREMENT;
use crate::arena_settings::{arena_door_delay_ticks, arena_door_mode};
use crate::vie::{randomized_door_mode_seed, vie_rng_advance};

pub(crate) const SERVER_DOOR_RNG_SEED: u32 = 2;
const INITIAL_DOOR_SEED: u8 = u8::MAX;
const DOOR_VALIDATION_GRACE_STEPS: i32 = 1;
/// A recovered browser normally crosses only a handful of phases. Bound the
/// authenticated reconstruction so an abandoned tab cannot turn one position
/// proof into an unbounded allocation/CPU request; beyond this window the
/// relocation proof fails closed and an ordinary lifecycle resync is needed.
pub(crate) const MAX_DOOR_PHASE_RECONSTRUCTION_STEPS: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClientDoorState {
    pub(crate) rng_seed: i32,
    initial_door_seed: u8,
    pub(crate) timestamp: u32,
}

/// One server-derived dynamic-door phase that a client can plausibly be
/// simulating around packet receipt. `started_tick` is the canonical door
/// update boundary, so retained input records can reject adjacent-seed
/// oscillation even though randomized door seed bytes are not ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClientDoorPhase {
    pub(crate) seed: u8,
    pub(crate) started_tick: u32,
}

impl ClientDoorState {
    pub(crate) fn for_challenge(timestamp: u32) -> Self {
        Self {
            rng_seed: SERVER_DOOR_RNG_SEED as i32,
            initial_door_seed: INITIAL_DOOR_SEED,
            timestamp,
        }
    }
}

impl Default for ClientDoorState {
    /// Phase 2 polish (door-aware projectile collision):
    /// `ServerState::default()` needs to seed its canonical
    /// `server_door_state` to the same `SERVER_DOOR_RNG_SEED` clients
    /// use, anchored at timestamp 0 so the seed cycle is purely
    /// `current_tick()`-driven. `for_challenge(0)` is the right
    /// default — it is **not** the same as `Self { ..zeroed() }`,
    /// which would zero the rng seed and corrupt the cycle.
    fn default() -> Self {
        Self::for_challenge(0)
    }
}

/// Cached door-seed advance state so per-packet door validation does not
/// recompute from `update_count = 0` on every position relay. Tracks the
/// smallest `update_count` evaluated in the most recent validation so that
/// subsequent calls only advance by the small monotonic delta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DoorSeedCache {
    update_count: usize,
    door_seed: u8,
    rng_seed: i32,
    door_mode: i16,
    state_rng_seed: i32,
    state_initial_door_seed: u8,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn client_door_validation_seeds_at_tick(
    state: Option<ClientDoorState>,
    cache: &mut Option<DoorSeedCache>,
    tick: u32,
) -> Vec<u8> {
    let Some(state) = state else {
        return Vec::new();
    };
    door_validation_seeds(
        cache,
        state,
        tick,
        arena_door_mode(),
        arena_door_delay_ticks(),
    )
}

/// Exact door seed used by focused movement-parity fixtures. Production
/// position admission and private replay use the exact phase resolved for each
/// authenticated input record; candidate phases must never be unioned for
/// endpoint, path, or collision authority.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn client_door_simulation_seed_at_tick(
    state: Option<ClientDoorState>,
    tick: u32,
) -> Option<u8> {
    let state = state?;
    let delay_ticks = arena_door_delay_ticks().max(1);
    let elapsed_ticks = continuum_tick_diff(tick, state.timestamp).max(0);
    let update_count = (elapsed_ticks / delay_ticks) as u64;
    let mut rng_seed = state.rng_seed;
    Some(
        advance_door_seed_by(
            state.initial_door_seed,
            &mut rng_seed,
            arena_door_mode(),
            update_count,
        )
        .0,
    )
}

/// Previous/current/next security-derived phases around `tick`. These are only
/// candidates for resolving an input record's compact seed byte. Input
/// admission caps a candidate's start at that record's plausible wall-clock
/// slot and orders retained phase ticks; position and collision admission then
/// use the one exact resolved phase rather than this candidate union.
pub(crate) fn client_door_phase_candidates_at_tick(
    state: Option<ClientDoorState>,
    tick: u32,
) -> Vec<ClientDoorPhase> {
    let Some(state) = state else {
        return Vec::new();
    };
    let delay_ticks = arena_door_delay_ticks().max(1);
    let elapsed_ticks = crate::continuum::continuum_tick_diff(tick, state.timestamp).max(0);
    let current_count = elapsed_ticks / delay_ticks;
    (-DOOR_VALIDATION_GRACE_STEPS..=DOOR_VALIDATION_GRACE_STEPS)
        .filter_map(|offset| {
            let count = current_count + offset;
            (count >= 0).then_some(count)
        })
        .map(|count| {
            let mut rng_seed = state.rng_seed;
            let seed = advance_door_seed_by(
                state.initial_door_seed,
                &mut rng_seed,
                arena_door_mode(),
                count as u64,
            )
            .0;
            ClientDoorPhase {
                seed,
                started_tick: crate::continuum::continuum_tick_add(
                    state.timestamp,
                    count.saturating_mul(delay_ticks) as u32,
                ),
            }
        })
        .collect()
}

/// Reconstruct every canonical phase from `from_started_tick` through
/// `through_started_tick`, inclusive. Both endpoints must be exact phase
/// boundaries derived from the authenticated challenge anchor. This is used
/// only for the exceptional door-close relocation proof; ordinary movement
/// continues to carry each record's exact resolved phase.
pub(crate) fn client_door_phase_sequence(
    state: Option<ClientDoorState>,
    from_started_tick: u32,
    through_started_tick: u32,
) -> Option<Vec<ClientDoorPhase>> {
    let state = state?;
    let delay_ticks = arena_door_delay_ticks().max(1);
    let from_elapsed = continuum_tick_diff(from_started_tick, state.timestamp);
    let through_elapsed = continuum_tick_diff(through_started_tick, state.timestamp);
    if from_elapsed < 0
        || through_elapsed < from_elapsed
        || from_elapsed % delay_ticks != 0
        || through_elapsed % delay_ticks != 0
    {
        return None;
    }
    let from_count = from_elapsed / delay_ticks;
    let transition_count = (through_elapsed - from_elapsed) / delay_ticks;
    let transition_count = usize::try_from(transition_count).ok()?;
    if transition_count > MAX_DOOR_PHASE_RECONSTRUCTION_STEPS {
        return None;
    }

    let mut rng_seed = state.rng_seed;
    let (mut seed, _) = advance_door_seed_by(
        state.initial_door_seed,
        &mut rng_seed,
        arena_door_mode(),
        from_count as u64,
    );
    let mut phases = Vec::with_capacity(transition_count + 1);
    phases.push(ClientDoorPhase {
        seed,
        started_tick: from_started_tick,
    });
    for offset in 1..=transition_count {
        seed = crate::door::next_door_seed(seed, &mut rng_seed, arena_door_mode());
        phases.push(ClientDoorPhase {
            seed,
            started_tick: crate::continuum::continuum_tick_add(
                from_started_tick,
                (offset as u32).saturating_mul(delay_ticks as u32),
            ),
        });
    }
    Some(phases)
}

#[cfg_attr(not(test), allow(dead_code))]
fn door_validation_seeds(
    cache: &mut Option<DoorSeedCache>,
    state: ClientDoorState,
    now_tick: u32,
    door_mode: i16,
    delay_ticks: i32,
) -> Vec<u8> {
    door_validation_seeds_with_work(cache, state, now_tick, door_mode, delay_ticks).0
}

#[cfg_attr(not(test), allow(dead_code))]
fn door_validation_seeds_with_work(
    cache: &mut Option<DoorSeedCache>,
    state: ClientDoorState,
    now_tick: u32,
    door_mode: i16,
    delay_ticks: i32,
) -> (Vec<u8>, usize) {
    let delay_ticks = delay_ticks.max(1);
    let elapsed_ticks = continuum_tick_diff(now_tick, state.timestamp).max(0);
    let update_count = elapsed_ticks / delay_ticks;

    // Candidate counts in increasing order so we can reuse the running seed
    // across the grace window without recomputing from zero.
    let candidates: Vec<usize> = (-DOOR_VALIDATION_GRACE_STEPS..=DOOR_VALIDATION_GRACE_STEPS)
        .filter_map(|offset| {
            let count = update_count + offset;
            (count >= 0).then_some(count as usize)
        })
        .collect();

    let smallest = candidates.first().copied().unwrap_or(0);

    let (mut current_count, mut door_seed, mut rng_seed) = match cache.as_ref() {
        Some(c)
            if c.door_mode == door_mode
                && c.state_rng_seed == state.rng_seed
                && c.state_initial_door_seed == state.initial_door_seed
                && c.update_count <= smallest =>
        {
            (c.update_count, c.door_seed, c.rng_seed)
        }
        _ => (0, state.initial_door_seed, state.rng_seed),
    };

    let mut seeds = Vec::with_capacity(candidates.len());
    let mut snapshot_at_smallest: Option<DoorSeedCache> = None;
    let mut work = 0;

    for target in candidates {
        if current_count < target {
            let (advanced_seed, advance_work) = advance_door_seed_by(
                door_seed,
                &mut rng_seed,
                door_mode,
                (target - current_count) as u64,
            );
            door_seed = advanced_seed;
            current_count = target;
            work += advance_work;
        }
        if snapshot_at_smallest.is_none() {
            snapshot_at_smallest = Some(DoorSeedCache {
                update_count: current_count,
                door_seed,
                rng_seed,
                door_mode,
                state_rng_seed: state.rng_seed,
                state_initial_door_seed: state.initial_door_seed,
            });
        }
        if !seeds.contains(&door_seed) {
            seeds.push(door_seed);
        }
    }

    if let Some(new_cache) = snapshot_at_smallest {
        *cache = Some(new_cache);
    }

    (seeds, work)
}

/// Advance a door stream in bounded work. Fixed and local-increment modes are
/// closed form; randomized modes jump the underlying affine RNG and generate
/// only the final door update. Small cached deltas still take the ordinary
/// incremental path inside `vie_rng_advance`.
fn advance_door_seed_by(
    door_seed: u8,
    rng_seed: &mut i32,
    door_mode: i16,
    updates: u64,
) -> (u8, usize) {
    if updates == 0 {
        return (door_seed, 0);
    }
    match door_mode {
        DOOR_MODE_LOCAL_INCREMENT => (door_seed.wrapping_add(updates as u8), 1),
        -2 => {
            let (value, work) = vie_rng_advance(rng_seed, updates);
            (value as u8, work)
        }
        -1 => {
            let skipped_rng_steps = updates.saturating_sub(1).saturating_mul(7);
            let (_, mut work) = vie_rng_advance(rng_seed, skipped_rng_steps);
            let seed = randomized_door_mode_seed(rng_seed);
            work += 7;
            (seed, work)
        }
        mode if mode >= 0 => (mode as u8, 1),
        _ => (door_seed, 1),
    }
}

/// Periodically fold completed door updates into a fresh short-range anchor.
/// The RNG and seed immediately before the current update are carried forward,
/// preserving the exact previous/current/next grace window while preventing a
/// challenge timestamp from living through the signed 31-bit half-range.
pub(crate) fn rebase_door_state_at_tick(
    state: &mut ClientDoorState,
    cache: &mut Option<DoorSeedCache>,
    now_tick: u32,
) {
    const DOOR_STATE_REBASE_TICKS: u32 = 24 * 60 * 60 * 100;
    rebase_door_state(
        state,
        cache,
        now_tick,
        arena_door_mode(),
        arena_door_delay_ticks(),
        DOOR_STATE_REBASE_TICKS,
    );
}

fn rebase_door_state(
    state: &mut ClientDoorState,
    cache: &mut Option<DoorSeedCache>,
    now_tick: u32,
    door_mode: i16,
    delay_ticks: i32,
    rebase_ticks: u32,
) -> usize {
    let elapsed_ticks = crate::continuum::continuum_tick_elapsed(now_tick, state.timestamp);
    if elapsed_ticks < rebase_ticks {
        return 0;
    }
    let delay_ticks = delay_ticks.max(1) as u32;
    let update_count = elapsed_ticks / delay_ticks;
    // Keep one completed update in the rebased elapsed interval. Anchoring at
    // the current seed would make the formerly valid N-1 candidate negative
    // and silently narrow [N-1, N, N+1] to [N, N+1] for one DoorDelay.
    let folded_update_count = update_count.saturating_sub(1);
    let (base_count, mut door_seed, mut rng_seed) = match cache.as_ref() {
        Some(c)
            if c.door_mode == door_mode
                && c.state_rng_seed == state.rng_seed
                && c.state_initial_door_seed == state.initial_door_seed
                && c.update_count <= folded_update_count as usize =>
        {
            (c.update_count as u32, c.door_seed, c.rng_seed)
        }
        _ => (0, state.initial_door_seed, state.rng_seed),
    };
    let (advanced_seed, work) = advance_door_seed_by(
        door_seed,
        &mut rng_seed,
        door_mode,
        u64::from(folded_update_count - base_count),
    );
    door_seed = advanced_seed;
    state.timestamp = crate::continuum::continuum_tick_add(
        state.timestamp,
        folded_update_count.saturating_mul(delay_ticks),
    );
    state.initial_door_seed = door_seed;
    state.rng_seed = rng_seed;
    *cache = None;
    work
}

// Canonical `continuum_tick_diff` lives in `crate::continuum`; re-import
// here so the function-local body can't drift from the client copy.
use crate::continuum::continuum_tick_diff;

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_advance(
        mut door_seed: u8,
        mut rng_seed: i32,
        door_mode: i16,
        updates: usize,
    ) -> (u8, i32) {
        for _ in 0..updates {
            door_seed = crate::door::next_door_seed(door_seed, &mut rng_seed, door_mode);
        }
        (door_seed, rng_seed)
    }

    #[test]
    fn door_validation_seeds_include_current_neighboring_steps() {
        let state = ClientDoorState {
            rng_seed: 1,
            initial_door_seed: INITIAL_DOOR_SEED,
            timestamp: 900,
        };
        let mut cache = None;

        assert_eq!(
            door_validation_seeds(&mut cache, state, 920, -2, 50),
            vec![0xff, 34]
        );
        assert_eq!(
            door_validation_seeds(&mut cache, state, 1_000, -2, 50),
            vec![34, 169, 108]
        );
    }

    #[test]
    fn door_validation_seed_handles_wrapped_continuum_ticks() {
        let state = ClientDoorState {
            rng_seed: 1,
            initial_door_seed: INITIAL_DOOR_SEED,
            timestamp: 0x7fff_fffe,
        };
        let mut cache = None;

        assert_eq!(
            door_validation_seeds(&mut cache, state, 1, i16::MIN, 2),
            vec![0xff, 0, 1]
        );
    }

    #[test]
    fn simulation_seed_selects_one_exact_current_candidate_including_wrap() {
        let state = ClientDoorState {
            rng_seed: SERVER_DOOR_RNG_SEED as i32,
            initial_door_seed: INITIAL_DOOR_SEED,
            timestamp: 0x7fff_ff00,
        };
        let delay = arena_door_delay_ticks().max(1) as u32;
        let mode = arena_door_mode();

        for updates in [0_u32, 1, 2, 17] {
            let tick = crate::continuum::continuum_tick_add(
                state.timestamp,
                updates.saturating_mul(delay),
            );
            let (expected, _) = reference_advance(
                state.initial_door_seed,
                state.rng_seed,
                mode,
                updates as usize,
            );
            let exact = client_door_simulation_seed_at_tick(Some(state), tick);
            assert_eq!(exact, Some(expected));

            let mut cache = None;
            let validation = client_door_validation_seeds_at_tick(Some(state), &mut cache, tick);
            assert!(validation.contains(&expected));
        }
        assert_eq!(client_door_simulation_seed_at_tick(None, 123), None);
    }

    #[test]
    fn authenticated_phase_sequence_reconstructs_multi_phase_skips_and_tick_wrap() {
        let delay = arena_door_delay_ticks().max(1) as u32;
        let mode = arena_door_mode();
        let state = ClientDoorState::for_challenge(0x7fff_ff00);
        for transitions in [1_u32, 2, 3, 17] {
            let from_count = 2_u32;
            let from_tick = crate::continuum::continuum_tick_add(
                state.timestamp,
                from_count.saturating_mul(delay),
            );
            let through_tick =
                crate::continuum::continuum_tick_add(from_tick, transitions.saturating_mul(delay));
            let phases = client_door_phase_sequence(Some(state), from_tick, through_tick)
                .expect("canonical wrapped phase sequence");
            assert_eq!(phases.len(), transitions as usize + 1);
            for (offset, phase) in phases.iter().enumerate() {
                let (expected, _) = reference_advance(
                    state.initial_door_seed,
                    state.rng_seed,
                    mode,
                    from_count as usize + offset,
                );
                assert_eq!(phase.seed, expected);
                assert_eq!(
                    phase.started_tick,
                    crate::continuum::continuum_tick_add(
                        from_tick,
                        (offset as u32).saturating_mul(delay),
                    )
                );
            }
        }
    }

    #[test]
    fn authenticated_phase_sequence_preserves_repeated_rng_bytes_and_rejects_non_boundaries() {
        let delay = arena_door_delay_ticks().max(1) as u32;
        let state = ClientDoorState::for_challenge(500);
        let phases = client_door_phase_sequence(
            Some(state),
            state.timestamp,
            crate::continuum::continuum_tick_add(state.timestamp, 512 * delay),
        )
        .expect("bounded canonical sequence");
        let repeated = phases.iter().enumerate().find_map(|(later, phase)| {
            phases[..later]
                .iter()
                .position(|prior| prior.seed == phase.seed)
                .map(|prior| (prior, later, phase.seed))
        });
        assert!(
            repeated.is_some(),
            "513 u8 phase values must contain a repeated byte"
        );
        assert_eq!(phases[0].started_tick, state.timestamp);
        assert!(
            client_door_phase_sequence(
                Some(state),
                crate::continuum::continuum_tick_add(state.timestamp, 1),
                crate::continuum::continuum_tick_add(state.timestamp, delay),
            )
            .is_none()
        );
    }

    #[test]
    fn door_validation_seeds_cache_avoids_recomputing_from_zero() {
        let state = ClientDoorState {
            rng_seed: 1,
            initial_door_seed: INITIAL_DOOR_SEED,
            timestamp: 0,
        };
        let mut cache = None;

        // Burn in a long history so the cache must hold a non-trivial
        // update_count; subsequent calls only advance by 1 update_count.
        let baseline = door_validation_seeds(&mut cache, state, 10_000, -2, 50);
        assert_eq!(baseline.len(), 3);
        let cached_after_first = cache.expect("cache populated");
        // Cache anchors on the smallest candidate of the first call.
        assert_eq!(cached_after_first.update_count, 199);

        let second = door_validation_seeds(&mut cache, state, 10_050, -2, 50);
        assert_eq!(second.len(), 3);
        let cached_after_second = cache.expect("cache populated");
        // Smallest candidate moved forward by one update_count.
        assert_eq!(cached_after_second.update_count, 200);

        // Equivalent fresh-cache run produces the same seed list, so caching
        // does not change observable behaviour.
        let mut fresh = None;
        assert_eq!(
            door_validation_seeds(&mut fresh, state, 10_050, -2, 50),
            second
        );
    }

    #[test]
    fn bounded_advance_matches_reference_for_all_door_modes() {
        for mode in [DOOR_MODE_LOCAL_INCREMENT, -2, -1, 0, 17, -3] {
            for updates in [0, 1, 2, 3, 31, 32, 33, 999, 10_000] {
                let expected = reference_advance(INITIAL_DOOR_SEED, 2, mode, updates);
                let mut rng_seed = 2;
                let (actual_seed, _) =
                    advance_door_seed_by(INITIAL_DOOR_SEED, &mut rng_seed, mode, updates as u64);
                assert_eq!(
                    (actual_seed, rng_seed),
                    expected,
                    "mode {mode}, updates {updates}"
                );
            }
        }
    }

    #[test]
    fn chaos_validation_work_is_bounded_for_huge_door_delay_one_counts() {
        let state = ClientDoorState::for_challenge(0);
        let mut moderate_cache = None;
        let (moderate, moderate_work) =
            door_validation_seeds_with_work(&mut moderate_cache, state, 10_000_000, -2, 1);
        let mut huge_cache = None;
        let (huge, huge_work) =
            door_validation_seeds_with_work(&mut huge_cache, state, 1_000_000_000, -2, 1);

        assert_eq!(moderate.len(), 3);
        assert_eq!(huge.len(), 3);
        assert_eq!(moderate_work, 67, "moderate work changed unexpectedly");
        assert_eq!(
            huge_work, moderate_work,
            "large-count work must be independent of update count"
        );
    }

    #[test]
    fn rebase_two_hundred_aged_chaos_clients_has_bounded_total_work() {
        const CLIENTS: usize = 200;
        const NOW_TICK: u32 = 1_000_000_000;
        let mut clients = vec![(ClientDoorState::for_challenge(0), None); CLIENTS];
        let total_work: usize = clients
            .iter_mut()
            .map(|(state, cache)| rebase_door_state(state, cache, NOW_TICK, -2, 1, 1_000))
            .sum();

        assert!(
            total_work == CLIENTS * 65,
            "aged-client maintenance did {total_work} units of RNG work"
        );
        assert!(clients.iter().all(|(state, _)| {
            crate::continuum::continuum_tick_elapsed(NOW_TICK, state.timestamp) <= 1
        }));
    }

    #[test]
    fn huge_rebase_preserves_exact_neighbor_window_across_tick_wrap() {
        const ELAPSED: u32 = 500_000_000;
        const ANCHOR: u32 = 0x7000_0000;
        let now = crate::continuum::continuum_tick_add(ANCHOR, ELAPSED);
        let original = ClientDoorState::for_challenge(ANCHOR);
        let mut original_cache = None;
        let before = door_validation_seeds(&mut original_cache, original, now, -2, 1);
        assert_eq!(before.len(), 3, "N-1/N/N+1 must all be represented");

        let mut rebased = original;
        let work = rebase_door_state(&mut rebased, &mut original_cache, now, -2, 1, 1_000);
        let after = door_validation_seeds(&mut original_cache, rebased, now, -2, 1);
        assert_eq!(after, before);
        assert!(
            work <= 3,
            "the cached N-1 anchor should make rebase incremental"
        );
    }

    fn assert_rebase_preserves_validation_window(anchor_tick: u32, elapsed_ticks: u32) {
        const REBASE_TICKS: u32 = 1_000;
        const DELAY_TICKS: i32 = 50;
        let now_tick = crate::continuum::continuum_tick_add(anchor_tick, elapsed_ticks);
        let original = ClientDoorState::for_challenge(anchor_tick);
        let mut before_cache = None;
        let before = door_validation_seeds(&mut before_cache, original, now_tick, -2, DELAY_TICKS);
        assert_eq!(
            before.len(),
            3,
            "test point must exercise the full grace set"
        );

        let mut rebased = original;
        let mut rebased_cache = before_cache;
        rebase_door_state(
            &mut rebased,
            &mut rebased_cache,
            now_tick,
            -2,
            DELAY_TICKS,
            REBASE_TICKS,
        );
        let after = door_validation_seeds(&mut rebased_cache, rebased, now_tick, -2, DELAY_TICKS);
        assert_eq!(
            after, before,
            "rebasing must preserve the exact validation seed set at elapsed {elapsed_ticks}"
        );

        let next_tick = crate::continuum::continuum_tick_add(now_tick, 1);
        let mut original_next_cache = None;
        let original_next = door_validation_seeds(
            &mut original_next_cache,
            original,
            next_tick,
            -2,
            DELAY_TICKS,
        );
        let rebased_next =
            door_validation_seeds(&mut rebased_cache, rebased, next_tick, -2, DELAY_TICKS);
        assert_eq!(
            rebased_next, original_next,
            "the seed window immediately after rebase must remain equivalent"
        );
    }

    #[test]
    fn door_rebase_preserves_validation_grace_window_before_and_after_threshold() {
        for elapsed_ticks in [999, 1_000, 1_001, 1_049] {
            assert_rebase_preserves_validation_window(100, elapsed_ticks);
        }
    }

    #[test]
    fn door_rebase_preserves_validation_grace_window_across_tick_wrap() {
        for elapsed_ticks in [1_000, 1_001, 1_049] {
            assert_rebase_preserves_validation_window(0x7fff_ff00, elapsed_ticks);
        }
    }

    #[test]
    fn door_anchor_rebases_across_half_range_and_multiple_cycles() {
        const REBASE_TICKS: u32 = 10_000_000;
        const DELAY_TICKS: i32 = 1_000;
        let modulus = 1_u64 << 31;
        let boundaries = [modulus / 2, modulus, modulus + modulus / 2, 2 * modulus];
        let mut state = ClientDoorState::for_challenge(0);
        let mut cache = None;
        let mut unwrapped = 0_u64;

        while unwrapped < 2 * modulus {
            let regular = unwrapped + u64::from(REBASE_TICKS);
            let next_boundary = boundaries
                .into_iter()
                .find(|boundary| *boundary > unwrapped)
                .unwrap_or(2 * modulus);
            unwrapped = regular.min(next_boundary).min(2 * modulus);
            let tick = (unwrapped % modulus) as u32;
            rebase_door_state(&mut state, &mut cache, tick, -2, DELAY_TICKS, REBASE_TICKS);
            assert!(
                crate::continuum::continuum_tick_elapsed(tick, state.timestamp) < REBASE_TICKS,
                "door anchor must remain in its bounded comparison window at {unwrapped}"
            );
        }
    }
}

use std::sync::{Arc, Mutex};

use crate::arena_settings::{
    arena_prize_factor, arena_prize_hide_count, arena_prize_lifetime_max_ticks,
    arena_prize_lifetime_min_ticks, arena_prize_minimum_virtual, arena_prize_negative_factor,
    arena_prize_spawn_delay_ticks, arena_prize_upgrade_virtual, arena_prize_weights,
};
use crate::authority_prizes::{SECURITY_PRIZE_SEED, generate_weighted_prize_id};
use crate::state::{ServerHiddenPrize, ServerPrizeClaim, ServerState};
use crate::vie::vie_rng_next;
use crate::zone::{MAP_TILES, TILE_SIZE_PX, ZoneMap, tile_at_world};
use tracing::debug;

pub(crate) const HIDDEN_PRIZE_SPAWN: u8 = 0x60;
pub(crate) const HIDDEN_PRIZE_SPAWN_PACKET_LEN: usize = 13;
pub(crate) const SERVER_MAX_ACTIVE_COLLECTIBLES: usize = 256;

pub(crate) fn hidden_prize_spawn_packet(
    collectible_id: u32,
    x: u16,
    y: u16,
    prize_id: i16,
    ttl_ticks: u16,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(HIDDEN_PRIZE_SPAWN_PACKET_LEN);
    packet.push(HIDDEN_PRIZE_SPAWN);
    packet.extend(collectible_id.to_le_bytes());
    packet.extend(x.to_le_bytes());
    packet.extend(y.to_le_bytes());
    packet.extend(prize_id.to_le_bytes());
    packet.extend(ttl_ticks.to_le_bytes());
    packet
}

#[cfg(test)]
pub(crate) fn current_hidden_prize_packets(
    state: &Arc<Mutex<ServerState>>,
    tick: u32,
) -> Vec<Vec<u8>> {
    let mut state = state.lock().expect("server state should not be poisoned");
    current_hidden_prize_packets_for_state_at_tick(&mut state, tick)
}

pub(crate) fn current_hidden_prize_packets_for_state_at_tick(
    state: &mut ServerState,
    tick: u32,
) -> Vec<Vec<u8>> {
    expire_hidden_prizes(state, tick);
    state
        .active_hidden_prizes
        .iter()
        .filter_map(|prize| {
            ttl_ticks_until(prize.expires_at_tick, tick).map(|ttl_ticks| {
                hidden_prize_spawn_packet(
                    prize.collectible_id,
                    prize.x,
                    prize.y,
                    prize.prize_id,
                    ttl_ticks,
                )
            })
        })
        .collect()
}

pub(crate) fn step_hidden_prizes(state: &Arc<Mutex<ServerState>>, tick: u32) -> Vec<Vec<u8>> {
    step_hidden_prizes_with_factor(state, tick, arena_prize_factor())
}

fn step_hidden_prizes_with_factor(
    state: &Arc<Mutex<ServerState>>,
    tick: u32,
    prize_factor: u16,
) -> Vec<Vec<u8>> {
    let mut state = state.lock().expect("server state should not be poisoned");
    expire_hidden_prizes(&mut state, tick);

    let joined_players = joined_prize_player_count(&state);
    if joined_players == 0 {
        state.next_hidden_prize_spawn_at_tick = None;
        return Vec::new();
    }

    let player_count = joined_players.saturating_add(state.bots.len());
    let max_hidden_prizes = max_hidden_prize_count(prize_factor, player_count);

    let delay_ticks = arena_prize_spawn_delay_ticks();
    let next_spawn = state
        .next_hidden_prize_spawn_at_tick
        .get_or_insert_with(|| crate::continuum::continuum_tick_add(tick, delay_ticks));
    if !crate::continuum::continuum_tick_reached(tick, *next_spawn) {
        return Vec::new();
    }
    state.next_hidden_prize_spawn_at_tick =
        Some(crate::continuum::continuum_tick_add(tick, delay_ticks));

    if state.hidden_prize_rng_seed == 0 {
        state.hidden_prize_rng_seed = std::env::var("ZUBSPACE_HIDDEN_PRIZE_RNG_SEED")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .filter(|seed| *seed != 0)
            .unwrap_or(SECURITY_PRIZE_SEED);
    }

    let map = state.zone_map();
    let (weights, weight_total) = hidden_prize_weights();
    let mut rng_seed = state.hidden_prize_rng_seed;
    let mut packets = Vec::new();
    let spawn_count = arena_prize_hide_count();

    for _ in 0..spawn_count {
        // The reference game consumes one x/y/lifetime record for every
        // PrizeHideCount slot. A nonzero weight table additionally consumes
        // weighted/negative selection before lifetime, even when the result
        // is blocked or the active-prize cap is already full. Keeping the
        // stream independent of registry admission makes later spawns
        // reproducible from the arena seed.
        let Some((mut prize, mut ttl_ticks)) = draw_hidden_prize_candidate(
            map,
            &weights,
            weight_total,
            player_count,
            tick,
            &mut rng_seed,
        ) else {
            continue;
        };
        if prize.prize_id.unsigned_abs() == 14
            && std::env::var("ZUBSPACE_E2E_PRIZES_PHASE1").as_deref() == Ok("1")
            && let Ok(value) = std::env::var("ZUBSPACE_SERVER_TEST_HIDDEN_PRIZE_TTL_TICKS")
            && let Ok(value) = value.parse::<u16>()
            && value > 0
        {
            ttl_ticks = value;
            prize.expires_at_tick = crate::continuum::continuum_tick_add(tick, u32::from(value));
        }
        if state.active_hidden_prizes.len() >= max_hidden_prizes
            || tile_at_world(map, prize.x, prize.y) != 0
        {
            continue;
        }
        let Some(collectible_id) = state.allocate_collectible_id() else {
            // Registry identity pressure suppresses admission only. Later
            // PrizeHideCount slots still consume their complete RNG records.
            continue;
        };
        prize.collectible_id = collectible_id;
        packets.push(hidden_prize_spawn_packet(
            prize.collectible_id,
            prize.x,
            prize.y,
            prize.prize_id,
            ttl_ticks,
        ));
        debug!(
            x = prize.x,
            y = prize.y,
            collectible_id = prize.collectible_id,
            prize_id = prize.prize_id,
            ttl_ticks,
            expires_at_tick = prize.expires_at_tick,
            "server hidden prize spawned"
        );
        state.active_hidden_prizes.push(prize);
    }

    state.hidden_prize_rng_seed = rng_seed;
    packets
}

fn hidden_prize_weights() -> ([u8; 28], u16) {
    let mut weights = arena_prize_weights();
    // Every checked-in phase-one arena has Glue/EngineShutdown weight zero,
    // so a normal hidden stream cannot exercise the overlapping
    // EngineShutdown+EMP browser contract. This narrowly guarded test tuning
    // changes only hidden-green selection; claims, effects, snapshots, and
    // movement still use the production server paths and exact arena timers.
    if std::env::var("ZUBSPACE_E2E_PRIZES_PHASE1").as_deref() == Ok("1")
        && let Ok(value) = std::env::var("ZUBSPACE_SERVER_TEST_HIDDEN_ENGINE_SHUTDOWN_WEIGHT")
        && let Ok(value) = value.parse::<u8>()
    {
        weights[13] = value;
    }
    let total = weights.iter().map(|weight| u16::from(*weight)).sum();
    (weights, total)
}

pub(crate) fn active_collectible_claim(
    state: &mut ServerState,
    collectible_id: u32,
    tick: u32,
) -> Option<ServerPrizeClaim> {
    expire_hidden_prizes(state, tick);
    let prize = state.active_hidden_prizes.iter().find(|prize| {
        prize.collectible_id == collectible_id
            && !crate::continuum::continuum_tick_reached(tick, prize.expires_at_tick)
    })?;
    Some(ServerPrizeClaim {
        collectible_id: prize.collectible_id,
        x: prize.x,
        y: prize.y,
        prize_id: prize.prize_id,
    })
}

pub(crate) fn remove_active_collectible(state: &mut ServerState, collectible_id: u32) -> bool {
    let Some(index) = state
        .active_hidden_prizes
        .iter()
        .position(|prize| prize.collectible_id == collectible_id)
    else {
        return false;
    };
    state.active_hidden_prizes.swap_remove(index);
    true
}

pub(crate) fn create_death_collectible(
    state: &mut ServerState,
    x: u16,
    y: u16,
    prize_id: i16,
    tick: u32,
    lifetime_ticks: u32,
) -> Option<u32> {
    expire_hidden_prizes(state, tick);
    if state.active_hidden_prizes.len() >= SERVER_MAX_ACTIVE_COLLECTIBLES || lifetime_ticks == 0 {
        return None;
    }
    let collectible_id = state.allocate_collectible_id()?;
    state.active_hidden_prizes.push(ServerHiddenPrize {
        collectible_id,
        x,
        y,
        prize_id,
        expires_at_tick: crate::continuum::continuum_tick_add(tick, lifetime_ticks),
    });
    Some(collectible_id)
}

fn draw_hidden_prize_candidate<const N: usize>(
    map: &ZoneMap,
    weights: &[u8; N],
    weight_total: u16,
    player_count: usize,
    tick: u32,
    rng_seed: &mut i32,
) -> Option<(ServerHiddenPrize, u16)> {
    let position = hidden_prize_spawn_position(
        map,
        arena_prize_minimum_virtual(),
        arena_prize_upgrade_virtual(),
        player_count,
        rng_seed,
    );
    let weighted = if weight_total == 0 {
        // A zero-weight slot performs position and lifetime selection only.
        // GeneratePrize is never entered, so neither weighted nor negative
        // draws advance the shared hidden stream.
        None
    } else {
        generate_weighted_prize_id(
            weights,
            weight_total,
            arena_prize_negative_factor(),
            true,
            rng_seed,
        )
    };
    // Position rejection and zero weights suppress admission only. Lifetime
    // is the fifth draw for nonzero weights and the third draw for zero.
    let ttl_ticks = hidden_prize_lifetime_ticks(rng_seed);
    let (x, y) = position?;
    let (base_prize_id, negative) = weighted?;
    let prize_id = if negative {
        -base_prize_id
    } else {
        base_prize_id
    };
    Some((
        ServerHiddenPrize {
            collectible_id: 0,
            x,
            y,
            prize_id,
            expires_at_tick: crate::continuum::continuum_tick_add(tick, ttl_ticks as u32),
        },
        ttl_ticks,
    ))
}

fn hidden_prize_spawn_position(
    map: &ZoneMap,
    minimum_virtual: u16,
    upgrade_virtual: u16,
    player_count: usize,
    rng_seed: &mut i32,
) -> Option<(u16, u16)> {
    let map_tiles = map
        .tiles
        .len()
        .checked_div(MAP_TILES)
        .unwrap_or(MAP_TILES)
        .min(MAP_TILES)
        .max(1) as u32;
    let mut spawn_extent = minimum_virtual as u32
        + upgrade_virtual as u32 * player_count.min(u16::MAX as usize) as u32;
    spawn_extent = spawn_extent.clamp(3, map_tiles);

    let range = spawn_extent - 2;
    let offset = (map_tiles - spawn_extent) / 2;
    let x_tile = (vie_rng_next(rng_seed) % range) + 1 + offset;
    let y_tile = (vie_rng_next(rng_seed) % range) + 1 + offset;
    if x_tile >= MAP_TILES as u32 || y_tile >= MAP_TILES as u32 {
        return None;
    }

    Some((
        ((x_tile as f32 + 0.5) * TILE_SIZE_PX).round() as u16,
        ((y_tile as f32 + 0.5) * TILE_SIZE_PX).round() as u16,
    ))
}

fn hidden_prize_lifetime_ticks(rng_seed: &mut i32) -> u16 {
    let min_ticks = arena_prize_lifetime_min_ticks() as u32;
    let max_ticks = arena_prize_lifetime_max_ticks().max(arena_prize_lifetime_min_ticks()) as u32;
    let roll = vie_rng_next(rng_seed);
    if max_ticks <= min_ticks {
        return min_ticks.min(u16::MAX as u32) as u16;
    }

    let roll = roll % (max_ticks - min_ticks + 1);
    (min_ticks + roll).min(u16::MAX as u32) as u16
}

fn max_hidden_prize_count(prize_factor: u16, player_count: usize) -> usize {
    ((prize_factor as usize).saturating_mul(player_count) / 1000)
        .min(SERVER_MAX_ACTIVE_COLLECTIBLES)
}

fn joined_prize_player_count(state: &ServerState) -> usize {
    state
        .clients
        .values()
        .filter(|client| client.joined && client.alive && client.player.ship < 8)
        .count()
}

fn expire_hidden_prizes(state: &mut ServerState, tick: u32) -> usize {
    let before = state.active_hidden_prizes.len();
    state
        .active_hidden_prizes
        .retain(|prize| !crate::continuum::continuum_tick_reached(tick, prize.expires_at_tick));
    before.saturating_sub(state.active_hidden_prizes.len())
}

fn ttl_ticks_until(expires_at_tick: u32, tick: u32) -> Option<u16> {
    let ticks = crate::continuum::continuum_tick_diff(expires_at_tick, tick);
    (ticks > 0).then_some((ticks as u32).min(u16::MAX as u32) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{join_arena, register_connection};
    use crate::arena_settings::arena_prize_weight_total;
    use std::sync::mpsc;

    #[test]
    fn hidden_prize_spawn_packet_uses_zubspace_custom_layout() {
        assert_eq!(
            hidden_prize_spawn_packet(0x1122_3344, 512, 640, -21, 1234),
            vec![
                HIDDEN_PRIZE_SPAWN,
                0x44,
                0x33,
                0x22,
                0x11,
                0,
                2,
                128,
                2,
                235,
                255,
                210,
                4,
            ]
        );
    }

    #[test]
    fn step_hidden_prizes_spawns_server_owned_prizes_for_joined_players() {
        let tick = 1_000;
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        while rx.try_recv().is_ok() {}
        {
            let mut state = state.lock().expect("state");
            state.next_hidden_prize_spawn_at_tick = Some(tick);
            state.hidden_prize_rng_seed = SECURITY_PRIZE_SEED;
        }

        let packets = step_hidden_prizes(&state, tick);

        assert!(!packets.is_empty());
        for packet in &packets {
            assert_eq!(packet[0], HIDDEN_PRIZE_SPAWN);
            assert_eq!(packet.len(), HIDDEN_PRIZE_SPAWN_PACKET_LEN);
        }
        let state = state.lock().expect("state");
        assert_eq!(state.active_hidden_prizes.len(), packets.len());
        for prize in &state.active_hidden_prizes {
            assert_eq!(tile_at_world(state.zone_map(), prize.x, prize.y), 0);
            assert!(crate::continuum::continuum_tick_diff(prize.expires_at_tick, tick) > 0);
        }
    }

    #[test]
    fn current_hidden_prize_packets_send_remaining_ttl_to_late_joiners() {
        let tick = 1_000;
        let state = Arc::new(Mutex::new(ServerState::default()));
        {
            let mut state = state.lock().expect("state");
            state.active_hidden_prizes.push(ServerHiddenPrize {
                collectible_id: 7,
                x: 512,
                y: 640,
                prize_id: 15,
                expires_at_tick: crate::continuum::continuum_tick_add(tick, 300),
            });
        }

        assert_eq!(
            current_hidden_prize_packets(&state, tick),
            vec![hidden_prize_spawn_packet(7, 512, 640, 15, 300)]
        );
    }

    #[test]
    fn active_collectible_claim_canonicalizes_from_server_registry() {
        let tick = 1_000;
        let mut state = ServerState::default();
        state.active_hidden_prizes.push(ServerHiddenPrize {
            collectible_id: 123,
            x: 512,
            y: 640,
            prize_id: 21,
            expires_at_tick: crate::continuum::continuum_tick_add(tick, 1_000),
        });
        let claim = active_collectible_claim(&mut state, 123, tick).expect("claim");

        assert_eq!(
            claim,
            ServerPrizeClaim {
                collectible_id: 123,
                x: 512,
                y: 640,
                prize_id: 21,
            }
        );
        assert_eq!(state.active_hidden_prizes.len(), 1);
        assert!(remove_active_collectible(&mut state, 123));
    }

    #[test]
    fn hidden_prize_spawn_respects_prize_delay_cadence() {
        // The cadence test from the Phase 4 prize-RNG TODO: spawns
        // honor `arena_prize_spawn_delay_ticks()` (PrizeDelay) so a tick
        // before the next-spawn deadline emits nothing, and a tick
        // at-or-after the deadline emits up to `arena_prize_hide_count()`
        // (PrizeHideCount).
        let tick = 1_000;
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        while rx.try_recv().is_ok() {}

        // Schedule the first spawn for this tick; first step fires it.
        {
            let mut state = state.lock().expect("state");
            state.next_hidden_prize_spawn_at_tick = Some(tick);
            state.hidden_prize_rng_seed = SECURITY_PRIZE_SEED;
        }
        let first = step_hidden_prizes(&state, tick);
        assert!(!first.is_empty(), "first step at deadline must emit");
        let delay_ticks = arena_prize_spawn_delay_ticks();

        // A step before the new deadline emits nothing.
        let mid = crate::continuum::continuum_tick_add(tick, delay_ticks - 1);
        let mid_packets = step_hidden_prizes(&state, mid);
        assert!(
            mid_packets.is_empty(),
            "no emission before next prize_delay deadline"
        );

        // Clear active prizes so the per-player cap doesn't suppress
        // the second emission. The cadence is what's under test here,
        // not the cap.
        {
            let mut state = state.lock().expect("state");
            state.active_hidden_prizes.clear();
        }

        // A step AT the new deadline fires the next batch.
        let next = crate::continuum::continuum_tick_add(tick, delay_ticks);
        let next_packets = step_hidden_prizes(&state, next);
        assert!(
            !next_packets.is_empty(),
            "deadline-aligned step must emit again"
        );
    }

    #[test]
    fn zero_cap_cadences_consume_complete_stream_before_third_player_at_factor_400() {
        const PRIZE_FACTOR_400: u16 = 400;
        let state = Arc::new(Mutex::new(ServerState::default()));
        let weights = arena_prize_weights();
        let total = arena_prize_weight_total();
        let spawn_count = arena_prize_hide_count();
        let map = crate::zone::zone_map_for_id("chaos-zone").expect("map");
        let mut expected_seed = SECURITY_PRIZE_SEED;

        for (index, player_count) in [1_usize, 2, 3].into_iter().enumerate() {
            let (tx, rx) = mpsc::channel();
            let player_id = register_connection(&state, tx);
            join_arena(&state, player_id, 0);
            while rx.try_recv().is_ok() {}
            let tick = 1_000 + index as u32 * 100;
            {
                let mut guard = state.lock().expect("state");
                guard.next_hidden_prize_spawn_at_tick = Some(tick);
                if index == 0 {
                    guard.hidden_prize_rng_seed = SECURITY_PRIZE_SEED;
                }
            }
            let packets = step_hidden_prizes_with_factor(&state, tick, PRIZE_FACTOR_400);
            for _ in 0..spawn_count {
                let _ = draw_hidden_prize_candidate(
                    map,
                    &weights,
                    total,
                    player_count,
                    tick,
                    &mut expected_seed,
                );
            }
            assert_eq!(
                state.lock().expect("state").hidden_prize_rng_seed,
                expected_seed
            );
            if player_count < 3 {
                assert!(packets.is_empty());
                assert_eq!(max_hidden_prize_count(PRIZE_FACTOR_400, player_count), 0);
            } else {
                assert_eq!(max_hidden_prize_count(PRIZE_FACTOR_400, player_count), 1);
            }
        }
    }

    #[test]
    fn zero_weights_consume_only_position_and_lifetime_draws() {
        let map = crate::zone::zone_map_for_id("chaos-zone").expect("map");
        let weights = [0_u8; 28];
        let mut seed = SECURITY_PRIZE_SEED;
        let mut expected_seed = seed;
        for _ in 0..3 {
            let _ = vie_rng_next(&mut expected_seed);
        }

        assert!(draw_hidden_prize_candidate(map, &weights, 0, 2, 1_000, &mut seed).is_none());
        assert_eq!(seed, expected_seed);
    }

    #[test]
    fn hidden_prize_spawn_is_deterministic_for_a_given_seed() {
        // The RNG-determinism test: same seed → same prize layout
        // across independent runs. Pins that the prize stream is
        // reproducible (matters for client-side challenge sync).
        let run = |seed: i32| -> Vec<(u16, u16, i16)> {
            let tick = 1_000;
            let state = Arc::new(Mutex::new(ServerState::default()));
            let (tx, rx) = mpsc::channel();
            let pid = register_connection(&state, tx);
            join_arena(&state, pid, 0);
            while rx.try_recv().is_ok() {}
            {
                let mut state = state.lock().expect("state");
                state.next_hidden_prize_spawn_at_tick = Some(tick);
                state.hidden_prize_rng_seed = seed;
            }
            step_hidden_prizes(&state, tick);
            let state = state.lock().expect("state");
            state
                .active_hidden_prizes
                .iter()
                .map(|p| (p.x, p.y, p.prize_id))
                .collect()
        };

        let a = run(SECURITY_PRIZE_SEED);
        let b = run(SECURITY_PRIZE_SEED);
        assert_eq!(a, b, "deterministic for the same seed");
        // A different seed should diverge (extremely unlikely to
        // collide with the same set of positions + prize ids).
        let c = run(SECURITY_PRIZE_SEED.wrapping_add(0x1234_5678));
        assert_ne!(a, c, "different seeds should produce different output");
    }

    #[test]
    fn movement_soak_seed_naturally_spawns_movement_prizes() {
        let map = crate::zone::zone_map_for_id("chaos-zone").expect("chaos map");
        let weights = arena_prize_weights();
        let total = arena_prize_weight_total();
        let mut seed = 15_i32;
        let mut active = Vec::new();
        for _ in 0..arena_prize_hide_count() {
            if let Some((prize, _)) =
                draw_hidden_prize_candidate(map, &weights, total, 2, 1_000, &mut seed)
                && tile_at_world(map, prize.x, prize.y) == 0
            {
                active.push(prize);
            }
        }
        assert!(
            active
                .iter()
                .any(|prize| { prize.prize_id == -11 && prize.x == 8280 && prize.y == 8152 }),
            "spawned prizes: {active:?}"
        );
        assert!(
            active
                .iter()
                .any(|prize| { prize.prize_id == 11 && prize.x == 8296 && prize.y == 8344 })
        );
    }

    #[test]
    fn natural_prize_e2e_seed_starts_with_an_isolated_accessible_brick() {
        let map = crate::zone::zone_map_for_id("chaos-zone").expect("chaos map");
        let weights = arena_prize_weights();
        let total = arena_prize_weight_total();
        let found = (1_i32..=20_000).find_map(|initial_seed| {
            let mut seed = initial_seed;
            let active = (0..arena_prize_hide_count())
                .filter_map(|_| {
                    let (prize, _) =
                        draw_hidden_prize_candidate(map, &weights, total, 1, 1_000, &mut seed)?;
                    (tile_at_world(map, prize.x, prize.y) == 0).then_some(prize)
                })
                .take(max_hidden_prize_count(arena_prize_factor(), 1))
                .collect::<Vec<_>>();
            active.iter().find_map(|prize| {
                (prize.prize_id == 26
                    // This central chamber coordinate is pinned by the real-key
                    // browser route; an empty destination tile alone does not
                    // prove that the surrounding Chaos geometry is reachable.
                    && prize.x == 8_248
                    && prize.y == 8_136
                    && active.iter().all(|other| {
                        std::ptr::eq(other, prize)
                            || (f32::from(other.x) - f32::from(prize.x))
                                .hypot(f32::from(other.y) - f32::from(prize.y))
                                >= 48.0
                    }))
                .then_some((initial_seed, prize.x, prize.y))
            })
        });
        assert_eq!(found, Some((377, 8_248, 8_136)));
    }

    #[test]
    fn warp_e2e_seed_naturally_spawns_accessible_positive_warp() {
        let map = crate::zone::zone_map_for_id("chaos-zone").expect("chaos map");
        let weights = arena_prize_weights();
        let total = arena_prize_weight_total();
        let mut seed = 412_i32;
        let mut active = Vec::new();
        for _ in 0..arena_prize_hide_count() {
            if let Some((prize, _)) =
                draw_hidden_prize_candidate(map, &weights, total, 2, 1_000, &mut seed)
                && tile_at_world(map, prize.x, prize.y) == 0
            {
                active.push(prize);
            }
        }
        assert!(
            active
                .iter()
                .any(|prize| prize.prize_id == 7 && prize.x == 8152 && prize.y == 8200),
            "spawned prizes: {active:?}"
        );
    }

    #[test]
    fn portal_phase_two_e2e_seed_naturally_spawns_accessible_positive_portal() {
        let map = crate::zone::zone_map_for_id("chaos-zone").expect("chaos map");
        let weights = arena_prize_weights();
        let total = arena_prize_weight_total();
        let found = (1_i32..=20_000).find_map(|initial_seed| {
            let mut seed = initial_seed;
            (0..arena_prize_hide_count()).find_map(|_| {
                let (prize, _) =
                    draw_hidden_prize_candidate(map, &weights, total, 2, 1_000, &mut seed)?;
                (tile_at_world(map, prize.x, prize.y) == 0
                    && prize.prize_id == 28
                    && (f32::from(prize.x) - 8_200.0).hypot(f32::from(prize.y) - 8_200.0) < 96.0)
                    .then_some((initial_seed, prize.x, prize.y))
            })
        });
        assert_eq!(found, Some((22, 8_200, 8_232)));
    }

    #[test]
    fn prize_phase_one_e2e_seed_naturally_spawns_accessible_positive_shield() {
        let map = crate::zone::zone_map_for_id("chaos-zone").expect("chaos map");
        let weights = arena_prize_weights();
        let total = arena_prize_weight_total();
        let found = (1_i32..=20_000).find_map(|initial_seed| {
            let mut seed = initial_seed;
            (0..arena_prize_hide_count()).find_map(|_| {
                let (prize, _) =
                    draw_hidden_prize_candidate(map, &weights, total, 2, 1_000, &mut seed)?;
                (tile_at_world(map, prize.x, prize.y) == 0
                    && prize.prize_id == 18
                    && (f32::from(prize.x) - 8_200.0).hypot(f32::from(prize.y) - 8_200.0) < 96.0
                    && prize.y >= 8_152)
                    .then_some((initial_seed, prize.x, prize.y))
            })
        });
        assert_eq!(found, Some((67, 8_264, 8_264)));
    }

    #[test]
    fn prize_phase_one_test_tuning_seed_spawns_exact_shield_and_shutdown_identities() {
        let map = crate::zone::zone_map_for_id("chaos-zone").expect("chaos map");
        let mut weights = arena_prize_weights();
        weights[13] = 40;
        let total = weights.iter().map(|weight| u16::from(*weight)).sum();
        let mut seed = 708_182;
        let active = (0..arena_prize_hide_count())
            .filter_map(|_| {
                let (prize, _) =
                    draw_hidden_prize_candidate(map, &weights, total, 2, 1_000, &mut seed)?;
                (tile_at_world(map, prize.x, prize.y) == 0).then_some(prize)
            })
            .collect::<Vec<_>>();
        assert!(
            active
                .iter()
                .any(|prize| { prize.prize_id == 18 && prize.x == 8_296 && prize.y == 8_184 })
        );
        assert!(
            active
                .iter()
                .any(|prize| { prize.prize_id == -14 && prize.x == 8_136 && prize.y == 8_280 })
        );
    }
}

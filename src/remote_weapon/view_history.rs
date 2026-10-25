//! Bounded historical draw state, independent of the live weapon simulator.
//!
//! A recovered future pose cannot be integrated backwards through walls or
//! Repel. Missing past samples therefore produce no draw pose. Only explicit
//! frozen motion can span a gap without additional motion evidence.

use super::view_clock::{ViewDecision, WeaponBirthClock};
use super::*;
use std::collections::{BTreeMap, VecDeque};

pub(crate) const MAX_WEAPON_VIEW_RECORDS: usize = 4096;
pub(crate) const MAX_WEAPON_VIEW_SAMPLES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteWeaponViewKey {
    server_session: u64,
    arena_stream: u64,
    source: RemoteWeaponSourceIdentity,
    group: u64,
    weapon_type: u8,
    child: u8,
}

impl RemoteWeaponViewKey {
    pub(crate) fn from_source(
        source: RemoteWeaponSource,
        weapon_data: u16,
        child: u8,
    ) -> Option<Self> {
        let scope = source.arena_scope?;
        if scope.server_session_id == 0
            || scope.arena_stream_id == 0
            || !matches!(source.identity, RemoteWeaponSourceIdentity::Authenticated { session_id, life_seq, .. }
                if session_id != 0 && life_seq != 0)
            || !matches!((weapon_data & 0x1f) as u8, 1 | 2 | 3 | 4 | 7 | 8 | 15)
        {
            return None;
        }
        let group = source.shot_group_id.filter(|group| *group != 0)?;
        Some(Self {
            server_session: scope.server_session_id,
            arena_stream: scope.arena_stream_id,
            source: source.identity,
            group,
            weapon_type: (weapon_data & 0x1f) as u8,
            child,
        })
    }

    pub(crate) fn from_projectile(projectile: &RemoteProjectile) -> Option<Self> {
        Self::from_source(
            projectile.source,
            projectile.weapon_data,
            projectile.source_projectile_index,
        )
    }

    fn terminal_key(mut self) -> Self {
        // The reference game links all gun children; fragments and burst remain per-child.
        if matches!(self.weapon_type, 1 | 2) {
            self.child = u8::MAX;
        }
        self
    }

    pub(crate) fn group_key(mut self) -> Self {
        self.child = 0;
        self
    }
}

#[derive(Clone, Copy, Debug)]
struct ViewSample {
    tick: u32,
    position: Vec2,
    velocity: Vec2,
    age: f32,
    expiry_tick: u32,
    kind: RemoteProjectileKind,
    weapon_data: u16,
    bounces_remaining: u8,
    frozen: bool,
    last_repel_event: Option<u32>,
}

impl ViewSample {
    fn capture(projectile: &RemoteProjectile, tick: u32) -> Option<Self> {
        if !projectile.pos.is_finite()
            || !projectile.vel.is_finite()
            || !projectile.age.is_finite()
            || !projectile.ttl.is_finite()
            || projectile.ttl < 0.0
        {
            return None;
        }
        Some(Self {
            tick,
            position: projectile.pos,
            velocity: projectile.vel,
            age: projectile.age,
            expiry_tick: projectile.presentation_clock.map_or_else(
                || {
                    crate::continuum::continuum_tick_add(
                        tick,
                        (projectile.ttl * 100.0).round() as u32,
                    )
                },
                |clock| clock.expiry_tick,
            ),
            kind: projectile.kind,
            weapon_data: projectile.weapon_data,
            bounces_remaining: projectile.bounces_remaining,
            frozen: projectile
                .presentation_clock
                .is_some_and(|clock| clock.motion_frozen),
            last_repel_event: projectile.last_repel_mutation_event_id,
        })
    }
}

#[derive(Debug)]
struct ViewRecord {
    template: RemoteProjectile,
    samples: VecDeque<ViewSample>,
    last_write: u64,
    birth_clock: Option<WeaponBirthClock>,
}

#[derive(Clone, Copy, Debug)]
struct Terminal {
    tick: u32,
    inserted: u64,
}

#[derive(Default, Debug)]
pub(crate) struct RemoteWeaponViewHistory {
    records: BTreeMap<RemoteWeaponViewKey, ViewRecord>,
    terminals: BTreeMap<RemoteWeaponViewKey, Terminal>,
    serial: u64,
    evictions: u64,
    stats: WeaponViewStats,
}

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct WeaponViewStats {
    pub(crate) waiting_frames: u64,
    pub(crate) historical_frames: u64,
    pub(crate) current_frames: u64,
    pub(crate) missing_history_frames: u64,
}

impl RemoteWeaponViewHistory {
    pub(crate) fn clear(&mut self) {
        self.records.clear();
        self.terminals.clear();
    }

    pub(crate) fn evictions(&self) -> u64 {
        self.evictions
    }

    pub(crate) fn stats(&self) -> WeaponViewStats {
        self.stats
    }

    /// Only an original carrier that actually materialized live children may
    /// opt into birth recovery. Active-state recovery alone stays current.
    pub(crate) fn mark_original_carrier(&mut self, key: RemoteWeaponViewKey) {
        if let Some(record) = self.records.get_mut(&key)
            && record.birth_clock.is_none()
            && let Some(birth) = record.template.source.realtime_server_tick
        {
            record.birth_clock = Some(WeaponBirthClock::new(birth));
        }
    }

    /// Prepare a draw copy from a still-live runtime object. Retired/expired
    /// objects cannot acquire extra display lifetime through historical data.
    pub(crate) fn prepare_live_view(
        &mut self,
        live: &RemoteProjectile,
        now: u32,
        actor_tick: Option<u32>,
        partial: f32,
    ) -> Option<RemoteProjectile> {
        if live.ttl <= 0.0
            || live.presentation_clock.is_some_and(|clock| {
                crate::continuum::continuum_tick_reached(now, clock.expiry_tick)
            })
        {
            return None;
        }
        let Some(key) = RemoteWeaponViewKey::from_projectile(live) else {
            return Some(*live);
        };
        if self.terminals.contains_key(&key.terminal_key()) {
            return None;
        }
        let decision = self
            .records
            .get_mut(&key)
            .and_then(|record| record.birth_clock.as_mut())
            .map_or(ViewDecision::Current, |clock| clock.select(now, actor_tick));
        match decision {
            ViewDecision::Hidden => {
                self.stats.waiting_frames = self.stats.waiting_frames.saturating_add(1);
                None
            }
            ViewDecision::Current => {
                self.stats.current_frames = self.stats.current_frames.saturating_add(1);
                Some(*live)
            }
            ViewDecision::History { tick, rate } => {
                let fraction = if partial.is_finite() {
                    partial.clamp(0.0, 0.01) * rate as f32
                } else {
                    0.0
                };
                let whole_ticks = (fraction * 100.0).floor() as u32;
                let tick = crate::continuum::continuum_tick_add(tick, whole_ticks);
                let fraction = (fraction - whole_ticks as f32 / 100.0).max(0.0);
                if crate::continuum::continuum_tick_reached(tick, now) {
                    if let Some(clock) = self
                        .records
                        .get_mut(&key)
                        .and_then(|record| record.birth_clock.as_mut())
                    {
                        clock.current();
                    }
                    self.stats.current_frames = self.stats.current_frames.saturating_add(1);
                    return Some(*live);
                }
                if let Some(view) = self.view(key, tick, fraction) {
                    self.stats.historical_frames = self.stats.historical_frames.saturating_add(1);
                    Some(view)
                } else {
                    // Never manufacture the missing old path or let a future
                    // capture keep a dangerous weapon invisible.
                    if let Some(clock) = self
                        .records
                        .get_mut(&key)
                        .and_then(|record| record.birth_clock.as_mut())
                    {
                        clock.current();
                    }
                    self.stats.missing_history_frames =
                        self.stats.missing_history_frames.saturating_add(1);
                    Some(*live)
                }
            }
        }
    }

    pub(crate) fn contains_projectile(&self, projectile: &RemoteProjectile) -> bool {
        RemoteWeaponViewKey::from_projectile(projectile)
            .is_some_and(|key| self.records.contains_key(&key))
    }

    /// Record an actual forward-simulation or authoritative capture point.
    /// A newer correction at an earlier tick replaces its future path, never
    /// interpolates backwards through the path it supersedes.
    pub(crate) fn record(&mut self, projectile: &RemoteProjectile, tick: u32) -> bool {
        let Some(key) = RemoteWeaponViewKey::from_projectile(projectile) else {
            return false;
        };
        if projectile
            .source
            .realtime_server_tick
            .is_none_or(|birth| crate::continuum::continuum_tick_diff(tick, birth) < 0)
        {
            return false;
        }
        if self
            .terminals
            .get(&key.terminal_key())
            .is_some_and(|terminal| crate::continuum::continuum_tick_reached(tick, terminal.tick))
        {
            return false;
        }
        let Some(sample) = ViewSample::capture(projectile, tick) else {
            return false;
        };
        self.serial = self.serial.saturating_add(1);
        if !self.records.contains_key(&key) && self.records.len() >= MAX_WEAPON_VIEW_RECORDS {
            let oldest = self
                .records
                .iter()
                .min_by_key(|(_, record)| record.last_write)
                .map(|(key, _)| *key);
            if let Some(oldest) = oldest {
                self.records.remove(&oldest);
                self.evictions = self.evictions.saturating_add(1);
            }
        }
        let record = self.records.entry(key).or_insert_with(|| ViewRecord {
            template: *projectile,
            samples: VecDeque::new(),
            last_write: self.serial,
            birth_clock: None,
        });
        let original_source = record.template.source;
        record.template = *projectile;
        record.template.source = original_source;
        record.last_write = self.serial;
        while record
            .samples
            .back()
            .is_some_and(|old| crate::continuum::continuum_tick_diff(old.tick, tick) >= 0)
        {
            record.samples.pop_back();
        }
        if record.samples.len() >= MAX_WEAPON_VIEW_SAMPLES {
            record.samples.pop_front();
        }
        record.samples.push_back(sample);
        true
    }

    /// Terminal receipt is immediate authority; its historical draw cutoff
    /// belongs to the event tick. Keep the tombstone after pruning the views
    /// so a delayed pre-terminal recovery cannot revive a completed flight.
    pub(crate) fn terminate(&mut self, key: RemoteWeaponViewKey, tick: u32) {
        let key = key.terminal_key();
        if let Some(terminal) = self.terminals.get_mut(&key) {
            if crate::continuum::continuum_tick_diff(tick, terminal.tick) < 0 {
                terminal.tick = tick;
            }
            return;
        }
        self.serial = self.serial.saturating_add(1);
        if self.terminals.len() >= MAX_WEAPON_VIEW_RECORDS {
            let oldest = self
                .terminals
                .iter()
                .min_by_key(|(_, terminal)| terminal.inserted)
                .map(|(key, _)| *key);
            if let Some(oldest) = oldest {
                self.terminals.remove(&oldest);
                // Never retain a path whose cutoff was evicted. Protocol event
                // replay admission, outside this cosmetic cache, owns old facts.
                self.records
                    .retain(|record, _| record.terminal_key() != oldest);
                self.evictions = self.evictions.saturating_add(1);
            }
        }
        self.terminals.insert(
            key,
            Terminal {
                tick,
                inserted: self.serial,
            },
        );
    }

    pub(crate) fn view(
        &self,
        key: RemoteWeaponViewKey,
        tick: u32,
        partial_seconds: f32,
    ) -> Option<RemoteProjectile> {
        if self
            .terminals
            .get(&key.terminal_key())
            .is_some_and(|terminal| crate::continuum::continuum_tick_reached(tick, terminal.tick))
        {
            return None;
        }
        let record = self.records.get(&key)?;
        let sample = record
            .samples
            .iter()
            .rev()
            .find(|sample| crate::continuum::continuum_tick_diff(tick, sample.tick) >= 0)?;
        let age_ticks = crate::continuum::continuum_tick_diff(tick, sample.tick);
        if age_ticks != 0 && !sample.frozen {
            return None;
        }
        if crate::continuum::continuum_tick_reached(tick, sample.expiry_tick) {
            return None;
        }
        let mut projectile = record.template;
        projectile.pos = sample.position;
        projectile.vel = sample.velocity;
        projectile.age = sample.age + crate::continuum::ticks_to_seconds_i32(age_ticks);
        projectile.ttl = crate::continuum::ticks_to_seconds_i32(
            crate::continuum::continuum_tick_diff(sample.expiry_tick, tick),
        );
        projectile.kind = sample.kind;
        projectile.weapon_data = sample.weapon_data;
        projectile.bounces_remaining = sample.bounces_remaining;
        if projectile.last_repel_mutation_event_id != sample.last_repel_event {
            projectile.authoritative_repel_state = None;
        }
        projectile.last_repel_mutation_event_id = sample.last_repel_event;
        if projectile.authority_binding.is_some_and(|binding| {
            crate::continuum::continuum_tick_diff(binding.state_tick, tick) > 0
        }) {
            projectile.authority_binding = None;
        }
        projectile.presentation_clock = Some(RemoteProjectileClock {
            pose_tick: tick,
            expiry_tick: sample.expiry_tick,
            motion_frozen: sample.frozen,
            render_partial_seconds: if sample.frozen || !partial_seconds.is_finite() {
                0.0
            } else {
                partial_seconds.clamp(0.0, 0.01).min(projectile.ttl)
            },
        });
        Some(projectile)
    }

    pub(crate) fn views_at(
        &self,
        tick: u32,
        partial_seconds: f32,
    ) -> impl Iterator<Item = RemoteProjectile> + '_ {
        self.records
            .keys()
            .filter_map(move |key| self.view(*key, tick, partial_seconds))
    }

    pub(crate) fn prune(&mut self, tick: u32) {
        self.records.retain(|key, record| {
            !self
                .terminals
                .get(&key.terminal_key())
                .is_some_and(|terminal| {
                    crate::continuum::continuum_tick_reached(tick, terminal.tick)
                })
                && record.samples.back().is_some_and(|sample| {
                    !crate::continuum::continuum_tick_reached(tick, sample.expiry_tick)
                })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projectile(group: u64, tick: u32) -> RemoteProjectile {
        let tuning = ArenaTuning::continuum_skirmish_default();
        let ship = tuning.ship(0);
        let source = RemoteWeaponSource::capture(
            RemoteWeaponSourceIdentity::authenticated(1, 2, 3).unwrap(),
            RemoteWeaponArenaScope::authenticated(4, 5),
            vec2(100.0, 100.0),
            vec2(150.0, 0.0),
            0.25,
            6,
            0,
            ship,
            false,
        )
        .unwrap()
        .with_realtime_order(7, tick, group)
        .unwrap();
        let mut result = remote_weapon_projectiles(source, 1, ship, &tuning).remove(0);
        result.ttl = 1.0;
        result.bind_presentation_clock(tick);
        result
    }

    #[test]
    fn seven_tick_actor_delay_selects_the_original_pose_instead_of_the_future_runtime() {
        let mut history = RemoteWeaponViewHistory::default();
        let mut live = projectile(8, 100);
        let key = RemoteWeaponViewKey::from_projectile(&live).unwrap();
        let original = live.pos;
        for tick in 100..=107 {
            live.pos = original + live.vel * ((tick - 100) as f32 / 100.0);
            live.age = (tick - 100) as f32 / 100.0;
            live.ttl = 1.0 - live.age;
            assert!(history.record(&live, tick));
        }
        let view = history.view(key, 100, 0.006).unwrap();
        assert_eq!(view.pos, original);
        assert_ne!(view.pos, live.pos);
        assert_eq!(view.presentation_clock.unwrap().pose_tick, 100);
        assert_eq!(
            view.presentation_clock.unwrap().render_partial_seconds,
            0.006
        );
        assert_eq!(live.age, 0.07, "view queries cannot mutate live simulation");
    }

    #[test]
    fn future_recovery_never_invents_a_backwards_path_or_future_birth() {
        let mut history = RemoteWeaponViewHistory::default();
        let recovered = projectile(8, 107);
        let key = RemoteWeaponViewKey::from_projectile(&recovered).unwrap();
        history.record(&recovered, 107);
        assert!(history.view(key, 100, 0.0).is_none());
        assert_eq!(history.view(key, 107, 0.0).unwrap().pos, recovered.pos);
        assert!(
            history.view(key, 108, 0.0).is_none(),
            "missing motion is not a stationary hold"
        );
    }

    #[test]
    fn terminal_before_birth_keeps_only_the_proven_pre_terminal_segment() {
        let mut history = RemoteWeaponViewHistory::default();
        let mut shot = projectile(8, 100);
        let key = RemoteWeaponViewKey::from_projectile(&shot).unwrap();
        history.terminate(key, 103);
        for tick in 100..=104 {
            shot.pos.x = tick as f32;
            assert_eq!(history.record(&shot, tick), tick < 103);
        }
        assert_eq!(history.view(key, 102, 0.0).unwrap().pos.x, 102.0);
        assert!(history.view(key, 103, 0.0).is_none());
        history.prune(103);
        assert!(history.records.is_empty());
        assert!(!history.record(&shot, 104));
        assert!(history.view(key, 104, 0.0).is_none());
    }

    #[test]
    fn exact_repel_replaces_future_path_without_rewriting_the_earlier_segment() {
        let mut history = RemoteWeaponViewHistory::default();
        let mut shot = projectile(8, 100);
        let key = RemoteWeaponViewKey::from_projectile(&shot).unwrap();
        for tick in 100..=110 {
            shot.pos.x = tick as f32;
            history.record(&shot, tick);
        }
        shot.pos = vec2(103.0, 100.0);
        shot.vel = vec2(-400.0, 0.0);
        shot.presentation_clock.as_mut().unwrap().expiry_tick = 300;
        history.record(&shot, 103);
        assert_eq!(history.view(key, 102, 0.0).unwrap().pos.x, 102.0);
        assert!(
            history.view(key, 104, 0.0).is_none(),
            "superseded future samples must be removed"
        );
        for tick in 104..=110 {
            shot.pos.x = 103.0 - (tick - 103) as f32 * 4.0;
            history.record(&shot, tick);
        }
        assert_eq!(history.view(key, 107, 0.0).unwrap().pos.x, 87.0);
        assert_eq!(
            history
                .view(key, 107, 0.0)
                .unwrap()
                .presentation_clock
                .unwrap()
                .expiry_tick,
            300
        );
    }

    #[test]
    fn frozen_contact_holds_position_but_ages_to_its_own_deadline() {
        let mut history = RemoteWeaponViewHistory::default();
        let mut shot = projectile(8, 100);
        shot.presentation_clock.as_mut().unwrap().motion_frozen = true;
        let key = RemoteWeaponViewKey::from_projectile(&shot).unwrap();
        history.record(&shot, 100);
        let held = history.view(key, 150, 0.009).unwrap();
        assert_eq!(held.pos, shot.pos);
        assert_eq!(held.age, 0.5);
        assert_eq!(held.ttl, 0.5);
        assert_eq!(held.presentation_clock.unwrap().render_partial_seconds, 0.0);
        assert!(history.view(key, 200, 0.0).is_none());
    }

    #[test]
    fn linked_guns_retire_together_while_fragments_keep_exact_child_identity() {
        let mut history = RemoteWeaponViewHistory::default();
        let first = projectile(8, 100);
        let mut sibling = first;
        sibling.source_projectile_index = 1;
        let first_key = RemoteWeaponViewKey::from_projectile(&first).unwrap();
        let sibling_key = RemoteWeaponViewKey::from_projectile(&sibling).unwrap();
        history.record(&first, 100);
        history.record(&sibling, 100);
        history.terminate(first_key, 100);
        assert!(history.view(sibling_key, 100, 0.0).is_none());
        let mut fragment = first;
        fragment.weapon_data = 15;
        let mut fragment_sibling = fragment;
        fragment_sibling.source_projectile_index = 1;
        let fragment_key = RemoteWeaponViewKey::from_projectile(&fragment).unwrap();
        let other = RemoteWeaponViewKey::from_projectile(&fragment_sibling).unwrap();
        history.record(&fragment, 100);
        history.record(&fragment_sibling, 100);
        history.terminate(fragment_key, 100);
        assert!(history.view(other, 100, 0.0).is_some());
    }

    #[test]
    fn wrapped_ticks_and_scope_keys_cannot_reuse_another_life_or_arena() {
        let mut history = RemoteWeaponViewHistory::default();
        let mut shot = projectile(8, 0x7fff_fffe);
        let key = RemoteWeaponViewKey::from_projectile(&shot).unwrap();
        for tick in [0x7fff_fffe, 0x7fff_ffff, 0, 1] {
            history.record(&shot, tick);
        }
        assert!(history.view(key, 0, 0.0).is_some());
        history.terminate(key, 1);
        assert!(history.view(key, 1, 0.0).is_none());
        shot.source.arena_scope.as_mut().unwrap().arena_stream_id += 1;
        let new_scope = RemoteWeaponViewKey::from_projectile(&shot).unwrap();
        assert!(history.record(&shot, 1));
        assert!(history.view(new_scope, 1, 0.0).is_some());
        history.clear();
        assert_eq!(history.views_at(1, 0.0).count(), 0);
    }

    #[test]
    fn samples_records_and_terminal_memory_are_explicitly_bounded() {
        let mut history = RemoteWeaponViewHistory::default();
        let mut shot = projectile(1, 0);
        for tick in 0..256 {
            history.record(&shot, tick);
        }
        assert_eq!(
            history.records.values().next().unwrap().samples.len(),
            MAX_WEAPON_VIEW_SAMPLES
        );
        for group in 1..=MAX_WEAPON_VIEW_RECORDS as u64 + 4 {
            shot.source.shot_group_id = Some(group);
            history.record(&shot, 0);
            history.terminate(RemoteWeaponViewKey::from_projectile(&shot).unwrap(), 1);
        }
        assert!(history.records.len() <= MAX_WEAPON_VIEW_RECORDS);
        assert_eq!(history.terminals.len(), MAX_WEAPON_VIEW_RECORDS);
        assert!(history.evictions() >= 4);
        assert!(
            std::mem::size_of::<ViewSample>() * MAX_WEAPON_VIEW_SAMPLES * MAX_WEAPON_VIEW_RECORDS
                < 32 * 1024 * 1024
        );
    }

    #[test]
    fn successor_life_and_reused_player_id_never_adopt_an_older_terminal() {
        let mut history = RemoteWeaponViewHistory::default();
        let mut shot = projectile(8, 100);
        let old = RemoteWeaponViewKey::from_projectile(&shot).unwrap();
        history.record(&shot, 100);
        history.terminate(old, 100);
        for identity in [
            RemoteWeaponSourceIdentity::authenticated(1, 2, 4).unwrap(),
            RemoteWeaponSourceIdentity::authenticated(1, 3, 3).unwrap(),
        ] {
            shot.source.identity = identity;
            let key = RemoteWeaponViewKey::from_projectile(&shot).unwrap();
            assert_ne!(key, old);
            assert!(history.record(&shot, 100));
            assert_eq!(
                history.view(key, 100, 0.0).unwrap().source.identity,
                identity
            );
        }
        assert!(history.view(old, 100, 0.0).is_none());
    }

    #[test]
    fn invalid_unordered_or_pre_birth_samples_cannot_supply_a_draw_pose() {
        let mut history = RemoteWeaponViewHistory::default();
        let shot = projectile(8, 100);
        assert!(!history.record(&shot, 99));
        let mut invalid = shot;
        invalid.pos.x = f32::NAN;
        assert!(!history.record(&invalid, 100));
        invalid = shot;
        invalid.ttl = f32::INFINITY;
        assert!(!history.record(&invalid, 100));
        invalid = shot;
        invalid.source.realtime_server_tick = None;
        assert!(!history.record(&invalid, 100));
        invalid = shot;
        invalid.source.identity = RemoteWeaponSourceIdentity::Legacy {
            player_id: 1,
            life_seq: Some(3),
        };
        assert!(!history.record(&invalid, 100));
        assert_eq!(history.views_at(100, 0.0).count(), 0);
    }
}

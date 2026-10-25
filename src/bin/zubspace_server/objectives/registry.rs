use std::collections::BTreeMap;

pub(crate) const POWERBALL_SCORE_BUCKETS: usize = 8;

use super::super::authority_identity::PlayerRuntimeIdentity;
use super::profile::{ServerObjectiveProfile, ServerPowerballProfile};

const OBJECTIVE_VERSION_MASK: u32 = crate::continuum::CONTINUUM_TICK_MASK;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObjectiveVersion(u32);

impl ObjectiveVersion {
    pub(crate) const FIRST: Self = Self(1);

    pub(crate) fn get(self) -> u32 {
        self.0
    }

    pub(crate) fn new(value: u32) -> Option<Self> {
        (value != 0 && value <= OBJECTIVE_VERSION_MASK).then_some(Self(value))
    }

    fn next(self) -> Result<Self, ObjectiveRegistryError> {
        if self.0 >= OBJECTIVE_VERSION_MASK {
            return Err(ObjectiveRegistryError::VersionExhausted);
        }
        Ok(Self(self.0 + 1))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FlagIdentity {
    pub(crate) id: u16,
    pub(crate) generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PowerballIdentity {
    pub(crate) id: u8,
    pub(crate) generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PowerballMotion {
    /// Reference-game-compatible fixed-point position: legacy wire coordinate × 1000.
    pub(crate) position_x: u32,
    pub(crate) position_y: u32,
    pub(crate) velocity_x: i16,
    pub(crate) velocity_y: i16,
    pub(crate) friction: u32,
    pub(crate) friction_delta: i16,
    pub(crate) last_step_tick: u32,
    pub(crate) last_publish_tick: u32,
    pub(crate) last_touch: Option<PlayerRuntimeIdentity>,
    pub(crate) last_touch_frequency: Option<u16>,
}

impl PowerballMotion {
    pub(crate) const FRICTION_SCALE: u32 = 1_000_000;
    pub(crate) const POSITION_SCALE: u32 = 1_000;
    pub(crate) const TILE_SCALE: u32 = 16_000;

    pub(crate) fn from_wire_position(
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
        friction_delta: i16,
        tick: u32,
    ) -> Self {
        Self {
            position_x: u32::from(x) * Self::POSITION_SCALE,
            position_y: u32::from(y) * Self::POSITION_SCALE,
            velocity_x,
            velocity_y,
            friction: if velocity_x != 0 || velocity_y != 0 {
                Self::FRICTION_SCALE
            } else {
                0
            },
            friction_delta,
            last_step_tick: tick,
            last_publish_tick: tick,
            last_touch: None,
            last_touch_frequency: None,
        }
    }

    pub(crate) fn wire_position(self) -> (u16, u16) {
        (
            (self.position_x / Self::POSITION_SCALE).min(u32::from(u16::MAX)) as u16,
            (self.position_y / Self::POSITION_SCALE).min(u32::from(u16::MAX)) as u16,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlagState {
    Waiting {
        respawn_at_tick: Option<u32>,
    },
    OnMap {
        x: u16,
        y: u16,
        owner: Option<u16>,
    },
    Carried {
        carrier: PlayerRuntimeIdentity,
        x: u16,
        y: u16,
        owner: Option<u16>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PowerballState {
    Waiting {
        respawn_at_tick: Option<u32>,
    },
    OnMap {
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
        pickup_unlock_tick: Option<u32>,
    },
    Carried {
        carrier: PlayerRuntimeIdentity,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FlagSnapshot {
    pub(crate) identity: FlagIdentity,
    pub(crate) version: ObjectiveVersion,
    pub(crate) state: FlagState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PowerballSnapshot {
    pub(crate) identity: PowerballIdentity,
    pub(crate) version: ObjectiveVersion,
    pub(crate) state: PowerballState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObjectiveReplay {
    pub(crate) generation: u64,
    pub(crate) flags: Vec<FlagSnapshot>,
    pub(crate) powerballs: Vec<PowerballSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObjectiveRegistry {
    profile: ServerObjectiveProfile,
    generation: u64,
    next_flag_id: u16,
    next_powerball_id: u8,
    flags: BTreeMap<u16, FlagSnapshot>,
    powerballs: BTreeMap<u8, PowerballSnapshot>,
    powerball_motion: BTreeMap<u8, PowerballMotion>,
    powerball_killer_pickup: BTreeMap<u8, (PlayerRuntimeIdentity, u32)>,
    flag_carried_since: BTreeMap<u16, u32>,
    powerball_carried_since: BTreeMap<u8, u32>,
    flag_scores: BTreeMap<u16, i64>,
    powerball_scores: BTreeMap<u16, i64>,
    powerball_score_version: ObjectiveVersion,
    pending_flag_win: Option<(u16, u32)>,
    action_windows: BTreeMap<PlayerRuntimeIdentity, ObjectiveActionWindow>,
    rng_seed: i32,
}

impl Default for ObjectiveRegistry {
    fn default() -> Self {
        Self::new(ServerObjectiveProfile::disabled())
    }
}

impl ObjectiveRegistry {
    pub(crate) fn new(profile: ServerObjectiveProfile) -> Self {
        let powerball_scores = initial_powerball_scores(&profile);
        Self {
            profile,
            generation: 1,
            next_flag_id: 0,
            next_powerball_id: 0,
            flags: BTreeMap::new(),
            powerballs: BTreeMap::new(),
            powerball_motion: BTreeMap::new(),
            powerball_killer_pickup: BTreeMap::new(),
            flag_carried_since: BTreeMap::new(),
            powerball_carried_since: BTreeMap::new(),
            flag_scores: BTreeMap::new(),
            powerball_scores,
            powerball_score_version: ObjectiveVersion::FIRST,
            pending_flag_win: None,
            action_windows: BTreeMap::new(),
            rng_seed: 1,
        }
    }

    pub(crate) fn profile(&self) -> &ServerObjectiveProfile {
        &self.profile
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.flags.is_empty() && self.powerballs.is_empty()
    }

    pub(crate) fn reset(
        &mut self,
        profile: ServerObjectiveProfile,
    ) -> Result<(), ObjectiveRegistryError> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(ObjectiveRegistryError::GenerationExhausted)?;
        self.profile = profile;
        self.generation = generation;
        self.next_flag_id = 0;
        self.next_powerball_id = 0;
        self.flags.clear();
        self.powerballs.clear();
        self.powerball_motion.clear();
        self.powerball_killer_pickup.clear();
        self.flag_carried_since.clear();
        self.powerball_carried_since.clear();
        self.flag_scores.clear();
        self.powerball_scores = initial_powerball_scores(&self.profile);
        self.powerball_score_version = ObjectiveVersion::FIRST;
        self.pending_flag_win = None;
        self.action_windows.clear();
        Ok(())
    }

    /// Fail-safe retirement for a hard player lifecycle boundary. A fresh
    /// generation makes recovery independent of exhausted per-objective
    /// versions. Objects tied to the retiring runtime are quarantined in a
    /// retryable waiting state; unrelated canonical state and score values
    /// are preserved and republished under the new generation.
    pub(crate) fn recover_hard_player_lifecycle(
        &mut self,
        runtime: PlayerRuntimeIdentity,
        tick: u32,
    ) -> Result<ObjectiveReplay, ObjectiveRegistryError> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(ObjectiveRegistryError::GenerationExhausted)?;
        self.generation = generation;
        for (&id, flag) in &mut self.flags {
            flag.identity.generation = generation;
            flag.version = ObjectiveVersion::FIRST;
            if matches!(flag.state, FlagState::Carried { carrier, .. } if carrier == runtime) {
                flag.state = FlagState::Waiting {
                    respawn_at_tick: Some(tick),
                };
                self.flag_carried_since.remove(&id);
            }
        }
        for (&id, ball) in &mut self.powerballs {
            ball.identity.generation = generation;
            ball.version = ObjectiveVersion::FIRST;
            if matches!(ball.state, PowerballState::Carried { carrier } if carrier == runtime) {
                ball.state = PowerballState::Waiting {
                    respawn_at_tick: Some(tick),
                };
                self.powerball_carried_since.remove(&id);
                self.powerball_motion.remove(&id);
                self.powerball_killer_pickup.remove(&id);
                continue;
            }
            if let Some(motion) = self.powerball_motion.get_mut(&id)
                && motion.last_touch == Some(runtime)
            {
                motion.last_touch = None;
                motion.last_touch_frequency = None;
            }
            if self
                .powerball_killer_pickup
                .get(&id)
                .is_some_and(|(killer, _)| *killer == runtime)
            {
                self.powerball_killer_pickup.remove(&id);
            }
        }
        self.powerball_score_version = ObjectiveVersion::FIRST;
        self.pending_flag_win = None;
        self.action_windows.remove(&runtime);
        Ok(self.replay())
    }

    pub(crate) fn create_flag(
        &mut self,
        state: FlagState,
    ) -> Result<FlagSnapshot, ObjectiveRegistryError> {
        let count = self.profile.flags.count();
        if count == 0 {
            return Err(ObjectiveRegistryError::Disabled);
        }
        if self.flags.len() >= count as usize {
            return Err(ObjectiveRegistryError::Capacity);
        }
        let id = self.next_flag_id;
        self.next_flag_id = self
            .next_flag_id
            .checked_add(1)
            .ok_or(ObjectiveRegistryError::IdentityExhausted)?;
        let snapshot = FlagSnapshot {
            identity: FlagIdentity {
                id,
                generation: self.generation,
            },
            version: ObjectiveVersion::FIRST,
            state,
        };
        self.flags.insert(id, snapshot);
        Ok(snapshot)
    }

    pub(crate) fn create_powerball(
        &mut self,
        state: PowerballState,
    ) -> Result<PowerballSnapshot, ObjectiveRegistryError> {
        let count = self.profile.powerballs.count();
        if count == 0 {
            return Err(ObjectiveRegistryError::Disabled);
        }
        if self.powerballs.len() >= count as usize {
            return Err(ObjectiveRegistryError::Capacity);
        }
        let id = self.next_powerball_id;
        self.next_powerball_id = self
            .next_powerball_id
            .checked_add(1)
            .ok_or(ObjectiveRegistryError::IdentityExhausted)?;
        let snapshot = PowerballSnapshot {
            identity: PowerballIdentity {
                id,
                generation: self.generation,
            },
            version: ObjectiveVersion::FIRST,
            state,
        };
        self.powerballs.insert(id, snapshot);
        Ok(snapshot)
    }

    pub(crate) fn flag(&self, id: u16) -> Option<FlagSnapshot> {
        self.flags.get(&id).copied()
    }

    pub(crate) fn powerball(&self, id: u8) -> Option<PowerballSnapshot> {
        self.powerballs.get(&id).copied()
    }

    pub(crate) fn powerball_motion(&self, id: u8) -> Option<PowerballMotion> {
        self.powerball_motion.get(&id).copied()
    }

    pub(crate) fn set_powerball_motion(
        &mut self,
        identity: PowerballIdentity,
        motion: PowerballMotion,
    ) -> Result<(), ObjectiveRegistryError> {
        self.powerballs
            .get(&identity.id)
            .filter(|current| current.identity == identity)
            .ok_or(ObjectiveRegistryError::UnknownIdentity)?;
        self.powerball_motion.insert(identity.id, motion);
        Ok(())
    }

    pub(crate) fn set_flag_carried_since(&mut self, identity: FlagIdentity, tick: u32) {
        if self
            .flags
            .get(&identity.id)
            .is_some_and(|current| current.identity == identity)
        {
            self.flag_carried_since.insert(identity.id, tick);
        }
    }

    pub(crate) fn flag_carried_since(&self, id: u16) -> Option<u32> {
        self.flag_carried_since.get(&id).copied()
    }

    pub(crate) fn set_powerball_carried_since(&mut self, identity: PowerballIdentity, tick: u32) {
        if self
            .powerballs
            .get(&identity.id)
            .is_some_and(|current| current.identity == identity)
        {
            self.powerball_carried_since.insert(identity.id, tick);
        }
    }

    pub(crate) fn powerball_carried_since(&self, id: u8) -> Option<u32> {
        self.powerball_carried_since.get(&id).copied()
    }

    #[cfg(test)]
    pub(crate) fn flag_score(&self, frequency: u16) -> i64 {
        self.flag_scores.get(&frequency).copied().unwrap_or(0)
    }

    pub(crate) fn powerball_score(&self, frequency: u16) -> i64 {
        self.powerball_scores.get(&frequency).copied().unwrap_or(0)
    }

    pub(crate) fn add_flag_score(&mut self, frequency: u16, points: i64) {
        let score = self.flag_scores.entry(frequency).or_default();
        *score = score.saturating_add(points);
    }

    pub(crate) fn add_powerball_score(&mut self, frequency: u16, points: i64) {
        let score = self.powerball_scores.entry(frequency).or_default();
        *score = score.saturating_add(points);
    }

    pub(crate) fn set_powerball_score(&mut self, frequency: u16, points: i64) {
        self.powerball_scores.insert(frequency, points.max(0));
    }

    pub(crate) fn reset_powerball_scores(&mut self) {
        self.powerball_scores = initial_powerball_scores(&self.profile);
    }

    pub(crate) fn powerball_score_version(&self) -> ObjectiveVersion {
        self.powerball_score_version
    }

    pub(crate) fn advance_powerball_score_version(
        &mut self,
    ) -> Result<ObjectiveVersion, ObjectiveRegistryError> {
        self.powerball_score_version = self.powerball_score_version.next()?;
        Ok(self.powerball_score_version)
    }

    pub(crate) fn powerball_score_snapshot(&self) -> [(u16, u32); POWERBALL_SCORE_BUCKETS] {
        std::array::from_fn(|frequency| {
            let frequency = frequency as u16;
            let score = self
                .powerball_score(frequency)
                .clamp(0, i64::from(u32::MAX)) as u32;
            (frequency, score)
        })
    }

    pub(crate) fn pending_flag_win(&self) -> Option<(u16, u32)> {
        self.pending_flag_win
    }

    pub(crate) fn set_pending_flag_win(&mut self, pending: Option<(u16, u32)>) {
        self.pending_flag_win = pending;
    }

    pub(crate) fn consume_action(
        &mut self,
        runtime: PlayerRuntimeIdentity,
        action_id: u32,
    ) -> bool {
        if action_id == 0 {
            return false;
        }
        let window = self.action_windows.entry(runtime).or_default();
        if window.seen(action_id) {
            return false;
        }
        window.remember(action_id);
        true
    }

    pub(crate) fn clear_player_actions(&mut self, runtime: PlayerRuntimeIdentity) {
        self.action_windows.remove(&runtime);
    }

    pub(crate) fn powerball_killer_pickup(&self, id: u8) -> Option<(PlayerRuntimeIdentity, u32)> {
        self.powerball_killer_pickup.get(&id).copied()
    }

    pub(crate) fn set_powerball_killer_pickup(
        &mut self,
        identity: PowerballIdentity,
        pickup: Option<(PlayerRuntimeIdentity, u32)>,
    ) -> Result<(), ObjectiveRegistryError> {
        if !self
            .powerballs
            .get(&identity.id)
            .is_some_and(|ball| ball.identity == identity)
        {
            return Err(ObjectiveRegistryError::UnknownIdentity);
        }
        if let Some(pickup) = pickup {
            self.powerball_killer_pickup.insert(identity.id, pickup);
        } else {
            self.powerball_killer_pickup.remove(&identity.id);
        }
        Ok(())
    }

    pub(crate) fn clear_powerball_killer_pickups_for_player(
        &mut self,
        runtime: PlayerRuntimeIdentity,
    ) -> Vec<u8> {
        let cleared = self
            .powerball_killer_pickup
            .iter()
            .filter_map(|(&id, &(killer, _))| (killer == runtime).then_some(id))
            .collect::<Vec<_>>();
        for id in &cleared {
            self.powerball_killer_pickup.remove(id);
        }
        cleared
    }

    pub(crate) fn rng_seed_mut(&mut self) -> &mut i32 {
        &mut self.rng_seed
    }

    #[cfg(test)]
    pub(crate) fn set_rng_seed(&mut self, seed: i32) {
        self.rng_seed = seed.max(1);
    }

    pub(crate) fn transition_flag(
        &mut self,
        identity: FlagIdentity,
        expected_version: ObjectiveVersion,
        state: FlagState,
    ) -> Result<FlagSnapshot, ObjectiveRegistryError> {
        let current = self
            .flags
            .get_mut(&identity.id)
            .filter(|current| current.identity == identity)
            .ok_or(ObjectiveRegistryError::UnknownIdentity)?;
        if current.version != expected_version {
            return Err(ObjectiveRegistryError::StaleVersion);
        }
        let next_version = current.version.next()?;
        tracing::debug!(
            flag = identity.id,
            generation = identity.generation,
            version = next_version.get(),
            from = ?current.state,
            to = ?state,
            "objective flag transition",
        );
        current.version = next_version;
        current.state = state;
        if !matches!(state, FlagState::Carried { .. }) {
            self.flag_carried_since.remove(&identity.id);
        }
        Ok(*current)
    }

    pub(crate) fn transition_powerball(
        &mut self,
        identity: PowerballIdentity,
        expected_version: ObjectiveVersion,
        state: PowerballState,
    ) -> Result<PowerballSnapshot, ObjectiveRegistryError> {
        let current = self
            .powerballs
            .get_mut(&identity.id)
            .filter(|current| current.identity == identity)
            .ok_or(ObjectiveRegistryError::UnknownIdentity)?;
        if current.version != expected_version {
            return Err(ObjectiveRegistryError::StaleVersion);
        }
        let next_version = current.version.next()?;
        current.version = next_version;
        current.state = state;
        self.powerball_killer_pickup.remove(&identity.id);
        if !matches!(state, PowerballState::OnMap { .. }) {
            self.powerball_motion.remove(&identity.id);
        }
        if !matches!(state, PowerballState::Carried { .. }) {
            self.powerball_carried_since.remove(&identity.id);
        }
        Ok(*current)
    }

    pub(crate) fn replay(&self) -> ObjectiveReplay {
        ObjectiveReplay {
            generation: self.generation,
            flags: self.flags.values().copied().collect(),
            powerballs: self.powerballs.values().copied().collect(),
        }
    }

    pub(crate) fn flag_carrier_count(&self, carrier: PlayerRuntimeIdentity) -> u16 {
        self.flags
            .values()
            .filter(|flag| matches!(flag.state, FlagState::Carried { carrier: value, .. } if value == carrier))
            .count()
            .min(u16::MAX as usize) as u16
    }

    pub(crate) fn carried_powerball_ids(&self, carrier: PlayerRuntimeIdentity) -> Vec<u8> {
        self.powerballs
            .values()
            .filter_map(|ball| match ball.state {
                PowerballState::Carried { carrier: value } if value == carrier => {
                    Some(ball.identity.id)
                }
                _ => None,
            })
            .collect()
    }

    pub(crate) fn expire_powerball_pickup_locks(&mut self, tick: u32) {
        for ball in self.powerballs.values_mut() {
            let PowerballState::OnMap {
                x,
                y,
                velocity_x,
                velocity_y,
                pickup_unlock_tick: Some(deadline),
            } = ball.state
            else {
                continue;
            };
            if crate::continuum::continuum_tick_reached(tick, deadline) {
                ball.state = PowerballState::OnMap {
                    x,
                    y,
                    velocity_x,
                    velocity_y,
                    pickup_unlock_tick: None,
                };
                self.powerball_killer_pickup.remove(&ball.identity.id);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn force_flag_version(&mut self, id: u16, version: u32) -> bool {
        if version == 0 || version > OBJECTIVE_VERSION_MASK {
            return false;
        }
        let Some(flag) = self.flags.get_mut(&id) else {
            return false;
        };
        flag.version = ObjectiveVersion(version);
        true
    }

    #[cfg(test)]
    pub(crate) fn force_powerball_version(&mut self, id: u8, version: u32) -> bool {
        if version == 0 || version > OBJECTIVE_VERSION_MASK {
            return false;
        }
        let Some(ball) = self.powerballs.get_mut(&id) else {
            return false;
        };
        ball.version = ObjectiveVersion(version);
        true
    }

    #[cfg(test)]
    pub(crate) fn force_generation(&mut self, generation: u64) {
        self.generation = generation;
        for flag in self.flags.values_mut() {
            flag.identity.generation = generation;
        }
        for ball in self.powerballs.values_mut() {
            ball.identity.generation = generation;
        }
    }
}

fn initial_powerball_scores(profile: &ServerObjectiveProfile) -> BTreeMap<u16, i64> {
    let mut scores = BTreeMap::new();
    if let ServerPowerballProfile::Enabled(profile) = &profile.powerballs
        && profile.policy.capture_points >= 0
    {
        for frequency in 0..POWERBALL_SCORE_BUCKETS as u16 {
            scores.insert(frequency, i64::from(profile.policy.capture_points));
        }
    }
    scores
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ObjectiveActionWindow {
    latest: Option<u32>,
    mask: u64,
}

impl ObjectiveActionWindow {
    fn seen(self, action_id: u32) -> bool {
        let Some(latest) = self.latest else {
            return false;
        };
        let ahead = action_id.wrapping_sub(latest);
        if ahead != 0 && ahead < (1 << 31) {
            return false;
        }
        let behind = latest.wrapping_sub(action_id);
        behind >= u64::BITS || self.mask & (1_u64 << behind) != 0
    }

    fn remember(&mut self, action_id: u32) {
        let Some(latest) = self.latest else {
            self.latest = Some(action_id);
            self.mask = 1;
            return;
        };
        let ahead = action_id.wrapping_sub(latest);
        if ahead != 0 && ahead < (1 << 31) {
            self.mask = if ahead >= u64::BITS {
                0
            } else {
                self.mask << ahead
            };
            self.mask |= 1;
            self.latest = Some(action_id);
            return;
        }
        let behind = latest.wrapping_sub(action_id);
        if behind < u64::BITS {
            self.mask |= 1_u64 << behind;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectiveRegistryError {
    Disabled,
    Capacity,
    SpawnUnavailable,
    IdentityExhausted,
    GenerationExhausted,
    VersionExhausted,
    UnknownIdentity,
    StaleVersion,
}

impl std::fmt::Display for ObjectiveRegistryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Disabled => "objective profile is disabled",
            Self::Capacity => "objective profile capacity is exhausted",
            Self::SpawnUnavailable => "objective spawn area has no reachable empty tile",
            Self::IdentityExhausted => "objective identity space is exhausted",
            Self::GenerationExhausted => "objective generation space is exhausted",
            Self::VersionExhausted => "objective version space is exhausted",
            Self::UnknownIdentity => {
                "objective identity is absent or belongs to another generation"
            }
            Self::StaleVersion => "objective mutation names a stale version",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ObjectiveRegistryError {}

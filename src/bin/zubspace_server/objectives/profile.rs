use std::num::{NonZeroU8, NonZeroU16};

use super::super::limits::MAX_PLAYER_CLIENTS;

pub(crate) const MAX_SERVER_FLAGS: u16 = 256;
pub(crate) const MAX_SERVER_POWERBALLS: u8 = 8;
pub(crate) const MAX_OBJECTIVE_TILE_COORDINATE: u16 = 1023;
pub(crate) const MAX_OBJECTIVE_RADIUS_TILES: u16 = 1024;
pub(crate) const MIN_POWERBALL_SEND_CADENCE_TICKS: u16 = 25;
pub(crate) const MAX_POWERBALL_SEND_CADENCE_TICKS: u16 = 500;
pub(crate) const MAX_WRAP_SAFE_OBJECTIVE_DELAY_TICKS: u32 = (1 << 30) - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObjectiveSpawnArea {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) radius: u16,
}

impl ObjectiveSpawnArea {
    fn validate(self, objective: &str) -> Result<(), ObjectiveProfileError> {
        if self.x > MAX_OBJECTIVE_TILE_COORDINATE || self.y > MAX_OBJECTIVE_TILE_COORDINATE {
            return Err(ObjectiveProfileError::new(format!(
                "{objective} spawn ({}, {}) exceeds map coordinate maximum {MAX_OBJECTIVE_TILE_COORDINATE}",
                self.x, self.y
            )));
        }
        if self.radius > MAX_OBJECTIVE_RADIUS_TILES {
            return Err(ObjectiveProfileError::new(format!(
                "{objective} spawn radius {} exceeds maximum {MAX_OBJECTIVE_RADIUS_TILES}",
                self.radius
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ObjectiveTeamLimits {
    pub(crate) min_players: u16,
    pub(crate) min_teams: u16,
    pub(crate) max_players_per_team: u16,
}

impl ObjectiveTeamLimits {
    fn validate(self, objective: &str) -> Result<(), ObjectiveProfileError> {
        let maximum = MAX_PLAYER_CLIENTS as u16;
        for (field, value) in [
            ("min_players", self.min_players),
            ("min_teams", self.min_teams),
            ("max_players_per_team", self.max_players_per_team),
        ] {
            if value > maximum {
                return Err(ObjectiveProfileError::new(format!(
                    "{objective} {field} {value} exceeds player identity capacity {maximum}"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnabledFlagProfile {
    count: NonZeroU16,
    pub(crate) spawn: ObjectiveSpawnArea,
    pub(crate) carry_timeout_ticks: u32,
    pub(crate) reset_delay_ticks: u32,
    pub(crate) reward: i32,
    pub(crate) team_limits: ObjectiveTeamLimits,
    pub(crate) policy: FlagPolicy,
}

impl EnabledFlagProfile {
    pub(crate) fn count(&self) -> u16 {
        self.count.get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ServerFlagProfile {
    Disabled,
    Enabled(EnabledFlagProfile),
}

impl Default for ServerFlagProfile {
    fn default() -> Self {
        Self::Disabled
    }
}

impl ServerFlagProfile {
    pub(crate) fn validate(
        count: u16,
        settings: Option<FlagProfileSettings>,
    ) -> Result<Self, ObjectiveProfileError> {
        match (NonZeroU16::new(count), settings) {
            (None, None) => Ok(Self::Disabled),
            (None, Some(_)) => Err(ObjectiveProfileError::new(
                "disabled flag profile must not retain dormant settings",
            )),
            (Some(_), None) => Err(ObjectiveProfileError::new(
                "positive flag count requires complete server-only settings",
            )),
            (Some(count), Some(settings)) => {
                if count.get() > MAX_SERVER_FLAGS {
                    return Err(ObjectiveProfileError::new(format!(
                        "flag count {} exceeds maximum {MAX_SERVER_FLAGS}",
                        count.get()
                    )));
                }
                settings.spawn.validate("flag")?;
                validate_delay("flag carry timeout", settings.carry_timeout_ticks)?;
                validate_delay("flag reset delay", settings.reset_delay_ticks)?;
                settings.team_limits.validate("flag")?;
                settings.policy.validate()?;
                Ok(Self::Enabled(EnabledFlagProfile {
                    count,
                    spawn: settings.spawn,
                    carry_timeout_ticks: settings.carry_timeout_ticks,
                    reset_delay_ticks: settings.reset_delay_ticks,
                    reward: settings.reward,
                    team_limits: settings.team_limits,
                    policy: settings.policy,
                }))
            }
        }
    }

    pub(crate) fn count(&self) -> u16 {
        match self {
            Self::Disabled => 0,
            Self::Enabled(profile) => profile.count(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FlagProfileSettings {
    pub(crate) spawn: ObjectiveSpawnArea,
    pub(crate) carry_timeout_ticks: u32,
    pub(crate) reset_delay_ticks: u32,
    pub(crate) reward: i32,
    pub(crate) team_limits: ObjectiveTeamLimits,
    pub(crate) policy: FlagPolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlagWinMode {
    CarryAll,
    OwnAllDropped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlagRewardSplit {
    FullPerPlayer,
    SplitAmongTeam,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlagDropBehavior {
    NeutralAtCarrier,
    OwnedAtCarrier,
    NeutralAtCenter,
    OwnedAtCenter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FlagPolicy {
    /// The reference game's flag pickup uses each claimant ship's collision hull against
    /// the one-tile flag box. Values are synthetic until a shipped profile is sourced.
    pub(crate) collision_radius_pixels: [u16; 8],
    pub(crate) carry_limit: u16,
    pub(crate) win_mode: FlagWinMode,
    pub(crate) win_delay_ticks: u32,
    pub(crate) reward_split: FlagRewardSplit,
    pub(crate) drop_radius_tiles: u16,
    pub(crate) requested_drop: FlagDropBehavior,
    pub(crate) safe_drop: FlagDropBehavior,
    pub(crate) timeout_drop: FlagDropBehavior,
    pub(crate) death_drop: FlagDropBehavior,
    pub(crate) friendly_kill_drop: FlagDropBehavior,
    pub(crate) transfer_overflow_drop: FlagDropBehavior,
    pub(crate) ship_or_frequency_drop: FlagDropBehavior,
    pub(crate) disconnect_drop: FlagDropBehavior,
    pub(crate) transfer_on_enemy_kill: bool,
    pub(crate) transfer_on_friendly_kill: bool,
}

impl FlagPolicy {
    fn validate(self) -> Result<(), ObjectiveProfileError> {
        if self.collision_radius_pixels.contains(&0) {
            return Err(ObjectiveProfileError::new(
                "every ship flag collision radius must be positive",
            ));
        }
        if self.carry_limit == 0 || self.carry_limit > MAX_SERVER_FLAGS {
            return Err(ObjectiveProfileError::new(format!(
                "flag carry limit {} is outside 1..={MAX_SERVER_FLAGS}",
                self.carry_limit
            )));
        }
        if self.drop_radius_tiles > MAX_OBJECTIVE_RADIUS_TILES {
            return Err(ObjectiveProfileError::new(format!(
                "flag drop radius {} exceeds maximum {MAX_OBJECTIVE_RADIUS_TILES}",
                self.drop_radius_tiles
            )));
        }
        validate_delay("flag win delay", self.win_delay_ticks)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnabledPowerballProfile {
    count: NonZeroU8,
    pub(crate) spawns: Vec<ObjectiveSpawnArea>,
    pub(crate) send_cadence_ticks: u16,
    pub(crate) goal_delay_ticks: u32,
    pub(crate) reward: i32,
    pub(crate) win_by: u32,
    pub(crate) team_limits: ObjectiveTeamLimits,
    pub(crate) policy: PowerballPolicy,
}

impl EnabledPowerballProfile {
    pub(crate) fn count(&self) -> u8 {
        self.count.get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ServerPowerballProfile {
    Disabled,
    Enabled(EnabledPowerballProfile),
}

impl Default for ServerPowerballProfile {
    fn default() -> Self {
        Self::Disabled
    }
}

impl ServerPowerballProfile {
    pub(crate) fn validate(
        count: u8,
        settings: Option<PowerballProfileSettings>,
    ) -> Result<Self, ObjectiveProfileError> {
        match (NonZeroU8::new(count), settings) {
            (None, None) => Ok(Self::Disabled),
            (None, Some(_)) => Err(ObjectiveProfileError::new(
                "disabled powerball profile must not retain dormant settings",
            )),
            (Some(_), None) => Err(ObjectiveProfileError::new(
                "positive powerball count requires complete server-only settings",
            )),
            (Some(count), Some(settings)) => {
                if count.get() > MAX_SERVER_POWERBALLS {
                    return Err(ObjectiveProfileError::new(format!(
                        "powerball count {} exceeds maximum {MAX_SERVER_POWERBALLS}",
                        count.get()
                    )));
                }
                if settings.spawns.is_empty() || settings.spawns.len() > count.get() as usize {
                    return Err(ObjectiveProfileError::new(format!(
                        "powerball profile with count {} requires between 1 and {} spawn definitions",
                        count.get(),
                        count.get()
                    )));
                }
                for spawn in &settings.spawns {
                    spawn.validate("powerball")?;
                }
                if !(MIN_POWERBALL_SEND_CADENCE_TICKS..=MAX_POWERBALL_SEND_CADENCE_TICKS)
                    .contains(&settings.send_cadence_ticks)
                {
                    return Err(ObjectiveProfileError::new(format!(
                        "powerball send cadence {} is outside {MIN_POWERBALL_SEND_CADENCE_TICKS}..={MAX_POWERBALL_SEND_CADENCE_TICKS}",
                        settings.send_cadence_ticks
                    )));
                }
                validate_delay("powerball goal delay", settings.goal_delay_ticks)?;
                if settings.win_by > i32::MAX as u32 {
                    return Err(ObjectiveProfileError::new(format!(
                        "powerball win threshold {} exceeds signed scoring range",
                        settings.win_by
                    )));
                }
                settings.team_limits.validate("powerball")?;
                settings.policy.validate()?;
                Ok(Self::Enabled(EnabledPowerballProfile {
                    count,
                    spawns: settings.spawns,
                    send_cadence_ticks: settings.send_cadence_ticks,
                    goal_delay_ticks: settings.goal_delay_ticks,
                    reward: settings.reward,
                    win_by: settings.win_by,
                    team_limits: settings.team_limits,
                    policy: settings.policy,
                }))
            }
        }
    }

    pub(crate) fn count(&self) -> u8 {
        match self {
            Self::Disabled => 0,
            Self::Enabled(profile) => profile.count(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PowerballProfileSettings {
    pub(crate) spawns: Vec<ObjectiveSpawnArea>,
    pub(crate) send_cadence_ticks: u16,
    pub(crate) goal_delay_ticks: u32,
    pub(crate) reward: i32,
    pub(crate) win_by: u32,
    pub(crate) team_limits: ObjectiveTeamLimits,
    pub(crate) policy: PowerballPolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PowerballPolicy {
    pub(crate) pickup_radius_pixels: [u16; 8],
    pub(crate) pass_delay_ticks: u32,
    pub(crate) carry_timeout_ticks: [u32; 8],
    pub(crate) throw_speed: [i16; 8],
    pub(crate) friction_delta: [i16; 8],
    pub(crate) require_line_of_sight: bool,
    pub(crate) bounce: bool,
    pub(crate) soccer_mode: u8,
    pub(crate) capture_points: i32,
    pub(crate) death_scores_goal: bool,
    pub(crate) killer_ignore_pass_delay_ticks: u32,
    pub(crate) new_game_delay_ticks: i32,
}

impl PowerballPolicy {
    fn validate(self) -> Result<(), ObjectiveProfileError> {
        if self.pickup_radius_pixels.contains(&0) {
            return Err(ObjectiveProfileError::new(
                "every ship powerball pickup radius must be positive",
            ));
        }
        validate_delay("powerball pass delay", self.pass_delay_ticks)?;
        for timeout in self.carry_timeout_ticks {
            validate_delay("powerball carry timeout", timeout)?;
        }
        validate_delay(
            "powerball killer pass-delay trim",
            self.killer_ignore_pass_delay_ticks,
        )?;
        validate_signed_delay("powerball new-game delay", self.new_game_delay_ticks)?;
        if !self.bounce {
            return Err(ObjectiveProfileError::new(
                "the enabled powerball engine currently requires BallBounce=true",
            ));
        }
        if self.soccer_mode > 6 {
            return Err(ObjectiveProfileError::new(format!(
                "powerball soccer mode {} is outside 0..=6",
                self.soccer_mode
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServerObjectiveProfile {
    pub(crate) flags: ServerFlagProfile,
    pub(crate) powerballs: ServerPowerballProfile,
}

impl ServerObjectiveProfile {
    pub(crate) const fn disabled() -> Self {
        Self {
            flags: ServerFlagProfile::Disabled,
            powerballs: ServerPowerballProfile::Disabled,
        }
    }

    pub(crate) fn is_disabled(&self) -> bool {
        self.flags.count() == 0 && self.powerballs.count() == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObjectiveProfileError(String);

impl ObjectiveProfileError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for ObjectiveProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for ObjectiveProfileError {}

fn validate_delay(name: &str, ticks: u32) -> Result<(), ObjectiveProfileError> {
    if ticks > MAX_WRAP_SAFE_OBJECTIVE_DELAY_TICKS {
        return Err(ObjectiveProfileError::new(format!(
            "{name} {ticks} exceeds unambiguous wrap-safe maximum {MAX_WRAP_SAFE_OBJECTIVE_DELAY_TICKS}"
        )));
    }
    Ok(())
}

fn validate_signed_delay(name: &str, ticks: i32) -> Result<(), ObjectiveProfileError> {
    let magnitude = ticks.unsigned_abs();
    validate_delay(name, magnitude)
}

#[cfg(test)]
pub(crate) fn synthetic_profile(flag_count: u16, powerball_count: u8) -> ServerObjectiveProfile {
    let flags = ServerFlagProfile::validate(
        flag_count,
        (flag_count > 0).then_some(FlagProfileSettings {
            spawn: ObjectiveSpawnArea {
                x: 1,
                y: 1,
                radius: 0,
            },
            carry_timeout_ticks: 1,
            reset_delay_ticks: 1,
            reward: 0,
            team_limits: ObjectiveTeamLimits::default(),
            policy: synthetic_flag_policy(),
        }),
    )
    .expect("synthetic flag profile must be valid");
    let powerballs = ServerPowerballProfile::validate(
        powerball_count,
        (powerball_count > 0).then_some(PowerballProfileSettings {
            spawns: vec![ObjectiveSpawnArea {
                x: 1,
                y: 1,
                radius: 0,
            }],
            send_cadence_ticks: MIN_POWERBALL_SEND_CADENCE_TICKS,
            goal_delay_ticks: 0,
            reward: 0,
            win_by: 0,
            team_limits: ObjectiveTeamLimits::default(),
            policy: synthetic_powerball_policy(),
        }),
    )
    .expect("synthetic powerball profile must be valid");
    ServerObjectiveProfile { flags, powerballs }
}

#[cfg(test)]
pub(crate) const fn synthetic_flag_policy() -> FlagPolicy {
    FlagPolicy {
        collision_radius_pixels: [10; 8],
        carry_limit: MAX_SERVER_FLAGS,
        win_mode: FlagWinMode::CarryAll,
        win_delay_ticks: 1,
        reward_split: FlagRewardSplit::FullPerPlayer,
        drop_radius_tiles: 0,
        requested_drop: FlagDropBehavior::NeutralAtCarrier,
        safe_drop: FlagDropBehavior::NeutralAtCenter,
        timeout_drop: FlagDropBehavior::NeutralAtCarrier,
        death_drop: FlagDropBehavior::NeutralAtCarrier,
        friendly_kill_drop: FlagDropBehavior::NeutralAtCarrier,
        transfer_overflow_drop: FlagDropBehavior::NeutralAtCarrier,
        ship_or_frequency_drop: FlagDropBehavior::NeutralAtCenter,
        disconnect_drop: FlagDropBehavior::NeutralAtCenter,
        transfer_on_enemy_kill: true,
        transfer_on_friendly_kill: false,
    }
}

#[cfg(test)]
pub(crate) const fn synthetic_powerball_policy() -> PowerballPolicy {
    PowerballPolicy {
        pickup_radius_pixels: [18; 8],
        pass_delay_ticks: 5,
        carry_timeout_ticks: [100; 8],
        throw_speed: [160; 8],
        friction_delta: [1_000; 8],
        require_line_of_sight: true,
        bounce: true,
        soccer_mode: 1,
        capture_points: -1,
        death_scores_goal: false,
        killer_ignore_pass_delay_ticks: 0,
        new_game_delay_ticks: 1,
    }
}

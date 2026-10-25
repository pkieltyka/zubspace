//! Continuum arena-settings packet builder for checked-in Subgame INI profiles.
//!
//! The field order and bit widths follow the reference game's `ArenaSettings`, ASSS
//! `clientset.def`, and SubspaceServer `S2C_ClientSettings.cs`. Keeping the
//! source INI beside each bundled zone makes the values auditable instead of
//! copying another zone's packet or transcribing a second, easily-divergent
//! Rust table.

use std::collections::BTreeMap;

use crate::objectives::{
    FlagDropBehavior, FlagPolicy, FlagProfileSettings, FlagRewardSplit, FlagWinMode,
    MAX_SERVER_FLAGS, ObjectiveSpawnArea, ObjectiveTeamLimits, ServerFlagProfile,
    ServerObjectiveProfile,
};
use crate::wire::{put_i16, put_i32, put_u16, put_u32};

use super::{
    ARENA_SETTINGS, CONTINUUM_ARENA_SETTINGS_SIZE, CONTINUUM_BYTE_SETTINGS_OFFSET,
    CONTINUUM_GLOBAL_INTS_OFFSET, CONTINUUM_PRIZE_WEIGHT_COUNT, CONTINUUM_PRIZE_WEIGHTS_OFFSET,
    CONTINUUM_SHIP_SETTINGS_OFFSET, CONTINUUM_SHIP_SETTINGS_SIZE, CONTINUUM_SHORT_SETTINGS_OFFSET,
    CONTINUUM_SPAWN_SETTINGS_OFFSET,
};

const SHIP_SECTIONS: [&str; 8] = [
    "warbird",
    "javelin",
    "spider",
    "leviathan",
    "terrier",
    "weasel",
    "lancaster",
    "shark",
];

#[derive(Debug)]
struct IniSettings {
    values: BTreeMap<(String, String), i64>,
}

impl IniSettings {
    fn parse(text: &str) -> Result<Self, String> {
        let mut section = String::new();
        let mut values = BTreeMap::new();
        for raw_line in text.lines() {
            let line = raw_line
                .split_once(';')
                .map_or(raw_line, |(value, _)| value)
                .trim();
            if line.is_empty() || line.starts_with("#include") {
                continue;
            }
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
            {
                section = normalize(name);
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().parse::<i64>().map_err(|error| {
                format!("invalid numeric arena setting {section}:{key}={value}: {error}")
            })?;
            values.insert((section.clone(), normalize(key)), value);
        }
        Ok(Self { values })
    }

    fn optional(&self, section: &str, key: &str) -> Option<i64> {
        self.values
            .get(&(normalize(section), normalize(key)))
            .copied()
    }

    /// True when the INI authors any key under `section` besides `except`.
    fn authors_any_besides(&self, section: &str, except: &[&str]) -> bool {
        let section = normalize(section);
        self.values.keys().any(|(key_section, key)| {
            *key_section == section && !except.iter().any(|name| normalize(name) == *key)
        })
    }

    fn required(&self, section: &str, key: &str) -> Result<i64, String> {
        self.values
            .get(&(normalize(section), normalize(key)))
            .copied()
            .ok_or_else(|| format!("missing arena setting {section}:{key}"))
    }

    fn ship_required(&self, section: &str, key: &str) -> Result<i64, String> {
        self.values
            .get(&(normalize(section), normalize(key)))
            .or_else(|| self.values.get(&("allships".into(), normalize(key))))
            .or_else(|| self.values.get(&("all".into(), normalize(key))))
            .copied()
            .ok_or_else(|| format!("missing arena ship setting {section}:{key}"))
    }

    fn required_range(&self, section: &str, key: &str, min: i64, max: i64) -> Result<i64, String> {
        let value = self.required(section, key)?;
        if !(min..=max).contains(&value) {
            return Err(format!(
                "arena setting {section}:{key} must be between {min} and {max}, got {value}"
            ));
        }
        Ok(value)
    }

    fn ship_required_range(
        &self,
        section: &str,
        key: &str,
        min: i64,
        max: i64,
    ) -> Result<i64, String> {
        let value = self.ship_required(section, key)?;
        if !(min..=max).contains(&value) {
            return Err(format!(
                "arena ship setting {section}:{key} must be between {min} and {max}, got {value}"
            ));
        }
        Ok(value)
    }
}

fn normalize(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| !matches!(ch, ' ' | '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect()
}

pub(super) fn build_arena_settings_packet(text: &str) -> Result<Vec<u8>, String> {
    let settings = IniSettings::parse(text)?;
    let mut data = vec![0; CONTINUUM_ARENA_SETTINGS_SIZE];

    let mut header = 3_u32;
    header |= (settings.required_range("Bullet", "ExactDamage", 0, 1)? as u32) << 8;
    header |= (settings.required_range("Spectator", "HideFlags", 0, 1)? as u32) << 9;
    header |= (settings.required_range("Spectator", "NoXRadar", 0, 1)? as u32) << 10;
    header |= (settings.required_range("Misc", "SlowFrameCheck", 0, 7)? as u32) << 11;
    header |= (settings.required_range("Misc", "DisableScreenshot", 0, 1)? as u32) << 14;
    header |= (settings.required_range("Misc", "MaxTimerDrift", 0, 7)? as u32) << 16;
    header |= (settings.required_range("Soccer", "DisableWallPass", 0, 1)? as u32) << 19;
    header |= (settings.required_range("Soccer", "DisableBallKilling", 0, 1)? as u32) << 20;
    put_u32(&mut data, 0, header);

    for (ship_index, ship) in SHIP_SECTIONS.iter().enumerate() {
        write_ship(
            &mut data,
            CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE,
            &settings,
            ship,
        )?;
    }

    let global_ints = [
        ("Bullet", "BulletDamageLevel"),
        ("Bomb", "BombDamageLevel"),
        ("Bullet", "BulletAliveTime"),
        ("Bomb", "BombAliveTime"),
        ("Misc", "DecoyAliveTime"),
        ("Misc", "SafetyLimit"),
        ("Misc", "FrequencyShift"),
        ("Team", "MaxFrequency"),
        ("Repel", "RepelSpeed"),
        ("Mine", "MineAliveTime"),
        ("Burst", "BurstDamageLevel"),
        ("Bullet", "BulletDamageUpgrade"),
        ("Flag", "FlagDropDelay"),
        ("Flag", "EnterGameFlaggingDelay"),
        ("Rocket", "RocketThrust"),
        ("Rocket", "RocketSpeed"),
        ("Shrapnel", "InactiveShrapDamage"),
        ("Wormhole", "SwitchTime"),
        ("Misc", "ActivateAppShutdownTime"),
        ("Shrapnel", "ShrapnelSpeed"),
    ];
    for (index, (section, key)) in global_ints.iter().enumerate() {
        put_i32(
            &mut data,
            CONTINUUM_GLOBAL_INTS_OFFSET + index * 4,
            settings.required_range(section, key, i32::MIN as i64, i32::MAX as i64)? as i32,
        );
    }

    for team in 0..4 {
        let x = settings.required_range("Spawn", &format!("Team{team}-X"), -512, 511)? as i32;
        let y = settings.required_range("Spawn", &format!("Team{team}-Y"), -512, 511)? as i32;
        let radius =
            settings.required_range("Spawn", &format!("Team{team}-Radius"), 0, 0x1ff)? as u32;
        let packed = (x as u32 & 0x3ff) | ((y as u32 & 0x3ff) << 10) | ((radius & 0x1ff) << 20);
        put_u32(
            &mut data,
            CONTINUUM_SPAWN_SETTINGS_OFFSET + team * 4,
            packed,
        );
    }

    let shorts = [
        ("Latency", "SendRoutePercent"),
        ("Bomb", "BombExplodeDelay"),
        ("Misc", "SendPositionDelay"),
        ("Bomb", "BombExplodePixels"),
        ("Prize", "DeathPrizeTime"),
        ("Bomb", "JitterTime"),
        ("Kill", "EnterDelay"),
        ("Prize", "EngineShutdownTime"),
        ("Bomb", "ProximityDistance"),
        ("Kill", "BountyIncreaseForKill"),
        ("Misc", "BounceFactor"),
        ("Radar", "MapZoomFactor"),
        ("Kill", "MaxBonus"),
        ("Kill", "MaxPenalty"),
        ("Kill", "RewardBase"),
        ("Repel", "RepelTime"),
        ("Repel", "RepelDistance"),
        ("Misc", "TickerDelay"),
        ("Flag", "FlaggerOnRadar"),
        ("Flag", "FlaggerKillMultiplier"),
        ("Prize", "PrizeFactor"),
        ("Prize", "PrizeDelay"),
        ("Prize", "MinimumVirtual"),
        ("Prize", "UpgradeVirtual"),
        ("Prize", "PrizeMaxExist"),
        ("Prize", "PrizeMinExist"),
        ("Prize", "PrizeNegativeFactor"),
        ("Door", "DoorDelay"),
        ("Toggle", "AntiWarpPixels"),
        ("Door", "DoorMode"),
        ("Flag", "FlagBlankDelay"),
        ("Flag", "NoDataFlagDropDelay"),
        ("Prize", "MultiPrizeCount"),
        ("Brick", "BrickTime"),
        ("Misc", "WarpRadiusLimit"),
        ("Bomb", "EBombShutdownTime"),
        ("Bomb", "EBombDamagePercent"),
        ("Radar", "RadarNeutralSize"),
        ("Misc", "WarpPointDelay"),
        ("Misc", "NearDeathLevel"),
        ("Bomb", "BBombDamagePercent"),
        ("Shrapnel", "ShrapnelDamagePercent"),
        ("Latency", "ClientSlowPacketTime"),
        ("Flag", "FlagDropResetReward"),
        ("Flag", "FlaggerFireCostPercent"),
        ("Flag", "FlaggerDamagePercent"),
        ("Flag", "FlaggerBombFireDelay"),
        ("Soccer", "PassDelay"),
        ("Soccer", "BallBlankDelay"),
        ("Latency", "S2CNoDataKickoutDelay"),
        ("Flag", "FlaggerThrustAdjustment"),
        ("Flag", "FlaggerSpeedAdjustment"),
        ("Latency", "ClientSlowPacketSampleSize"),
    ];
    for (index, (section, key)) in shorts.iter().enumerate() {
        put_i16(
            &mut data,
            CONTINUUM_SHORT_SETTINGS_OFFSET + index * 2,
            settings.required_range(section, key, i16::MIN as i64, i16::MAX as i64)? as i16,
        );
    }

    let bytes = [
        ("Shrapnel", "Random"),
        ("Soccer", "BallBounce"),
        ("Soccer", "AllowBombs"),
        ("Soccer", "AllowGuns"),
        ("Soccer", "Mode"),
        ("Team", "MaxPerTeam"),
        ("Team", "MaxPerPrivateTeam"),
        ("Mine", "TeamMaxMines"),
        ("Wormhole", "GravityBombs"),
        ("Bomb", "BombSafety"),
        ("Message", "MessageReliable"),
        ("Prize", "TakePrizeReliable"),
        ("Message", "AllowAudioMessages"),
        ("Prize", "PrizeHideCount"),
        ("Misc", "ExtraPositionData"),
        ("Misc", "SlowFrameCheck"),
        ("Flag", "CarryFlags"),
        ("Misc", "AllowSavedShips"),
        ("Radar", "RadarMode"),
        ("Misc", "VictoryMusic"),
        ("Flag", "FlaggerGunUpgrade"),
        ("Flag", "FlaggerBombUpgrade"),
        ("Soccer", "UseFlagger"),
        ("Soccer", "BallLocation"),
        ("Misc", "AntiWarpSettleDelay"),
    ];
    for (index, (section, key)) in bytes.iter().enumerate() {
        data[CONTINUUM_BYTE_SETTINGS_OFFSET + index] =
            settings.required_range(section, key, 0, u8::MAX as i64)? as u8;
    }

    // This is protocol prize-id order, as defined by ASSS clientset.def.
    let prize_weights: [&str; CONTINUUM_PRIZE_WEIGHT_COUNT] = [
        "QuickCharge",
        "Energy",
        "Rotation",
        "Stealth",
        "Cloak",
        "XRadar",
        "Warp",
        "Gun",
        "Bomb",
        "BouncingBullets",
        "Thruster",
        "TopSpeed",
        "Recharge",
        "Glue",
        "MultiFire",
        "Proximity",
        "AllWeapons",
        "Shields",
        "Shrapnel",
        "AntiWarp",
        "Repel",
        "Burst",
        "Decoy",
        "Thor",
        "MultiPrize",
        "Brick",
        "Rocket",
        "Portal",
    ];
    for (index, key) in prize_weights.iter().enumerate() {
        data[CONTINUUM_PRIZE_WEIGHTS_OFFSET + index] =
            settings.required_range("PrizeWeight", key, 0, u8::MAX as i64)? as u8;
    }

    data[0] = ARENA_SETTINGS;
    Ok(data)
}

/// Server-side `Brick:BrickSpan` is not part of Continuum's 1428-byte
/// arena-settings packet. Keep it in the same authored per-zone INI, but
/// parse it independently rather than aliasing the packet's byte slot 7
/// (`Mine:TeamMaxMines`).
pub(super) fn server_brick_span(text: &str) -> Result<u8, String> {
    let settings = IniSettings::parse(text)?;
    Ok(settings.required_range("Brick", "BrickSpan", 1, u8::MAX as i64)? as u8)
}

/// Server-only `[Objective]` section: the authoritative objectives a zone runs.
/// Continuum's 1428-byte arena-settings packet carries no objective spawn
/// geometry, so these keys live in the same authored per-zone INI but never
/// enter the packet, exactly like `Brick:BrickSpan`. A zone without the section
/// keeps the disabled profile, so authored map flag tiles alone can never start
/// an objective game. An authored section that states `Flags = 0` is the
/// documented off-switch and also keeps the disabled profile: the section stays
/// loadable and its remaining rules stay inert.
///
/// Every key is required once `Flags` is positive: the enabled engine consumes
/// these values as gameplay rules, so they must be auditable in the INI rather
/// than defaulted in Rust. A section that authors objective rules without ever
/// stating `Flags` is rejected instead of silently disabling, because that INI
/// reads as an objective game while running none.
pub(super) fn server_objective_profile(
    text: &str,
    flag_collision_radii: [u16; 8],
) -> Result<ServerObjectiveProfile, String> {
    let settings = IniSettings::parse(text)?;
    let flag_count = match settings.optional("Objective", "Flags") {
        Some(count) if count > 0 => count,
        Some(_) => return Ok(ServerObjectiveProfile::disabled()),
        None => {
            if settings.authors_any_besides("Objective", &["Flags"]) {
                return Err(
                    "an [Objective] section must state Objective:Flags alongside its rules"
                        .to_string(),
                );
            }
            return Ok(ServerObjectiveProfile::disabled());
        }
    };
    let flag_settings = FlagProfileSettings {
        spawn: ObjectiveSpawnArea {
            x: settings.required_tile("Objective", "FlagSpawnX")?,
            y: settings.required_tile("Objective", "FlagSpawnY")?,
            radius: settings.required_tile("Objective", "FlagSpawnRadius")?,
        },
        // 0 means "no carry-expiry rule", which the engine honours by never
        // timing a carried flag out (`step_objective_deadlines`).
        carry_timeout_ticks: settings.required_ticks("Objective", "FlagCarryTimeoutTicks")?,
        reset_delay_ticks: settings.required_ticks("Objective", "FlagResetDelayTicks")?,
        reward: settings.required_range(
            "Objective",
            "FlagReward",
            i64::from(i32::MIN),
            i64::from(i32::MAX),
        )? as i32,
        team_limits: ObjectiveTeamLimits {
            min_players: settings.required_capacity("Objective", "FlagMinPlayers")?,
            min_teams: settings.required_capacity("Objective", "FlagMinTeams")?,
            max_players_per_team: settings
                .required_capacity("Objective", "FlagMaxPlayersPerTeam")?,
        },
        policy: FlagPolicy {
            // The reference game's pickup is hull-against-flag-box, so the radii come from
            // this zone's own per-ship settings rather than a second table.
            collision_radius_pixels: flag_collision_radii,
            // The reference game's `CarryFlags` 0 means "no per-player limit"; the engine
            // models that as the whole-flag capacity.
            carry_limit: match settings.optional("Flag", "CarryFlags").unwrap_or(0) {
                0 => MAX_SERVER_FLAGS,
                limit => u16::try_from(limit)
                    .map_err(|_| format!("Flag:CarryFlags = {limit} does not fit u16"))?,
            },
            win_mode: match settings.required_range("Objective", "FlagWinMode", 0, 1)? {
                0 => FlagWinMode::CarryAll,
                _ => FlagWinMode::OwnAllDropped,
            },
            win_delay_ticks: settings.required_ticks("Objective", "FlagWinDelayTicks")?,
            reward_split: match settings.required_range("Objective", "FlagRewardSplit", 0, 1)? {
                0 => FlagRewardSplit::FullPerPlayer,
                _ => FlagRewardSplit::SplitAmongTeam,
            },
            drop_radius_tiles: settings.required_tile("Objective", "FlagDropRadiusTiles")?,
            requested_drop: settings.required_drop_behavior("Objective", "FlagRequestedDrop")?,
            safe_drop: settings.required_drop_behavior("Objective", "FlagSafeDrop")?,
            timeout_drop: settings.required_drop_behavior("Objective", "FlagTimeoutDrop")?,
            death_drop: settings.required_drop_behavior("Objective", "FlagDeathDrop")?,
            friendly_kill_drop: settings
                .required_drop_behavior("Objective", "FlagFriendlyKillDrop")?,
            transfer_overflow_drop: settings
                .required_drop_behavior("Objective", "FlagTransferOverflowDrop")?,
            ship_or_frequency_drop: settings
                .required_drop_behavior("Objective", "FlagShipOrFrequencyDrop")?,
            disconnect_drop: settings.required_drop_behavior("Objective", "FlagDisconnectDrop")?,
            transfer_on_enemy_kill: settings
                .required_bool("Objective", "FlagTransferOnEnemyKill")?,
            transfer_on_friendly_kill: settings
                .required_bool("Objective", "FlagTransferOnFriendlyKill")?,
        },
    };
    let flags = ServerFlagProfile::validate(
        u16::try_from(flag_count)
            .map_err(|_| format!("Objective:Flags = {flag_count} does not fit u16"))?,
        Some(flag_settings),
    )
    .map_err(|error| format!("Objective profile is invalid: {error}"))?;
    Ok(ServerObjectiveProfile {
        flags,
        // Powerballs stay disabled until `[Objective]` can derive them from
        // the zone's authored `[Soccer]` values.
        powerballs: crate::objectives::ServerPowerballProfile::Disabled,
    })
}

impl IniSettings {
    fn required_tile(&self, section: &str, key: &str) -> Result<u16, String> {
        self.required_integer_in(section, key, 1_024)
    }

    fn required_capacity(&self, section: &str, key: &str) -> Result<u16, String> {
        self.required_integer_in(section, key, crate::limits::MAX_PLAYER_CLIENTS as i64)
    }

    fn required_ticks(&self, section: &str, key: &str) -> Result<u32, String> {
        let value = self.required_range(
            section,
            key,
            0,
            i64::from(crate::objectives::MAX_WRAP_SAFE_OBJECTIVE_DELAY_TICKS),
        )?;
        u32::try_from(value)
            .map_err(|_| format!("arena setting {section}:{key} = {value} is too large"))
    }

    fn required_bool(&self, section: &str, key: &str) -> Result<bool, String> {
        Ok(self.required_range(section, key, 0, 1)? == 1)
    }

    fn required_drop_behavior(&self, section: &str, key: &str) -> Result<FlagDropBehavior, String> {
        Ok(match self.required_range(section, key, 0, 3)? {
            0 => FlagDropBehavior::NeutralAtCarrier,
            1 => FlagDropBehavior::OwnedAtCarrier,
            2 => FlagDropBehavior::NeutralAtCenter,
            _ => FlagDropBehavior::OwnedAtCenter,
        })
    }

    fn required_integer_in(&self, section: &str, key: &str, max: i64) -> Result<u16, String> {
        let value = self.required_range(section, key, 0, max)?;
        u16::try_from(value)
            .map_err(|_| format!("arena setting {section}:{key} = {value} does not fit u16"))
    }
}

fn write_ship(
    data: &mut [u8],
    offset: usize,
    settings: &IniSettings,
    ship: &str,
) -> Result<(), String> {
    let u32_fields = [(0, "SuperTime"), (4, "ShieldsTime")];
    for (field_offset, key) in u32_fields {
        put_u32(
            data,
            offset + field_offset,
            settings.ship_required_range(ship, key, 0, u32::MAX as i64)? as u32,
        );
    }

    for (field_offset, key) in [
        (8, "Gravity"),
        (10, "GravityTopSpeed"),
        (76, "TurretThrustPenalty"),
        (78, "TurretSpeedPenalty"),
    ] {
        put_i16(
            data,
            offset + field_offset,
            settings.ship_required_range(ship, key, i16::MIN as i64, i16::MAX as i64)? as i16,
        );
    }

    let u16_fields = [
        (12, "BulletFireEnergy"),
        (14, "MultiFireEnergy"),
        (16, "BombFireEnergy"),
        (18, "BombFireEnergyUpgrade"),
        (20, "LandmineFireEnergy"),
        (22, "LandmineFireEnergyUpgrade"),
        (24, "BulletSpeed"),
        (26, "BombSpeed"),
        (30, "MultiFireAngle"),
        (32, "CloakEnergy"),
        (34, "StealthEnergy"),
        (36, "AntiWarpEnergy"),
        (38, "XRadarEnergy"),
        (40, "MaximumRotation"),
        (42, "MaximumThrust"),
        (44, "MaximumSpeed"),
        (46, "MaximumRecharge"),
        (48, "MaximumEnergy"),
        (50, "InitialRotation"),
        (52, "InitialThrust"),
        (54, "InitialSpeed"),
        (56, "InitialRecharge"),
        (58, "InitialEnergy"),
        (60, "UpgradeRotation"),
        (62, "UpgradeThrust"),
        (64, "UpgradeSpeed"),
        (66, "UpgradeRecharge"),
        (68, "UpgradeEnergy"),
        (70, "AfterburnerEnergy"),
        (72, "BombThrust"),
        (74, "BurstSpeed"),
        (80, "BulletFireDelay"),
        (82, "MultiFireDelay"),
        (84, "BombFireDelay"),
        (86, "LandmineFireDelay"),
        (88, "RocketTime"),
        (90, "InitialBounty"),
        (92, "DamageFactor"),
        (94, "PrizeShareLimit"),
        (96, "AttachBounty"),
        (98, "SoccerThrowTime"),
        (100, "SoccerBallFriction"),
        (102, "SoccerBallProximity"),
        (104, "SoccerBallSpeed"),
    ];
    for (field_offset, key) in u16_fields {
        put_u16(
            data,
            offset + field_offset,
            settings.ship_required_range(ship, key, 0, u16::MAX as i64)? as u16,
        );
    }

    // The Continuum Radius field is an eight-bit pixel count. ASSS and
    // SubspaceServer both copy the config value directly into this bitfield.
    let radius = settings.ship_required_range(ship, "Radius", 0, u8::MAX as i64)? as u16;
    let visibility = (settings.ship_required_range(ship, "SeeBombLevel", 0, 3)? as u16)
        | ((settings.ship_required_range(ship, "DisableFastShooting", 0, 1)? as u16) << 2)
        | ((radius & 0xff) << 3);
    put_u16(data, offset + 28, visibility);

    let byte_fields = [
        (106, "TurretLimit"),
        (107, "BurstShrapnel"),
        (108, "MaxMines"),
        (109, "RepelMax"),
        (110, "BurstMax"),
        (111, "DecoyMax"),
        (112, "ThorMax"),
        (113, "BrickMax"),
        (114, "RocketMax"),
        (115, "PortalMax"),
        (116, "InitialRepel"),
        (117, "InitialBurst"),
        (118, "InitialBrick"),
        (119, "InitialRocket"),
        (120, "InitialThor"),
        (121, "InitialDecoy"),
        (122, "InitialPortal"),
        (123, "BombBounceCount"),
    ];
    for (field_offset, key) in byte_fields {
        data[offset + field_offset] =
            settings.ship_required_range(ship, key, 0, u8::MAX as i64)? as u8;
    }

    let capability = (settings.ship_required_range(ship, "ShrapnelMax", 0, 0x1f)? as u32)
        | ((settings.ship_required_range(ship, "ShrapnelRate", 0, 0x1f)? as u32) << 5)
        | ((settings.ship_required_range(ship, "CloakStatus", 0, 3)? as u32) << 10)
        | ((settings.ship_required_range(ship, "StealthStatus", 0, 3)? as u32) << 12)
        | ((settings.ship_required_range(ship, "XRadarStatus", 0, 3)? as u32) << 14)
        | ((settings.ship_required_range(ship, "AntiWarpStatus", 0, 3)? as u32) << 16)
        | ((settings.ship_required_range(ship, "InitialGuns", 0, 3)? as u32) << 18)
        | ((settings.ship_required_range(ship, "MaxGuns", 0, 3)? as u32) << 20)
        | ((settings.ship_required_range(ship, "InitialBombs", 0, 3)? as u32) << 22)
        | ((settings.ship_required_range(ship, "MaxBombs", 0, 3)? as u32) << 24)
        | ((settings.ship_required_range(ship, "DoubleBarrel", 0, 1)? as u32) << 26)
        | ((settings.ship_required_range(ship, "EmpBomb", 0, 1)? as u32) << 27)
        | ((settings.ship_required_range(ship, "SeeMines", 0, 1)? as u32) << 28);
    put_u32(data, offset + 124, capability);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_strips_comments_and_normalizes_names() {
        let settings = IniSettings::parse("[Team]\nMax-Frequency = 100 ; source note\n").unwrap();
        assert_eq!(settings.required("team", "max_frequency").unwrap(), 100);
    }

    #[test]
    fn checked_in_profiles_are_complete_and_build_without_defaults() {
        // Key counts are pinned per profile so an authored edit is a deliberate
        // change; the packet size proves no builder-side default filled a gap.
        for (text, keys) in [(super::super::EXTREME_ARENA_SETTINGS_TEXT, 819)] {
            let settings = IniSettings::parse(text).unwrap();
            assert_eq!(settings.values.len(), keys);
            assert_eq!(build_arena_settings_packet(text).unwrap().len(), 1428);
        }
    }

    #[test]
    fn chaos_authored_ini_builds_the_complete_production_packet_without_defaults() {
        let authored = build_arena_settings_packet(super::super::CHAOS_ARENA_SETTINGS_TEXT)
            .expect("Chaos must remain editable through the generic disk-profile loader");
        let production = super::super::chaos::build_arena_settings_packet();
        let differences = authored
            .iter()
            .zip(&production)
            .enumerate()
            .filter_map(|(offset, (actual, expected))| {
                (actual != expected).then_some((offset, *actual, *expected))
            })
            .collect::<Vec<_>>();
        assert!(
            differences.is_empty(),
            "generic/production packet byte differences: {differences:?}"
        );
    }

    #[test]
    fn server_brick_span_is_independent_from_packet_team_max_mines() {
        assert_eq!(
            server_brick_span("[Brick]\nBrickSpan=10\n[Mine]\nTeamMaxMines=231\n").unwrap(),
            10
        );
    }

    #[test]
    fn authored_zero_flag_count_is_the_documented_objective_off_switch() {
        // The arena INI documents `Flags = 0` as "runs no objectives". That
        // has to stay a loadable zone: an off switch that instead fails the
        // profile makes the embedded zone panic at boot.
        let text = "[Objective]\nFlags=0\nFlagSpawnX=512\nFlagSpawnY=512\nFlagWinMode=1\n";
        assert!(
            server_objective_profile(text, [16; 8])
                .expect("Flags=0 must disable objectives, not fail the zone")
                .is_disabled()
        );
    }

    #[test]
    fn objective_rules_without_a_flag_count_are_rejected_not_silently_disabled() {
        let error =
            server_objective_profile("[Objective]\nFlagSpawnY=512\nFlagWinMode=1\n", [16; 8])
                .expect_err("an authored [Objective] section must state Objective:Flags");
        assert!(
            error.contains("must state Objective:Flags"),
            "unexpected rejection: {error}"
        );
    }

    #[test]
    fn absent_objective_section_keeps_the_disabled_profile() {
        assert!(
            server_objective_profile("[Team]\nMaxPlayers=32\n", [16; 8])
                .expect("a zone without the section is valid")
                .is_disabled()
        );
    }

    #[test]
    fn ship_radius_uses_the_full_raw_wire_byte() {
        let text = super::super::EXTREME_ARENA_SETTINGS_TEXT.replacen("Radius=0", "Radius=255", 1);
        let packet = build_arena_settings_packet(&text).unwrap();
        let visibility = u16::from_le_bytes(
            packet[CONTINUUM_SHIP_SETTINGS_OFFSET + 28..CONTINUUM_SHIP_SETTINGS_OFFSET + 30]
                .try_into()
                .unwrap(),
        );

        assert_eq!((visibility >> 3) & 0xff, 255);
    }

    #[test]
    fn packet_builder_does_not_silently_default_missing_profile_values() {
        assert!(build_arena_settings_packet("[Bullet]\nExactDamage=1\n").is_err());
    }

    #[test]
    fn profile_numbers_and_brick_span_are_checked_without_lossy_wrapping() {
        assert!(IniSettings::parse("[Brick]\nBrickSpan=not-a-number\n").is_err());
        for value in ["-1", "0", "256", "999999999999999999999999"] {
            assert!(
                server_brick_span(&format!("[Brick]\nBrickSpan={value}\n")).is_err(),
                "BrickSpan={value} must fail"
            );
        }

        let byte_overflow = super::super::EXTREME_ARENA_SETTINGS_TEXT.replacen(
            "TeamMaxMines=21",
            "TeamMaxMines=256",
            1,
        );
        assert!(build_arena_settings_packet(&byte_overflow).is_err());

        let short_overflow = super::super::EXTREME_ARENA_SETTINGS_TEXT.replacen(
            "BombExplodeDelay=30",
            "BombExplodeDelay=32768",
            1,
        );
        assert!(build_arena_settings_packet(&short_overflow).is_err());

        let bit_overflow = super::super::EXTREME_ARENA_SETTINGS_TEXT.replacen(
            "SeeBombLevel=0",
            "SeeBombLevel=4",
            1,
        );
        assert!(build_arena_settings_packet(&bit_overflow).is_err());
    }
}

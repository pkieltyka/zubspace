use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(test)]
use std::time::Duration;

use crate::authority_damage::ArenaWeaponDamageSettings;
use crate::continuum_weapon::{
    continuum_weapon_alternate, continuum_weapon_level, continuum_weapon_type,
};
use crate::respawn::respawn_delay_seconds;

// Retain the previous packet builder only as a byte-parity regression oracle.
// Production always reads the authored INI, including embedded Chaos.
#[cfg(test)]
#[path = "arena_settings/chaos.rs"]
mod chaos;
#[path = "arena_settings/profile.rs"]
mod profile;

pub(crate) const ARENA_SETTINGS: u8 = 0x0f;
pub(crate) const CONTINUUM_ARENA_SETTINGS_SIZE: usize = 1428;
pub(crate) const CONTINUUM_SHIP_SETTINGS_SIZE: usize = 144;
pub(crate) const CONTINUUM_SHIP_SETTINGS_OFFSET: usize = 4;
pub(crate) const CONTINUUM_GLOBAL_INTS_OFFSET: usize =
    CONTINUUM_SHIP_SETTINGS_OFFSET + CONTINUUM_SHIP_SETTINGS_SIZE * 8;
const CONTINUUM_SPAWN_SETTINGS_OFFSET: usize = CONTINUUM_GLOBAL_INTS_OFFSET + 20 * 4;
pub(crate) const CONTINUUM_SHORT_SETTINGS_OFFSET: usize = CONTINUUM_SPAWN_SETTINGS_OFFSET + 4 * 4;
pub(crate) const CONTINUUM_BYTE_SETTINGS_OFFSET: usize = CONTINUUM_SHORT_SETTINGS_OFFSET + 58 * 2;
pub(crate) const CONTINUUM_PRIZE_WEIGHTS_OFFSET: usize = CONTINUUM_BYTE_SETTINGS_OFFSET + 32;
pub(crate) const CONTINUUM_PRIZE_WEIGHT_COUNT: usize = 28;

static ACTIVE_SETTINGS_PROFILE: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
const CHAOS_ARENA_SETTINGS_TEXT: &str = include_str!("../../../assets/zones/chaos-zone/arena.conf");
#[cfg(test)]
const EXTREME_ARENA_SETTINGS_TEXT: &str = include_str!("../../../assets/zones/extreme/arena.conf");

pub(crate) fn arena_settings_packet() -> &'static [u8] {
    crate::zone::server_zone_maps()
        .get(ACTIVE_SETTINGS_PROFILE.load(Ordering::Acquire))
        .map(|zone| zone.settings_packet.as_ref())
        .expect("active arena-settings profile must name a loaded zone")
}

pub(crate) fn arena_settings_packet_for_zone(zone_id: &str) -> Option<&'static [u8]> {
    crate::zone::zone_map_for_id(zone_id).map(|zone| zone.settings_packet.as_ref())
}

/// Selects the one active arena profile for this single-arena server process.
/// Zone changes are permitted only while the arena is empty, so every packet
/// reader, gameplay authority path, and security checksum observes the same
/// atomic profile before a player can join.
pub(crate) fn activate_settings_profile(zone_id: &str) -> Result<(), &'static str> {
    let profile = crate::zone::server_zone_maps()
        .iter()
        .position(|zone| zone.zone_id == zone_id)
        .ok_or("unknown arena-settings profile")?;
    // Rust's unit-test harness constructs multiple independent ServerState
    // values in parallel inside one process. Production has exactly one
    // active arena, while tests use `arena_settings_packet_for_zone` for
    // explicit profile assertions; do not let one test's arena login mutate
    // the process-wide profile observed by unrelated tests.
    #[cfg(not(test))]
    ACTIVE_SETTINGS_PROFILE.store(profile, Ordering::Release);
    #[cfg(test)]
    let _ = profile;
    Ok(())
}

/// Which authored settings profile each bundled zone runs.
pub(crate) fn settings_profile_zone_for(zone_id: &str) -> Option<&'static str> {
    crate::zone::zone_map_for_id(zone_id).map(|zone| zone.zone_id.as_ref())
}

pub(crate) fn settings_packet_from_bundle(zone_id: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|err| format!("arena settings for zone {zone_id:?} are not UTF-8: {err}"))?;
    let packet = profile::build_arena_settings_packet(text)
        .map_err(|err| format!("arena settings for zone {zone_id:?} are invalid: {err}"))?;
    if packet.len() != CONTINUUM_ARENA_SETTINGS_SIZE || packet.first() != Some(&ARENA_SETTINGS) {
        return Err(format!(
            "arena settings for zone {zone_id:?} did not build the complete Continuum packet"
        ));
    }
    Ok(packet)
}

pub(crate) fn brick_span_from_bundle(zone_id: &str, bytes: &[u8]) -> Result<u8, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|err| format!("arena settings for zone {zone_id:?} are not UTF-8: {err}"))?;
    profile::server_brick_span(text)
        .map_err(|err| format!("arena settings for zone {zone_id:?} are invalid: {err}"))
}

/// Derives a zone's server-only objective profile from its authored INI, using
/// the zone's own built settings packet for hull-derived policy values. The
/// packet is passed in because zone build happens before the process-global
/// active-settings profile names this zone.
pub(crate) fn objective_profile_from_bundle(
    zone_id: &str,
    bytes: &[u8],
    settings_packet: &[u8],
) -> Result<crate::objectives::ServerObjectiveProfile, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|err| format!("arena settings for zone {zone_id:?} are not UTF-8: {err}"))?;
    let collision_radius_pixels = std::array::from_fn(|ship| {
        // The policy stores whole pixels; a hull always claims a box, so a
        // rounded-down zero must never make a ship unable to pick a flag up.
        (ship_radius_pixels_from(settings_packet, ship as u8).round() as i64).clamp(1, 65_535)
            as u16
    });
    profile::server_objective_profile(text, collision_radius_pixels)
        .map_err(|err| format!("arena settings for zone {zone_id:?} are invalid: {err}"))
}

pub(crate) fn arena_door_delay_ticks() -> i32 {
    read_short_i16(54).max(1) as i32
}

pub(crate) fn arena_door_mode() -> i16 {
    read_short_i16(58)
}

/// FlaggerDamagePercent setting (×1000) as a raw thousandths value.
pub(crate) fn arena_flagger_damage_percent() -> u16 {
    read_short_i16(90).max(0) as u16
}

/// FlaggerDamagePercent as the multiplier the authoritative damage path applies
/// to a player carrying a flag or the ball (the reference game
/// `damage = damage * FlaggerDamagePercent / 1000`). The bundled Chaos profile
/// ships 1000 (neutral); Extreme ships 800.
pub(crate) fn arena_flagger_damage_multiplier() -> f32 {
    #[cfg(test)]
    if let Some(percent) = FLAGGER_DAMAGE_PERCENT_OVERRIDE.with(|cell| cell.get()) {
        return f32::from(percent) / 1000.0;
    }
    f32::from(arena_flagger_damage_percent()) / 1000.0
}

// Thread-local storage for the test-only override below.
#[cfg(test)]
thread_local! {
    static FLAGGER_DAMAGE_PERCENT_OVERRIDE:
        std::cell::Cell<Option<u16>> = const { std::cell::Cell::new(None) };
}

/// Test-only, thread-local override for [`arena_flagger_damage_multiplier`].
///
/// The active settings profile is a process global, and `cargo test` runs the
/// server suite on parallel threads, so switching profiles to observe a
/// non-neutral flagger percent would perturb every sibling damage test. A
/// scoped guard on the test's own thread keeps the observation local, and the
/// whole seam compiles out of the server binary.
#[cfg(test)]
pub(crate) struct FlaggerDamagePercentOverride;

#[cfg(test)]
impl FlaggerDamagePercentOverride {
    pub(crate) fn set(percent: u16) -> Self {
        FLAGGER_DAMAGE_PERCENT_OVERRIDE.with(|cell| cell.set(Some(percent)));
        Self
    }
}

#[cfg(test)]
impl Drop for FlaggerDamagePercentOverride {
    fn drop(&mut self) {
        FLAGGER_DAMAGE_PERCENT_OVERRIDE.with(|cell| cell.set(None));
    }
}

/// FlaggerThrustAdjustment setting in px/s² (Continuum stores as ×0.1).
/// Added to the player's base thrust when carrying flags or the ball.
/// May be negative (slows the flagger). Mirrors the client's
/// `tuning.flagger_thrust_adjustment` parsed at short setting +100.
pub(crate) fn arena_flagger_thrust_adjustment() -> f32 {
    read_short_i16(100) as f32 * 10.0
}

/// FlaggerSpeedAdjustment setting in px/s (Continuum stores as ×10).
/// Added to the player's base top speed when flagger-active.
pub(crate) fn arena_flagger_speed_adjustment() -> f32 {
    read_short_i16(102) as f32 / 10.0
}

pub(crate) fn arena_flagger_fire_cost_multiplier() -> f32 {
    read_short_i16(88).max(0) as f32 / 1000.0
}

pub(crate) fn arena_flagger_gun_upgrade() -> bool {
    arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET + 20] != 0
}

pub(crate) fn arena_flagger_bomb_upgrade() -> bool {
    arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET + 21] != 0
}

pub(crate) fn arena_ball_carrier_allows_bombs() -> bool {
    arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET + 2] != 0
}

pub(crate) fn arena_ball_carrier_allows_guns() -> bool {
    arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET + 3] != 0
}

pub(crate) fn arena_use_flagger_for_ball() -> bool {
    arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET + 22] != 0
}

/// Per-ship TurretThrustPenalty in px/s² (Continuum stores as ×0.1).
/// Subtracted from the host's thrust while at least one rider is
/// attached. Read from ship offset 76.
pub(crate) fn arena_ship_turret_thrust_penalty(ship: u8) -> f32 {
    read_ship_i16(ship, 76) as f32 * 10.0
}

/// Per-ship TurretSpeedPenalty in px/s (Continuum stores as ×10).
/// Subtracted from the host's top speed while at least one rider is
/// attached. Read from ship offset 78.
pub(crate) fn arena_ship_turret_speed_penalty(ship: u8) -> f32 {
    read_ship_i16(ship, 78) as f32 / 10.0
}

/// Per-ship BombThrust recoil in px/s (offset 72, ÷10). Non-mine bombs
/// apply this impulse after the movement/collision step and before the client
/// publishes the position carrying that weapon.
pub(crate) fn arena_ship_bomb_recoil_speed(ship: u8) -> f32 {
    read_ship_u16(ship, 72) as f32 / 10.0
}

/// InitialThrust in px/s² (offset 52, ×10). The base thrust before
/// any prize-derived upgrades.
pub(crate) fn arena_ship_initial_thrust_pixels_per_second_squared(ship: u8) -> f32 {
    (read_ship_u16(ship, 52) as f32 * 10.0)
        .min(arena_ship_max_thrust_pixels_per_second_squared(ship))
}

/// InitialSpeed in px/s (offset 54, ÷10). The base top speed before
/// any prize-derived upgrades.
pub(crate) fn arena_ship_initial_top_speed_pixels_per_second(ship: u8) -> f32 {
    (read_ship_u16(ship, 54) as f32 / 10.0).min(arena_ship_max_speed_pixels_per_second(ship))
}

/// InitialRotation in 0..1 rotations/sec (offset 50, ÷400).
pub(crate) fn arena_ship_initial_rotation_per_second(ship: u8) -> f32 {
    (read_ship_u16(ship, 50) as f32 / 400.0).min(arena_ship_max_rotation_per_second(ship))
}

/// UpgradeRotation in 0..1 rotations/sec (offset 60, ÷400). Each
/// rotation prize bumps the player's effective rotation by this step
/// (clamped at MaximumRotation).
pub(crate) fn arena_ship_rotation_upgrade_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 60) as f32 / 400.0
}

/// UpgradeThrust in px/s² (offset 62, ×10). Each thrust prize bumps
/// the player's effective thrust by this step, clamped at
/// MaximumThrust.
pub(crate) fn arena_ship_thrust_upgrade_pixels_per_second_squared(ship: u8) -> f32 {
    read_ship_u16(ship, 62) as f32 * 10.0
}

/// UpgradeSpeed in px/s (offset 64, ÷10). Each speed prize bumps the
/// player's effective top speed by this step, clamped at MaximumSpeed.
pub(crate) fn arena_ship_top_speed_upgrade_pixels_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 64) as f32 / 10.0
}

/// UpgradeRecharge in energy/sec (offset 66, ÷10). Each recharge prize
/// bumps the player's effective recharge by this step, clamped at
/// MaximumRecharge.
pub(crate) fn arena_ship_recharge_upgrade_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 66) as f32 / 10.0
}

/// MaximumRotation in 0..1 rotations/sec (offset 40, ÷400). The live
/// rotation value is clamped to this ceiling after every prize operation.
pub(crate) fn arena_ship_max_rotation_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 40) as f32 / 400.0
}

/// MaximumRecharge in energy/sec (offset 46, ÷10). The post-upgrade
/// ceiling for recharge.
pub(crate) fn arena_ship_max_recharge_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 46) as f32 / 10.0
}

/// Per-ship Gravity setting in raw Continuum units (offset 8, signed).
/// Used by the sim to compute wormhole gravity attraction;
/// `wormhole_gravity_velocity` formula uses `|gravity| * 1000` as the
/// influence-radius² and scales the per-tick impulse by `gravity *
/// 1000 / dist_sq * 10`.
pub(crate) fn arena_gravity_bombs() -> bool {
    arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET + 8] != 0
}

pub(crate) fn arena_ship_wormhole_gravity(ship: u8) -> i16 {
    read_ship_i16(ship, 8)
}

/// Per-ship GravityTopSpeed in px/s (offset 10, ÷10). The speed cap
/// applied while the ship is being pulled by a wormhole — overrides
/// the normal MaximumSpeed cap so a ship being slingshot through a
/// well can briefly exceed its rated top speed.
pub(crate) fn arena_ship_wormhole_top_speed(ship: u8) -> f32 {
    read_ship_i16(ship, 10).max(0) as f32 / 10.0
}

/// RepelSpeed in px/s (global int at offset 32, ÷10). Top-speed cap
/// applied to a ship while a hostile repel pulse is still pushing — the
/// client uses `max(base_top_speed, repel_speed)` during that window,
/// mirrored by the sim.
pub(crate) fn arena_repel_speed_pixels_per_second() -> f32 {
    read_global_i32(32) as f32 / 10.0
}

pub(crate) fn arena_repel_lifetime_ticks() -> u32 {
    read_short_i16(30).max(0) as u32
}

pub(crate) fn arena_repel_radius_pixels() -> f32 {
    read_short_i16(32).max(0) as f32
}

pub(crate) fn arena_antiwarp_radius_pixels() -> f32 {
    read_short_i16(56).max(0) as f32
}

/// Misc:WarpPointDelay in Continuum 10 ms ticks.
pub(crate) fn arena_portal_lifetime_ticks() -> u32 {
    read_short_i16(76).max(0) as u32
}

pub(crate) fn arena_decoy_lifetime_ticks() -> u32 {
    read_global_i32(16).max(0) as u32
}

/// Misc:AntiWarpSettleDelay in Continuum 10 ms ticks.
pub(crate) fn arena_antiwarp_settle_ticks() -> u32 {
    arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET + 24] as u32
}

pub(crate) fn arena_rocket_thrust_pixels_per_second_squared() -> f32 {
    read_global_i32(56).max(0) as f32 * 10.0
}

pub(crate) fn arena_rocket_speed_pixels_per_second() -> f32 {
    read_global_i32(60).max(0) as f32 / 10.0
}

/// Per-ship RocketTime setting in Continuum's 10 ms ticks.
pub(crate) fn arena_ship_rocket_lifetime_ticks(ship: u8) -> u32 {
    read_ship_u16(ship, 88) as u32
}

pub(crate) fn arena_projectile_initialization_tuning(
    ship: u8,
) -> crate::gameplay_rules::ProjectileInitializationTuning {
    crate::gameplay_rules::ProjectileInitializationTuning {
        bullet_speed: read_ship_u16(ship, 24) as f32 / 10.0,
        bomb_speed: read_ship_u16(ship, 26) as f32 / 10.0,
        burst_speed: read_ship_u16(ship, 74) as f32 / 10.0,
        ship_radius: arena_ship_radius_pixels(ship),
        multifire_angle: read_ship_u16(ship, 30),
        double_barrel: ship_capability_bits(ship, 26, 1) != 0,
        burst_count: read_ship_u8(ship, 107) as usize,
        bomb_bounce_count: arena_ship_bomb_bounce_count(ship),
    }
}

/// Per-ship BombBounceCount in raw count (offset 123, single byte).
/// Phase 2c uses this to seed `bounces_remaining` for bombs spawned
/// with the bouncing-bomb alternate bit set. Regular bombs (no
/// alternate bit) explode on first wall hit and don't read this
/// setting.
pub(crate) fn arena_ship_bomb_bounce_count(ship: u8) -> u8 {
    let ship_index = ship.min(7) as usize;
    let offset = CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE;
    arena_settings_packet()[offset + 123]
}

/// Per-weapon lifetime in Continuum ticks. Phase 2a uses this to
/// compute `expiry_tick = spawn_tick + lifetime`. Returns `None` for
/// weapon types whose lifetime is governed elsewhere (mines have
/// their own arena setting, prizes/decoys aren't tracked in the
/// projectile registry).
pub(crate) fn arena_weapon_lifetime_ticks(weapon_data: u16) -> Option<u32> {
    use crate::continuum_weapon::{continuum_weapon_is_mine, continuum_weapon_type};
    let ty = continuum_weapon_type(weapon_data);
    if continuum_weapon_is_mine(weapon_data) {
        return Some(arena_mine_lifetime_ticks());
    }
    match ty {
        1 | 2 | 7 | 15 => Some(read_global_i32(8).max(0) as u32), // BulletAliveTime
        3 | 4 | 8 => Some(read_global_i32(12).max(0) as u32),     // BombAliveTime
        _ => None,
    }
}

pub(crate) fn arena_explosion_fragment_tuning(
    ship: u8,
) -> crate::authority_projectiles::ExplosionFragmentTuning {
    crate::authority_projectiles::ExplosionFragmentTuning {
        initial_projectile: arena_projectile_initialization_tuning(ship),
        speed: read_global_i32(76).max(0) as f32 / 10.0,
        alive_ticks: read_global_i32(8).max(0) as u32,
        random: arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET] != 0,
    }
}

/// BountyIncreaseForKill setting — bounty awarded to the killer per kill of
/// a positive-bounty victim. Mirrors the reference game's `PlayerManager:748-750`:
/// `if (killed->bounty > 0) killer->bounty += BountyIncreaseForKill`.
pub(crate) fn arena_bounty_increase_for_kill() -> u16 {
    read_short_i16(18).max(0) as u16
}

/// EBombShutdownTime in continuum ticks (`ShipController:2006-2014`).
/// Used to set the victim's EMP shutdown timer on EMP-bomb hits.
pub(crate) fn arena_emp_bomb_shutdown_ticks() -> i32 {
    read_short_i16(70).max(0) as i32
}

/// EngineShutdownTime in Continuum ticks (`Prize:EngineShutdownTime`).
pub(crate) fn arena_engine_shutdown_ticks() -> u32 {
    read_short_i16(14).max(0) as u32
}

/// Ship has EMP-bomb capability bit (capability bit 27). When true, all
/// bombs fired by this ship are EMP bombs.
pub(crate) fn arena_ship_emp_bomb(ship: u8) -> bool {
    ship_capability_bits(ship, 27, 1) != 0
}

/// AttachBounty setting per ship — minimum bounty required to attach as a
/// turret. Mirrors the reference game's `PlayerManager:1314-1345`.
pub(crate) fn arena_ship_attach_bounty(ship: u8) -> u16 {
    read_ship_u16(ship, 96)
}

/// TurretLimit per ship — maximum turret riders that can attach to this
/// ship. 0 means "no riders allowed".
pub(crate) fn arena_ship_turret_limit(ship: u8) -> u8 {
    let ship_index = ship.min(7) as usize;
    let offset = CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE;
    arena_settings_packet()[offset + 106]
}

/// Brick lifetime derived from arena `BrickTime` (in centiseconds, i.e. 100
/// ticks per second), matching the client's `tuning.brick_lifetime` so the
/// server and client agree on when bricks expire. Mirrors the reference game's
/// `BrickManager` use of `connection.settings.BrickTime`.
pub(crate) fn arena_brick_lifetime_ticks() -> u32 {
    read_short_i16(66).max(0) as u32
}

/// Respawn delay for the chaos arena, matching the client's shared
/// `respawn_delay_seconds` math. Continuum EnterDelay still contributes
/// the configured delay, but zubspace applies a two-second floor so death
/// and respawn are observable and synchronized in network play.
pub(crate) fn arena_respawn_delay() -> std::time::Duration {
    let enter_delay_ticks = read_short_i16(12).max(0) as f32;
    let enter_delay_seconds = enter_delay_ticks / 100.0;
    std::time::Duration::from_secs_f32(respawn_delay_seconds(enter_delay_seconds))
}

pub(crate) fn arena_respawn_delay_ticks() -> u32 {
    crate::continuum::seconds_to_ticks_u32(arena_respawn_delay().as_secs_f32())
}

pub(crate) fn arena_prize_weights() -> [u8; CONTINUUM_PRIZE_WEIGHT_COUNT] {
    let packet = arena_settings_packet();
    let mut weights = [0_u8; CONTINUUM_PRIZE_WEIGHT_COUNT];
    weights.copy_from_slice(
        &packet[CONTINUUM_PRIZE_WEIGHTS_OFFSET
            ..CONTINUUM_PRIZE_WEIGHTS_OFFSET + CONTINUUM_PRIZE_WEIGHT_COUNT],
    );
    weights
}

pub(crate) fn arena_prize_weight_total() -> u16 {
    arena_prize_weights().iter().map(|w| *w as u16).sum()
}

pub(crate) fn arena_prize_factor() -> u16 {
    read_short_i16(40).max(0) as u16
}

pub(crate) fn arena_prize_spawn_delay_ticks() -> u32 {
    read_short_i16(42).max(1) as u32
}

pub(crate) fn arena_prize_minimum_virtual() -> u16 {
    read_short_i16(44).max(0) as u16
}

pub(crate) fn arena_prize_upgrade_virtual() -> u16 {
    read_short_i16(46).max(0) as u16
}

pub(crate) fn arena_prize_lifetime_max_ticks() -> u16 {
    read_short_i16(48).max(0) as u16
}

pub(crate) fn arena_prize_lifetime_min_ticks() -> u16 {
    read_short_i16(50).max(0) as u16
}

pub(crate) fn arena_prize_negative_factor() -> u16 {
    read_short_i16(52).max(0) as u16
}

/// The reference game's per-ship `DamageFactor` (ship field offset 92). Drives the random
/// per-damage-event downgrade roll: when a victim with positive
/// bounty takes non-lethal damage, the reference game
/// (`ShipController:2083-2097`) rolls
/// `1 / ((DamageFactor * 200_000) / (damage * 1_000) + 1)` chance
/// to remove a random prize via the negative-prize path. A
/// `DamageFactor` of 0 disables the roll entirely.
pub(crate) fn arena_ship_damage_factor(ship: u8) -> u16 {
    read_ship_u16(ship, 92)
}

/// Server-only `Brick:BrickSpan`. This setting is deliberately independent
/// from Continuum packet byte slot 7, which is `Mine:TeamMaxMines`.
pub(crate) fn arena_brick_span() -> u8 {
    crate::zone::server_zone_maps()
        .get(ACTIVE_SETTINGS_PROFILE.load(Ordering::Acquire))
        .map(|zone| zone.brick_span)
        .expect("active brick-span profile must name a loaded zone")
}

#[cfg(test)]
pub(crate) fn arena_brick_span_for_zone(zone_id: &str) -> Option<u8> {
    crate::zone::zone_map_for_id(zone_id).map(|zone| zone.brick_span)
}

pub(crate) fn arena_prize_hide_count() -> usize {
    arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET + 13] as usize
}

pub(crate) fn arena_multiprize_count() -> u16 {
    read_short_i16(64).max(0) as u16
}

#[cfg(test)]
pub(crate) fn arena_prize_claim_retention() -> Duration {
    Duration::from_millis(u64::from(arena_prize_claim_retention_ticks()).saturating_mul(10))
}

pub(crate) fn arena_prize_claim_retention_ticks() -> u32 {
    let hidden_prize_ticks = read_short_i16(48).max(0) as u32;
    let death_prize_ticks = read_short_i16(8).max(0) as u32;
    hidden_prize_ticks.max(death_prize_ticks)
}

pub(crate) fn arena_death_prize_lifetime_ticks() -> u32 {
    read_short_i16(8).max(0) as u32
}

#[cfg(test)]
pub(crate) fn arena_soccer_mode() -> u8 {
    arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET + 4]
}

pub(crate) fn arena_bomb_safety_enabled() -> bool {
    arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET + 9] != 0
}

pub(crate) fn arena_bomb_explode_delay_ticks() -> u32 {
    read_short_i16(2).max(0) as u32
}

pub(crate) fn arena_bomb_explode_pixels() -> f32 {
    read_short_i16(6).max(0) as f32
}

pub(crate) fn arena_proximity_distance() -> f32 {
    read_short_i16(16).max(0) as f32
}

pub(crate) fn arena_team_max_mines() -> usize {
    arena_settings_packet()[CONTINUUM_BYTE_SETTINGS_OFFSET + 7] as usize
}

#[cfg(test)]
pub(crate) fn arena_mine_lifetime() -> Duration {
    Duration::from_millis(u64::from(arena_mine_lifetime_ticks()) * 10)
}

pub(crate) fn arena_mine_lifetime_ticks() -> u32 {
    read_global_i32(36).max(0) as u32
}

pub(crate) fn arena_max_frequency() -> u16 {
    i32::from_le_bytes(
        arena_settings_packet()
            [CONTINUUM_GLOBAL_INTS_OFFSET + 28..CONTINUUM_GLOBAL_INTS_OFFSET + 32]
            .try_into()
            .expect("arena settings packet is complete"),
    )
    .clamp(1, u16::MAX as i32) as u16
}

pub(crate) fn arena_ship_radius_pixels(ship: u8) -> f32 {
    ship_radius_pixels_from(arena_settings_packet(), ship)
}

fn ship_radius_pixels_from(packet: &[u8], ship: u8) -> f32 {
    let ship_index = ship.min(7) as usize;
    let offset = CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE;
    let visibility = u16::from_le_bytes(
        packet[offset + 28..offset + 30]
            .try_into()
            .expect("arena settings packet is complete"),
    );
    let radius = ((visibility >> 3) & 0xff) as u8;
    crate::continuum::continuum_radius_pixels(radius)
}

pub(crate) fn arena_ship_super_lifetime_ticks(ship: u8) -> u32 {
    read_ship_u32(ship, 0)
}

pub(crate) fn arena_ship_shield_lifetime_ticks(ship: u8) -> u32 {
    read_ship_u32(ship, 4)
}

pub(crate) fn arena_ship_max_speed_pixels_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 44) as f32 / 10.0
}

pub(crate) fn arena_ship_max_thrust_pixels_per_second_squared(ship: u8) -> f32 {
    read_ship_u16(ship, 42) as f32 * 10.0
}

/// Initial (pre-prize) max speed in px/s. Production Afterburner authority
/// uses the exact accepted input witness; this accessor is retained only for
/// tuning-boundary regression tests.
#[cfg(test)]
pub(crate) fn arena_ship_base_max_speed_pixels_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 54) as f32 / 10.0
}

/// AfterburnerEnergy in energy units per second (Continuum stores it ×10).
/// Used to drain energy server-side when the player's observed motion
/// indicates afterburner thrust, matching the reference game's
/// `ShipController:244-264` energy update order
/// (afterburners → recharge → status costs).
pub(crate) fn arena_ship_afterburner_energy_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 70) as f32 / 10.0
}

/// ExactDamage flag (bit 8 of the arena flags word). When set, weapon
/// damage is deterministic; when clear, Continuum and the reference game re-roll each
/// hit as `sqrt((rand() * 1000) % (damage^2 + 1))`. Read by the damage
/// authority path so server-owned projectile hits apply the same arena
/// setting as the client.
pub(crate) fn arena_exact_damage() -> bool {
    let flags = u32::from_le_bytes(arena_settings_packet()[0..4].try_into().unwrap());
    flags & (1 << 8) != 0
}

/// ShipChangeCost in energy units (defaults to 0). Continuum's binary
/// arena settings packet does not carry this field — it's an ASSS module
/// setting layered on top. Returning 0 matches the ASSS default and the
/// behaviour Continuum clients expect from a stock arena. The plumbing is
/// in place at the ship-change path so that an arena config with a
/// non-zero cost can be enforced authoritatively without further code
/// changes.
pub(crate) fn arena_ship_change_cost() -> u16 {
    0
}

pub(crate) fn arena_ship_cloak_energy_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 32) as f32 / 10.0
}

pub(crate) fn arena_ship_stealth_energy_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 34) as f32 / 10.0
}

pub(crate) fn arena_ship_antiwarp_energy_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 36) as f32 / 10.0
}

pub(crate) fn arena_ship_xradar_energy_per_second(ship: u8) -> f32 {
    read_ship_u16(ship, 38) as f32 / 10.0
}

pub(crate) fn arena_ship_max_energy(ship: u8) -> u16 {
    read_ship_u16(ship, 48)
}

pub(crate) fn arena_ship_initial_energy(ship: u8) -> u16 {
    read_ship_u16(ship, 58).min(arena_ship_max_energy(ship))
}

pub(crate) fn arena_ship_initial_bounty(ship: u8) -> u16 {
    read_ship_u16(ship, 90)
}

pub(crate) fn arena_ship_energy_upgrade(ship: u8) -> u16 {
    read_ship_u16(ship, 68)
}

pub(crate) fn arena_ship_initial_recharge_per_second(ship: u8) -> f32 {
    (read_ship_u16(ship, 56) as f32 / 10.0).min(arena_ship_max_recharge_per_second(ship))
}

pub(crate) fn arena_ship_weapon_energy_cost(ship: u8, weapon_data: u16) -> u16 {
    let level = continuum_weapon_level(weapon_data);
    match continuum_weapon_type(weapon_data) {
        1 | 2 if continuum_weapon_alternate(weapon_data) => {
            leveled_gun_energy_cost(read_ship_u16(ship, 14), level)
        }
        1 | 2 => leveled_gun_energy_cost(read_ship_u16(ship, 12), level),
        3 | 4 if continuum_weapon_alternate(weapon_data) => {
            leveled_weapon_energy_cost(read_ship_u16(ship, 20), read_ship_u16(ship, 22), level)
        }
        3 | 4 => {
            leveled_weapon_energy_cost(read_ship_u16(ship, 16), read_ship_u16(ship, 18), level)
        }
        _ => 0,
    }
}

pub(crate) fn arena_weapon_damage_settings() -> ArenaWeaponDamageSettings {
    ArenaWeaponDamageSettings {
        bullet_damage_level: read_global_i32(0).max(0) as u32,
        bomb_damage_level: read_global_i32(4).max(0) as u32,
        burst_damage_level: read_global_i32(40).max(0) as u32,
        bullet_damage_upgrade: read_global_i32(44).max(0) as u32,
        inactive_shrapnel_damage: read_global_i32(64).max(0) as u32,
        emp_bomb_damage_percent: read_short_i16(72).max(0) as u16,
        bouncing_bomb_damage_percent: read_short_i16(80).max(0) as u16,
        shrapnel_damage_percent: read_short_i16(82).max(0) as u16,
    }
}

pub(crate) fn arena_ship_initial_gun_level(ship: u8) -> u8 {
    stored_weapon_level(ship_capability_bits(ship, 18, 2) as u8).min(arena_ship_max_gun_level(ship))
}

pub(crate) fn arena_ship_max_gun_level(ship: u8) -> u8 {
    stored_weapon_level(ship_capability_bits(ship, 20, 2) as u8)
}

pub(crate) fn arena_ship_initial_bomb_level(ship: u8) -> u8 {
    stored_weapon_level(ship_capability_bits(ship, 22, 2) as u8)
        .min(arena_ship_max_bomb_level(ship))
}

pub(crate) fn arena_ship_max_bomb_level(ship: u8) -> u8 {
    stored_weapon_level(ship_capability_bits(ship, 24, 2) as u8)
}

pub(crate) fn arena_ship_max_mines(ship: u8) -> usize {
    read_ship_u8(ship, 108) as usize
}

pub(crate) fn arena_ship_shrapnel_max(ship: u8) -> u8 {
    ship_capability_bits(ship, 0, 5) as u8
}

pub(crate) fn arena_ship_shrapnel_rate(ship: u8) -> u8 {
    ship_capability_bits(ship, 5, 5) as u8
}

pub(crate) fn arena_ship_cloak_status(ship: u8) -> u8 {
    ship_capability_bits(ship, 10, 2) as u8
}

pub(crate) fn arena_ship_stealth_status(ship: u8) -> u8 {
    ship_capability_bits(ship, 12, 2) as u8
}

pub(crate) fn arena_ship_xradar_status(ship: u8) -> u8 {
    ship_capability_bits(ship, 14, 2) as u8
}

pub(crate) fn arena_ship_antiwarp_status(ship: u8) -> u8 {
    ship_capability_bits(ship, 16, 2) as u8
}

pub(crate) fn arena_ship_initial_repel_count(ship: u8) -> u8 {
    read_ship_u8(ship, 116).min(arena_ship_max_repel_count(ship))
}

pub(crate) fn arena_ship_max_repel_count(ship: u8) -> u8 {
    read_ship_u8(ship, 109)
}

pub(crate) fn arena_ship_initial_burst_count(ship: u8) -> u8 {
    read_ship_u8(ship, 117).min(arena_ship_max_burst_count(ship))
}

pub(crate) fn arena_ship_max_burst_count(ship: u8) -> u8 {
    read_ship_u8(ship, 110)
}

pub(crate) fn arena_ship_initial_decoy_count(ship: u8) -> u8 {
    read_ship_u8(ship, 121).min(arena_ship_max_decoy_count(ship))
}

pub(crate) fn arena_ship_max_decoy_count(ship: u8) -> u8 {
    read_ship_u8(ship, 111)
}

pub(crate) fn arena_ship_initial_thor_count(ship: u8) -> u8 {
    read_ship_u8(ship, 120).min(arena_ship_max_thor_count(ship))
}

pub(crate) fn arena_ship_max_thor_count(ship: u8) -> u8 {
    read_ship_u8(ship, 112)
}

pub(crate) fn arena_ship_initial_brick_count(ship: u8) -> u8 {
    read_ship_u8(ship, 118).min(arena_ship_max_brick_count(ship))
}

pub(crate) fn arena_ship_max_brick_count(ship: u8) -> u8 {
    read_ship_u8(ship, 113)
}

pub(crate) fn arena_ship_initial_rocket_count(ship: u8) -> u8 {
    read_ship_u8(ship, 119).min(arena_ship_max_rocket_count(ship))
}

pub(crate) fn arena_ship_prize_share_limit(ship: u8) -> u16 {
    // The headed prize authority replay needs an eligible teammate under a
    // share limit it chooses, so this narrowly named process-local test seam
    // overrides the arena's PrizeShareLimit. It changes only recipient
    // admission; ordinary server runs always read the arena packet.
    if std::env::var("ZUBSPACE_E2E_PRIZES_PHASE1").as_deref() == Ok("1")
        && let Ok(value) = std::env::var("ZUBSPACE_SERVER_TEST_PRIZE_SHARE_LIMIT")
        && let Ok(value) = value.parse::<u16>()
    {
        return value;
    }
    read_ship_u16(ship, 94)
}

pub(crate) fn arena_ship_initial_portal_count(ship: u8) -> u8 {
    read_ship_u8(ship, 122).min(arena_ship_max_portal_count(ship))
}

pub(crate) fn arena_ship_max_portal_count(ship: u8) -> u8 {
    read_ship_u8(ship, 115)
}

pub(crate) fn arena_ship_max_rocket_count(ship: u8) -> u8 {
    read_ship_u8(ship, 114)
}

pub(crate) fn arena_ship_weapon_fire_delay_ticks(ship: u8, weapon_type: u8) -> u32 {
    match weapon_type {
        1 | 2 => read_ship_u16(ship, 80) as u32,
        // Type4 is a moving proximity bomb, not a mine. The reference game's bomb
        // and dropped-mine inputs both use BombFireDelay; the alternate bit
        // selects a mine and does not turn type4 into LandmineFireDelay.
        3 | 4 | 8 => read_ship_u16(ship, 84) as u32,
        5..=7 => read_ship_u16(ship, 84) as u32,
        _ => 0,
    }
}

pub(crate) fn arena_ship_multifire_delay_ticks(ship: u8) -> u32 {
    read_ship_u16(ship, 82) as u32
}

#[cfg(test)]
pub(crate) fn arena_weapon_max_origin_distance_pixels(
    ship: u8,
    weapon_data: u16,
    hit_delay_ticks: u32,
) -> Option<f32> {
    let weapon_type = continuum_weapon_type(weapon_data);
    let (speed, lifetime_ticks) = match weapon_type {
        1 | 2 => (read_ship_u16(ship, 24) as f32 / 10.0, read_global_i32(8)),
        3 | 4 | 8 => (read_ship_u16(ship, 26) as f32 / 10.0, read_global_i32(12)),
        _ => return None,
    };
    let elapsed_ticks = hit_delay_ticks.min(lifetime_ticks.max(0) as u32);
    let elapsed_seconds = crate::continuum::ticks_to_seconds_u32(elapsed_ticks);
    Some(speed * elapsed_seconds + weapon_range_grace_pixels(weapon_type))
}

#[cfg(test)]
fn weapon_range_grace_pixels(weapon_type: u8) -> f32 {
    match weapon_type {
        1 | 2 => 512.0,
        3 | 4 | 8 => 1024.0,
        _ => 0.0,
    }
}

fn read_ship_u16(ship: u8, field_offset: usize) -> u16 {
    let ship_index = ship.min(7) as usize;
    let offset = CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE;
    u16::from_le_bytes(
        arena_settings_packet()[offset + field_offset..offset + field_offset + 2]
            .try_into()
            .expect("arena settings packet is complete"),
    )
}

fn read_ship_i16(ship: u8, field_offset: usize) -> i16 {
    read_ship_u16(ship, field_offset) as i16
}

fn read_global_i32(field_offset: usize) -> i32 {
    i32::from_le_bytes(
        arena_settings_packet()[CONTINUUM_GLOBAL_INTS_OFFSET + field_offset
            ..CONTINUUM_GLOBAL_INTS_OFFSET + field_offset + 4]
            .try_into()
            .expect("arena settings packet is complete"),
    )
}

fn read_short_i16(field_offset: usize) -> i16 {
    i16::from_le_bytes(
        arena_settings_packet()[CONTINUUM_SHORT_SETTINGS_OFFSET + field_offset
            ..CONTINUUM_SHORT_SETTINGS_OFFSET + field_offset + 2]
            .try_into()
            .expect("arena settings packet is complete"),
    )
}

fn read_ship_u8(ship: u8, field_offset: usize) -> u8 {
    let ship_index = ship.min(7) as usize;
    let offset = CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE;
    arena_settings_packet()[offset + field_offset]
}

fn read_ship_u32(ship: u8, field_offset: usize) -> u32 {
    let ship_index = ship.min(7) as usize;
    let offset = CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE;
    u32::from_le_bytes(
        arena_settings_packet()[offset + field_offset..offset + field_offset + 4]
            .try_into()
            .expect("arena settings packet is complete"),
    )
}

fn ship_capability_bits(ship: u8, shift: u8, width: u8) -> u32 {
    let mask = if width >= 32 {
        u32::MAX
    } else {
        (1_u32 << width) - 1
    };
    (read_ship_u32(ship, 124) >> shift) & mask
}

fn stored_weapon_level(bits: u8) -> u8 {
    // Arena capability bits store settings weapon levels 0..=3 directly.
    // Per-shot weapon_data level bits use a different level-1 encoding.
    crate::continuum_weapon::settings_weapon_level_from_bits(bits)
}

fn leveled_weapon_energy_cost(base_cost: u16, upgrade_cost: u16, level: u8) -> u16 {
    base_cost.saturating_add(upgrade_cost.saturating_mul(level.saturating_sub(1) as u16))
}

fn leveled_gun_energy_cost(base_cost: u16, level: u8) -> u16 {
    base_cost.saturating_mul(level.max(1) as u16)
}

pub(crate) fn continuum_settings_checksum(key: u32, data: &[u8]) -> u32 {
    data[..CONTINUUM_ARENA_SETTINGS_SIZE]
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) ^ key)
        .fold(0_u32, u32::wrapping_add)
}

#[cfg(test)]
mod per_zone_profile_tests {
    use super::*;

    #[test]
    fn shrapnel_lifetime_and_repel_refresh_use_bullet_alive_time() {
        for raw_level in 0..=3 {
            assert_eq!(
                arena_weapon_lifetime_ticks(15 | (raw_level << 5)),
                arena_weapon_lifetime_ticks(1)
            );
            assert!(arena_weapon_lifetime_ticks(15 | (raw_level << 5)).is_some());
        }
    }

    fn ship_u16(packet: &[u8], ship: usize, field_offset: usize) -> u16 {
        let offset = CONTINUUM_SHIP_SETTINGS_OFFSET + ship * CONTINUUM_SHIP_SETTINGS_SIZE;
        u16::from_le_bytes(
            packet[offset + field_offset..offset + field_offset + 2]
                .try_into()
                .unwrap(),
        )
    }

    fn ship_radius_wire(packet: &[u8], ship: usize) -> u8 {
        ((ship_u16(packet, ship, 28) >> 3) & 0xff) as u8
    }

    fn global_i32(packet: &[u8], field_offset: usize) -> i32 {
        i32::from_le_bytes(
            packet[CONTINUUM_GLOBAL_INTS_OFFSET + field_offset
                ..CONTINUUM_GLOBAL_INTS_OFFSET + field_offset + 4]
                .try_into()
                .unwrap(),
        )
    }

    fn short_i16(packet: &[u8], field_offset: usize) -> i16 {
        i16::from_le_bytes(
            packet[CONTINUUM_SHORT_SETTINGS_OFFSET + field_offset
                ..CONTINUUM_SHORT_SETTINGS_OFFSET + field_offset + 2]
                .try_into()
                .unwrap(),
        )
    }

    #[test]
    fn every_bundled_identity_has_a_distinct_complete_packet() {
        let chaos = arena_settings_packet_for_zone(crate::zone::CHAOS_ZONE_ID).unwrap();
        let extreme = arena_settings_packet_for_zone(crate::zone::EXTREME_ZONE_ID).unwrap();

        for packet in [chaos, extreme] {
            assert_eq!(packet.len(), CONTINUUM_ARENA_SETTINGS_SIZE);
            assert_eq!(packet[0], ARENA_SETTINGS);
        }
        assert_ne!(extreme, chaos);
        assert_eq!(
            settings_profile_zone_for(crate::zone::EXTREME_ZONE_ID),
            Some("extreme")
        );
        // The reference game's packed header and unchanged checksum implementation independently
        // verify these goldens. Chaos uses the shared central range176 and
        // the same three-brick maximum for every ship.
        assert_eq!(short_i16(chaos, 68), 176);
        assert_eq!(crate::wire::continuum_file_crc32(chaos), 0xec15_cf5c);
        assert_eq!(crate::wire::continuum_file_crc32(extreme), 0x9ee4_b78d);
        assert_eq!(
            continuum_settings_checksum(crate::security::SECURITY_CHECKSUM_KEY, chaos),
            0xf951_9fa7
        );
        assert_eq!(
            continuum_settings_checksum(crate::security::SECURITY_CHECKSUM_KEY, extreme),
            0xb78e_be99
        );

        for zone_id in crate::zone::supported_server_zone_ids() {
            assert_eq!(settings_profile_zone_for(zone_id), Some(*zone_id));
            assert!(arena_settings_packet_for_zone(zone_id).is_some());
            assert!(arena_brick_span_for_zone(zone_id).is_some());
        }
        assert_eq!(settings_profile_zone_for("unknown-zone"), None);
        assert_eq!(arena_settings_packet_for_zone("unknown-zone"), None);
        assert_eq!(arena_brick_span_for_zone("unknown-zone"), None);
    }

    #[test]
    fn ship_radius_wire_values_match_reference_pixel_contract() {
        // ASSS clientset.c assigns cfg Radius directly to the eight-bit field;
        // SubspaceServer ClientSettings.cs does the same and documents pixels.
        // Keep these source-independent constants so encoder/decoder symmetry
        // cannot make a scaled packet appear correct.
        let chaos = arena_settings_packet_for_zone(crate::zone::CHAOS_ZONE_ID).unwrap();
        let extreme = arena_settings_packet_for_zone(crate::zone::EXTREME_ZONE_ID).unwrap();

        assert_eq!(
            std::array::from_fn::<_, 8, _>(|ship| ship_radius_wire(chaos, ship)),
            [14; 8],
            "Chaos Radius=14 is emitted as raw pixel byte 14"
        );
        assert_eq!(
            std::array::from_fn::<_, 8, _>(|ship| ship_radius_wire(extreme, ship)),
            [0; 8],
            "the upstream EG profile explicitly leaves every Radius at zero"
        );
    }

    #[test]
    fn server_brick_span_is_explicit_and_independent_from_team_max_mines() {
        let expected_team_max_mines = [12_u8, 21];
        for (zone_id, team_max_mines) in crate::zone::supported_server_zone_ids()
            .iter()
            .zip(expected_team_max_mines)
        {
            let packet = arena_settings_packet_for_zone(zone_id).unwrap();
            assert_eq!(
                packet[CONTINUUM_BYTE_SETTINGS_OFFSET + 7],
                team_max_mines,
                "{zone_id} packet byte 7 remains Mine:TeamMaxMines"
            );
            assert_eq!(
                arena_brick_span_for_zone(zone_id),
                Some(10),
                "{zone_id} uses the explicit ASSS BrickSpan default"
            );
        }
    }

    #[test]
    fn extreme_packet_preserves_upstream_eg_values() {
        let packet = arena_settings_packet_for_zone(crate::zone::EXTREME_ZONE_ID).unwrap();
        let flags = u32::from_le_bytes(packet[0..4].try_into().unwrap());
        assert_eq!(flags & (1 << 8), 0, "ExactDamage");
        assert_eq!(global_i32(packet, 0), 210_000, "BulletDamageLevel");
        assert_eq!(global_i32(packet, 4), 5_600_000, "BombDamageLevel");
        assert_eq!(global_i32(packet, 8), 65, "BulletAliveTime");
        assert_eq!(global_i32(packet, 12), 250, "BombAliveTime");
        assert_eq!(global_i32(packet, 28), 10_000, "Team:MaxFrequency");
        assert_eq!(ship_u16(packet, 0, 24), 4_100, "Warbird BulletSpeed");
        assert_eq!(ship_u16(packet, 0, 26), 4_300, "Warbird BombSpeed");
        assert_eq!(ship_u16(packet, 0, 40), 360, "Warbird MaximumRotation");
        assert_eq!(ship_u16(packet, 0, 44), 3_500, "Warbird MaximumSpeed");
        assert_eq!(ship_u16(packet, 0, 48), 2_800, "Warbird MaximumEnergy");
        assert_eq!(ship_u16(packet, 0, 54), 3_200, "Warbird InitialSpeed");
        assert_eq!(short_i16(packet, 40), 6_000, "PrizeFactor");
        assert_eq!(packet[CONTINUUM_BYTE_SETTINGS_OFFSET + 16], 1, "CarryFlags");
        assert_eq!(
            packet[CONTINUUM_PRIZE_WEIGHTS_OFFSET + 5],
            23,
            "XRadar weight"
        );
        assert_eq!(
            packet[CONTINUUM_PRIZE_WEIGHTS_OFFSET + 19],
            10,
            "AntiWarp weight"
        );
        assert_eq!(
            packet[CONTINUUM_PRIZE_WEIGHTS_OFFSET + 24],
            5,
            "MultiPrize weight"
        );
        assert_eq!(
            packet[CONTINUUM_PRIZE_WEIGHTS_OFFSET + 27],
            0,
            "Portal weight"
        );
    }
}

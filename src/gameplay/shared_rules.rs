//! Engine-independent gameplay rules compiled into both binaries.

#![allow(dead_code)]

#[path = "weapon_motion.rs"]
pub(crate) mod weapon_motion;

/// Canonical client prediction cadence and its existing bounded catch-up batch.
/// Presentation uses this same elapsed-time boundary to distinguish a regular
/// low-rate frame from a suspension that discards gameplay time.
pub(crate) const GAMEPLAY_STEP_SECONDS: f32 = 1.0 / 60.0;
pub(crate) const MAX_GAMEPLAY_CATCH_UP_STEPS: u8 = 8;

/// Maximum time a network client may advance dynamic doors without a newer
/// authenticated server-clock sample. Server input admission uses the same
/// bound to reconstruct only deterministic phases reachable while an RTC
/// packet waited behind a long authoritative step.
pub(crate) const DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS: u32 = 100;

/// The reference game's terminal ship effect on entering a wormhole. Keep the
/// proportional drain in the engine-independent rules module so browser
/// prediction and server authority cannot silently choose different values.
pub(crate) fn wormhole_energy_after(current: f32) -> f32 {
    (current * 0.2).max(1.0)
}

/// Dynamic-door tiles in the Continuum map format. Keeping the tile identity
/// in this shared rule prevents a normal wall (or an out-of-bounds sentinel)
/// from ever becoming relocation authority merely because a caller's generic
/// collision predicate changed between two samples.
const DYNAMIC_DOOR_TILE_MIN: u8 = 162;
const DYNAMIC_DOOR_TILE_MAX: u8 = 169;

/// The reference game's `BoxBoxIntersect`/`BoxBoxOverlap` verbatim, with the moving
/// player hull as `first` and the map tile as `second`. The reference rule is
/// intentionally asymmetric: touching the tile's minimum edge overlaps
/// (`first_max >= second_min`), while touching its maximum edge does not
/// (`first_min < second_max`). Operand order is therefore part of the API.
pub(crate) fn reference_game_player_hull_overlaps_tile(
    hull_min_x: f32,
    hull_min_y: f32,
    hull_max_x: f32,
    hull_max_y: f32,
    tile_min_x: f32,
    tile_min_y: f32,
    tile_max_x: f32,
    tile_max_y: f32,
) -> bool {
    hull_max_x >= tile_min_x
        && hull_min_x < tile_max_x
        && hull_max_y >= tile_min_y
        && hull_min_y < tile_max_y
}

/// Return whether the pixel-truncated reference-game AABB overlaps a *specific*
/// dynamic door tile which was open under `before_seed` and closed under
/// `after_seed`.
///
/// Both binaries provide their native map lookup and door-solid predicate;
/// tile identity and geometry live here so browser relocation and server
/// replay proof cannot drift apart.
#[allow(clippy::too_many_arguments)]
pub(crate) fn newly_closed_door_overlap(
    position_x: f32,
    position_y: f32,
    radius: f32,
    map_tiles: usize,
    tile_size: f32,
    before_seed: u8,
    after_seed: u8,
    mut tile_at: impl FnMut(i32, i32) -> Option<u8>,
    mut blocks_with_seed: impl FnMut(u8, u8) -> bool,
) -> bool {
    if !position_x.is_finite()
        || !position_y.is_finite()
        || !radius.is_finite()
        || radius < 0.0
        || map_tiles == 0
        || !tile_size.is_finite()
        || tile_size <= 0.0
    {
        return false;
    }

    // Map samples the local player's live position directly here. Unlike
    // PlayerManager's wall sweep, a newly closed door does not PixelRound the
    // center before constructing the hull.
    let hull_min_x = position_x - radius;
    let hull_min_y = position_y - radius;
    let hull_max_x = position_x + radius;
    let hull_max_y = position_y + radius;
    let min_tile_x = (hull_min_x / tile_size).floor() as i32;
    let min_tile_y = (hull_min_y / tile_size).floor() as i32;
    let max_tile_x = (hull_max_x / tile_size).floor() as i32;
    let max_tile_y = (hull_max_y / tile_size).floor() as i32;
    let map_tiles = map_tiles as i32;

    for tile_y in min_tile_y..=max_tile_y {
        for tile_x in min_tile_x..=max_tile_x {
            if tile_x < 0 || tile_y < 0 || tile_x >= map_tiles || tile_y >= map_tiles {
                continue;
            }
            let Some(tile) = tile_at(tile_x, tile_y) else {
                continue;
            };
            if !(DYNAMIC_DOOR_TILE_MIN..=DYNAMIC_DOOR_TILE_MAX).contains(&tile)
                || blocks_with_seed(tile, before_seed)
                || !blocks_with_seed(tile, after_seed)
            {
                continue;
            }

            let tile_min_x = tile_x as f32 * tile_size;
            let tile_min_y = tile_y as f32 * tile_size;
            let tile_max_x = tile_min_x + tile_size;
            let tile_max_y = tile_min_y + tile_size;
            if reference_game_player_hull_overlaps_tile(
                hull_min_x, hull_min_y, hull_max_x, hull_max_y, tile_min_x, tile_min_y, tile_max_x,
                tile_max_y,
            ) {
                return true;
            }
        }
    }
    false
}

/// Shared wire contract: bit 5 is one activation pulse, never RocketTime's
/// held active state.
pub(crate) const ROCKET_ACTIVATION_PULSE_BIT: u16 = 1 << 5;
/// One-record client intent proving Continuum's legal post-movement velocity
/// stop after a weapon key is consumed inside server-owned safe geometry.
pub(crate) const SAFE_ZONE_WEAPON_STOP_BIT: u16 = 1 << 6;

/// Prove that a source-authenticated 60 Hz input cursor has covered a weapon
/// cooldown expressed in Continuum's 100 Hz ticks. The server combines this
/// with its own wall clock and permits only the final one-tick quantization
/// boundary; logical input is never an independent time authority.
#[inline]
pub(crate) fn weapon_sequence_cooldown_elapsed(
    previous_sequence: u16,
    current_sequence: u16,
    cooldown_ticks: u32,
) -> bool {
    let steps = current_sequence.wrapping_sub(previous_sequence);
    steps > 0 && steps < (1 << 15) && u64::from(steps) * 5 >= u64::from(cooldown_ticks) * 3
}

/// Exact server-owned RepelSpeed interval after the application record. Server
/// time is 100 Hz and live movement is 60 Hz, so ceil(`ticks * 60 / 100`)
/// subsequent logical records are active. This integer rule avoids f32 timer
/// countdown drift and is identical across native and WASM builds.
#[inline]
pub(crate) fn repel_sequence_window(
    application_sequence: u16,
    remaining_server_ticks: u32,
) -> (u16, u16) {
    let active_steps = repel_active_steps_for_server_ticks(remaining_server_ticks);
    repel_sequence_window_from_active_steps(application_sequence, active_steps)
}

#[inline]
pub(crate) fn repel_active_steps_for_server_ticks(remaining_server_ticks: u32) -> u32 {
    remaining_server_ticks.saturating_mul(3).saturating_add(4) / 5
}

#[inline]
pub(crate) fn repel_sequence_window_from_active_steps(
    application_sequence: u16,
    active_steps: u32,
) -> (u16, u16) {
    let bounded_steps = active_steps.min((1_u32 << 15) - 2) as u16;
    (
        application_sequence,
        application_sequence.wrapping_add(bounded_steps.saturating_add(1)),
    )
}

#[inline]
pub(crate) fn repel_sequence_window_contains(window: (u16, u16), sequence: u16) -> bool {
    let span = window.1.wrapping_sub(window.0);
    let distance = sequence.wrapping_sub(window.0);
    span > 1 && span < (1 << 15) && distance > 0 && distance < span
}

/// Whether a forward logical cursor has reached the exclusive end of an
/// installed Repel window. Callers retire the window at the first true result;
/// retaining it for a complete u16 epoch would make the same wire sequences
/// active a second time.
#[inline]
pub(crate) fn repel_sequence_window_end_reached(window: (u16, u16), current_sequence: u16) -> bool {
    current_sequence.wrapping_sub(window.1) < (1 << 15)
}

/// Server positions may commit several authenticated logical records at once.
/// This proves that their bounded forward interval includes or passes the
/// exclusive end, so the exact window can be retired only after the complete
/// replay transaction succeeds.
#[inline]
pub(crate) fn repel_sequence_window_end_crossed(
    window: (u16, u16),
    previous_sequence: u16,
    current_sequence: u16,
) -> bool {
    let advanced = current_sequence.wrapping_sub(previous_sequence);
    let distance_to_end = window.1.wrapping_sub(previous_sequence);
    advanced > 0 && advanced < (1 << 15) && distance_to_end <= advanced
}

#[inline]
pub(crate) fn rocket_activation_sequence_after_movement(current_input_sequence: u16) -> u16 {
    current_input_sequence.wrapping_add(1)
}

#[inline]
pub(crate) fn rocket_activation_pulse_for_sequence(
    pending_activation_sequence: Option<u16>,
    current_input_sequence: u16,
) -> bool {
    pending_activation_sequence == Some(current_input_sequence)
}

/// Return the number of logical movement samples consumed from an activation
/// through `observed_sequence`, inclusive. The half-range guard makes the
/// subtraction unambiguous across `u16` wrap.
#[inline]
pub(crate) fn rocket_elapsed_logical_steps(
    activation_sequence: u16,
    observed_sequence: u16,
) -> Option<u32> {
    let offset = observed_sequence.wrapping_sub(activation_sequence);
    (offset < (1 << 15)).then_some(u32::from(offset) + 1)
}

/// The client decrements RocketTime before each 60 Hz movement sample. With
/// server lifetimes expressed in 100 Hz ticks, sample `steps` remains active
/// exactly while `steps * 100 / 60 < lifetime_ticks`.
#[inline]
pub(crate) fn rocket_sequence_is_active(
    activation_sequence: u16,
    lifetime_ticks: u32,
    sequence: u16,
) -> bool {
    rocket_elapsed_logical_steps(activation_sequence, sequence)
        .is_some_and(|steps| u64::from(steps) * 5 < u64::from(lifetime_ticks) * 3)
}

/// Keep movement on the same server-authorized logical Rocket interval even
/// when the presentation timer has already reconciled to zero at its wall
/// deadline. A headed browser can produce fewer than 60 logical records per
/// wall second; the wall timer controls presentation and the private snapshot
/// active flag, while the sequence interval remains the deterministic movement
/// and retrigger authority until its exact final sample. A locally predicted
/// activation still uses the timer until a matching authoritative sequence
/// arrives.
#[inline]
pub(crate) fn rocket_movement_is_active(
    predicted_timer_active: bool,
    authoritative_activation_sequence: Option<u16>,
    lifetime_ticks: u32,
    current_sequence: u16,
) -> bool {
    predicted_timer_active
        || authoritative_activation_sequence.is_some_and(|activation_sequence| {
            rocket_sequence_is_active(activation_sequence, lifetime_ticks, current_sequence)
        })
}

/// Remaining 100 Hz ticks after the observed logical movement sample. Round
/// upward only for the wire deadline representation; logical activity and
/// retrigger eligibility continue to use `rocket_sequence_is_active` exactly.
#[inline]
pub(crate) fn rocket_remaining_ticks_after_sequence(
    activation_sequence: u16,
    lifetime_ticks: u32,
    observed_sequence: u16,
) -> Option<u32> {
    let steps = u64::from(rocket_elapsed_logical_steps(
        activation_sequence,
        observed_sequence,
    )?);
    let remaining_thirds = u64::from(lifetime_ticks)
        .saturating_mul(3)
        .saturating_sub(steps.saturating_mul(5));
    Some(((remaining_thirds + 2) / 3).min(u64::from(u32::MAX)) as u32)
}

/// Limit a matching self-Rocket snapshot by both clocks the client already
/// owns: its decremented prediction and the activation-to-input sequence span.
/// A delayed or reordered authoritative deadline may shorten the timer but can
/// never resurrect elapsed RocketTime.
pub(crate) fn reconciled_rocket_timer_seconds(
    predicted_seconds: f32,
    lifetime_seconds: f32,
    logical_step_seconds: f32,
    activation_sequence: Option<u16>,
    last_consumed_input_sequence: u16,
    authoritative_remaining_seconds: f32,
    authoritative_active: bool,
) -> f32 {
    if !authoritative_active {
        return 0.0;
    }
    let Some(activation_sequence) = activation_sequence else {
        return 0.0;
    };
    let Some(elapsed_steps) =
        rocket_elapsed_logical_steps(activation_sequence, last_consumed_input_sequence)
    else {
        return 0.0;
    };
    let sequence_remaining =
        (lifetime_seconds - elapsed_steps as f32 * logical_step_seconds).max(0.0);
    predicted_seconds
        .max(0.0)
        .min(sequence_remaining)
        .min(authoritative_remaining_seconds.max(0.0))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RocketMovementStep {
    pub(crate) active: bool,
    pub(crate) activation_pulse: bool,
}

/// Sample one logical movement record before the ordered weapon phase. The
/// strict decrement-before-active check mirrors the reference game and the server's
/// logical authorization interval.
pub(crate) fn begin_rocket_movement_step(
    timer_seconds: &mut f32,
    dt_seconds: f32,
    pending_activation_sequence: Option<u16>,
    current_input_sequence: u16,
) -> RocketMovementStep {
    *timer_seconds -= dt_seconds;
    RocketMovementStep {
        active: *timer_seconds > 0.0,
        activation_pulse: rocket_activation_pulse_for_sequence(
            pending_activation_sequence,
            current_input_sequence,
        ),
    }
}

/// Apply one reference-game movement-stat prize to the live value. Clamping each
/// operation preserves partial-cap reversals (for example 325 -> 300 with a
/// 25-point speed step), which an integer upgrade count cannot represent.
pub(crate) fn stat_value_after_prize(
    current: f32,
    initial: f32,
    step: f32,
    max_value: f32,
    negative: bool,
) -> f32 {
    let minimum = initial.min(max_value);
    let step = step.max(0.0);
    if negative {
        (current - step).max(minimum)
    } else {
        (current + step).min(max_value)
    }
}

pub(crate) fn shrapnel_prize_amount_after(
    current: u8,
    rate: u8,
    max_amount: u8,
    negative: bool,
) -> u8 {
    if rate == 0 {
        return current;
    }
    if negative {
        if current >= rate {
            current - rate
        } else {
            current
        }
    } else {
        current.saturating_add(rate).min(max_amount)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FireEnergyStatus {
    pub(crate) flagger_active: bool,
    pub(crate) flagger_cost_multiplier: f32,
    pub(crate) super_active: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FireEnergyCost {
    /// Cost used for Continuum's strict `required < energy` eligibility gate.
    pub(crate) required: f32,
    /// Cost deducted after an accepted shot. Super changes only this value.
    pub(crate) charged: f32,
}

/// The reference game computes the leveled base cost and flagger multiplier before it
/// checks eligibility. Super makes an accepted shot free; it does not bypass
/// the normal strict energy requirement.
pub(crate) fn effective_fire_energy_cost(
    base_cost: f32,
    status: FireEnergyStatus,
) -> FireEnergyCost {
    let mut cost = base_cost.max(0.0);
    if status.flagger_active {
        cost *= status.flagger_cost_multiplier.max(0.0);
    }
    FireEnergyCost {
        required: cost,
        charged: if status.super_active { 0.0 } else { cost },
    }
}

/// Continuum's firing boundary is strict: energy exactly equal to cost does
/// not fire. Callers include any fractional recharge remainder in `energy`.
pub(crate) fn can_fire(energy: f32, cost: f32) -> bool {
    energy > cost.max(0.0)
}

pub(crate) fn effective_weapon_level(level: u8, flagger_upgrade: bool, maximum: u8) -> u8 {
    if flagger_upgrade {
        level.saturating_add(1).min(maximum)
    } else {
        level
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrizeEligibilityContext {
    InitialBounty,
    Multiprize,
    DamageLoss,
}

/// Context-aware reference-game prize eligibility. The three exclusion lists are
/// intentionally different and must not be collapsed into one generic list.
pub(crate) fn prize_eligible(context: PrizeEligibilityContext, prize_id: i16) -> bool {
    !match context {
        PrizeEligibilityContext::InitialBounty => {
            matches!(prize_id, 7 | 13 | 14 | 17 | 18 | 26)
        }
        PrizeEligibilityContext::Multiprize => {
            matches!(prize_id, 7 | 14 | 17 | 18 | 25 | 26)
        }
        PrizeEligibilityContext::DamageLoss => {
            matches!(prize_id, 7 | 13 | 14 | 17 | 18 | 25)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RulesVector {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

impl RulesVector {
    pub(crate) const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub(crate) const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }

    fn scale(self, factor: f32) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }
}

/// The reference game's repel uses an axis-aligned square range, then replaces target
/// velocity with a normalized vector away from the source. Returning the
/// velocity instead of mutating engine vectors keeps the exact rule shared by
/// the browser and authoritative server observer.
pub(crate) fn repel_velocity_after(
    origin: RulesVector,
    target: RulesVector,
    speed: f32,
    radius: f32,
) -> Option<RulesVector> {
    let delta = RulesVector::new(target.x - origin.x, target.y - origin.y);
    if delta.x.abs() >= radius || delta.y.abs() >= radius {
        return None;
    }
    let length = (delta.x * delta.x + delta.y * delta.y).sqrt();
    if length <= f32::EPSILON {
        return Some(RulesVector::ZERO);
    }
    Some(delta.scale(speed.max(0.0) / length))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WeaponDataView {
    pub(crate) weapon_type: u8,
    /// One-indexed gameplay/render level, 1..=4.
    pub(crate) level: u8,
    /// Raw two-bit wire level, 0..=3.
    pub(crate) raw_level: u8,
    pub(crate) shrapnel_bouncing: bool,
    pub(crate) shrapnel_level: u8,
    pub(crate) shrapnel_count: u8,
    pub(crate) alternate: bool,
}

impl WeaponDataView {
    pub(crate) fn decode(weapon_data: u16) -> Self {
        let raw_level = ((weapon_data >> 5) & 0x03) as u8;
        Self {
            weapon_type: (weapon_data & 0x1f) as u8,
            level: raw_level + 1,
            raw_level,
            shrapnel_bouncing: weapon_data & 0x80 != 0,
            shrapnel_level: (((weapon_data >> 8) & 0x03) as u8) + 1,
            shrapnel_count: ((weapon_data >> 10) & 0x1f) as u8,
            alternate: weapon_data & 0x8000 != 0,
        }
    }
}

pub(crate) fn encode_weapon_data(
    weapon_type: u8,
    level: u8,
    shrapnel_bouncing: bool,
    shrapnel_level: u8,
    shrapnel_count: u8,
    alternate: bool,
) -> u16 {
    let level = level.clamp(1, 4) - 1;
    let shrapnel_level = shrapnel_level.clamp(1, 4) - 1;
    (u16::from(weapon_type) & 0x1f)
        | (u16::from(level) << 5)
        | (u16::from(shrapnel_bouncing) << 7)
        | (u16::from(shrapnel_level) << 8)
        | (u16::from(shrapnel_count.min(31)) << 10)
        | (u16::from(alternate) << 15)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProjectileInitializationTuning {
    pub(crate) bullet_speed: f32,
    pub(crate) bomb_speed: f32,
    pub(crate) burst_speed: f32,
    pub(crate) ship_radius: f32,
    /// Raw Continuum `MultiFireAngle` setting. The reference game divides this by
    /// 111 degrees only at launch; retaining the integer avoids a platform
    /// libm/f32 reconstruction disagreement.
    pub(crate) multifire_angle: u16,
    pub(crate) double_barrel: bool,
    pub(crate) burst_count: usize,
    pub(crate) bomb_bounce_count: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProjectileInitialization {
    pub(crate) position: RulesVector,
    pub(crate) velocity: RulesVector,
    pub(crate) level: u8,
    pub(crate) raw_level: u8,
    pub(crate) bounces_remaining: u8,
    pub(crate) behavior: ProjectileBehavior,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectileBehavior {
    Standard,
    BouncingBullet,
    /// Bursts cannot damage a player until their first wall collision.
    Burst {
        active: bool,
    },
}

pub(crate) fn gun_projectile_layout(
    orientation: f32,
    canonical_center_heading: Option<RulesVector>,
    double_barrel: bool,
    multifire: bool,
    multifire_angle: u16,
    ship_radius: f32,
) -> Vec<(RulesVector, RulesVector)> {
    let center_heading =
        canonical_center_heading.unwrap_or_else(|| orientation_to_heading(orientation));
    let mut layout = Vec::with_capacity(if double_barrel { 4 } else { 3 });
    if double_barrel {
        let side = RulesVector::new(-center_heading.y, center_heading.x).scale(ship_radius * 0.75);
        // The reference game passes each perpendicular offset through s32 before
        // subtracting/adding it to the packet's pixel position. Truncate the
        // offset itself (toward zero), preserving the symmetric barrel pair.
        let side = RulesVector::new(side.x.trunc(), side.y.trunc());
        layout.push((center_heading, side.scale(-1.0)));
        layout.push((center_heading, side));
    } else {
        layout.push((center_heading, RulesVector::ZERO));
    }
    if multifire {
        layout.push((
            rotate_heading_raw(center_heading, multifire_angle, false),
            RulesVector::ZERO,
        ));
        layout.push((
            rotate_heading_raw(center_heading, multifire_angle, true),
            RulesVector::ZERO,
        ));
    }
    layout
}

/// Initialize every projectile represented by one accepted weapon packet.
/// This includes double-barrel/multifire layouts and burst fan-out, preventing
/// the authoritative registry from silently representing only one visual shot.
pub(crate) fn initialize_projectiles(
    weapon_data: u16,
    ship_position: RulesVector,
    ship_velocity: RulesVector,
    orientation: f32,
    tuning: ProjectileInitializationTuning,
) -> Vec<ProjectileInitialization> {
    initialize_projectiles_with_heading(
        weapon_data,
        ship_position,
        ship_velocity,
        orientation,
        None,
        tuning,
    )
}

fn initialize_projectiles_with_heading(
    weapon_data: u16,
    ship_position: RulesVector,
    ship_velocity: RulesVector,
    orientation: f32,
    canonical_heading: Option<RulesVector>,
    tuning: ProjectileInitializationTuning,
) -> Vec<ProjectileInitialization> {
    let weapon = WeaponDataView::decode(weapon_data);
    let heading = canonical_heading.unwrap_or_else(|| orientation_to_heading(orientation));
    match weapon.weapon_type {
        1 | 2 => {
            gun_projectile_layout(
                orientation,
                canonical_heading,
                tuning.double_barrel,
                weapon.alternate,
                tuning.multifire_angle,
                tuning.ship_radius,
            )
            .into_iter()
            .map(|(projectile_heading, offset)| ProjectileInitialization {
                position: ship_position.add(offset),
                velocity: ship_velocity.add(projectile_heading.scale(tuning.bullet_speed)),
                level: weapon.level,
                raw_level: weapon.raw_level,
                // Bouncing bullets use their weapon kind rather than a
                // finite counter. This field is reserved for bomb bounces.
                bounces_remaining: 0,
                behavior: if weapon.weapon_type == 2 {
                    ProjectileBehavior::BouncingBullet
                } else {
                    ProjectileBehavior::Standard
                },
            })
            .collect()
        }
        3 | 4 if weapon.alternate => vec![ProjectileInitialization {
            position: ship_position,
            velocity: RulesVector::ZERO,
            level: weapon.level,
            raw_level: weapon.raw_level,
            bounces_remaining: 0,
            behavior: ProjectileBehavior::Standard,
        }],
        3 | 4 => vec![ProjectileInitialization {
            position: ship_position,
            velocity: ship_velocity.add(heading.scale(tuning.bomb_speed)),
            level: weapon.level,
            raw_level: weapon.raw_level,
            bounces_remaining: tuning.bomb_bounce_count,
            behavior: ProjectileBehavior::Standard,
        }],
        7 => (0..tuning.burst_count)
            .map(|index| ProjectileInitialization {
                position: ship_position,
                // The reference game deliberately truncates at both integer divisions:
                // `(i * 40000) / count * 9` millidegrees.
                velocity: burst_heading(index, tuning.burst_count).scale(tuning.burst_speed),
                level: weapon.level,
                raw_level: weapon.raw_level,
                bounces_remaining: 0,
                behavior: ProjectileBehavior::Burst { active: false },
            })
            .collect(),
        8 => vec![ProjectileInitialization {
            position: ship_position,
            velocity: ship_velocity.add(heading.scale(tuning.bomb_speed)),
            level: weapon.level,
            raw_level: weapon.raw_level,
            bounces_remaining: 0,
            behavior: ProjectileBehavior::Standard,
        }],
        _ => Vec::new(),
    }
}

/// Canonical adapter used by both the local prediction and authoritative
/// server paths. Projectile headings are generated from the 40-step wire
/// direction, never from a client's higher-precision visual orientation.
pub(crate) fn initialize_projectiles_for_protocol_direction(
    weapon_data: u16,
    ship_position: RulesVector,
    ship_velocity: RulesVector,
    direction: u8,
    tuning: ProjectileInitializationTuning,
) -> Vec<ProjectileInitialization> {
    let ship_position = RulesVector::new(
        crate::continuum_position::protocol_position_coord(ship_position.x) as f32,
        crate::continuum_position::protocol_position_coord(ship_position.y) as f32,
    );
    let ship_velocity = RulesVector::new(
        crate::continuum_position::protocol_velocity_pixels_per_second(
            crate::continuum_position::protocol_velocity_component(ship_velocity.x),
        ),
        crate::continuum_position::protocol_velocity_pixels_per_second(
            crate::continuum_position::protocol_velocity_component(ship_velocity.y),
        ),
    );
    let orientation = crate::continuum_position::protocol_direction_to_orientation(direction);
    let (heading_x, heading_y) =
        crate::continuum_position::protocol_direction_heading_components(direction);
    initialize_projectiles_with_heading(
        weapon_data,
        ship_position,
        ship_velocity,
        orientation,
        Some(RulesVector::new(heading_x, heading_y)),
        tuning,
    )
}

fn rotate_heading_raw(heading: RulesVector, multifire_angle: u16, positive: bool) -> RulesVector {
    let numerator = if positive {
        i64::from(multifire_angle)
    } else {
        -i64::from(multifire_angle)
    };
    let (sin, cos) = deterministic_sin_cos_turn_fraction(numerator, 111 * 360);
    RulesVector::new(
        heading.x * cos - heading.y * sin,
        heading.x * sin + heading.y * cos,
    )
}

fn orientation_to_heading(orientation: f32) -> RulesVector {
    let direction = crate::continuum_position::protocol_direction(orientation);
    let (x, y) = crate::continuum_position::protocol_direction_heading_components(direction);
    RulesVector::new(x, y)
}

fn burst_heading(index: usize, count: usize) -> RulesVector {
    let count = count.max(1) as i64;
    let orientation_millidegrees = (index as i64 * 40_000) / count * 9;
    let (sin, cos) = deterministic_sin_cos_turn_fraction(orientation_millidegrees, 360_000);
    RulesVector::new(sin, -cos)
}

/// The reference game's CalculateRngSeed uses the packed, zero-indexed parent level.
/// Callers use our one-indexed level; convert it here, independently of the
/// child fragment's power. Velocity is the initial explosive velocity, after
/// launch speed has been added (zero for a mine).
pub(crate) fn shrapnel_rng_seed(
    position: RulesVector,
    velocity: RulesVector,
    count: usize,
    parent_level: u8,
    frequency: u16,
) -> u32 {
    let x = crate::continuum_position::protocol_position_coord(position.x) as u32;
    let y = crate::continuum_position::protocol_position_coord(position.y) as u32;
    // GenerateWeapon casts the final launch velocity to s32; it does not
    // round it with the outgoing player-position packet's i16 encoder.
    let velocity_x = (velocity.x * 10.0).trunc() as i32 as u32;
    let velocity_y = (velocity.y * 10.0).trunc() as i32 as u32;
    (count as u32)
        .wrapping_add(u32::from(crate::continuum_weapon::weapon_level_to_bits(
            parent_level,
        )))
        .wrapping_add(x.wrapping_mul(1000))
        .wrapping_add(y.wrapping_mul(1000))
        .wrapping_add(velocity_x)
        .wrapping_add(velocity_y)
        .wrapping_add(u32::from(frequency))
}

/// Reference integer-angle distribution, with deterministic native/WASM
/// trigonometry. Each child is independent; none inherits parent velocity.
pub(crate) fn shrapnel_direction(
    index: usize,
    count: usize,
    random: bool,
    seed: u32,
) -> RulesVector {
    let orientation_units = if random {
        let mut seed = seed as i32;
        let mut roll = 0;
        for _ in 0..=index {
            roll = crate::vie::vie_rng_next(&mut seed) % 40_000;
        }
        i64::from(roll)
    } else {
        (index as i64 * 40_000) / count.max(1) as i64
    };
    let (sin, cos) = deterministic_sin_cos_turn_fraction(orientation_units * 9, 360_000);
    let length = (sin * sin + cos * cos).sqrt();
    RulesVector::new(sin / length, -cos / length)
}

/// Geometry and child power for an explosive's fragment fan. Source identity,
/// lifetime, initial solid-tile rejection and damage transactions belong to
/// the caller. This does not admit a type-15 client fire request.
pub(crate) fn initialize_shrapnel_projectiles(
    parent_weapon_data: u16,
    explosion_position: RulesVector,
    seed: u32,
    speed: f32,
    random: bool,
) -> Vec<ProjectileInitialization> {
    let parent = WeaponDataView::decode(parent_weapon_data);
    if !matches!(parent.weapon_type, 3 | 4 | 8) {
        return Vec::new();
    }
    (0..usize::from(parent.shrapnel_count))
        .map(|index| ProjectileInitialization {
            position: explosion_position,
            velocity: shrapnel_direction(index, usize::from(parent.shrapnel_count), random, seed)
                .scale(speed),
            level: parent.shrapnel_level,
            raw_level: parent.shrapnel_level - 1,
            bounces_remaining: 0,
            behavior: if parent.shrapnel_bouncing {
                ProjectileBehavior::BouncingBullet
            } else {
                ProjectileBehavior::Standard
            },
        })
        .collect()
}

/// Deterministic integer CORDIC. Angles are exact rational turns and every
/// operation before the final Q30 conversion is integer-only, so native and
/// WASM produce identical bits without target `sin`/`cos` implementations.
fn deterministic_sin_cos_turn_fraction(numerator: i64, denominator: i64) -> (f32, f32) {
    debug_assert!(denominator > 0);
    let full_turn = 1_i64 << 32;
    let mut turn =
        ((i128::from(numerator) * i128::from(full_turn)) / i128::from(denominator)) as i64;
    turn %= full_turn;
    if turn < 0 {
        turn += full_turn;
    }
    if turn == 0 {
        return (0.0, 1.0);
    }
    if turn == full_turn / 4 {
        return (1.0, 0.0);
    }
    if turn == full_turn / 2 {
        return (0.0, -1.0);
    }
    if turn == full_turn * 3 / 4 {
        return (-1.0, 0.0);
    }

    let mut signed = if turn > full_turn / 2 {
        turn - full_turn
    } else {
        turn
    };
    let mut sign = 1_i64;
    if signed > full_turn / 4 {
        signed -= full_turn / 2;
        sign = -1;
    } else if signed < -(full_turn / 4) {
        signed += full_turn / 2;
        sign = -1;
    }

    const CORDIC_GAIN_Q30: i64 = 652_032_874;
    const ATAN_TURNS_Q32: [i64; 31] = [
        0x20000000, 0x12e4051e, 0x09fb385b, 0x051111d4, 0x028b0d43, 0x0145d7e1, 0x00a2f61e,
        0x00517c55, 0x0028be53, 0x00145f2f, 0x000a2f98, 0x000517cc, 0x00028be6, 0x000145f3,
        0x0000a2fa, 0x0000517d, 0x000028be, 0x0000145f, 0x00000a30, 0x00000518, 0x0000028c,
        0x00000146, 0x000000a3, 0x00000051, 0x00000029, 0x00000014, 0x0000000a, 0x00000005,
        0x00000003, 0x00000001, 0x00000001,
    ];
    let mut x = CORDIC_GAIN_Q30;
    let mut y = 0_i64;
    let mut z = signed;
    for (shift, angle) in ATAN_TURNS_Q32.into_iter().enumerate() {
        let old_x = x;
        if z >= 0 {
            x -= y >> shift;
            y += old_x >> shift;
            z -= angle;
        } else {
            x += y >> shift;
            y -= old_x >> shift;
            z += angle;
        }
    }
    const Q30: f32 = 1_073_741_824.0;
    let sin = ((y * sign) as f32 / Q30).clamp(-1.0, 1.0);
    let cos = ((x * sign) as f32 / Q30).clamp(-1.0, 1.0);
    (sin, cos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrapnel_seed_uses_raw_parent_level_and_signed_wire_velocity() {
        let position = RulesVector::new(10.0, 20.0);
        let velocity = RulesVector::new(3.4, 5.6);
        // WeaponManager::CalculateRngSeed: count + raw level + x*1000
        // + y*1000 + initial weapon velocity (tenths px/s) + frequency.
        assert_eq!(shrapnel_rng_seed(position, velocity, 4, 2, 7), 30_102);
        for level in 1..=4 {
            assert_eq!(
                shrapnel_rng_seed(position, velocity, 4, level, 7),
                30_101 + u32::from(level - 1)
            );
        }
        assert_eq!(
            shrapnel_rng_seed(RulesVector::ZERO, RulesVector::new(-3.4, -5.6), 4, 1, 7),
            u32::MAX - 78
        );
        assert_eq!(
            shrapnel_rng_seed(position, RulesVector::new(3.49, -8.25), 4, 2, 7),
            29_964
        );
    }

    #[test]
    fn shrapnel_directions_match_retained_reference_game_oracle() {
        let fixture = include_str!("../../tests/fixtures/reference_game_shrapnel_directions.csv");
        let mut checked = 0;
        for row in fixture.lines().filter(|row| !row.starts_with('#')) {
            let fields: Vec<_> = row.split(',').collect();
            let random = fields[0] == "1";
            let seed = fields[1].parse().unwrap();
            let count = fields[2].parse().unwrap();
            let index = fields[3].parse().unwrap();
            let expected_x: f32 = fields[4].parse().unwrap();
            let expected_y: f32 = fields[5].parse().unwrap();
            let actual = shrapnel_direction(index, count, random, seed);
            assert!(
                (actual.x - expected_x).abs() < 0.000_001
                    && (actual.y - expected_y).abs() < 0.000_001,
                "the reference game's direction mismatch: {row}; actual={actual:?}"
            );
            checked += 1;
        }
        assert_eq!(checked, 376);
    }

    #[test]
    fn shrapnel_initializer_preserves_parent_child_traits_for_every_wire_count() {
        let position = RulesVector::new(100.5, 200.25);
        for parent_kind in [3, 4, 8] {
            for count in 0..=31 {
                for level in 1..=4 {
                    for bouncing in [false, true] {
                        let parent =
                            encode_weapon_data(parent_kind, 2, bouncing, level, count, false);
                        let children =
                            initialize_shrapnel_projectiles(parent, position, 123, 250.0, true);
                        assert_eq!(children.len(), usize::from(count));
                        for child in children {
                            assert_eq!(child.position, position);
                            assert_eq!(child.level, level);
                            assert_eq!(child.raw_level, level - 1);
                            assert_eq!(child.bounces_remaining, 0);
                            assert_eq!(
                                child.behavior,
                                if bouncing {
                                    ProjectileBehavior::BouncingBullet
                                } else {
                                    ProjectileBehavior::Standard
                                }
                            );
                            let speed = (child.velocity.x * child.velocity.x
                                + child.velocity.y * child.velocity.y)
                                .sqrt();
                            assert!((speed - 250.0).abs() < 0.000_1);
                        }
                    }
                }
            }
        }
        for kind in [1, 2, 5, 6, 7, 15] {
            assert!(
                initialize_shrapnel_projectiles(
                    encode_weapon_data(kind, 2, true, 3, 8, false),
                    position,
                    123,
                    250.0,
                    false
                )
                .is_empty()
            );
        }
    }

    #[test]
    fn weapon_sequence_cooldown_proof_is_exact_forward_and_wrap_safe() {
        assert!(!weapon_sequence_cooldown_elapsed(100, 114, 24));
        assert!(weapon_sequence_cooldown_elapsed(100, 115, 24));
        assert!(weapon_sequence_cooldown_elapsed(u16::MAX - 5, 9, 24));
        assert!(!weapon_sequence_cooldown_elapsed(100, 100, 24));
        assert!(!weapon_sequence_cooldown_elapsed(100, 99, 24));
    }

    fn door_overlap_fixture(
        tile: u8,
        x: f32,
        y: f32,
        radius: f32,
        before_seed: u8,
        after_seed: u8,
    ) -> bool {
        newly_closed_door_overlap(
            x,
            y,
            radius,
            4,
            16.0,
            before_seed,
            after_seed,
            |tile_x, tile_y| ((tile_x, tile_y) == (1, 1)).then_some(tile),
            |candidate, seed| {
                if (DYNAMIC_DOOR_TILE_MIN..=DYNAMIC_DOOR_TILE_MAX).contains(&candidate) {
                    seed & 1 != 0
                } else {
                    candidate != 0
                }
            },
        )
    }

    #[test]
    fn newly_closed_door_authority_rejects_walls_already_closed_and_no_change() {
        assert!(!door_overlap_fixture(1, 24.0, 24.0, 8.0, 0, 1));
        assert!(!door_overlap_fixture(162, 24.0, 24.0, 8.0, 1, 1));
        assert!(!door_overlap_fixture(162, 24.0, 24.0, 8.0, 0, 0));
        assert!(!door_overlap_fixture(162, 24.0, 24.0, 8.0, 1, 0));
    }

    #[test]
    fn newly_closed_door_golden_uses_live_center_and_reference_operand_order() {
        assert!(door_overlap_fixture(162, 24.9, 24.9, 1.0, 0, 1));
        // Door (1,1) spans [16,32]. Player hull is the first operand, so
        // contact at its maximum edge is inclusive and contact at its minimum
        // edge is exclusive, exactly like the reference game's Map + Math.
        assert!(door_overlap_fixture(162, 12.0, 24.0, 4.0, 0, 1));
        assert!(!door_overlap_fixture(162, 36.0, 24.0, 4.0, 0, 1));
        assert!(door_overlap_fixture(162, 24.0, 12.0, 4.0, 0, 1));
        assert!(!door_overlap_fixture(162, 24.0, 36.0, 4.0, 0, 1));
        assert!(door_overlap_fixture(162, 12.001, 12.001, 4.0, 0, 1));
        assert!(!door_overlap_fixture(162, 36.0, 36.0, 4.0, 0, 1));
        assert!(door_overlap_fixture(162, 35.999, 35.999, 4.0, 0, 1));
        assert!(!door_overlap_fixture(162, 11.999, 24.0, 4.0, 0, 1));
    }

    #[test]
    fn rocket_sequence_relative_remainder_is_wrap_safe_and_can_be_wholly_past() {
        let start = u16::MAX - 10;
        let observed = start.wrapping_add(24);
        assert_eq!(rocket_elapsed_logical_steps(start, observed), Some(25));
        assert!(rocket_sequence_is_active(start, 1_000, observed));
        assert_eq!(
            rocket_remaining_ticks_after_sequence(start, 1_000, observed),
            Some(959),
        );

        let short_lifetime = 30;
        assert!(!rocket_sequence_is_active(start, short_lifetime, observed));
        assert_eq!(
            rocket_remaining_ticks_after_sequence(start, short_lifetime, observed),
            Some(0),
            "a wholly past delayed pulse cannot resurrect a current timer",
        );
        assert_eq!(rocket_elapsed_logical_steps(100, 99), None);
    }

    #[test]
    fn authoritative_logical_rocket_keeps_movement_exact_after_wall_timer_zero() {
        let activation = u16::MAX - 20;
        let last_active = activation.wrapping_add(598);
        assert!(rocket_movement_is_active(
            false,
            Some(activation),
            1_000,
            last_active,
        ));
        assert!(rocket_movement_is_active(true, None, 1_000, activation,));
        assert!(!rocket_movement_is_active(
            false,
            Some(activation),
            1_000,
            last_active.wrapping_add(1),
        ));
        assert!(!rocket_movement_is_active(
            false,
            Some(activation),
            1_000,
            activation.wrapping_sub(1),
        ));
    }

    #[test]
    fn rocket_timer_reconciliation_only_shortens_matching_prediction() {
        let predicted = 9.4;
        assert_eq!(
            reconciled_rocket_timer_seconds(
                predicted,
                10.0,
                1.0 / 60.0,
                Some(100),
                124,
                10.0,
                true,
            ),
            predicted,
        );
        assert_eq!(
            reconciled_rocket_timer_seconds(predicted, 10.0, 1.0 / 60.0, Some(100), 124, 9.0, true,),
            9.0,
        );
        assert_eq!(
            reconciled_rocket_timer_seconds(
                predicted,
                10.0,
                1.0 / 60.0,
                Some(100),
                124,
                10.0,
                false,
            ),
            0.0,
        );
    }

    #[test]
    fn golden_repel_rule_uses_square_range_and_velocity_replacement() {
        assert_eq!(
            repel_velocity_after(RulesVector::ZERO, RulesVector::new(3.0, 4.0), 500.0, 10.0,),
            Some(RulesVector::new(300.0, 400.0)),
        );
        let diagonal_speed =
            repel_velocity_after(RulesVector::ZERO, RulesVector::new(9.0, 9.0), 500.0, 10.0)
                .map(|velocity| (velocity.x * velocity.x + velocity.y * velocity.y).sqrt())
                .expect("diagonal target should be inside the square range");
        assert!((diagonal_speed - 500.0).abs() < 0.001);
        assert_eq!(
            repel_velocity_after(RulesVector::ZERO, RulesVector::new(10.0, 0.0), 500.0, 10.0,),
            None,
        );
        assert_eq!(
            repel_velocity_after(RulesVector::ZERO, RulesVector::ZERO, 500.0, 10.0),
            Some(RulesVector::ZERO),
        );
    }

    #[test]
    fn golden_fire_cost_fixture_covers_status_and_exact_boundary() {
        let plain = FireEnergyStatus {
            flagger_cost_multiplier: 0.8,
            ..Default::default()
        };
        let flagger = FireEnergyStatus {
            flagger_active: true,
            ..plain
        };
        let super_flagger = FireEnergyStatus {
            super_active: true,
            ..flagger
        };
        assert_eq!(
            effective_fire_energy_cost(125.0, plain),
            FireEnergyCost {
                required: 125.0,
                charged: 125.0,
            }
        );
        assert_eq!(
            effective_fire_energy_cost(125.0, flagger),
            FireEnergyCost {
                required: 100.0,
                charged: 100.0,
            }
        );
        assert_eq!(
            effective_fire_energy_cost(125.0, super_flagger),
            FireEnergyCost {
                required: 100.0,
                charged: 0.0,
            }
        );
        assert!(!can_fire(100.0, 100.0));
        assert!(can_fire(100.001, 100.0));
        assert!(!can_fire(0.0, 0.0));
        assert_eq!(effective_weapon_level(3, true, 4), 4);
        assert_eq!(effective_weapon_level(4, true, 4), 4);
    }

    #[test]
    fn golden_prize_context_fixture_pins_all_protocol_ids() {
        let initial_excluded = [7, 13, 14, 17, 18, 26];
        let multi_excluded = [7, 14, 17, 18, 25, 26];
        let damage_excluded = [7, 13, 14, 17, 18, 25];
        for prize_id in 1..=28 {
            assert_eq!(
                prize_eligible(PrizeEligibilityContext::InitialBounty, prize_id),
                !initial_excluded.contains(&prize_id)
            );
            assert_eq!(
                prize_eligible(PrizeEligibilityContext::Multiprize, prize_id),
                !multi_excluded.contains(&prize_id)
            );
            assert_eq!(
                prize_eligible(PrizeEligibilityContext::DamageLoss, prize_id),
                !damage_excluded.contains(&prize_id)
            );
        }
    }

    #[test]
    fn double_barrel_offsets_match_reference_game_pixel_truncation_at_all_forty_headings() {
        // Generated by compiling the actual reference-game Math at revision
        // 7ebdbc53a052fa62ff246b5f6a8b3347c395e5a4 and applying the s32 casts
        // from WeaponManager:826-838, with Chaos's 14-pixel ship radius.
        // This fixed oracle deliberately does not derive expected offsets
        // using the implementation's heading or projectile-layout helpers.
        const OFFSETS: [(i32, i32); 40] = [
            (10, 0),
            (10, 1),
            (9, 3),
            (9, 4),
            (8, 6),
            (7, 7),
            (6, 8),
            (4, 9),
            (3, 9),
            (1, 10),
            (0, 10),
            (-1, 10),
            (-3, 9),
            (-4, 9),
            (-6, 8),
            (-7, 7),
            (-8, 6),
            (-9, 4),
            (-9, 3),
            (-10, 1),
            (-10, 0),
            (-10, -1),
            (-9, -3),
            (-9, -4),
            (-8, -6),
            (-7, -7),
            (-6, -8),
            (-4, -9),
            (-3, -9),
            (-1, -10),
            (0, -10),
            (1, -10),
            (3, -9),
            (4, -9),
            (6, -8),
            (7, -7),
            (8, -6),
            (9, -4),
            (9, -3),
            (10, -1),
        ];
        let center = RulesVector::new(8200.0, 8200.0);
        for (direction, (x, y)) in OFFSETS.into_iter().enumerate() {
            for multifire in [false, true] {
                let tuning = ProjectileInitializationTuning {
                    bullet_speed: 200.0,
                    bomb_speed: 200.0,
                    burst_speed: 300.0,
                    ship_radius: 14.0,
                    multifire_angle: 500,
                    double_barrel: true,
                    burst_count: 24,
                    bomb_bounce_count: 0,
                };
                let shots = initialize_projectiles_for_protocol_direction(
                    encode_weapon_data(1, 3, false, 1, 0, multifire),
                    center,
                    RulesVector::ZERO,
                    direction as u8,
                    tuning,
                );
                assert_eq!(shots.len(), if multifire { 4 } else { 2 });
                let offset = RulesVector::new(x as f32, y as f32);
                assert_eq!(
                    shots[0].position,
                    center.add(offset.scale(-1.0)),
                    "heading {direction}"
                );
                assert_eq!(shots[1].position, center.add(offset), "heading {direction}");
                assert_eq!(shots[0].velocity, shots[1].velocity);
                for extra in &shots[2..] {
                    assert_eq!(extra.position, center, "multifire extras stay centered");
                }
            }
        }
    }

    #[test]
    fn golden_projectile_fixture_pins_levels_mines_and_multifire_layout() {
        let weapon = encode_weapon_data(2, 4, true, 3, 9, true);
        let decoded = WeaponDataView::decode(weapon);
        assert_eq!(decoded.level, 4);
        assert_eq!(decoded.raw_level, 3);
        assert_eq!(decoded.shrapnel_level, 3);
        assert_eq!(decoded.shrapnel_count, 9);

        let tuning = ProjectileInitializationTuning {
            bullet_speed: 100.0,
            bomb_speed: 80.0,
            burst_speed: 60.0,
            ship_radius: 16.0,
            multifire_angle: 1_998,
            double_barrel: true,
            burst_count: 12,
            bomb_bounce_count: 2,
        };
        let shots = initialize_projectiles(
            weapon,
            RulesVector::new(100.0, 200.0),
            RulesVector::new(10.0, 0.0),
            0.25,
            tuning,
        );
        assert_eq!(shots.len(), 4);
        assert!(shots.iter().all(|shot| shot.level == 4));
        assert!(shots.iter().all(|shot| shot.bounces_remaining == 0));
        assert!(
            shots
                .iter()
                .all(|shot| shot.behavior == ProjectileBehavior::BouncingBullet)
        );

        let mine = initialize_projectiles(
            encode_weapon_data(3, 2, false, 1, 0, true),
            RulesVector::new(100.0, 200.0),
            RulesVector::new(50.0, 50.0),
            0.0,
            tuning,
        );
        assert_eq!(mine[0].velocity, RulesVector::ZERO);

        let bomb = initialize_projectiles(
            encode_weapon_data(3, 3, false, 1, 0, false),
            RulesVector::new(100.0, 200.0),
            RulesVector::new(10.0, 0.0),
            0.25,
            tuning,
        );
        assert_eq!(bomb.len(), 1);
        assert_eq!(bomb[0].level, 3);
        assert_eq!(bomb[0].raw_level, 2);
        assert_eq!(bomb[0].bounces_remaining, 2);
        close(bomb[0].velocity.x, 90.0);
        close(bomb[0].velocity.y, 0.0);

        let burst = initialize_projectiles(
            encode_weapon_data(7, 1, false, 1, 0, false),
            RulesVector::new(100.0, 200.0),
            RulesVector::new(10.0, 0.0),
            0.25,
            tuning,
        );
        assert_eq!(burst.len(), 12);
        assert!(
            burst
                .iter()
                .all(|shot| shot.behavior == ProjectileBehavior::Burst { active: false })
        );
        close(burst[0].velocity.x, 0.0);
        close(burst[0].velocity.y, -60.0);
    }

    #[test]
    fn a_burst_activation_fans_burst_count_distinct_evenly_spread_pieces() {
        // The QA report was "a ton of burst circles that stay together as a clump". The fan-out is authored:
        // one activation produces `BurstShrapnel` independent pieces, each with its own heading dividing a full
        // turn evenly, all launched from the ship position with zero inherited velocity. The heading helper is
        // pinned against the reference elsewhere (`deterministic_projectile_trig_tracks_reference_game_reference_for_
        // all_wire_settings`); this pins the activation shape - count, shared origin, no inherited velocity, and
        // one distinct heading per piece - so a spawn or rendering regression cannot hide behind the helper.
        let count = 24usize;
        let speed = 300.0f32;
        let tuning = ProjectileInitializationTuning {
            bullet_speed: 100.0,
            bomb_speed: 80.0,
            burst_speed: speed,
            ship_radius: 16.0,
            multifire_angle: 1_998,
            double_barrel: false,
            burst_count: count,
            bomb_bounce_count: 2,
        };
        let origin = RulesVector::new(1234.0, 5678.0);
        let shots = initialize_projectiles(
            encode_weapon_data(7, 1, false, 1, 0, false),
            origin,
            // A deliberately non-zero inherited velocity: the reference gives
            // burst pieces zero inherited velocity, so this must be ignored.
            RulesVector::new(37.0, -11.0),
            0.25,
            tuning,
        );
        assert_eq!(
            shots.len(),
            count,
            "one activation yields exactly BurstShrapnel pieces"
        );
        for (index, shot) in shots.iter().enumerate() {
            assert_eq!(shot.position, origin, "every piece originates at the ship");
            assert_eq!(shot.behavior, ProjectileBehavior::Burst { active: false });
            let expected = burst_heading(index, count).scale(speed);
            close(shot.velocity.x, expected.x);
            close(shot.velocity.y, expected.y);
        }
        for index in 0..count {
            for other in (index + 1)..count {
                assert_ne!(
                    shots[index].velocity, shots[other].velocity,
                    "burst pieces must not collapse into one heading"
                );
            }
        }
    }

    #[test]
    fn repel_sequence_window_uses_exact_100hz_to_60hz_integer_boundary() {
        let start = u16::MAX - 1;
        let window = repel_sequence_window(start, 225);
        assert_eq!(window.1.wrapping_sub(window.0), 136);
        assert!(repel_sequence_window_contains(
            window,
            start.wrapping_add(1)
        ));
        assert!(repel_sequence_window_contains(
            window,
            start.wrapping_add(135)
        ));
        assert!(!repel_sequence_window_contains(
            window,
            start.wrapping_add(136)
        ));
        assert_eq!(repel_sequence_window(start, 224), window);
        assert_eq!(repel_sequence_window(start, 1).1.wrapping_sub(start), 2);
        assert_eq!(repel_sequence_window(start, 0).1.wrapping_sub(start), 1);
    }

    #[test]
    fn repel_sequence_window_end_detection_is_forward_and_wrap_safe() {
        let window = repel_sequence_window_from_active_steps(u16::MAX - 2, 3);
        assert_eq!(window, (u16::MAX - 2, 1));
        assert!(!repel_sequence_window_end_reached(window, u16::MAX));
        assert!(!repel_sequence_window_end_reached(window, 0));
        assert!(repel_sequence_window_end_reached(window, 1));
        assert!(repel_sequence_window_end_crossed(window, u16::MAX, 1));
        assert!(repel_sequence_window_end_crossed(window, 0, 2));
        assert!(!repel_sequence_window_end_crossed(window, u16::MAX, 0));
    }

    #[test]
    fn protocol_direction_adapter_pins_truncating_bins_wrap_and_reference_heading() {
        let tuning = ProjectileInitializationTuning {
            bullet_speed: 100.0,
            bomb_speed: 80.0,
            burst_speed: 60.0,
            ship_radius: 16.0,
            multifire_angle: 1_998,
            double_barrel: false,
            burst_count: 12,
            bomb_bounce_count: 2,
        };
        let weapon = encode_weapon_data(1, 1, false, 1, 0, false);
        for orientation in [0.01249, 0.0125, 0.9875, 0.9999, 1.0125, -0.0125] {
            let direction = crate::continuum_position::protocol_direction(orientation);
            let canonical = initialize_projectiles_for_protocol_direction(
                weapon,
                RulesVector::ZERO,
                RulesVector::ZERO,
                direction,
                tuning,
            );
            let (heading_x, heading_y) =
                crate::continuum_position::protocol_direction_heading_components(direction);
            assert_eq!(canonical.len(), 1);
            assert_eq!(
                canonical[0].velocity.x.to_bits(),
                (heading_x * tuning.bullet_speed).to_bits(),
                "orientation {orientation} x",
            );
            assert_eq!(
                canonical[0].velocity.y.to_bits(),
                (heading_y * tuning.bullet_speed).to_bits(),
                "orientation {orientation} y",
            );
        }
    }

    #[test]
    fn protocol_projectile_launch_quantizes_subpixel_position_and_velocity_once() {
        let tuning = ProjectileInitializationTuning {
            bullet_speed: 100.0,
            bomb_speed: 80.0,
            burst_speed: 60.0,
            ship_radius: 16.0,
            multifire_angle: 2_200,
            double_barrel: false,
            burst_count: 3,
            bomb_bounce_count: 2,
        };
        let bullet = initialize_projectiles_for_protocol_direction(
            encode_weapon_data(1, 1, false, 1, 0, false),
            RulesVector::new(500.49, 700.51),
            RulesVector::new(19.94, -3.26),
            0,
            tuning,
        );
        assert_eq!(bullet[0].position, RulesVector::new(500.0, 701.0));
        let (heading_x, heading_y) =
            crate::continuum_position::protocol_direction_heading_components(0);
        assert_eq!(
            bullet[0].velocity.x.to_bits(),
            (19.9_f32 + heading_x * 100.0).to_bits()
        );
        assert_eq!(
            bullet[0].velocity.y.to_bits(),
            (-3.3_f32 + heading_y * 100.0).to_bits()
        );

        let bomb = initialize_projectiles_for_protocol_direction(
            encode_weapon_data(3, 1, false, 1, 0, false),
            RulesVector::new(500.51, 700.49),
            // This is the post-recoil ship velocity carried on the position
            // packet; both browser and server initialize from its exact wire
            // projection.
            RulesVector::new(-12.26, 8.24),
            10,
            tuning,
        );
        assert_eq!(bomb[0].position, RulesVector::new(501.0, 700.0));
        assert_eq!(bomb[0].velocity.x.to_bits(), (-12.3_f32 + 80.0).to_bits());
        assert_eq!(bomb[0].velocity.y.to_bits(), 8.2_f32.to_bits());
    }

    #[test]
    fn deterministic_burst_uses_reference_game_integer_truncation_and_pinned_bits() {
        let headings = (0..3)
            .map(|index| burst_heading(index, 3))
            .collect::<Vec<_>>();
        assert_eq!(
            headings
                .iter()
                .map(|heading| (heading.x.to_bits(), heading.y.to_bits()))
                .collect::<Vec<_>>(),
            vec![
                (0x0000_0000, 0xbf80_0000),
                (0x3f5d_b58e, 0x3eff_fa0f),
                (0xbf5d_b069, 0x3f00_05f1),
            ],
        );
        // The middle angle is 119,997 millidegrees, not ideal 120 degrees.
        let ideal = deterministic_sin_cos_turn_fraction(120_000, 360_000);
        assert_ne!(headings[1].x.to_bits(), ideal.0.to_bits());
        assert_ne!(headings[1].y.to_bits(), (-ideal.1).to_bits());
    }

    #[test]
    fn deterministic_projectile_trig_tracks_reference_game_reference_for_all_wire_settings() {
        for count in 1_usize..=u8::MAX as usize {
            for index in 0..count {
                let millidegrees = (index as i32 * 40_000) / count as i32 * 9;
                let radians = (millidegrees as f32 / 1_000.0).to_radians();
                let expected = RulesVector::new(radians.sin(), -radians.cos());
                let actual = burst_heading(index, count);
                assert!(
                    (actual.x - expected.x).abs() <= 0.000_001
                        && (actual.y - expected.y).abs() <= 0.000_001,
                    "count={count} index={index} actual={actual:?} expected={expected:?}",
                );
            }
        }

        let heading = RulesVector::new(0.6, -0.8);
        for raw in [0, 1, 110, 111, 2_200, u16::MAX] {
            let radians = (raw as f32 / 111.0).to_radians();
            let expected = RulesVector::new(
                heading.x * radians.cos() - heading.y * radians.sin(),
                heading.x * radians.sin() + heading.y * radians.cos(),
            );
            let actual = rotate_heading_raw(heading, raw, true);
            assert!(
                (actual.x - expected.x).abs() <= 0.000_001
                    && (actual.y - expected.y).abs() <= 0.000_001,
                "raw={raw} actual={actual:?} expected={expected:?}",
            );
        }
    }

    fn close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    }
}

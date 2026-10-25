//! Engine-independent Continuum player movement rules.
//!
//! This file is compiled directly into both the browser client and the
//! headless server. Keep it free of Macroquad, map, input, rendering, audio,
//! and networking types. Runtime callers sample their environment (wormhole
//! tiles, doors, and frequency-aware bricks) and pass those facts into this
//! core.

#![allow(dead_code)]

/// A tiny engine-free vector used at the client/server rules boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MovementVector {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

impl MovementVector {
    pub(crate) const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub(crate) const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub(crate) fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    pub(crate) fn length_squared(self) -> f32 {
        self.x * self.x + self.y * self.y
    }

    fn normalized_or_zero(self) -> Self {
        let length = self.length();
        if length > 0.0 {
            Self::new(self.x / length, self.y / length)
        } else {
            Self::ZERO
        }
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }

    fn subtract(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }

    fn scale(self, factor: f32) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlayerMovementInput {
    pub(crate) thrust: bool,
    pub(crate) reverse: bool,
    pub(crate) rotate_left: bool,
    pub(crate) rotate_right: bool,
    pub(crate) afterburner_held: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlayerMovementState {
    /// Continuum orientation in turns: `0.0..1.0`, not radians.
    pub(crate) orientation: f32,
    pub(crate) velocity: MovementVector,
    pub(crate) energy: f32,
    pub(crate) max_energy: f32,
}

/// Base tuning before temporary movement-state modifiers are applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlayerMovementTuning {
    pub(crate) rotation_per_second: f32,
    pub(crate) shutdown_rotation_per_second: f32,
    pub(crate) base_thrust_acceleration: f32,
    pub(crate) afterburner_thrust_acceleration: f32,
    pub(crate) rocket_thrust_acceleration: f32,
    pub(crate) base_top_speed: f32,
    pub(crate) afterburner_top_speed: f32,
    pub(crate) rocket_top_speed: f32,
    pub(crate) afterburner_energy_per_second: f32,
    pub(crate) recharge_per_second: f32,
    pub(crate) turret_thrust_penalty: f32,
    pub(crate) turret_speed_penalty: f32,
    pub(crate) repel_top_speed: f32,
    pub(crate) flagger_thrust_adjustment: f32,
    pub(crate) flagger_speed_adjustment: f32,
}

/// Per-step facts sampled by a runtime caller.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PlayerMovementEnvironment {
    /// Prize-owned Shutdown clears thrust and forces shutdown rotation.
    /// The reference game derives this from shutdown_end_tick, independently of EMP.
    pub(crate) engine_shutdown: bool,
    /// EMP-only suppression freezes recharge independently of movement. The Shutdown
    /// *prize* zeroes thrust but the reference still recharges through it
    /// (`ShipController:244-259` gates recharge on `emped_time` alone).
    pub(crate) emp_shutdown: bool,
    pub(crate) rocket_active: bool,
    pub(crate) repel_active: bool,
    pub(crate) flagger_active: bool,
    pub(crate) has_attached_riders: bool,
    /// Velocity impulse sampled from every influencing wormhole before the
    /// movement step, matching the reference game's gravity-before-thrust order.
    pub(crate) wormhole_velocity_delta: MovementVector,
    /// Present only when a gravity well's absolute thrust is at least one.
    pub(crate) gravity_top_speed: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppliedThrust {
    None,
    Forward,
    Reverse,
    Rocket,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlayerMovementOutcome {
    pub(crate) state: PlayerMovementState,
    pub(crate) afterburning: bool,
    pub(crate) applied_thrust: AppliedThrust,
    /// Exact acceleration selected before the current thrust impulse. This is
    /// retained for browser authority diagnostics; it is not a second movement
    /// source.
    pub(crate) selected_thrust_acceleration: f32,
    pub(crate) thrust_heading: MovementVector,
    pub(crate) display_heading: MovementVector,
    /// The exact cap used before collision. A caller re-applies this after a
    /// bounce because Continuum clamps reflected velocity as well.
    pub(crate) top_speed: f32,
}

/// One shared client/server movement step, ordered like the reference game
/// `ShipController::Update`:
///
/// 1. wormhole gravity;
/// 2. thrust along the pre-turn heading (rocket overrides EMP/turret thrust);
/// 3. rotation;
/// 4. gravity/turret/repel/flagger top-speed modifiers and clamp;
/// 5. afterburner drain, then recharge.
pub(crate) fn step_player_movement(
    state: PlayerMovementState,
    input: PlayerMovementInput,
    tuning: PlayerMovementTuning,
    environment: PlayerMovementEnvironment,
    dt: f32,
) -> PlayerMovementOutcome {
    let dt = dt.max(0.0);
    let mut velocity = state.velocity.add(environment.wormhole_velocity_delta);
    let thrust_heading = orientation_to_heading(state.orientation);

    let wants_afterburner = input.afterburner_held && (input.thrust || input.reverse);
    let afterburner_cost = tuning.afterburner_energy_per_second * dt;
    let afterburning = wants_afterburner
        && !environment.rocket_active
        && !environment.engine_shutdown
        && state.energy > afterburner_cost;

    let mut thrust_acceleration = if environment.rocket_active {
        tuning.rocket_thrust_acceleration
    } else if afterburning {
        tuning.afterburner_thrust_acceleration
    } else {
        tuning.base_thrust_acceleration
    };
    if environment.has_attached_riders && !environment.rocket_active {
        thrust_acceleration = (thrust_acceleration - tuning.turret_thrust_penalty).max(0.0);
    }
    if environment.engine_shutdown && !environment.rocket_active {
        thrust_acceleration = 0.0;
    }
    if environment.flagger_active {
        thrust_acceleration = (thrust_acceleration + tuning.flagger_thrust_adjustment).max(0.0);
    }

    let applied_thrust = if environment.rocket_active {
        velocity = velocity.add(thrust_heading.scale(thrust_acceleration * dt));
        AppliedThrust::Rocket
    } else if input.reverse && !environment.engine_shutdown {
        velocity = velocity.subtract(thrust_heading.scale(thrust_acceleration * dt));
        AppliedThrust::Reverse
    } else if input.thrust && !environment.engine_shutdown {
        velocity = velocity.add(thrust_heading.scale(thrust_acceleration * dt));
        AppliedThrust::Forward
    } else {
        AppliedThrust::None
    };

    let rotation_per_second = if environment.engine_shutdown {
        tuning.shutdown_rotation_per_second
    } else {
        tuning.rotation_per_second
    };
    let orientation = rotated_orientation(
        state.orientation,
        input.rotate_left,
        input.rotate_right,
        rotation_per_second,
        dt,
    );

    let mut top_speed = if environment.rocket_active {
        tuning.rocket_top_speed
    } else if afterburning {
        tuning.afterburner_top_speed
    } else {
        tuning.base_top_speed
    };
    if let Some(gravity_top_speed) = environment.gravity_top_speed {
        top_speed = top_speed.max(gravity_top_speed);
    }
    if environment.has_attached_riders {
        top_speed = (top_speed - tuning.turret_speed_penalty).max(0.0);
    }
    if environment.repel_active {
        top_speed = top_speed.max(tuning.repel_top_speed);
    }
    if environment.flagger_active {
        top_speed = (top_speed + tuning.flagger_speed_adjustment).max(0.0);
    }
    velocity = truncate_movement_vector(velocity, top_speed);

    let mut energy = state.energy;
    if afterburning {
        energy -= afterburner_cost;
    }
    if !environment.emp_shutdown {
        energy = (energy + tuning.recharge_per_second * dt).min(state.max_energy);
    }

    PlayerMovementOutcome {
        state: PlayerMovementState {
            orientation,
            velocity,
            energy,
            max_energy: state.max_energy,
        },
        afterburning,
        applied_thrust,
        selected_thrust_acceleration: thrust_acceleration,
        thrust_heading,
        display_heading: orientation_to_heading(orientation),
        top_speed,
    }
}

pub(crate) fn truncate_movement_vector(vector: MovementVector, maximum: f32) -> MovementVector {
    let length = vector.length();
    if length > maximum && length > 0.0 {
        vector.scale(maximum.max(0.0) / length)
    } else {
        vector
    }
}

fn rotated_orientation(
    orientation: f32,
    rotate_left: bool,
    rotate_right: bool,
    rotation_per_second: f32,
    dt: f32,
) -> f32 {
    rotate_orientation_step(
        orientation,
        rotate_left,
        rotate_right,
        rotation_per_second * dt,
    )
}

/// Apply the reference game's two ordered turn branches to one exact f32 orientation.
///
/// This deliberately does not use `rem_euclid`: the reference subtracts and
/// conditionally adds one for Left, then adds and conditionally subtracts one
/// for Right.  A tiny negative Left result can round to exactly `1.0` when one
/// is added; that raw f32 is legitimate canonical state and remains unchanged
/// on a later neutral frame.
pub(crate) fn rotate_orientation_step(
    orientation: f32,
    rotate_left: bool,
    rotate_right: bool,
    rotation_step: f32,
) -> f32 {
    let mut next = orientation;
    if rotate_left {
        next -= rotation_step;
        if next < 0.0 {
            next += 1.0;
        }
    }
    if rotate_right {
        next += rotation_step;
        if next >= 1.0 {
            next -= 1.0;
        }
    }
    next
}

fn orientation_to_heading(orientation: f32) -> MovementVector {
    let (x, y) = crate::continuum_position::canonical_discrete_heading_components(orientation);
    MovementVector::new(x, y)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CollisionIntegration {
    pub(crate) position: MovementVector,
    pub(crate) velocity: MovementVector,
    pub(crate) bounced: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CollisionAxis {
    X,
    Y,
}

/// One live reference-game `SimulatePlayer(dt)` collision call: exactly one X axis
/// followed by one Y axis for the caller's complete logical-frame `dt`.
///
/// X collision mutates the velocity before Y displacement is calculated. The
/// bounce factor is selected before either axis, and the caller observes the
/// combined result only after both axes, matching the reference's bounce-latch
/// update after the pair. This authoritative live-player path deliberately
/// does not add 10 ms anti-tunnelling slices: the reference game uses those only while
/// catching a remote position packet up to the local clock.
pub(crate) fn integrate_player_collision<F>(
    position: MovementVector,
    mut velocity: MovementVector,
    dt: f32,
    bounce_factor: f32,
    mut blocked: F,
) -> CollisionIntegration
where
    F: FnMut(MovementVector, CollisionAxis, f32) -> bool,
{
    let dt = dt.max(0.0);
    let start = position;
    let mut position = position;
    let mut bounced = false;

    let delta_x = velocity.x * dt;
    let candidate_x = MovementVector::new(start.x + delta_x, start.y);
    if blocked(candidate_x, CollisionAxis::X, delta_x) {
        velocity.x = -velocity.x * bounce_factor;
        velocity.y *= bounce_factor;
        bounced = true;
    } else {
        position.x = candidate_x.x;
    }

    // The reference game recomputes this displacement after the X-axis bounce has
    // potentially scaled the perpendicular velocity.
    let delta_y = velocity.y * dt;
    let candidate_y = MovementVector::new(position.x, start.y + delta_y);
    if blocked(candidate_y, CollisionAxis::Y, delta_y) {
        velocity.y = -velocity.y * bounce_factor;
        velocity.x *= bounce_factor;
        bounced = true;
    } else {
        position.y = candidate_y.y;
    }

    CollisionIntegration {
        position,
        velocity,
        bounced,
    }
}

/// The reference game's `PlayerManager::SimulateAxis` tests only the movement-leading
/// column for X and movement-leading row for Y. The perpendicular scan uses
/// the reference `position - radius - 1 .. position + radius + 1` integer-tile
/// bounds, while the final player-first AABB test retains pixel-truncated ship
/// geometry. Tile semantics (doors and team-passable bricks) stay in the
/// callback so this exact query runs in both binaries.
pub(crate) fn movement_axis_overlaps_blocked_tile(
    position: MovementVector,
    radius: f32,
    map_tiles: usize,
    tile_size: f32,
    axis: CollisionAxis,
    signed_delta: f32,
    mut blocked_tile: impl FnMut(i32, i32) -> bool,
) -> bool {
    if !position.x.is_finite()
        || !position.y.is_finite()
        || !radius.is_finite()
        || radius < 0.0
        || map_tiles == 0
        || !tile_size.is_finite()
        || tile_size <= 0.0
        || !signed_delta.is_finite()
    {
        return true;
    }
    let center = MovementVector::new(position.x.trunc(), position.y.trunc());
    let hull_min = MovementVector::new(center.x - radius, center.y - radius);
    let hull_max = MovementVector::new(center.x + radius, center.y + radius);

    let (axis_position, perpendicular_position) = match axis {
        CollisionAxis::X => (position.x, position.y),
        CollisionAxis::Y => (position.y, position.x),
    };
    // In the reference these values are in map-tile units. Zubspace keeps
    // world positions in protocol pixels, so divide before reproducing the
    // reference game's float-to-integer conversions. Negative movement explicitly floors
    // the leading edge; nonnegative movement truncates it through `u16`.
    let leading = if signed_delta < 0.0 {
        ((axis_position - radius) / tile_size).floor() as i32
    } else {
        ((axis_position + radius) / tile_size).trunc() as i32
    };
    if leading < 0 || leading >= map_tiles as i32 {
        return true;
    }
    let perpendicular_start =
        ((perpendicular_position - radius - tile_size) / tile_size).trunc() as i32;
    let perpendicular_end =
        ((perpendicular_position + radius + tile_size) / tile_size).trunc() as i32;

    for perpendicular in perpendicular_start..perpendicular_end {
        let (tile_x, tile_y) = match axis {
            CollisionAxis::X => (leading, perpendicular),
            CollisionAxis::Y => (perpendicular, leading),
        };
        let out_of_bounds =
            tile_x < 0 || tile_y < 0 || tile_x >= map_tiles as i32 || tile_y >= map_tiles as i32;
        if (out_of_bounds || blocked_tile(tile_x, tile_y))
            && movement_aabb_overlaps_tile(hull_min, hull_max, tile_x, tile_y, tile_size)
        {
            return true;
        }
    }
    false
}

pub(crate) fn movement_aabb_overlaps_tile(
    hull_min: MovementVector,
    hull_max: MovementVector,
    tile_x: i32,
    tile_y: i32,
    tile_size: f32,
) -> bool {
    let tile_min = MovementVector::new(tile_x as f32 * tile_size, tile_y as f32 * tile_size);
    let tile_max = MovementVector::new(tile_min.x + tile_size, tile_min.y + tile_size);
    crate::gameplay_rules::reference_game_player_hull_overlaps_tile(
        hull_min.x, hull_min.y, hull_max.x, hull_max.y, tile_min.x, tile_min.y, tile_max.x,
        tile_max.y,
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WormholeGravitySample {
    pub(crate) velocity_delta: MovementVector,
    pub(crate) top_speed: Option<f32>,
}

/// Read-only tile view; no derived cache or additional map ownership.
pub(crate) struct WormholeTileGrid<'a> {
    pub(crate) tiles: &'a [u8],
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) wormhole_tile: u8,
}

pub(crate) fn sample_wormhole_gravity_in_grid(
    position: MovementVector,
    gravity: i16,
    gravity_top_speed: f32,
    dt: f32,
    tile_size: f32,
    grid: WormholeTileGrid<'_>,
) -> WormholeGravitySample {
    if gravity == 0 || tile_size <= 0.0 || dt <= 0.0 {
        return WormholeGravitySample::default();
    }
    let (min_x, max_x, min_y, max_y) = wormhole_tile_window(position, gravity, tile_size);
    let left = min_x.max(0) as usize;
    let right = (i64::from(max_x).max(-1) + 1).min(grid.width as i64) as usize;
    let top = min_y.max(0) as usize;
    let bottom = (i64::from(max_y).max(-1) + 1).min(grid.height as i64) as usize;
    // A contiguous u8 contains query can skip an empty row without invoking
    // the scalar tile callback for every cell. A positive result is NOT an
    // anchor or physics decision; the unchanged sampler still owns both.
    let any = left < right
        && (top..bottom).any(|y| {
            let start = y * grid.width + left;
            let end = (y * grid.width + right).min(grid.tiles.len());
            grid.tiles
                .get(start..end)
                .is_some_and(|row| row.contains(&grid.wormhole_tile))
        });
    if !any {
        return WormholeGravitySample::default();
    }
    sample_wormhole_gravity(
        position,
        gravity,
        gravity_top_speed,
        dt,
        tile_size,
        |x, y| {
            x >= 0
                && y >= 0
                && (x as usize) < grid.width
                && (y as usize) < grid.height
                && grid.tiles.get(y as usize * grid.width + x as usize) == Some(&grid.wormhole_tile)
        },
    )
}

fn wormhole_tile_window(
    position: MovementVector,
    gravity: i16,
    tile_size: f32,
) -> (i32, i32, i32, i32) {
    let influence_radius = (i32::from(gravity).unsigned_abs() as f32 * 1000.0).sqrt();
    let radius = (influence_radius / tile_size).ceil() as i32 + 1;
    let cx = (position.x / tile_size).floor() as i32;
    let cy = (position.y / tile_size).floor() as i32;
    (cx - radius, cx + radius, cy - radius, cy + radius)
}

/// Sample all nearby 5x5 wormhole anchors without depending on either map
/// implementation. `is_wormhole_tile` must return false out of bounds.
pub(crate) fn sample_wormhole_gravity<F>(
    position: MovementVector,
    gravity: i16,
    gravity_top_speed: f32,
    dt: f32,
    tile_size: f32,
    mut is_wormhole_tile: F,
) -> WormholeGravitySample
where
    F: FnMut(i32, i32) -> bool,
{
    if gravity == 0 || tile_size <= 0.0 || dt <= 0.0 {
        return WormholeGravitySample::default();
    }

    let gravity_magnitude = i32::from(gravity).unsigned_abs();
    let (min_x, max_x, min_y, max_y) = wormhole_tile_window(position, gravity, tile_size);
    // The reference game's `ShipController:93-104` truncates the ship to an integer
    // tile coordinate before converting it back to protocol pixels. Preserve
    // the u16 multiply/wrap and s16 displacement semantics exactly; using the
    // smooth float distance here changes both the influence boundary and the
    // integer gravity impulse.
    let tile_size_u16 = tile_size as u16;
    let player_tile_x = (position.x / tile_size).trunc() as i64 as u16;
    let player_tile_y = (position.y / tile_size).trunc() as i64 as u16;
    let player_pixel_x = player_tile_x.wrapping_mul(tile_size_u16);
    let player_pixel_y = player_tile_y.wrapping_mul(tile_size_u16);
    let mut sample = WormholeGravitySample::default();

    for tile_y in min_y..=max_y {
        for tile_x in min_x..=max_x {
            if !is_wormhole_tile(tile_x, tile_y)
                || is_wormhole_tile(tile_x - 1, tile_y)
                || is_wormhole_tile(tile_x, tile_y - 1)
            {
                continue;
            }

            let center = MovementVector::new(
                (tile_x as f32 + 2.5) * tile_size,
                (tile_y as f32 + 2.5) * tile_size,
            );
            let wormhole_pixel_x = (tile_x as u16)
                .wrapping_add(2)
                .wrapping_mul(tile_size_u16)
                .wrapping_add(tile_size_u16 / 2);
            let wormhole_pixel_y = (tile_y as u16)
                .wrapping_add(2)
                .wrapping_mul(tile_size_u16)
                .wrapping_add(tile_size_u16 / 2);
            let dx = i32::from(player_pixel_x.wrapping_sub(wormhole_pixel_x) as i16);
            let dy = i32::from(player_pixel_y.wrapping_sub(wormhole_pixel_y) as i16);
            let distance_squared =
                i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy) + 1;
            if distance_squared >= i64::from(gravity_magnitude) * 1000 {
                continue;
            }

            let gravity_thrust = i32::from(gravity) * 1000 / distance_squared as i32;
            let direction = center.subtract(position).normalized_or_zero();
            sample.velocity_delta = sample
                .velocity_delta
                .add(direction.scale(gravity_thrust as f32 * 10.0 * dt));
            if gravity_thrust.abs() >= 1 {
                sample.top_speed = Some(gravity_top_speed);
            }
        }
    }
    sample
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_row_gravity_filter_matches_scalar_tile_oracle() {
        let mut tested = 0;
        let mut nonzero = 0;
        for (width, height) in [(19_usize, 17_usize), (97, 83), (8192, 5)] {
            for pattern in 0..3 {
                let mut tiles = vec![0_u8; width * height];
                for y in 0..height {
                    for x in 0..width {
                        let marked = match pattern {
                            0 => false,
                            1 => (x % 31 < 5 && y % 29 < 5) || (x == width - 1 && y == height - 1),
                            _ => (x.wrapping_mul(37) ^ y.wrapping_mul(71)) % 113 == 0,
                        };
                        if marked {
                            tiles[y * width + x] = 220;
                        }
                    }
                }
                for (x, y, gravity) in [
                    (-16.5, -0.5, 100),
                    (0.0, 0.0, -100),
                    (44.0, 72.0, 100),
                    (72.0, 72.0, 0),
                    (83.75 * 16.0, 100.75 * 16.0, 100),
                    (83.75 * 16.0, 99.75 * 16.0, 100),
                    (width as f32 * 16.0 - 0.01, height as f32 * 16.0, 100),
                    (44.0 + 65536.0, 72.0, -100),
                    (100.5, 78.0, i16::MIN),
                    (100.5, 78.0, i16::MAX),
                ] {
                    let position = MovementVector::new(x, y);
                    let expected = sample_wormhole_gravity(
                        position,
                        gravity,
                        777.0,
                        1.0 / 60.0,
                        16.0,
                        |tx, ty| {
                            if tx < 0 || ty < 0 || tx as usize >= width || ty as usize >= height {
                                return false;
                            }
                            tiles[ty as usize * width + tx as usize] == 220
                        },
                    );
                    let actual = sample_wormhole_gravity_in_grid(
                        position,
                        gravity,
                        777.0,
                        1.0 / 60.0,
                        16.0,
                        WormholeTileGrid {
                            tiles: &tiles,
                            width,
                            height,
                            wormhole_tile: 220,
                        },
                    );
                    assert_eq!(
                        actual.velocity_delta.x.to_bits(),
                        expected.velocity_delta.x.to_bits()
                    );
                    assert_eq!(
                        actual.velocity_delta.y.to_bits(),
                        expected.velocity_delta.y.to_bits()
                    );
                    assert_eq!(actual.top_speed, expected.top_speed);
                    tested += 1;
                    nonzero += usize::from(expected.velocity_delta != MovementVector::ZERO);
                }
            }
        }
        assert_eq!(tested, 90);
        assert!(nonzero > 0);
    }

    /// Independent transcription of the reference game
    /// `PlayerManager::SimulateAxis`'s tile query. Keep this fixture oracle
    /// deliberately separate from `movement_axis_overlaps_blocked_tile`: the
    /// production helper must continue to agree with the original loop's
    /// leading-edge selection, integer conversions, and player-first AABB
    /// operand order.
    fn reference_game_axis_query_oracle(
        position: MovementVector,
        radius: f32,
        map_tiles: usize,
        tile_size: f32,
        axis: CollisionAxis,
        signed_delta: f32,
        blocked_tile: (i32, i32),
    ) -> bool {
        let (axis_position, other_position) = match axis {
            CollisionAxis::X => (position.x, position.y),
            CollisionAxis::Y => (position.y, position.x),
        };
        let check = if signed_delta < 0.0 {
            ((axis_position - radius) / tile_size).floor() as i32
        } else {
            ((axis_position + radius) / tile_size).trunc() as i32
        };
        if check < 0 || check >= map_tiles as i32 {
            return true;
        }

        let start = ((other_position - radius - tile_size) / tile_size).trunc() as i32;
        let end = ((other_position + radius + tile_size) / tile_size).trunc() as i32;
        let rounded_x = position.x.trunc();
        let rounded_y = position.y.trunc();
        let player_min_x = rounded_x - radius;
        let player_min_y = rounded_y - radius;
        let player_max_x = rounded_x + radius;
        let player_max_y = rounded_y + radius;
        for other in start..end {
            let (tile_x, tile_y) = match axis {
                CollisionAxis::X => (check, other),
                CollisionAxis::Y => (other, check),
            };
            let solid = tile_x < 0
                || tile_y < 0
                || tile_x >= map_tiles as i32
                || tile_y >= map_tiles as i32
                || (tile_x, tile_y) == blocked_tile;
            if !solid {
                continue;
            }
            let tile_min_x = tile_x as f32 * tile_size;
            let tile_min_y = tile_y as f32 * tile_size;
            let tile_max_x = tile_min_x + tile_size;
            let tile_max_y = tile_min_y + tile_size;
            // Math BoxBoxIntersect(player, tile): minimum-edge contact is
            // included while maximum-edge contact is excluded.
            if player_max_x >= tile_min_x
                && player_min_x < tile_max_x
                && player_max_y >= tile_min_y
                && player_min_y < tile_max_y
            {
                return true;
            }
        }
        false
    }

    fn axis_query(
        position: MovementVector,
        radius: f32,
        map_tiles: usize,
        tile_size: f32,
        axis: CollisionAxis,
        signed_delta: f32,
        blocked_tile: (i32, i32),
    ) -> bool {
        movement_axis_overlaps_blocked_tile(
            position,
            radius,
            map_tiles,
            tile_size,
            axis,
            signed_delta,
            |x, y| (x, y) == blocked_tile,
        )
    }

    fn reference_game_collision_oracle(
        mut position: MovementVector,
        mut velocity: MovementVector,
        dt: f32,
        radius: f32,
        bounce_factor: f32,
        map_tiles: usize,
        tile_size: f32,
        blocked_tile: (i32, i32),
    ) -> CollisionIntegration {
        // Literal PlayerManager::SimulatePlayer: one complete-dt X
        // SimulateAxis call, then one complete-dt Y call. This intentionally
        // does not share the production integrator or its control flow.
        let dt = dt.max(0.0);
        let start = position;
        let mut bounced = false;

        let x_delta = velocity.x * dt;
        let x_candidate = MovementVector::new(start.x + x_delta, start.y);
        if reference_game_axis_query_oracle(
            x_candidate,
            radius,
            map_tiles,
            tile_size,
            CollisionAxis::X,
            x_delta,
            blocked_tile,
        ) {
            velocity.x = -velocity.x * bounce_factor;
            velocity.y *= bounce_factor;
            bounced = true;
        } else {
            position.x = x_candidate.x;
        }

        let y_delta = velocity.y * dt;
        let y_candidate = MovementVector::new(position.x, start.y + y_delta);
        if reference_game_axis_query_oracle(
            y_candidate,
            radius,
            map_tiles,
            tile_size,
            CollisionAxis::Y,
            y_delta,
            blocked_tile,
        ) {
            velocity.y = -velocity.y * bounce_factor;
            velocity.x *= bounce_factor;
            bounced = true;
        } else {
            position.y = y_candidate.y;
        }
        CollisionIntegration {
            position,
            velocity,
            bounced,
        }
    }

    fn state() -> PlayerMovementState {
        PlayerMovementState {
            orientation: 0.0,
            velocity: MovementVector::ZERO,
            energy: 1_000.0,
            max_energy: 1_700.0,
        }
    }

    fn tuning() -> PlayerMovementTuning {
        PlayerMovementTuning {
            rotation_per_second: 0.5,
            shutdown_rotation_per_second: 0.1,
            base_thrust_acceleration: 150.0,
            afterburner_thrust_acceleration: 300.0,
            rocket_thrust_acceleration: 500.0,
            base_top_speed: 200.0,
            afterburner_top_speed: 400.0,
            rocket_top_speed: 600.0,
            afterburner_energy_per_second: 120.0,
            recharge_per_second: 200.0,
            turret_thrust_penalty: 40.0,
            turret_speed_penalty: 50.0,
            repel_top_speed: 800.0,
            flagger_thrust_adjustment: 25.0,
            flagger_speed_adjustment: 20.0,
        }
    }

    #[test]
    fn captured_held_repel_receipt_tail_differs_only_when_the_final_speed_cap_is_unknown() {
        // Real Warp/Repel lane6, acf55c4: accepted sequence24853, then genuine
        // thrust records24854/24855. Server-sealed Repel ends before24855;
        // the unreceipted browser still predicts the higher cap on that step.
        // Reproduce both exact velocity tails with the shared production core.
        let mut tuning = tuning();
        tuning.base_thrust_acceleration = 190.0;
        tuning.base_top_speed = 325.0;
        tuning.repel_top_speed = 500.0;
        let initial = PlayerMovementState {
            orientation: 0.26583338,
            velocity: MovementVector::new(-335.12585, -14.420603),
            energy: 1700.0,
            max_energy: 1700.0,
        };
        let input = PlayerMovementInput {
            thrust: true,
            ..Default::default()
        };
        let first = step_player_movement(
            initial,
            input,
            tuning,
            PlayerMovementEnvironment {
                repel_active: true,
                ..Default::default()
            },
            1.0 / 60.0,
        )
        .state;
        for (repel_active, expected_x_bits, expected_y_bits) in [
            (false, 3282196492, 3244546404),
            (true, 3282330994, 3244735178),
        ] {
            let final_state = step_player_movement(
                first,
                input,
                tuning,
                PlayerMovementEnvironment {
                    repel_active,
                    ..Default::default()
                },
                1.0 / 60.0,
            )
            .state;
            assert_eq!(final_state.velocity.x.to_bits(), expected_x_bits);
            assert_eq!(final_state.velocity.y.to_bits(), expected_y_bits);
            assert_eq!(
                final_state.orientation.to_bits(),
                initial.orientation.to_bits()
            );
        }
    }

    fn close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1e-3, "{actual} != {expected}");
    }

    #[test]
    fn reference_game_left_wrap_preserves_exact_f32_one_sentinel() {
        let rotation_step = 0.75_f32 * (1.0_f32 / 60.0_f32);
        assert_eq!(rotation_step.to_bits(), 0x3c4c_ccce);
        let mut orientation = 0.1_f32;
        let directions = [3_u8, 2, 2, 1, 1, 0, 0, 0];
        for direction in directions {
            orientation = rotate_orientation_step(orientation, true, false, rotation_step);
            assert_eq!(
                crate::continuum_position::protocol_direction(orientation),
                direction,
            );
        }
        assert_eq!(orientation.to_bits(), 1.0_f32.to_bits());
        assert_eq!(
            rotate_orientation_step(orientation, false, false, rotation_step).to_bits(),
            1.0_f32.to_bits(),
            "a neutral reference frame does not normalize the exact Left sentinel",
        );
        assert_eq!(
            rotate_orientation_step(orientation, true, false, rotation_step).to_bits(),
            (1.0_f32 - rotation_step).to_bits(),
        );
        assert_eq!(
            rotate_orientation_step(orientation, false, true, rotation_step).to_bits(),
            0x3c4c_cd00,
            "the reference's add-then-subtract wrap keeps its f32 cancellation bits",
        );
    }

    /// Golden cases copied from the reference game's update order. These existed before
    /// either production caller was cut over and remain the parity contract.
    #[test]
    fn golden_movement_branch_fixture() {
        struct Case {
            name: &'static str,
            input: PlayerMovementInput,
            environment: PlayerMovementEnvironment,
            velocity: MovementVector,
            orientation: f32,
            energy: f32,
            top_speed: f32,
            afterburning: bool,
        }

        let cases = [
            Case {
                name: "forward_turn_uses_pre_turn_heading",
                input: PlayerMovementInput {
                    thrust: true,
                    rotate_right: true,
                    ..Default::default()
                },
                environment: PlayerMovementEnvironment::default(),
                velocity: MovementVector::new(0.0, -15.0),
                orientation: 0.05,
                energy: 1_020.0,
                top_speed: 200.0,
                afterburning: false,
            },
            Case {
                name: "combined_prize_shutdown_and_emp_block_thrust_and_recharge",
                input: PlayerMovementInput {
                    thrust: true,
                    rotate_right: true,
                    afterburner_held: true,
                    ..Default::default()
                },
                environment: PlayerMovementEnvironment {
                    engine_shutdown: true,
                    emp_shutdown: true,
                    ..Default::default()
                },
                velocity: MovementVector::ZERO,
                orientation: 0.01,
                energy: 1_000.0,
                top_speed: 200.0,
                afterburning: false,
            },
            Case {
                name: "rocket_overrides_emp_and_turret_thrust",
                input: PlayerMovementInput::default(),
                environment: PlayerMovementEnvironment {
                    engine_shutdown: true,
                    emp_shutdown: true,
                    rocket_active: true,
                    flagger_active: true,
                    has_attached_riders: true,
                    ..Default::default()
                },
                velocity: MovementVector::new(0.0, -52.5),
                orientation: 0.0,
                energy: 1_000.0,
                top_speed: 570.0,
                afterburning: false,
            },
            Case {
                name: "gravity_turret_repel_flagger_speed_order",
                input: PlayerMovementInput::default(),
                environment: PlayerMovementEnvironment {
                    repel_active: true,
                    flagger_active: true,
                    has_attached_riders: true,
                    wormhole_velocity_delta: MovementVector::new(300.0, 0.0),
                    gravity_top_speed: Some(700.0),
                    ..Default::default()
                },
                velocity: MovementVector::new(300.0, 0.0),
                orientation: 0.0,
                energy: 1_020.0,
                top_speed: 820.0,
                afterburning: false,
            },
        ];

        for case in cases {
            let outcome =
                step_player_movement(state(), case.input, tuning(), case.environment, 0.1);
            close(outcome.state.velocity.x, case.velocity.x);
            close(outcome.state.velocity.y, case.velocity.y);
            close(outcome.state.orientation, case.orientation);
            close(outcome.state.energy, case.energy);
            close(outcome.top_speed, case.top_speed);
            assert_eq!(outcome.afterburning, case.afterburning, "{}", case.name);
        }
    }

    /// The reference engages afterburners only when the ship is actually thrusting
    /// (`ShipController:121` picks `MaximumThrust` for `afterburners`, which at `:74` is
    /// already `afterburners && (thrust_forward || thrust_backward)`), and it charges
    /// `AfterburnerEnergy / 10` per second only while they are engaged. Both halves shipped
    /// correctly, so both are pinned here: a future "let shift boost at any time" or "drain
    /// only on some other condition" change now fails a test instead of quietly diverging.
    #[test]
    fn afterburners_need_thrust_input_and_charge_only_while_engaged() {
        let held_alone = PlayerMovementInput {
            afterburner_held: true,
            ..Default::default()
        };
        let idle = step_player_movement(
            state(),
            held_alone,
            tuning(),
            PlayerMovementEnvironment::default(),
            1.0,
        );
        assert!(
            !idle.afterburning,
            "holding shift with no thrust or reverse must not engage afterburners"
        );

        let plain = step_player_movement(
            state(),
            PlayerMovementInput {
                thrust: true,
                ..Default::default()
            },
            tuning(),
            PlayerMovementEnvironment::default(),
            1.0,
        );
        let boosted = step_player_movement(
            state(),
            PlayerMovementInput {
                thrust: true,
                afterburner_held: true,
                ..Default::default()
            },
            tuning(),
            PlayerMovementEnvironment::default(),
            1.0,
        );

        assert!(boosted.afterburning, "shift plus forward thrust engages");
        assert!(matches!(boosted.applied_thrust, AppliedThrust::Forward));
        assert_eq!(
            boosted.selected_thrust_acceleration,
            tuning().afterburner_thrust_acceleration,
            "afterburners must select the ship's maximum thrust, not its base thrust"
        );
        assert!(
            plain.selected_thrust_acceleration < boosted.selected_thrust_acceleration,
            "engaging afterburners must raise the acceleration actually used"
        );
        assert_eq!(boosted.top_speed, tuning().afterburner_top_speed);
        // Compare two identical steps rather than one step against the starting energy, so the
        // recharge that runs in the same step cannot be mistaken for the afterburner charge.
        let charged = plain.state.energy - boosted.state.energy;
        assert!(
            (charged - tuning().afterburner_energy_per_second).abs() < 1e-3,
            "one second of afterburner must cost AfterburnerEnergy / 10, charged {charged}"
        );
    }

    #[test]
    fn exact_afterburner_boundary_requires_energy_strictly_above_cost() {
        let mut boundary = state();
        boundary.energy = 12.0;
        let input = PlayerMovementInput {
            thrust: true,
            afterburner_held: true,
            ..Default::default()
        };
        assert!(
            !step_player_movement(
                boundary,
                input,
                tuning(),
                PlayerMovementEnvironment::default(),
                0.1,
            )
            .afterburning
        );
        boundary.energy = 12.001;
        assert!(
            step_player_movement(
                boundary,
                input,
                tuning(),
                PlayerMovementEnvironment::default(),
                0.1,
            )
            .afterburning
        );
    }

    fn reference_frame(
        position: MovementVector,
        state: PlayerMovementState,
        input: PlayerMovementInput,
        environment: PlayerMovementEnvironment,
        dt: f32,
        blocked: impl FnMut(MovementVector, CollisionAxis, f32) -> bool,
    ) -> (MovementVector, PlayerMovementState, bool) {
        let collision = integrate_player_collision(position, state.velocity, dt, 0.5, blocked);
        let movement = step_player_movement(
            PlayerMovementState {
                velocity: collision.velocity,
                ..state
            },
            input,
            tuning(),
            environment,
            dt,
        );
        (collision.position, movement.state, collision.bounced)
    }

    #[test]
    fn reference_game_frame_order_reference_goldens_integrate_before_current_input() {
        let initial = PlayerMovementState {
            orientation: 0.024,
            velocity: MovementVector::new(10.0, 20.0),
            energy: 1_000.0,
            max_energy: 1_700.0,
        };
        let input = PlayerMovementInput {
            thrust: true,
            rotate_right: true,
            ..Default::default()
        };
        let (position_one, state_one, bounced_one) = reference_frame(
            MovementVector::new(100.0, 100.0),
            initial,
            input,
            PlayerMovementEnvironment::default(),
            0.1,
            |_, _, _| false,
        );
        let (position_two, state_two, bounced_two) = reference_frame(
            position_one,
            state_one,
            input,
            PlayerMovementEnvironment::default(),
            0.1,
            |_, _, _| false,
        );
        assert!(!bounced_one && !bounced_two);
        assert_eq!(
            (
                position_one.x.to_bits(),
                position_one.y.to_bits(),
                state_one.orientation.to_bits(),
                state_one.velocity.x.to_bits(),
                state_one.velocity.y.to_bits(),
            ),
            (
                0x42ca_0000,
                0x42cc_0000,
                0x3d97_8d50,
                0x4120_0012,
                0x40a0_0000
            ),
        );
        assert_eq!(
            (
                position_two.x.to_bits(),
                position_two.y.to_bits(),
                state_two.orientation.to_bits(),
                state_two.velocity.x.to_bits(),
                state_two.velocity.y.to_bits(),
            ),
            (
                0x42cc_0000,
                0x42cd_0000,
                0x3dfd_f3b6,
                0x416a_2a21,
                0xc114_40e5
            ),
        );
        assert_eq!(
            crate::continuum_position::protocol_direction(state_one.orientation),
            2,
        );
        assert_eq!(
            crate::continuum_position::protocol_direction(state_two.orientation),
            4,
        );
    }

    #[test]
    fn reference_game_frame_order_reference_goldens_cover_reverse_afterburn_rocket_and_repel() {
        let base = PlayerMovementState {
            orientation: 0.25,
            velocity: MovementVector::ZERO,
            energy: 1_000.0,
            max_energy: 1_700.0,
        };
        let cases = [
            (
                PlayerMovementInput {
                    reverse: true,
                    ..Default::default()
                },
                PlayerMovementEnvironment::default(),
                -15.0,
                0.0,
                1_020.0,
            ),
            (
                PlayerMovementInput {
                    thrust: true,
                    afterburner_held: true,
                    ..Default::default()
                },
                PlayerMovementEnvironment::default(),
                30.0,
                0.0,
                1_008.0,
            ),
            (
                PlayerMovementInput::default(),
                PlayerMovementEnvironment {
                    rocket_active: true,
                    ..Default::default()
                },
                50.0,
                0.0,
                1_020.0,
            ),
        ];
        for (input, environment, expected_x, expected_y, expected_energy) in cases {
            let (position, next, bounced) = reference_frame(
                MovementVector::new(64.0, 64.0),
                base,
                input,
                environment,
                0.1,
                |_, _, _| false,
            );
            assert_eq!(position, MovementVector::new(64.0, 64.0));
            close(next.velocity.x, expected_x);
            close(next.velocity.y, expected_y);
            assert_eq!(next.energy, expected_energy);
            assert!(!bounced);
        }

        let fast = PlayerMovementState {
            velocity: MovementVector::new(1_000.0, 0.0),
            ..base
        };
        let (position, next, _) = reference_frame(
            MovementVector::new(64.0, 64.0),
            fast,
            PlayerMovementInput::default(),
            PlayerMovementEnvironment {
                repel_active: true,
                ..Default::default()
            },
            0.1,
            |_, _, _| false,
        );
        assert_eq!(position.x.to_bits(), 0x4324_0000);
        assert_eq!(position.y.to_bits(), 64.0_f32.to_bits());
        assert_eq!(next.velocity, MovementVector::new(800.0, 0.0));
    }

    #[test]
    fn reference_game_x_bounce_recomputes_y_displacement_from_scaled_velocity() {
        let result = integrate_player_collision(
            MovementVector::ZERO,
            MovementVector::new(100.0, 20.0),
            0.02,
            0.5,
            |candidate, _, _| candidate.x > 1.0,
        );
        assert!(result.bounced);
        assert_eq!(result.position.x.to_bits(), 0.0_f32.to_bits());
        assert_eq!(result.position.y.to_bits(), 0x3e4c_cccc);
        assert_eq!(result.velocity.x.to_bits(), (-50.0_f32).to_bits());
        assert_eq!(result.velocity.y.to_bits(), 10.0_f32.to_bits());
        assert_eq!(result.position.x.round() as u16, 0);
        assert_eq!(result.position.y.round() as u16, 0);
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(result.velocity.x),
            -500
        );
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(result.velocity.y),
            100
        );
    }

    #[test]
    fn reference_game_corner_bounce_applies_factor_to_both_axes_before_latching() {
        let result = integrate_player_collision(
            MovementVector::ZERO,
            MovementVector::new(100.0, 20.0),
            0.02,
            0.5,
            |_, _, _| true,
        );
        assert!(result.bounced);
        assert_eq!(result.position.x.to_bits(), 0.0_f32.to_bits());
        assert_eq!(result.position.y.to_bits(), 0.0_f32.to_bits());
        assert_eq!(result.velocity.x.to_bits(), (-25.0_f32).to_bits());
        assert_eq!(result.velocity.y.to_bits(), (-5.0_f32).to_bits());
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(result.velocity.x),
            -250
        );
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(result.velocity.y),
            -50
        );
    }

    #[test]
    fn reference_game_axis_pair_uses_one_preselected_bounce_factor() {
        let result = integrate_player_collision(
            MovementVector::ZERO,
            MovementVector::new(100.0, 20.0),
            0.02,
            0.5,
            |candidate, _, _| candidate != MovementVector::ZERO,
        );
        assert!(result.bounced);
        assert_eq!(result.position, MovementVector::ZERO);
        assert_eq!(result.velocity.x.to_bits(), (-25.0_f32).to_bits());
        assert_eq!(result.velocity.y.to_bits(), (-5.0_f32).to_bits());
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(result.velocity.x),
            -250
        );
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(result.velocity.y),
            -50
        );
    }

    #[test]
    fn tunnel_bounce_speed_stays_within_the_top_speed_cap() {
        // Deterministic characterization of the reported tunnel ping-pong: thrust along +x inside
        // walls at x=20/x=60 with the authored Chaos factor 16/26 (BounceFactor=26). The
        // bounce must not add energy: the speed may oscillate as thrust opposes the reflected velocity,
        // but it must stay within the top-speed cap the reference truncates to.
        let dt = 1.0 / 60.0;
        let mut position = MovementVector::new(40.0, 0.0);
        let mut movement_state = state();
        movement_state.orientation = 0.25; // heading +x
        let input = PlayerMovementInput {
            thrust: true,
            ..Default::default()
        };
        let environment = PlayerMovementEnvironment::default();
        let tuning = tuning();
        let mut max_speed = 0.0_f32;
        for _ in 0..240 {
            let integrated = integrate_player_collision(
                position,
                movement_state.velocity,
                dt,
                1.0,
                |cand, _, _| cand.x < 20.0 || cand.x > 60.0,
            );
            position = integrated.position;
            movement_state.velocity = integrated.velocity;
            movement_state =
                step_player_movement(movement_state, input, tuning, environment, dt).state;
            max_speed = max_speed.max(movement_state.velocity.length());
        }
        assert!(
            max_speed <= tuning.base_top_speed + 0.1,
            "a tunnel bounce must never exceed the top-speed cap: {max_speed}"
        );
        assert!(position.x.is_finite() && position.y.is_finite());
    }

    #[test]
    fn authored_chaos_bounce_factor_damps_a_wall_hit() {
        // Chaos authors BounceFactor=26, so 16/26 < 1 and a wall bounce must lose speed. Chaos used to
        // ship 16 - the reference's "no speed loss" point - which made every bounce perfectly elastic and
        // let a thrusting ship keep top speed forever in a tunnel.
        let factor = crate::continuum::continuum_bounce_factor(26);
        assert!(
            factor < 1.0,
            "the authored Chaos bounce must damp: {factor}"
        );
        let before = MovementVector::new(212.18, 246.18).length();
        let result = integrate_player_collision(
            MovementVector::new(100.0, 100.0),
            MovementVector::new(212.18, 246.18),
            1.0 / 60.0,
            factor,
            |candidate, axis, _| axis == CollisionAxis::Y && candidate.y > 100.0,
        );
        assert!(result.bounced);
        assert!(
            result.velocity.length() < before,
            "a wall bounce under the authored factor must lose speed: {} !< {before}",
            result.velocity.length()
        );
        assert!(result.velocity.length() > 0.0);
    }

    #[test]
    fn reference_game_axis_query_uses_only_the_signed_leading_column_or_row() {
        struct Case {
            name: &'static str,
            position: MovementVector,
            axis: CollisionAxis,
            delta: f32,
            blocked_tile: (i32, i32),
            expected: bool,
        }
        let cases = [
            Case {
                name: "review reproduction ignores a non-leading tile",
                position: MovementVector::new(46.0, 34.0),
                axis: CollisionAxis::X,
                delta: 1.0,
                blocked_tile: (2, 3),
                expected: false,
            },
            Case {
                name: "same candidate hits the leading corner",
                position: MovementVector::new(46.0, 34.0),
                axis: CollisionAxis::X,
                delta: 1.0,
                blocked_tile: (3, 3),
                expected: true,
            },
            Case {
                name: "opposite approach penetrates the leading column",
                position: MovementVector::new(61.0, 56.0),
                axis: CollisionAxis::X,
                delta: -1.0,
                blocked_tile: (2, 3),
                expected: true,
            },
            Case {
                name: "opposite approach at tile maximum edge is excluded",
                position: MovementVector::new(62.0, 56.0),
                axis: CollisionAxis::X,
                delta: -1.0,
                blocked_tile: (2, 3),
                expected: false,
            },
            Case {
                name: "horizontal slide one pixel above wall",
                position: MovementVector::new(46.0, 33.0),
                axis: CollisionAxis::X,
                delta: 1.0,
                blocked_tile: (3, 3),
                expected: false,
            },
            Case {
                name: "horizontal slide one pixel below wall",
                position: MovementVector::new(46.0, 79.0),
                axis: CollisionAxis::X,
                delta: 1.0,
                blocked_tile: (3, 3),
                expected: false,
            },
            Case {
                name: "vertical slide beside wall maximum edge",
                position: MovementVector::new(62.0, 46.0),
                axis: CollisionAxis::Y,
                delta: 1.0,
                blocked_tile: (2, 3),
                expected: false,
            },
            Case {
                name: "vertical slide one pixel before wall minimum edge",
                position: MovementVector::new(33.0, 46.0),
                axis: CollisionAxis::Y,
                delta: 1.0,
                blocked_tile: (3, 3),
                expected: false,
            },
            Case {
                name: "Y leading corner preserves player-first minimum-edge contact",
                position: MovementVector::new(34.0, 46.0),
                axis: CollisionAxis::Y,
                delta: 1.0,
                blocked_tile: (3, 3),
                expected: true,
            },
            Case {
                name: "negative leading map column is solid",
                position: MovementVector::new(13.0, 40.0),
                axis: CollisionAxis::X,
                delta: -1.0,
                blocked_tile: (99, 99),
                expected: true,
            },
            Case {
                name: "positive leading map row is solid",
                position: MovementVector::new(40.0, 1_010.0),
                axis: CollisionAxis::Y,
                delta: 1.0,
                blocked_tile: (99, 99),
                expected: true,
            },
        ];

        for case in cases {
            let oracle = reference_game_axis_query_oracle(
                case.position,
                14.0,
                64,
                16.0,
                case.axis,
                case.delta,
                case.blocked_tile,
            );
            assert_eq!(oracle, case.expected, "oracle: {}", case.name);
            assert_eq!(
                axis_query(
                    case.position,
                    14.0,
                    64,
                    16.0,
                    case.axis,
                    case.delta,
                    case.blocked_tile,
                ),
                oracle,
                "production: {}",
                case.name,
            );
        }
    }

    #[test]
    fn reference_game_axis_collision_reproduction_and_reverse_approach_are_exact() {
        let reproduce = |position, velocity| {
            integrate_player_collision(position, velocity, 0.01, 0.5, |candidate, axis, delta| {
                axis_query(candidate, 14.0, 64, 16.0, axis, delta, (2, 3))
            })
        };
        let forward = reproduce(
            MovementVector::new(45.0, 34.0),
            MovementVector::new(100.0, -1.0),
        );
        let forward_oracle = reference_game_collision_oracle(
            MovementVector::new(45.0, 34.0),
            MovementVector::new(100.0, -1.0),
            0.01,
            14.0,
            0.5,
            64,
            16.0,
            (2, 3),
        );
        assert_eq!(forward, forward_oracle);
        assert_eq!(forward.position.x.to_bits(), 0x4238_0000);
        assert_eq!(forward.position.y.to_bits(), 0x4207_f5c3);
        assert_eq!(forward.velocity.x.to_bits(), 100.0_f32.to_bits());
        assert_eq!(forward.velocity.y.to_bits(), (-1.0_f32).to_bits());
        assert!(!forward.bounced);
        assert_eq!(
            (
                forward.position.x.round() as u16,
                forward.position.y.round() as u16
            ),
            (46, 34)
        );
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(forward.velocity.x),
            1_000,
        );
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(forward.velocity.y),
            -10,
        );

        let reverse = reproduce(
            MovementVector::new(62.0, 56.0),
            MovementVector::new(-100.0, 1.0),
        );
        let reverse_oracle = reference_game_collision_oracle(
            MovementVector::new(62.0, 56.0),
            MovementVector::new(-100.0, 1.0),
            0.01,
            14.0,
            0.5,
            64,
            16.0,
            (2, 3),
        );
        assert_eq!(reverse, reverse_oracle);
        assert_eq!(reverse.position.x.to_bits(), 62.0_f32.to_bits());
        assert_eq!(reverse.position.y.to_bits(), 0x4260_051f);
        assert_eq!(reverse.velocity.x.to_bits(), 50.0_f32.to_bits());
        assert_eq!(reverse.velocity.y.to_bits(), 0.5_f32.to_bits());
        assert!(reverse.bounced);
        assert_eq!(
            (
                reverse.position.x.round() as u16,
                reverse.position.y.round() as u16
            ),
            (62, 56)
        );
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(reverse.velocity.x),
            500,
        );
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(reverse.velocity.y),
            5,
        );
    }

    #[test]
    fn live_frame_uses_one_full_dt_axis_pair_not_two_10ms_slices() {
        let start = MovementVector::new(31.0, 56.0);
        let velocity = MovementVector::new(200.0, 0.0);
        let dt = 1.0_f32 / 60.0;
        let actual =
            integrate_player_collision(start, velocity, dt, 0.5, |candidate, axis, delta| {
                axis_query(candidate, 14.0, 64, 16.0, axis, delta, (3, 3))
            });
        let oracle =
            reference_game_collision_oracle(start, velocity, dt, 14.0, 0.5, 64, 16.0, (3, 3));
        assert_eq!(actual, oracle);
        assert_eq!(actual.position, start);
        assert_eq!(actual.velocity, MovementVector::new(-100.0, 0.0));
        assert!(actual.bounced);
        assert_eq!(actual.position.x.to_bits(), 31.0_f32.to_bits());
        assert_eq!(actual.position.y.to_bits(), 56.0_f32.to_bits());
        assert_eq!(actual.position.x.round() as u16, 31);
        assert_eq!(actual.position.y.round() as u16, 56);
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(actual.velocity.x),
            -1_000,
        );

        // The removed implementation split this same live frame into 10 ms
        // plus the remainder. Its first pair advanced to x=33 before the
        // second pair bounced, producing a reference-incompatible anchor.
        let first_slice =
            reference_game_collision_oracle(start, velocity, 0.01, 14.0, 0.5, 64, 16.0, (3, 3));
        assert_eq!(first_slice.position.x.to_bits(), 33.0_f32.to_bits());
        assert!(!first_slice.bounced);
        let second_slice = reference_game_collision_oracle(
            first_slice.position,
            first_slice.velocity,
            dt - 0.01,
            14.0,
            0.5,
            64,
            16.0,
            (3, 3),
        );
        assert_eq!(second_slice.position.x.to_bits(), 33.0_f32.to_bits());
        assert_eq!(second_slice.velocity.x.to_bits(), (-100.0_f32).to_bits());
        assert_ne!(second_slice.position, actual.position);
    }

    #[test]
    fn live_frame_dt_boundaries_and_consecutive_60hz_frames_match_reference() {
        let start = MovementVector::new(31.0, 56.0);
        let velocity = MovementVector::new(200.0, 0.0);
        let cases = [
            (0.005_f32, 32.0_f32, 200.0_f32, false),
            (0.01_f32, 33.0_f32, 200.0_f32, false),
            (0.015_f32, 31.0_f32, -100.0_f32, true),
            (1.0_f32 / 60.0, 31.0_f32, -100.0_f32, true),
        ];
        for (dt, expected_x, expected_velocity_x, expected_bounced) in cases {
            let actual =
                integrate_player_collision(start, velocity, dt, 0.5, |candidate, axis, delta| {
                    axis_query(candidate, 14.0, 64, 16.0, axis, delta, (3, 3))
                });
            let oracle =
                reference_game_collision_oracle(start, velocity, dt, 14.0, 0.5, 64, 16.0, (3, 3));
            assert_eq!(actual, oracle, "dt={dt}");
            assert_eq!(actual.position.x.to_bits(), expected_x.to_bits(), "dt={dt}");
            assert_eq!(
                actual.velocity.x.to_bits(),
                expected_velocity_x.to_bits(),
                "dt={dt}",
            );
            assert_eq!(actual.bounced, expected_bounced, "dt={dt}");
        }

        let frame_one = integrate_player_collision(
            start,
            velocity,
            1.0 / 60.0,
            0.5,
            |candidate, axis, delta| axis_query(candidate, 14.0, 64, 16.0, axis, delta, (3, 3)),
        );
        let frame_two = integrate_player_collision(
            frame_one.position,
            frame_one.velocity,
            1.0 / 60.0,
            0.5,
            |candidate, axis, delta| axis_query(candidate, 14.0, 64, 16.0, axis, delta, (3, 3)),
        );
        let frame_three = integrate_player_collision(
            frame_two.position,
            frame_two.velocity,
            1.0 / 60.0,
            0.5,
            |candidate, axis, delta| axis_query(candidate, 14.0, 64, 16.0, axis, delta, (3, 3)),
        );
        assert_eq!(frame_two.position.x.to_bits(), 0x41ea_aaab);
        assert_eq!(frame_three.position.x.to_bits(), 0x41dd_5556);
        assert!(!frame_two.bounced && !frame_three.bounced);
    }

    #[test]
    fn live_frame_diagonal_corner_and_high_speed_tunnelling_match_reference() {
        let diagonal = integrate_player_collision(
            MovementVector::new(31.0, 31.0),
            MovementVector::new(200.0, 200.0),
            1.0 / 60.0,
            0.5,
            |candidate, axis, delta| axis_query(candidate, 14.0, 64, 16.0, axis, delta, (3, 3)),
        );
        let diagonal_oracle = reference_game_collision_oracle(
            MovementVector::new(31.0, 31.0),
            MovementVector::new(200.0, 200.0),
            1.0 / 60.0,
            14.0,
            0.5,
            64,
            16.0,
            (3, 3),
        );
        assert_eq!(diagonal, diagonal_oracle);
        assert_eq!(diagonal.position.x.to_bits(), 0x4209_5555);
        assert_eq!(diagonal.position.y.to_bits(), 31.0_f32.to_bits());
        assert_eq!(diagonal.velocity, MovementVector::new(100.0, -100.0));
        assert!(diagonal.bounced);

        // The reference game's live SimulatePlayer is a discrete leading-edge query,
        // not a swept hull. Pin its high-speed tunnelling behavior rather than
        // adding anti-tunnelling slices that the reference does not perform.
        let fast = integrate_player_collision(
            MovementVector::new(31.0, 56.0),
            MovementVector::new(2_000.0, 0.0),
            1.0 / 60.0,
            0.5,
            |candidate, axis, delta| axis_query(candidate, 14.0, 64, 16.0, axis, delta, (3, 3)),
        );
        let fast_oracle = reference_game_collision_oracle(
            MovementVector::new(31.0, 56.0),
            MovementVector::new(2_000.0, 0.0),
            1.0 / 60.0,
            14.0,
            0.5,
            64,
            16.0,
            (3, 3),
        );
        assert_eq!(fast, fast_oracle);
        assert_eq!(fast.position.x.to_bits(), 0x4280_aaab);
        assert_eq!(fast.velocity.x.to_bits(), 2_000.0_f32.to_bits());
        assert!(!fast.bounced);
    }

    #[test]
    fn reference_game_axis_query_pins_radius_truncation_and_negative_direction_bounds() {
        let cases = [
            (
                "one-pixel radius reaches the leading minimum edge",
                MovementVector::new(31.9, 24.0),
                1.0,
                CollisionAxis::X,
                1.0,
                (2, 1),
                true,
            ),
            (
                "large radius reaches a leading corner",
                MovementVector::new(36.0, 64.0),
                28.0,
                CollisionAxis::X,
                1.0,
                (4, 4),
                true,
            ),
            (
                "zero-radius positive motion truncates a fractional negative coordinate to zero",
                MovementVector::new(-0.25, 40.0),
                0.0,
                CollisionAxis::X,
                1.0,
                (99, 99),
                false,
            ),
            (
                "the same fractional negative coordinate floors on negative motion",
                MovementVector::new(-0.25, 40.0),
                0.0,
                CollisionAxis::X,
                -1.0,
                (99, 99),
                true,
            ),
        ];
        for (name, position, radius, axis, delta, tile, expected) in cases {
            let oracle =
                reference_game_axis_query_oracle(position, radius, 64, 16.0, axis, delta, tile);
            assert_eq!(oracle, expected, "oracle: {name}");
            assert_eq!(
                axis_query(position, radius, 64, 16.0, axis, delta, tile),
                oracle,
                "production: {name}",
            );
        }
    }

    #[test]
    fn reference_game_leading_query_matches_single_call_oracle_and_x_mutated_y() {
        let initial_position = MovementVector::new(30.0, 56.0);
        let initial_velocity = MovementVector::new(200.0, 20.0);
        let actual = integrate_player_collision(
            initial_position,
            initial_velocity,
            0.02,
            0.5,
            |candidate, axis, delta| axis_query(candidate, 14.0, 64, 16.0, axis, delta, (3, 3)),
        );
        let oracle = reference_game_collision_oracle(
            initial_position,
            initial_velocity,
            0.02,
            14.0,
            0.5,
            64,
            16.0,
            (3, 3),
        );
        assert_eq!(actual, oracle);
        assert_eq!(actual.position.x.to_bits(), 30.0_f32.to_bits());
        assert_eq!(actual.position.y.to_bits(), 0x4260_cccd);
        assert_eq!(actual.velocity.x.to_bits(), (-100.0_f32).to_bits());
        assert_eq!(actual.velocity.y.to_bits(), 10.0_f32.to_bits());
        assert!(actual.bounced);
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(actual.velocity.x),
            -1_000,
        );
        assert_eq!(
            crate::continuum_position::protocol_velocity_component(actual.velocity.y),
            100,
        );
    }

    #[test]
    fn golden_collision_fixture_pins_pixel_truncation_and_reference_edge_asymmetry() {
        let tile = |min_x, min_y, max_x, max_y| {
            movement_aabb_overlaps_tile(
                MovementVector::new(min_x, min_y),
                MovementVector::new(max_x, max_y),
                1,
                1,
                16.0,
            )
        };
        assert!(tile(8.0, 20.0, 16.0, 28.0), "left/minimum-edge contact");
        assert!(!tile(32.0, 20.0, 40.0, 28.0), "right/maximum-edge contact");
        assert!(tile(20.0, 8.0, 28.0, 16.0), "top/minimum-edge contact");
        assert!(!tile(20.0, 32.0, 28.0, 40.0), "bottom/maximum-edge contact");
        assert!(tile(8.001, 8.001, 16.001, 16.001), "corner penetration");
        assert!(tile(31.0, 20.0, 39.0, 28.0), "one-pixel penetration");
        assert!(!tile(32.001, 20.0, 40.001, 28.0));
    }

    #[test]
    fn golden_wormhole_fixture_samples_anchor_once_and_sets_top_speed() {
        let position = MovementVector::new(44.0, 72.0);
        let sample = sample_wormhole_gravity(position, 100, 500.0, 0.1, 16.0, |x, y| {
            (2..=6).contains(&x) && (2..=6).contains(&y)
        });
        close(sample.velocity_delta.x, 60.0);
        close(sample.velocity_delta.y, 0.0);
        assert_eq!(sample.top_speed, Some(500.0));
    }

    #[test]
    fn wormhole_reference_uses_tile_quantized_distance_and_integer_division() {
        let attract = sample_wormhole_gravity(
            MovementVector::new(44.0, 72.0),
            100,
            500.0,
            0.1,
            16.0,
            |x, y| x == 2 && y == 2,
        );
        // The reference game: p=(32,64), wh=(72,72), dist²=1665,
        // gravity_thrust=100000/1665=60 (integer division).
        close(attract.velocity_delta.x, 60.0);
        close(attract.velocity_delta.y, 0.0);

        let repel = sample_wormhole_gravity(
            MovementVector::new(44.0, 72.0),
            -100,
            500.0,
            0.1,
            16.0,
            |x, y| x == 2 && y == 2,
        );
        close(repel.velocity_delta.x, -60.0);
        close(repel.velocity_delta.y, 0.0);
    }

    #[test]
    fn wormhole_reference_pins_zero_distance_and_quantized_influence_boundary() {
        let centered = sample_wormhole_gravity(
            MovementVector::new(72.0, 72.0),
            100,
            500.0,
            0.1,
            16.0,
            |x, y| x == 2 && y == 2,
        );
        assert_eq!(centered.velocity_delta, MovementVector::ZERO);
        assert_eq!(centered.top_speed, Some(500.0));

        // Anchor tile 100 has protocol center (1640,1640). These positions
        // quantize to dist² 98,945 (inside) and 100,481 (outside) for
        // |gravity|*1000 == 100,000.
        let inside = sample_wormhole_gravity(
            MovementVector::new(83.75 * 16.0, 100.75 * 16.0),
            100,
            777.0,
            0.1,
            16.0,
            |x, y| x == 100 && y == 100,
        );
        close(inside.velocity_delta.length(), 1.0);
        assert_eq!(inside.top_speed, Some(777.0));

        let outside = sample_wormhole_gravity(
            MovementVector::new(83.75 * 16.0, 99.75 * 16.0),
            100,
            777.0,
            0.1,
            16.0,
            |x, y| x == 100 && y == 100,
        );
        assert_eq!(outside, WormholeGravitySample::default());
    }

    #[test]
    fn wormhole_reference_u16_coordinate_wrap_and_short_trajectory_are_exact() {
        let base = sample_wormhole_gravity(
            MovementVector::new(44.0, 72.0),
            100,
            500.0,
            0.1,
            16.0,
            |x, y| x == 2 && y == 2,
        );
        let wrapped = sample_wormhole_gravity(
            MovementVector::new(44.0 + 65_536.0, 72.0 + 65_536.0),
            100,
            500.0,
            0.1,
            16.0,
            |x, y| x == 4_098 && y == 4_098,
        );
        close(wrapped.velocity_delta.x, base.velocity_delta.x);
        close(wrapped.velocity_delta.y, base.velocity_delta.y);
        assert_eq!(wrapped.top_speed, base.top_speed);

        let mut position = MovementVector::new(44.0, 72.0);
        let mut velocity = MovementVector::ZERO;
        for (expected_position_x, expected_velocity_x) in [
            (44.166_668, 10.0),
            (44.5, 20.0),
            (45.0, 30.0),
            (45.666_668, 40.0),
        ] {
            let sample = sample_wormhole_gravity(position, 100, 500.0, 1.0 / 60.0, 16.0, |x, y| {
                x == 2 && y == 2
            });
            velocity = velocity.add(sample.velocity_delta);
            position = position.add(velocity.scale(1.0 / 60.0));
            close(velocity.x, expected_velocity_x);
            close(position.x, expected_position_x);
            close(position.y, 72.0);
        }
    }
}

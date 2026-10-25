//! Engine-independent weapon terrain motion. Both authority and presentation
//! use this axis order; actor contacts and damage remain with their caller.

use super::RulesVector;

pub(crate) const STEP_SECONDS: f32 = 0.01;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TerrainContact {
    pub(crate) x: bool,
    pub(crate) y: bool,
}

impl TerrainContact {
    pub(crate) fn any(self) -> bool {
        self.x || self.y
    }
}

/// A swept point cannot skip a 16 px wall even at unusually high weapon
/// speeds. Substeps alone do not provide that guarantee. A blocked axis keeps
/// its old coordinate, as in the reference game, then Y uses the resolved X coordinate.
pub(crate) fn step_terrain(
    position: &mut RulesVector,
    velocity: &mut RulesVector,
    dt: f32,
    bounce: bool,
    passes_through_walls: bool,
    is_solid: impl Fn(f32, f32) -> bool,
) -> TerrainContact {
    fn swept(x: f32, y: f32, dx: f32, dy: f32, solid: &impl Fn(f32, f32) -> bool) -> bool {
        let samples = (dx.abs().max(dy.abs()) / 8.0).ceil().max(1.0) as u32;
        (1..=samples).any(|i| {
            let fraction = i as f32 / samples as f32;
            solid(x + dx * fraction, y + dy * fraction)
        })
    }

    let dx = velocity.x * dt;
    let dy = velocity.y * dt;
    let x = !passes_through_walls && swept(position.x, position.y, dx, 0.0, &is_solid);
    if x {
        if bounce {
            velocity.x = -velocity.x;
        }
    } else {
        position.x += dx;
    }
    let y = !passes_through_walls && swept(position.x, position.y, 0.0, dy, &is_solid);
    if y {
        if bounce {
            velocity.y = -velocity.y;
        }
    } else {
        position.y += dy;
    }
    TerrainContact { x, y }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounce_uses_remaining_substeps_and_spends_one_corner_contact() {
        let mut position = RulesVector::new(175.0, 175.0);
        let mut velocity = RulesVector::new(640.0, 640.0);
        let mut contacts = 0;
        for _ in 0..2 {
            contacts += step_terrain(
                &mut position,
                &mut velocity,
                STEP_SECONDS,
                true,
                false,
                |x, y| x >= 176.0 || y >= 176.0,
            )
            .any() as u32;
        }
        assert_eq!(contacts, 1);
        assert!((position.x - 168.6).abs() < 0.0001);
        assert_eq!(position.x, position.y);
        assert_eq!(velocity, RulesVector::new(-640.0, -640.0));
    }

    #[test]
    fn sweep_cannot_tunnel_through_one_tile_at_extreme_speed() {
        let mut position = RulesVector::new(0.0, 8.0);
        let mut velocity = RulesVector::new(10_000.0, 0.0);
        assert!(
            step_terrain(
                &mut position,
                &mut velocity,
                STEP_SECONDS,
                false,
                false,
                |x, _| (32.0..48.0).contains(&x)
            )
            .x
        );
        assert_eq!(position.x, 0.0);
    }
}

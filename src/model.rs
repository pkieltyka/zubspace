use crate::remote_weapon::{RemoteWeaponSource, RemoteWeaponSourceIdentity};
use macroquad::prelude::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WeaponLifetimeKind {
    Bullet,
    Bomb,
    Mine,
    Decoy,
    Burst,
    Thor,
    Shrapnel,
}

#[cfg_attr(test, derive(Default))]
pub(crate) struct Ship {
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) orientation: f32,
    pub(crate) ship_type: usize,
    pub(crate) multifire_capable: bool,
    pub(crate) multifire: bool,
    pub(crate) stealth_capable: bool,
    pub(crate) stealth: bool,
    pub(crate) cloak_capable: bool,
    pub(crate) cloak: bool,
    pub(crate) xradar_capable: bool,
    pub(crate) xradar: bool,
    pub(crate) antiwarp_capable: bool,
    pub(crate) antiwarp: bool,
    pub(crate) flash: bool,
    pub(crate) proximity_bombs: bool,
    pub(crate) bouncing_bullets: bool,
    pub(crate) super_timer: f32,
    pub(crate) shield_timer: f32,
    pub(crate) engine_shutdown_timer: f32,
    /// EMP pauses recharge only; `engine_shutdown_timer` independently owns
    /// the prize's movement/weapon restrictions.
    pub(crate) emp_shutdown_timer: f32,
    pub(crate) repel_timer: f32,
    pub(crate) repels: u32,
    pub(crate) bursts: u32,
    pub(crate) bricks: u32,
    pub(crate) decoys: u32,
    pub(crate) thors: u32,
    pub(crate) rockets: u32,
    pub(crate) portals: u32,
    pub(crate) gun_level: u8,
    pub(crate) bomb_level: u8,
    pub(crate) rotation_per_second: f32,
    pub(crate) thrust_acceleration: f32,
    pub(crate) top_speed: f32,
    pub(crate) recharge_per_second: f32,
    pub(crate) shrapnel_upgrades: u8,
    pub(crate) bullet_cooldown: f32,
    pub(crate) burst_cooldown: f32,
    pub(crate) bomb_cooldown: f32,
    pub(crate) repel_cooldown: f32,
    pub(crate) wormhole_cooldown: f32,
    pub(crate) fake_antiwarp_timer: f32,
    pub(crate) rocket_timer: f32,
    pub(crate) rocket_cooldown: f32,
    pub(crate) max_energy: f32,
    pub(crate) energy: f32,
}

pub(crate) struct BotShip {
    pub(crate) id: usize,
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) orientation: f32,
    pub(crate) ship_type: usize,
    pub(crate) energy: f32,
    pub(crate) max_energy: f32,
    pub(crate) bounty: u32,
    pub(crate) kills: u32,
    pub(crate) deaths: u32,
    pub(crate) flag_goals: u32,
    pub(crate) fire_cooldown: f32,
    pub(crate) exhaust_timer: f32,
    pub(crate) ball_goals: u32,
    pub(crate) carrying_flag: bool,
    pub(crate) super_timer: f32,
    pub(crate) shield_timer: f32,
    pub(crate) engine_shutdown_timer: f32,
    pub(crate) emp_shutdown_timer: f32,
    pub(crate) respawn_timer: f32,
    pub(crate) age: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BotBullet {
    pub(crate) owner_id: usize,
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) frequency: u16,
    pub(crate) ttl: f32,
    pub(crate) age: f32,
    pub(crate) trail_timer: f32,
    pub(crate) level: u8,
    pub(crate) damage: f32,
    pub(crate) bouncing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OwnedProjectilePrediction {
    pub(crate) activation: u64,
    pub(crate) life_seq: u32,
    pub(crate) input_sequence: Option<u16>,
    pub(crate) projectile_index: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bullet {
    pub(crate) prediction: Option<OwnedProjectilePrediction>,
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) frequency: u16,
    pub(crate) ttl: f32,
    pub(crate) age: f32,
    pub(crate) trail_timer: f32,
    pub(crate) bounces_remaining: u8,
    pub(crate) bouncing: bool,
    pub(crate) activates_on_wall: bool,
    pub(crate) active: bool,
    pub(crate) level: u8,
    pub(crate) damage: f32,
}

pub(crate) struct BulletTrail {
    pub(crate) pos: Vec2,
    pub(crate) age: f32,
    pub(crate) level: u8,
    pub(crate) bouncing: bool,
    #[allow(dead_code)]
    pub(crate) remote_source: Option<RemoteWeaponSourceIdentity>,
}

pub(crate) struct BulletImpact {
    pub(crate) pos: Vec2,
    pub(crate) age: f32,
    #[allow(dead_code)]
    pub(crate) remote_source: Option<RemoteWeaponSourceIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExplosiveShrapnel {
    pub(crate) count: usize,
    pub(crate) level: u8,
    pub(crate) bouncing: bool,
    pub(crate) rng_seed: u32,
}

impl ExplosiveShrapnel {
    pub(crate) fn none() -> Self {
        Self {
            count: 0,
            level: 0,
            bouncing: false,
            rng_seed: 0,
        }
    }

    pub(crate) fn without_fragments(self) -> Self {
        Self { count: 0, ..self }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProximitySensorState {
    pub(crate) target_id: u16,
    pub(crate) highest_axis_offset: f32,
    pub(crate) timer: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bomb {
    pub(crate) prediction: Option<OwnedProjectilePrediction>,
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) frequency: u16,
    pub(crate) ttl: f32,
    pub(crate) age: f32,
    pub(crate) trail_timer: f32,
    pub(crate) bounces_remaining: u8,
    pub(crate) level: u8,
    pub(crate) damage: f32,
    pub(crate) emp_shutdown_time: f32,
    pub(crate) shrapnel: ExplosiveShrapnel,
    pub(crate) proximity: bool,
    pub(crate) proximity_sensor: Option<ProximitySensorState>,
}

pub(crate) struct BombFlash {
    pub(crate) pos: Vec2,
    pub(crate) age: f32,
    #[allow(dead_code)]
    pub(crate) remote_source: Option<RemoteWeaponSourceIdentity>,
}

pub(crate) struct BombTrail {
    pub(crate) pos: Vec2,
    pub(crate) age: f32,
    #[allow(dead_code)]
    pub(crate) remote_source: Option<RemoteWeaponSourceIdentity>,
}

pub(crate) struct Mine {
    pub(crate) vel: Vec2,
    pub(crate) bomb_gravity: Option<i16>,
    pub(crate) prediction: Option<OwnedProjectilePrediction>,
    pub(crate) pos: Vec2,
    pub(crate) frequency: u16,
    pub(crate) ttl: f32,
    pub(crate) level: u8,
    pub(crate) damage: f32,
    pub(crate) emp_shutdown_time: f32,
    pub(crate) shrapnel: ExplosiveShrapnel,
    pub(crate) proximity: bool,
    pub(crate) proximity_sensor: Option<ProximitySensorState>,
    pub(crate) age: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shrapnel {
    pub(crate) level: u8,
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) frequency: u16,
    pub(crate) ttl: f32,
    pub(crate) age: f32,
    pub(crate) bouncing: bool,
    pub(crate) damage: f32,
    pub(crate) inactive_damage: f32,
}

pub(crate) struct Thor {
    pub(crate) prediction: Option<OwnedProjectilePrediction>,
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) ttl: f32,
    pub(crate) age: f32,
    pub(crate) damage: f32,
    pub(crate) shrapnel: ExplosiveShrapnel,
    pub(crate) proximity_sensor: Option<ProximitySensorState>,
    /// Raw 2-bit weapon-data level (0..=3). Used to size the explosion and
    /// proximity radii — the reference game: `prox = ProximityDistance + raw_level + 3`,
    /// `explode_pixels = BombExplodePixels * (4 + raw_level)`.
    pub(crate) raw_level: u8,
}

pub(crate) struct Brick {
    pub(crate) tile: IVec2,
    pub(crate) previous_tile: u8,
    pub(crate) team: u16,
    pub(crate) ttl: f32,
    pub(crate) age: f32,
    /// Authoritative creation time for network walls. Offline bricks use dt.
    pub(crate) server_timestamp: Option<u32>,
}

pub(crate) struct Decoy {
    pub(crate) authority_binding: Option<crate::remote_weapon::RemoteProjectileAuthorityBinding>,
    pub(crate) presentation_clock: Option<crate::remote_weapon::RemoteProjectileClock>,
    pub(crate) prediction: Option<OwnedProjectilePrediction>,
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) frequency: u16,
    pub(crate) orientation: f32,
    pub(crate) initial_orientation: f32,
    /// `None` denotes the local player's decoy. Remote decoys retain the
    /// complete immutable fire-time source so player-id reuse cannot steer
    /// their presentation or lifecycle.
    pub(crate) remote_source: Option<RemoteWeaponSource>,
    pub(crate) last_repel_mutation_event_id: Option<u32>,
    pub(crate) authoritative_repel_state: Option<crate::realtime_event::RealtimeRepelMutationState>,
    pub(crate) ship_type: usize,
    /// Immutable fire-time label state. A recycled player id cannot rename or
    /// recolor a Decoy that belongs to the earlier runtime.
    pub(crate) owner_name: String,
    pub(crate) owner_bounty: u32,
    pub(crate) ttl: f32,
    pub(crate) age: f32,
}

pub(crate) struct RepelEffect {
    pub(crate) pos: Vec2,
    pub(crate) age: f32,
    #[allow(dead_code)]
    pub(crate) remote_source: Option<RemoteWeaponSourceIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RemoteRepelPresentationFact {
    pub(crate) arena_scope: crate::remote_weapon::RemoteWeaponArenaScope,
    pub(crate) event_id: u32,
    pub(crate) server_tick: u32,
    pub(crate) origin: Vec2,
    pub(crate) source_frequency: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RemoteRepelAuthoritativeMutation {
    pub(crate) arena_scope: crate::remote_weapon::RemoteWeaponArenaScope,
    pub(crate) event_id: u32,
    pub(crate) server_tick: u32,
    pub(crate) state: crate::realtime_event::RealtimeRepelMutationState,
}

pub(crate) struct WarpEffect {
    pub(crate) pos: Vec2,
    pub(crate) age: f32,
}

pub(crate) struct Portal {
    pub(crate) pos: Vec2,
    pub(crate) ttl: f32,
    pub(crate) age: f32,
    /// Exact server action that established this endpoint. Offline portals
    /// leave this unset and continue to use the local relative lifetime.
    pub(crate) server_action_id: Option<u32>,
    pub(crate) server_placed_at_tick: Option<u32>,
    pub(crate) server_expires_at_tick: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortalEndpointAuthority {
    Active(u32),
    Consumed(u32),
    Expired(u32),
}

impl PortalEndpointAuthority {
    pub(crate) fn action_id(self) -> u32 {
        match self {
            Self::Active(action_id) | Self::Consumed(action_id) | Self::Expired(action_id) => {
                action_id
            }
        }
    }
}

pub(crate) struct Explosion {
    pub(crate) prediction: Option<OwnedProjectilePrediction>,
    pub(crate) pos: Vec2,
    pub(crate) damage_pos: Vec2,
    pub(crate) age: f32,
    pub(crate) kind: ExplosionKind,
    pub(crate) damages_bots: bool,
    pub(crate) damage: f32,
    pub(crate) radius: f32,
    pub(crate) emp_shutdown_time: f32,
    #[allow(dead_code)]
    pub(crate) remote_source: Option<RemoteWeaponSourceIdentity>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExplosionKind {
    Bomb,
    EmpBomb,
}

pub(crate) struct ShipExplosion {
    pub(crate) pos: Vec2,
    pub(crate) age: f32,
}

pub(crate) struct BurstEffect {
    pub(crate) pos: Vec2,
    pub(crate) age: f32,
    #[allow(dead_code)]
    pub(crate) remote_source: Option<RemoteWeaponSourceIdentity>,
}

pub(crate) struct ExhaustParticle {
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) age: f32,
}

pub(crate) struct RocketParticle {
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) age: f32,
}

pub(crate) struct Powerball {
    pub(crate) ball_id: u8,
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) frequency: u16,
    pub(crate) carried: bool,
    pub(crate) age: f32,
    pub(crate) trail_timer: f32,
    pub(crate) pass_cooldown: f32,
    pub(crate) carry_timer: f32,
    pub(crate) friction: f32,
    pub(crate) last_touch: PowerballTouch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PowerballTouch {
    Player,
    Bot(usize),
}

pub(crate) struct PowerballTrail {
    pub(crate) pos: Vec2,
    pub(crate) age: f32,
}

pub(crate) struct DroppedFlag {
    pub(crate) pos: Vec2,
    pub(crate) age: f32,
    pub(crate) ttl: f32,
}

pub(crate) struct PrizePickup {
    /// Zero for offline-only greens; nonzero identities are minted by the
    /// authoritative server and echoed unchanged in pickup claims.
    pub(crate) collectible_id: u32,
    pub(crate) pos: Vec2,
    pub(crate) kind: PrizeKind,
    pub(crate) negative: bool,
    pub(crate) age: f32,
    pub(crate) ttl: f32,
    pub(crate) claim_retry_timer: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrizeKind {
    Energy,
    FullCharge,
    Rocket,
    Repel,
    Burst,
    Brick,
    Decoy,
    Thor,
    Portal,
    Super,
    Shield,
    EngineShutdown,
    BouncingBullets,
    Stealth,
    Cloak,
    XRadar,
    Warp,
    Rotation,
    Thruster,
    TopSpeed,
    Recharge,
    Shrapnel,
    Guns,
    Bombs,
    Multifire,
    Proximity,
    Antiwarp,
    Multiprize,
}

impl PrizeKind {
    pub(crate) fn protocol_id(self) -> i16 {
        match self {
            Self::Recharge => 1,
            Self::Energy => 2,
            Self::Rotation => 3,
            Self::Stealth => 4,
            Self::Cloak => 5,
            Self::XRadar => 6,
            Self::Warp => 7,
            Self::Guns => 8,
            Self::Bombs => 9,
            Self::BouncingBullets => 10,
            Self::Thruster => 11,
            Self::TopSpeed => 12,
            Self::FullCharge => 13,
            Self::EngineShutdown => 14,
            Self::Multifire => 15,
            Self::Proximity => 16,
            Self::Super => 17,
            Self::Shield => 18,
            Self::Shrapnel => 19,
            Self::Antiwarp => 20,
            Self::Repel => 21,
            Self::Burst => 22,
            Self::Decoy => 23,
            Self::Thor => 24,
            Self::Multiprize => 25,
            Self::Brick => 26,
            Self::Rocket => 27,
            Self::Portal => 28,
        }
    }

    pub(crate) fn from_protocol_id(prize_id: i16) -> Option<(Self, bool)> {
        let negative = prize_id < 0;
        let id = prize_id.checked_abs()?;
        let kind = match id {
            1 => Self::Recharge,
            2 => Self::Energy,
            3 => Self::Rotation,
            4 => Self::Stealth,
            5 => Self::Cloak,
            6 => Self::XRadar,
            7 => Self::Warp,
            8 => Self::Guns,
            9 => Self::Bombs,
            10 => Self::BouncingBullets,
            11 => Self::Thruster,
            12 => Self::TopSpeed,
            13 => Self::FullCharge,
            14 => Self::EngineShutdown,
            15 => Self::Multifire,
            16 => Self::Proximity,
            17 => Self::Super,
            18 => Self::Shield,
            19 => Self::Shrapnel,
            20 => Self::Antiwarp,
            21 => Self::Repel,
            22 => Self::Burst,
            23 => Self::Decoy,
            24 => Self::Thor,
            25 => Self::Multiprize,
            26 => Self::Brick,
            27 => Self::Rocket,
            28 => Self::Portal,
            _ => return None,
        };
        Some((kind, negative))
    }

    pub(crate) fn random() -> Self {
        if macroquad::rand::gen_range(0, 24) == 0 {
            return Self::EngineShutdown;
        }

        match macroquad::rand::gen_range(0, 27) {
            0 => Self::Energy,
            1 => Self::FullCharge,
            2 => Self::Rocket,
            3 => Self::Repel,
            4 => Self::Burst,
            5 => Self::Brick,
            6 => Self::Decoy,
            7 => Self::Thor,
            8 => Self::Portal,
            9 => Self::Super,
            10 => Self::Shield,
            11 => Self::BouncingBullets,
            12 => Self::Stealth,
            13 => Self::Cloak,
            14 => Self::XRadar,
            15 => Self::Warp,
            16 => Self::Rotation,
            17 => Self::Thruster,
            18 => Self::TopSpeed,
            19 => Self::Recharge,
            20 => Self::Shrapnel,
            21 => Self::Guns,
            22 => Self::Bombs,
            23 => Self::Multifire,
            24 => Self::Proximity,
            25 => Self::Antiwarp,
            _ => Self::Multiprize,
        }
    }

    pub(crate) fn frame_index(self) -> usize {
        match self {
            Self::Energy => 0,
            Self::FullCharge => 1,
            Self::Rocket => 7,
            Self::Repel => 8,
            Self::Burst => 6,
            Self::Brick => 5,
            Self::Decoy => 4,
            Self::Thor => 3,
            Self::Portal => 9,
            Self::Super => 1,
            Self::Shield => 2,
            Self::EngineShutdown => 0,
            Self::BouncingBullets => 2,
            Self::Stealth => 4,
            Self::Cloak => 5,
            Self::XRadar => 6,
            Self::Warp => 9,
            Self::Rotation => 2,
            Self::Thruster => 3,
            Self::TopSpeed => 4,
            Self::Recharge => 1,
            Self::Shrapnel => 6,
            Self::Guns => 2,
            Self::Bombs => 3,
            Self::Multifire => 7,
            Self::Proximity => 8,
            Self::Antiwarp => 9,
            Self::Multiprize => 1,
        }
    }
}

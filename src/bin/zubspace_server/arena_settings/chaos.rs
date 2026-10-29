use crate::continuum_weapon::settings_weapon_level_to_bits;
use crate::wire::{put_i16, put_i32, put_u16, put_u32};

use super::{
    ARENA_SETTINGS, CONTINUUM_ARENA_SETTINGS_SIZE, CONTINUUM_BYTE_SETTINGS_OFFSET,
    CONTINUUM_GLOBAL_INTS_OFFSET, CONTINUUM_PRIZE_WEIGHT_COUNT, CONTINUUM_PRIZE_WEIGHTS_OFFSET,
    CONTINUUM_SHIP_SETTINGS_OFFSET, CONTINUUM_SHIP_SETTINGS_SIZE, CONTINUUM_SHORT_SETTINGS_OFFSET,
};

#[derive(Clone, Copy)]
struct ChaosShipProfile {
    maximum_rotation: u16,
    maximum_thrust: u16,
    maximum_speed: u16,
    maximum_recharge: u16,
    maximum_energy: u16,
    gravity: i16,
    bullet_fire_delay: u16,
    multifire_delay: u16,
    max_bombs: u8,
    shrapnel_max: u8,
    cloak_energy: u16,
    cloak_status: u8,
    double_barrel: bool,
    emp_bomb: bool,
    see_mines: bool,
    see_bomb_level: u8,
    bomb_bounces: u8,
}

const CHAOS_SHIP_PROFILES: [ChaosShipProfile; 8] = [
    ChaosShipProfile {
        maximum_rotation: 290,
        maximum_thrust: 19,
        maximum_speed: 3250,
        maximum_recharge: 1150,
        maximum_energy: 1700,
        gravity: 1500,
        bullet_fire_delay: 24,
        multifire_delay: 50,
        max_bombs: 2,
        shrapnel_max: 8,
        cloak_energy: 0,
        cloak_status: 0,
        double_barrel: false,
        emp_bomb: false,
        see_mines: false,
        see_bomb_level: 0,
        bomb_bounces: 0,
    },
    ChaosShipProfile {
        maximum_rotation: 230,
        maximum_thrust: 17,
        maximum_speed: 3750,
        maximum_recharge: 1150,
        maximum_energy: 1700,
        gravity: 1500,
        bullet_fire_delay: 24,
        multifire_delay: 50,
        max_bombs: 2,
        shrapnel_max: 8,
        cloak_energy: 0,
        cloak_status: 0,
        double_barrel: false,
        emp_bomb: false,
        see_mines: false,
        see_bomb_level: 0,
        bomb_bounces: 0,
    },
    ChaosShipProfile {
        maximum_rotation: 230,
        maximum_thrust: 18,
        maximum_speed: 3250,
        maximum_recharge: 1150,
        maximum_energy: 1700,
        gravity: 1500,
        bullet_fire_delay: 24,
        multifire_delay: 50,
        max_bombs: 2,
        shrapnel_max: 8,
        cloak_energy: 100,
        cloak_status: 2,
        double_barrel: false,
        emp_bomb: false,
        see_mines: false,
        see_bomb_level: 0,
        bomb_bounces: 0,
    },
    ChaosShipProfile {
        maximum_rotation: 230,
        maximum_thrust: 18,
        maximum_speed: 3250,
        maximum_recharge: 1150,
        maximum_energy: 1700,
        gravity: 1500,
        bullet_fire_delay: 24,
        multifire_delay: 50,
        max_bombs: 3,
        shrapnel_max: 8,
        cloak_energy: 0,
        cloak_status: 0,
        double_barrel: false,
        emp_bomb: false,
        see_mines: false,
        see_bomb_level: 0,
        bomb_bounces: 0,
    },
    ChaosShipProfile {
        maximum_rotation: 230,
        maximum_thrust: 18,
        maximum_speed: 3250,
        maximum_recharge: 1150,
        maximum_energy: 1700,
        gravity: 1500,
        bullet_fire_delay: 25,
        multifire_delay: 50,
        max_bombs: 2,
        shrapnel_max: 8,
        cloak_energy: 0,
        cloak_status: 0,
        double_barrel: true,
        emp_bomb: false,
        see_mines: false,
        see_bomb_level: 0,
        bomb_bounces: 0,
    },
    ChaosShipProfile {
        maximum_rotation: 230,
        maximum_thrust: 18,
        maximum_speed: 3250,
        maximum_recharge: 1150,
        maximum_energy: 1700,
        gravity: 1500,
        bullet_fire_delay: 24,
        multifire_delay: 50,
        max_bombs: 2,
        shrapnel_max: 0,
        cloak_energy: 0,
        cloak_status: 0,
        double_barrel: false,
        emp_bomb: true,
        see_mines: true,
        see_bomb_level: 0,
        bomb_bounces: 0,
    },
    ChaosShipProfile {
        maximum_rotation: 230,
        maximum_thrust: 18,
        maximum_speed: 3250,
        maximum_recharge: 1150,
        maximum_energy: 1700,
        gravity: 1500,
        bullet_fire_delay: 24,
        multifire_delay: 50,
        max_bombs: 2,
        shrapnel_max: 8,
        cloak_energy: 0,
        cloak_status: 0,
        double_barrel: false,
        emp_bomb: false,
        see_mines: false,
        see_bomb_level: 0,
        bomb_bounces: 1,
    },
    ChaosShipProfile {
        maximum_rotation: 260,
        maximum_thrust: 18,
        maximum_speed: 3250,
        maximum_recharge: 1150,
        maximum_energy: 1700,
        gravity: 0,
        bullet_fire_delay: 16,
        multifire_delay: 40,
        max_bombs: 2,
        shrapnel_max: 8,
        // zubspace owner override: Shark starts cloak-capable, like Spider.
        cloak_energy: 100,
        cloak_status: 2,
        double_barrel: false,
        emp_bomb: false,
        see_mines: false,
        see_bomb_level: 1,
        bomb_bounces: 0,
    },
];

const CHAOS_INITIAL_ROTATION: u16 = 200;
const CHAOS_INITIAL_THRUST: u16 = 15;
const CHAOS_INITIAL_SPEED: u16 = 2010;
const CHAOS_INITIAL_RECHARGE: u16 = 400;
// Initialised at MaximumEnergy so warbird spawns at full energy.
// Stock chaos used InitialEnergy=1000 but relied on emergent
// FullCharge sub-prizes from the MultiPrize cascade to top the
// player off; that's brittle to prize-weight tuning. Setting Initial
// = Maximum makes the spawn UX deterministic. The Energy upgrade
// prize (id=2) still applies but has no headroom to grow the cap
// (saturates at Maximum), which is fine — its job in chaos is to
// counter -Energy negatives, not act as a real progression knob.
const CHAOS_INITIAL_ENERGY: u16 = 1700;
const CHAOS_UPGRADE_ROTATION: u16 = 40;
const CHAOS_UPGRADE_THRUST: u16 = 2;
const CHAOS_UPGRADE_SPEED: u16 = 250;
const CHAOS_UPGRADE_RECHARGE: u16 = 166;
const CHAOS_UPGRADE_ENERGY: u16 = 100;
const CHAOS_SUPER_TIME: u32 = 6000;
const CHAOS_SHIELD_TIME: u32 = 4000;
const CHAOS_GRAVITY_TOP_SPEED: i16 = 100;
const CHAOS_BULLET_FIRE_ENERGY: u16 = 20;
const CHAOS_MULTIFIRE_ENERGY: u16 = 30;
const CHAOS_BOMB_FIRE_ENERGY: u16 = 300;
const CHAOS_BOMB_FIRE_ENERGY_UPGRADE: u16 = 50;
const CHAOS_LANDMINE_FIRE_ENERGY: u16 = 270;
const CHAOS_LANDMINE_FIRE_ENERGY_UPGRADE: u16 = 150;
const CHAOS_BULLET_SPEED: u16 = 2000;
const CHAOS_BOMB_SPEED: u16 = 2000;
const CHAOS_MULTIFIRE_ANGLE: u16 = 500;
const CHAOS_AFTERBURNER_ENERGY: u16 = 1200;
const CHAOS_BOMB_THRUST: u16 = 400;
const CHAOS_BURST_SPEED: u16 = 3000;
const CHAOS_TURRET_THRUST_PENALTY: i16 = 1;
const CHAOS_TURRET_SPEED_PENALTY: i16 = 125;
const CHAOS_SHIP_RADIUS_PIXELS: u16 = 14;
const CHAOS_BOMB_FIRE_DELAY: u16 = 150;
const CHAOS_LANDMINE_FIRE_DELAY: u16 = 125;
const CHAOS_ROCKET_TIME: u16 = 1000;
const CHAOS_DAMAGE_FACTOR: u16 = 30;
const CHAOS_PRIZE_SHARE_LIMIT: u16 = 100;
const CHAOS_SOCCER_BALL_THROW_TIMER: u16 = 1200;
const CHAOS_SOCCER_BALL_FRICTION: u16 = 12;
const CHAOS_SOCCER_BALL_PROXIMITY: u16 = 64;
const CHAOS_SOCCER_BALL_SPEED: u16 = 2500;
const CHAOS_TURRET_LIMIT: u8 = 5;
const CHAOS_MAX_MINES: u8 = 5;
const CHAOS_MAX_CONSUMABLE: u8 = 3;
const CHAOS_MAX_FREQUENCY: i32 = 10_000;
// PrizeWeights mirror `assets/zones/chaos-zone/arena.conf [PrizeWeight]`.
// Index 14 (MultiFire) is intentionally 0 in the current authored balance.
// This excludes both initial bounty grants and naturally generated greens;
// capability ownership never implies an active toggle. Positive capability
// paths are verified with an explicitly separate prize-weight fixture.
// Indices follow Continuum protocol prize ids: QuickCharge, Energy,
// Rotation, Stealth, Cloak, XRadar, Warp, Gun, Bomb, BouncingBullets,
// Thruster, TopSpeed, Recharge, Glue, MultiFire, Proximity, AllWeapons,
// Shields, Shrapnel, AntiWarp, Repel, Burst, Decoy, Thor, MultiPrize,
// Brick, Rocket, Portal.
const SVS_PRIZE_WEIGHTS: [u8; CONTINUUM_PRIZE_WEIGHT_COUNT] = [
    40, 40, 40, 40, 25, 40, 7, 25, 25, 25, 40, 40, 10, 0, 0, 25, 10, 10, 30, 25, 70, 70, 40, 30,
    130, 200, 50, 60,
];

pub(super) fn build_arena_settings_packet() -> Vec<u8> {
    let mut data = vec![0; CONTINUUM_ARENA_SETTINGS_SIZE];
    // Flags word: `Type = 3`, `ExactDamage = 1` (bit 8).
    //
    // Chaos plays with **exact** bullet/burst/shrapnel damage, and
    // `assets/zones/chaos-zone/arena.conf [Bullet] ExactDamage=1` is the
    // pinned source of truth. No live Chaos config was ever obtained, and the
    // SVS references this profile mirrors disagree on the key: ASSS
    // `dist/conf/svs/misc` omits it (ASSS default 0 = random) while
    // SubspaceServer `Zone/conf/svs/misc` sets 1. Owner decision:
    // Chaos stays exact; Extreme keeps the randomized behavior of its
    // hash-verified upstream Extreme Games config (`ExactDamage=0`).
    put_u32(&mut data, 0, 3 | (1 << 8));

    for (ship_index, profile) in CHAOS_SHIP_PROFILES.iter().copied().enumerate() {
        write_chaos_ship_settings(
            &mut data,
            CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE,
            profile,
        );
    }

    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET, 200_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 4, 750_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 8, 550);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 12, 8000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 16, 1500);
    put_i32(
        &mut data,
        CONTINUUM_GLOBAL_INTS_OFFSET + 28,
        CHAOS_MAX_FREQUENCY,
    );
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 32, 5000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 36, 12_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 40, 700_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 44, 100_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 48, 0);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 52, 12_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 56, 25);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 60, 5500);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 64, 3000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 76, 3000);

    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 2, 150);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 6, 80);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 8, 800);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 12, 0);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 14, 700);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 16, 3);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 18, 2);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 20, 26);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 22, 48);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 30, 225);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 32, 512);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 40, 10_000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 42, 700);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 44, 10);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 46, 6);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 48, 8000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 50, 4000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 52, 300);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 54, 120);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 56, 640);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 58, -1);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 64, 10);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 66, 12_000);
    // Owner-selected shared central range; match the authored Chaos profile.
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 68, 176);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 70, 400);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 72, 1000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 76, 2000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 80, 1000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 82, 1000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 88, 1000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 90, 1000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 92, 0);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 94, 20);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 100, 0);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 102, 0);

    data[CONTINUUM_BYTE_SETTINGS_OFFSET] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 1] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 2] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 3] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 4] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 7] = 12;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 8] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 9] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 11] = 0;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 13] = 10;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 16] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 18] = 4;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 19] = 0;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 20] = 0;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 21] = 0;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 22] = 0;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 23] = 0;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 24] = 50;

    data[CONTINUUM_PRIZE_WEIGHTS_OFFSET
        ..CONTINUUM_PRIZE_WEIGHTS_OFFSET + CONTINUUM_PRIZE_WEIGHT_COUNT]
        .copy_from_slice(&SVS_PRIZE_WEIGHTS);

    data[0] = ARENA_SETTINGS;
    data
}

fn write_chaos_ship_settings(data: &mut [u8], offset: usize, profile: ChaosShipProfile) {
    put_u32(data, offset, CHAOS_SUPER_TIME);
    put_u32(data, offset + 4, CHAOS_SHIELD_TIME);
    put_i16(data, offset + 8, profile.gravity);
    put_i16(data, offset + 10, CHAOS_GRAVITY_TOP_SPEED);
    put_u16(data, offset + 12, CHAOS_BULLET_FIRE_ENERGY);
    put_u16(data, offset + 14, CHAOS_MULTIFIRE_ENERGY);
    put_u16(data, offset + 16, CHAOS_BOMB_FIRE_ENERGY);
    put_u16(data, offset + 18, CHAOS_BOMB_FIRE_ENERGY_UPGRADE);
    put_u16(data, offset + 20, CHAOS_LANDMINE_FIRE_ENERGY);
    put_u16(data, offset + 22, CHAOS_LANDMINE_FIRE_ENERGY_UPGRADE);
    put_u16(data, offset + 24, CHAOS_BULLET_SPEED);
    put_u16(data, offset + 26, CHAOS_BOMB_SPEED);
    put_u16(
        data,
        offset + 28,
        (CHAOS_SHIP_RADIUS_PIXELS << 3) | profile.see_bomb_level as u16,
    );
    put_u16(data, offset + 30, CHAOS_MULTIFIRE_ANGLE);
    put_u16(data, offset + 32, profile.cloak_energy);
    put_u16(data, offset + 34, 333);
    put_u16(data, offset + 36, 100);
    put_u16(data, offset + 38, 250);
    put_u16(data, offset + 40, profile.maximum_rotation);
    put_u16(data, offset + 42, profile.maximum_thrust);
    put_u16(data, offset + 44, profile.maximum_speed);
    put_u16(data, offset + 46, profile.maximum_recharge);
    put_u16(data, offset + 48, profile.maximum_energy);
    put_u16(data, offset + 50, CHAOS_INITIAL_ROTATION);
    put_u16(data, offset + 52, CHAOS_INITIAL_THRUST);
    put_u16(data, offset + 54, CHAOS_INITIAL_SPEED);
    put_u16(data, offset + 56, CHAOS_INITIAL_RECHARGE);
    put_u16(data, offset + 58, CHAOS_INITIAL_ENERGY);
    put_u16(data, offset + 60, CHAOS_UPGRADE_ROTATION);
    put_u16(data, offset + 62, CHAOS_UPGRADE_THRUST);
    put_u16(data, offset + 64, CHAOS_UPGRADE_SPEED);
    put_u16(data, offset + 66, CHAOS_UPGRADE_RECHARGE);
    put_u16(data, offset + 68, CHAOS_UPGRADE_ENERGY);
    put_u16(data, offset + 70, CHAOS_AFTERBURNER_ENERGY);
    put_u16(data, offset + 72, CHAOS_BOMB_THRUST);
    put_u16(data, offset + 74, CHAOS_BURST_SPEED);
    put_i16(data, offset + 76, CHAOS_TURRET_THRUST_PENALTY);
    put_i16(data, offset + 78, CHAOS_TURRET_SPEED_PENALTY);
    put_u16(data, offset + 80, profile.bullet_fire_delay);
    put_u16(data, offset + 82, profile.multifire_delay);
    put_u16(data, offset + 84, CHAOS_BOMB_FIRE_DELAY);
    put_u16(data, offset + 86, CHAOS_LANDMINE_FIRE_DELAY);
    put_u16(data, offset + 88, CHAOS_ROCKET_TIME);
    put_u16(data, offset + 90, 30);
    put_u16(data, offset + 92, CHAOS_DAMAGE_FACTOR);
    put_u16(data, offset + 94, CHAOS_PRIZE_SHARE_LIMIT);
    put_u16(data, offset + 96, 0);
    put_u16(data, offset + 98, CHAOS_SOCCER_BALL_THROW_TIMER);
    put_u16(data, offset + 100, CHAOS_SOCCER_BALL_FRICTION);
    put_u16(data, offset + 102, CHAOS_SOCCER_BALL_PROXIMITY);
    put_u16(data, offset + 104, CHAOS_SOCCER_BALL_SPEED);
    data[offset + 106] = CHAOS_TURRET_LIMIT;
    data[offset + 107] = 24;
    data[offset + 108] = CHAOS_MAX_MINES;
    data[offset + 109] = CHAOS_MAX_CONSUMABLE;
    data[offset + 110] = CHAOS_MAX_CONSUMABLE;
    data[offset + 111] = CHAOS_MAX_CONSUMABLE;
    data[offset + 112] = CHAOS_MAX_CONSUMABLE;
    data[offset + 113] = CHAOS_MAX_CONSUMABLE;
    data[offset + 114] = CHAOS_MAX_CONSUMABLE;
    data[offset + 115] = CHAOS_MAX_CONSUMABLE;
    data[offset + 116] = 0;
    data[offset + 117] = 0;
    data[offset + 118] = 0;
    data[offset + 119] = 0;
    data[offset + 120] = 0;
    data[offset + 121] = 0;
    data[offset + 122] = 0;
    data[offset + 123] = profile.bomb_bounces;
    // Arena settings capability fields store 0..=3 directly. Do not use the
    // per-shot weapon_data level encoding here; InitialBombs=0 must remain
    // "no bombs" for Chaos Warbirds until they prize into bombs.
    let mut capability = profile.shrapnel_max as u32
        | (2 << 5)
        | ((profile.cloak_status as u32) << 10)
        | (1 << 12)
        | (1 << 14)
        | (1 << 16)
        | ((settings_weapon_level_to_bits(3) as u32) << 18)
        | ((settings_weapon_level_to_bits(3) as u32) << 20)
        | ((settings_weapon_level_to_bits(0) as u32) << 22)
        | ((settings_weapon_level_to_bits(profile.max_bombs) as u32) << 24);
    if profile.double_barrel {
        capability |= 1 << 26;
    }
    if profile.emp_bomb {
        capability |= 1 << 27;
    }
    if profile.see_mines {
        capability |= 1 << 28;
    }
    put_u32(data, offset + 124, capability);
}

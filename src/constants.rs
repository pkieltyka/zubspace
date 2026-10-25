pub(crate) const SHIP_FRAME: f32 = 36.0;
pub(crate) const TILE_SIZE: f32 = 16.0;
pub(crate) const MAP_TILES: i32 = 1024;
pub(crate) const SAFE_TILE_ID: u8 = 171;
pub(crate) const SPECTATOR_SHIP: u8 = 8;
pub(crate) const STATUS_STEALTH: u8 = 1 << 0;
pub(crate) const STATUS_CLOAK: u8 = 1 << 1;
pub(crate) const STATUS_XRADAR: u8 = 1 << 2;
pub(crate) const STATUS_ANTIWARP: u8 = 1 << 3;
pub(crate) const STATUS_FLASH: u8 = 1 << 4;
pub(crate) const STATUS_SAFETY: u8 = 1 << 5;
pub(crate) const FLAG_TILE_ID: u8 = 170;
pub(crate) const GOAL_TILE_ID: u8 = 172;
pub(crate) const DOOR_FIRST_TILE_ID: u8 = 162;
pub(crate) const DOOR_LAST_TILE_ID: u8 = 169;
pub(crate) const OPEN_DOOR_TILE_ID: u8 = DOOR_LAST_TILE_ID + 1;
pub(crate) const DOOR_CYCLE_INTERVAL: f32 = 1.2;
pub(crate) const DOOR_MODE_LOCAL_INCREMENT: i16 = i16::MIN;
pub(crate) const STAR_LAYER_COUNT: usize = 2;
pub(crate) const STAR_LAYER_CELLS: [f32; STAR_LAYER_COUNT] = [256.0, 128.0];
pub(crate) const STAR_LAYER_SPEEDS: [f32; STAR_LAYER_COUNT] = [0.35, 0.65];
pub(crate) const STAR_LAYER_ALPHA: [u8; STAR_LAYER_COUNT] = [135, 190];
#[allow(dead_code)]
pub(crate) const NETWORK_RTC_POSITION_INTERVAL: f32 = 1.0 / 60.0;
#[allow(dead_code)]
pub(crate) const NETWORK_TIME_SYNC_INTERVAL: f32 = 5.0;
#[cfg(target_arch = "wasm32")]
pub(crate) const NETWORK_VOICE_SOUND_TTL: f32 = 8.0;
pub(crate) const BULLET_LIFETIME: f32 = 1.25;
pub(crate) const BULLET_IMPACT_LIFETIME: f32 = 0.4;
pub(crate) const BULLET_TRAIL_LIFETIME: f32 = 0.14;
pub(crate) const BULLET_TRAIL_INTERVAL: f32 = 0.02;
pub(crate) const BULLET_TRAIL_FRAME_START: usize = 42;
pub(crate) const BULLET_TRAIL_FRAMES: usize = 14;
pub(crate) const WEAPON_LEVEL_MIN: u8 = 1;
pub(crate) const WEAPON_LEVEL_MAX: u8 = 4;
pub(crate) const BULLET_FRAMES_PER_LEVEL: usize = 4;
pub(crate) const BOUNCING_BULLET_FRAME_START: usize = 20;
pub(crate) const BOUNCING_BULLET_FRAMES: usize = 4;
/// The four-frame Burst loops live on the **bullet** sheet, not the EMP sheet: the
/// reference loads `bullet_sprites` and `CreateBulletAnimations` sets
/// `anim_burst_inactive = renderables + 16` and `anim_burst_active = renderables + count - 4`.
/// `bullets.bm2` is 40 frames, so inactive is 16..=19 and active is 36..=39.
pub(crate) const BURST_INACTIVE_BULLET_FRAME_START: usize = 16;
pub(crate) const BURST_PROJECTILE_FRAMES: usize = 4;

/// The four-frame loop one Burst piece draws from the bullet sheet. Inactive (green, not yet wall-armed)
/// pieces use the dedicated `16..=19` loop; armed pieces use the sheet's last four frames, exactly as
/// `Graphics::CreateBulletAnimations` assigns `anim_burst_inactive`/`anim_burst_active`. Shared by the
/// local and remote projectile renderers so they cannot drift apart.
pub(crate) const fn burst_projectile_sprite_frames(
    asset_frame_count: usize,
    active: bool,
) -> (usize, usize) {
    let last = asset_frame_count.saturating_sub(BURST_PROJECTILE_FRAMES);
    if active {
        (last, BURST_PROJECTILE_FRAMES)
    } else {
        // `usize::min` is not const-stable on this toolchain, so compare explicitly.
        let inactive = if BURST_INACTIVE_BULLET_FRAME_START < last {
            BURST_INACTIVE_BULLET_FRAME_START
        } else {
            last
        };
        (inactive, BURST_PROJECTILE_FRAMES)
    }
}

#[cfg(test)]
mod burst_sprite_tests {
    use super::{BURST_INACTIVE_BULLET_FRAME_START, burst_projectile_sprite_frames};

    #[test]
    fn burst_pieces_use_the_bullet_sheets_own_four_frame_loops() {
        // bullets.bm2 is 40 frames: 0..=15 level bullets, 16..=19 inactive burst, 20..=35 bouncing,
        // 36..=39 active burst. The inactive and active loops must not overlap a level or bouncing frame.
        let (inactive, inactive_len) = burst_projectile_sprite_frames(40, false);
        assert_eq!(
            (inactive, inactive_len),
            (BURST_INACTIVE_BULLET_FRAME_START, 4)
        );
        let (active, active_len) = burst_projectile_sprite_frames(40, true);
        assert_eq!((active, active_len), (36, 4));
        assert!(
            inactive + inactive_len <= 20,
            "inactive burst must not reach the bouncing frames"
        );
        assert!(
            inactive >= BURST_INACTIVE_BULLET_FRAME_START,
            "inactive burst must clear the 16 level-bullet frames"
        );
        assert!(
            active >= 20,
            "active burst must not overlap the level or bouncing frames"
        );
    }

    #[test]
    fn burst_sprite_frames_stay_in_range_for_a_short_sheet() {
        // A malformed/short sheet must still resolve to in-range frames rather than panicking.
        for frame_count in [0usize, 4, 8, 19, 20] {
            let (start, len) = burst_projectile_sprite_frames(frame_count, false);
            assert!(start + len <= frame_count.max(4));
            let (start, len) = burst_projectile_sprite_frames(frame_count, true);
            assert!(start + len <= frame_count.max(4));
        }
    }
}
#[cfg(test)]
pub(crate) const MULTIFIRE_BULLET_COUNT: usize = 3;
pub(crate) const MULTIFIRE_SPREAD: f32 = 0.055;
pub(crate) const MULTIFIRE_ENERGY_COST: f32 = 45.0;
pub(crate) const MULTIFIRE_COOLDOWN: f32 = 0.12;
pub(crate) const BURST_BULLET_COUNT: usize = 12;
pub(crate) const BURST_BULLET_SPEED: f32 = 620.0;
pub(crate) const BURST_ENERGY_COST: f32 = 180.0;
pub(crate) const BURST_COOLDOWN: f32 = 1.6;
pub(crate) const BURST_EFFECT_LIFETIME: f32 = 0.5;
/// The reference game's `Graphics::CreateBulletAnimations` gives both the inactive and
/// wall-activated four-frame Burst loops a 150 ms period, on the bullet sheet.
/// The EMP explosion (`empburst.bm2`) is a separate ten-frame 500 ms sheet used
/// for EMP bombs, so projectile rendering must not borrow its frames or period.
pub(crate) const BURST_PROJECTILE_ANIMATION_DURATION: f32 = 0.15;
pub(crate) const EMP_SPARK_LIFETIME: f32 = 0.5;
pub(crate) const BOMB_LIFETIME: f32 = 2.4;
pub(crate) const BOMB_TRAIL_LIFETIME: f32 = 0.35;
pub(crate) const BOMB_TRAIL_INTERVAL: f32 = 0.05;
pub(crate) const BOMB_FLASH_LIFETIME: f32 = 0.12;
pub(crate) const EMP_EXPLOSION_LIFETIME: f32 = 0.5;
pub(crate) const BOMB_BOUNCES: u8 = 1;
pub(crate) const BOMB_FRAMES_PER_LEVEL: usize = 10;
pub(crate) const EMP_BOMB_FRAME_START: usize = 40;
pub(crate) const BOUNCEABLE_BOMB_FRAME_START: usize = 80;
// L1 prox radius for ProximityDistance=3, matching the reference game's
// `prox = ProximityDistance + raw_level` (raw bits, 0..3): (3 + 0) * 18 = 54.
pub(crate) const PROXIMITY_BOMB_TRIGGER_RADIUS: f32 = 54.0;
pub(crate) use crate::continuum_weapon::CONTINUUM_PROXIMITY_BOMB_TRIGGER_SCALE as PROXIMITY_BOMB_TRIGGER_SCALE;
pub(crate) const MINE_FRAMES_PER_LEVEL: usize = 10;
pub(crate) const EMP_MINE_FRAME_START: usize = 40;
pub(crate) const MINE_LIFETIME: f32 = 45.0;
pub(crate) const MAX_MINES: usize = 8;
pub(crate) const SHRAPNEL_ANIM_DURATION: f32 = 0.6;
pub(crate) const SHRAPNEL_INACTIVE_DAMAGE_TIME: f32 = 0.25;
pub(crate) const MAX_SHRAPNEL_UPGRADES: u8 = 4;
pub(crate) const SHRAPNEL_SPEED: f32 = 430.0;
pub(crate) const REPEL_LIFETIME: f32 = 0.5;
pub(crate) const REPEL_REUSE_COOLDOWN: f32 = 0.5;
pub(crate) const REPEL_RADIUS: f32 = 160.0;
pub(crate) const REPEL_SPEED: f32 = 520.0;
pub(crate) const WORMHOLE_TILE_ID: u8 = 220;
pub(crate) const WARP_LIFETIME: f32 = 0.5;
pub(crate) const WORMHOLE_COOLDOWN: f32 = 1.0;
pub(crate) const PORTAL_LIFETIME: f32 = 20.0;
pub(crate) const PORTAL_ANIM_DURATION: f32 = 1.0;
pub(crate) const ENERGY_MAX: f32 = 1500.0;
pub(crate) const ENERGY_RECHARGE: f32 = 120.0;
pub(crate) const STAT_UPGRADE_CAP: u8 = 5;
pub(crate) const ROTATION_UPGRADE_STEP: f32 = 0.12;
pub(crate) const THRUST_UPGRADE_STEP: f32 = 42.0;
pub(crate) const TOP_SPEED_UPGRADE_STEP: f32 = 24.0;
pub(crate) const RECHARGE_UPGRADE_STEP: f32 = 18.0;
pub(crate) const AFTERBURNER_ENERGY_PER_SECOND: f32 = 180.0;
pub(crate) const THRUST_ACCELERATION: f32 = 360.0;
pub(crate) const AFTERBURNER_THRUST_ACCELERATION: f32 = 620.0;
pub(crate) const SHIP_TOP_SPEED: f32 = 320.0;
pub(crate) const AFTERBURNER_TOP_SPEED: f32 = 520.0;
pub(crate) const ROCKET_LIFETIME: f32 = 2.5;
pub(crate) const ROCKET_COOLDOWN: f32 = 0.0;
pub(crate) const ROCKET_THRUST_ACCELERATION: f32 = 920.0;
pub(crate) const ROCKET_TOP_SPEED: f32 = 700.0;
pub(crate) const ROCKET_PARTICLE_LIFETIME: f32 = 0.3;
pub(crate) const ANTIWARP_RADIUS: f32 = 640.0;
pub(crate) const FLAG_PICKUP_RADIUS: f32 = 18.0;
pub(crate) const FLAG_RESPAWN_DELAY: f32 = 1.5;
pub(crate) const DROPPED_FLAG_LIFETIME: f32 = 20.0;
pub(crate) const POWERBALL_PICKUP_RADIUS: f32 = 22.0;
pub(crate) const POWERBALL_KICK_SPEED: f32 = 520.0;
#[cfg(test)]
pub(crate) const BOT_POWERBALL_KICK_SPEED: f32 = 440.0;
pub(crate) const POWERBALL_FRICTION: f32 = 0.985;
pub(crate) const POWERBALL_TRAIL_INTERVAL: f32 = 0.06;
pub(crate) const POWERBALL_TRAIL_LIFETIME: f32 = 0.3;
pub(crate) const POWERBALL_FRAMES: usize = 10;
pub(crate) const POWERBALL_PHASED_FRAME_START: usize = 10;
pub(crate) const POWERBALL_TRAIL_FRAME_START: usize = 20;
pub(crate) const POWERBALL_WEAPON_DELAY: f32 = 0.5;
pub(crate) const FLAG_FRAMES_PER_TEAM: usize = 10;
pub(crate) const FLAG_TEAM_FRAME_START: usize = 10;
pub(crate) const GOAL_FRAMES_PER_TEAM: usize = 9;
pub(crate) const GOAL_SCORING_FRAME_START: usize = 9;
pub(crate) const EXPLOSION_LIFETIME: f32 = 1.25;
pub(crate) const SHIP_EXPLOSION_LIFETIME: f32 = 0.8;
pub(crate) const EXHAUST_LIFETIME: f32 = 0.3;
pub(crate) const EXHAUST_INTERVAL: f32 = 0.055;
pub(crate) const PRIZE_LIFETIME: f32 = 30.0;
pub(crate) const PRIZE_SPAWN_INTERVAL: f32 = 2.0;
pub(crate) const MAX_PRIZES: usize = 12;
pub(crate) const CONTINUUM_MAX_GREEN_COUNT: usize = 256;
pub(crate) const PRIZE_PROTOCOL_COUNT: u16 = 29;
pub(crate) const MULTIPRIZE_COUNT: usize = 3;
pub(crate) const SHIP_RADIUS: f32 = 14.0;
pub(crate) const BRICK_TILE_ID: u8 = 250;
pub(crate) const BRICK_LIFETIME: f32 = 12.0;
pub(crate) const DECOY_LIFETIME: f32 = 12.0;
pub(crate) const THOR_FRAME_START: usize = 120;
pub(crate) const THOR_FRAMES: usize = 10;
pub(crate) const SUPER_LIFETIME: f32 = 12.0;
pub(crate) const SHIELD_LIFETIME: f32 = 12.0;
pub(crate) const ENGINE_SHUTDOWN_LIFETIME: f32 = 4.0;
pub(crate) const ENGINE_SHUTDOWN_ROTATION_PER_SECOND: f32 = 0.1;
// Keep the 8x12 bitmap glyphs at their native size; fractional downscaling
// with nearest filtering drops strokes and shimmers as a label moves.
pub(crate) const SHIP_LABEL_FONT_SIZE: u16 = 12;
pub(crate) const INVALID_PLAYER_ID: u16 = 0xffff;
pub(crate) const PLAYER_BANNER_BYTES: usize = 12 * 8;
pub(crate) const DEFAULT_BANNER_PALETTE: [u32; 256] = [
    0x000000, 0x393939, 0x424242, 0x525252, 0x5a5a5a, 0x737373, 0x7b7b7b, 0x848484, 0x8c8c8c,
    0x949494, 0x9c9c9c, 0xa5a5a5, 0xadadad, 0xc6c6c6, 0xcecece, 0xbdb5b5, 0xb5adad, 0x9c9494,
    0x7b7373, 0x6b6363, 0xffdede, 0xefc6c6, 0x8c7373, 0xffcece, 0xefb5b5, 0x634a4a, 0xb58484,
    0xffb5b5, 0xe78484, 0x7b4242, 0x211010, 0xff7b7b, 0x842929, 0x521818, 0xce0808, 0xce2921,
    0xf76352, 0xce5a4a, 0x94635a, 0xa54231, 0xde3108, 0xff3908, 0x731800, 0x4d2d04, 0xa52900,
    0x7b635a, 0xff8452, 0xce6339, 0xbd4208, 0x632100, 0xffb584, 0xef6b18, 0x733910, 0xde8439,
    0x635a52, 0x948c84, 0xce8c4a, 0x845a29, 0xffa542, 0xf78c18, 0xffd6a5, 0xad8c63, 0x845208,
    0xad7310, 0xa56b08, 0xffe7bd, 0xb57b10, 0x946308, 0xf7dead, 0x9c8c6b, 0xd6b573, 0xa58442,
    0xf7b529, 0xefad21, 0x736b5a, 0x4a4231, 0xefce84, 0xffce63, 0xdead42, 0xffbd29, 0xb59442,
    0xefbd42, 0xffc631, 0x5a4208, 0xce9408, 0xffde84, 0xe7b521, 0xfff7de, 0xefe7ce, 0xb5ad94,
    0xb5a573, 0xffe79c, 0xffd65a, 0xa59c7b, 0x847b5a, 0xfff7d6, 0xffefad, 0x9c8418, 0xe7c629,
    0xad9c21, 0xffe721, 0xadada5, 0x9c9c8c, 0x4a4a42, 0xadad94, 0x313129, 0xa5a584, 0x5a5a18,
    0xcece31, 0xffff39, 0x7b8421, 0xd6e739, 0xa5b531, 0xadc639, 0x738439, 0x94b542, 0xb5ce7b,
    0x739431, 0x526b29, 0xbdf763, 0x7ba542, 0xa5de52, 0x7bb542, 0xe7f7de, 0x7b8c73, 0xa5bd9c,
    0x9cf77b, 0xc6e7bd, 0x7bb56b, 0x529442, 0x398429, 0x183910, 0x6bde5a, 0x295a21, 0x73ff63,
    0x5ace4a, 0x429439, 0x6bce63, 0x4aad42, 0xadb5ad, 0xb5ceb5, 0x394239, 0x213121, 0xa5f7a5,
    0x397339, 0x215221, 0x296b29, 0x399439, 0x185218, 0x084a31, 0x104239, 0x313952, 0x31398c,
    0x6b73ce, 0x4a52b5, 0xefefff, 0xe7e7ff, 0xbdbdd6, 0x6b6b7b, 0xcecef7, 0xc6c6f7, 0x42425a,
    0xa5a5e7, 0xb5b5ff, 0x4a4a6b, 0x9494d6, 0x9c9ce7, 0x7373ad, 0x6b6ba5, 0xa5a5ff, 0x8484ce,
    0x63639c, 0x7373b5, 0x525284, 0x7b7bc6, 0x42426b, 0x5a5a94, 0x4a4a7b, 0x6363a5, 0x393963,
    0x29294a, 0x42427b, 0x313163, 0x393973, 0x6363ce, 0x6363ff, 0x5252d6, 0x4242bd, 0x212163,
    0x5252ff, 0x3939b5, 0x4242de, 0x212173, 0x31297b, 0x635a94, 0x8c84ad, 0x635a8c, 0x4a29a5,
    0x4210c6, 0x290094, 0x4a2994, 0xb59ce7, 0x211831, 0x63429c, 0x3f2d0e, 0x210052, 0x31007b,
    0x8463ad, 0x8442d6, 0x6321b5, 0x390084, 0xb584ef, 0xad6bf7, 0x7342ad, 0x844ac6, 0x52297b,
    0x4a2173, 0x310063, 0x4a0094, 0x5200a5, 0x4a395a, 0x634a7b, 0xce94ff, 0x9c6bc6, 0x7b42ad,
    0x9c52de, 0x4a0884, 0x210039, 0xad63c6, 0x4a0063, 0x9c63a5, 0x630073, 0x7b737b, 0x0a190a,
    0x4a004a, 0x520052, 0x7b0073, 0x7b4a73, 0x4a3142, 0x632942, 0x846b73, 0x94737b, 0xff7394,
    0x633942, 0x946b73, 0x9c394a, 0xff8c9c, 0xbd6b73, 0xef6373, 0xce2939, 0xff394a, 0x7b1821,
    0xf7737b, 0xd63942, 0xa52129, 0xffffff,
];
pub(crate) const BOT_COUNT: usize = 4;
#[cfg(test)]
pub(crate) const BOT_ENERGY_MAX: f32 = 420.0;
pub(crate) const BOT_THRUST_ACCELERATION: f32 = 230.0;
pub(crate) const BOT_TOP_SPEED: f32 = 230.0;
pub(crate) const BOT_ROTATION_PER_SECOND: f32 = 1.15;
pub(crate) const BOT_FIRE_COOLDOWN: f32 = 1.25;
#[cfg(test)]
pub(crate) const BOT_BULLET_LIFETIME: f32 = 1.15;
#[cfg(test)]
pub(crate) const BOT_BULLET_DAMAGE: f32 = 95.0;
pub(crate) const BOT_RESPAWN_DELAY: f32 = 2.0;
pub(crate) const BOT_BULLET_RADIUS: f32 = 4.0;
pub(crate) const BOT_DECOY_TARGET_RADIUS: f32 = 720.0;
pub(crate) const BOT_PRIZE_SEEK_RADIUS: f32 = 900.0;
pub(crate) const BOT_GUN_VOLUME: f32 = 0.16;
pub(crate) const BOUNTY_INCREASE_FOR_KILL: u32 = 1;
pub(crate) const BULLET_DAMAGE: f32 = 90.0;
pub(crate) const BOMB_DAMAGE: f32 = 260.0;
pub(crate) const SHRAPNEL_DAMAGE: f32 = 70.0;
pub(crate) const EXPLOSION_DAMAGE_RADIUS: f32 = 72.0;
pub(crate) const LEVEL_PATHS: &[&str] = include!(concat!(env!("OUT_DIR"), "/levels.rs"));
pub(crate) const LVZ_PATHS: &[&str] = include!(concat!(env!("OUT_DIR"), "/lvz_archives.rs"));
pub(crate) const ZONE_SETTINGS_PATHS: &[&str] =
    include!(concat!(env!("OUT_DIR"), "/zone_settings.rs"));

#[derive(Clone, Copy)]
pub(crate) struct ZoneAssetPaths {
    pub(crate) id: &'static str,
    pub(crate) levels: &'static [&'static str],
    pub(crate) lvz: &'static [&'static str],
    pub(crate) settings: &'static [&'static str],
}
pub(crate) const ZONE_ASSET_PATHS: &[ZoneAssetPaths] =
    include!(concat!(env!("OUT_DIR"), "/zone_assets.rs"));

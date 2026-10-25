use super::*;

#[test]
fn decodes_original_ship_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/ships.bm2"),
        Transparency::Black,
    )
    .expect("ships.bm2 should decode");

    assert_eq!(decoded.width, 360);
    assert_eq!(decoded.height, 1152);
    assert_eq!(decoded.rgba.len(), 360 * 1152 * 4);
}

#[test]
fn decodes_original_turret_sheet_dimensions() {
    let large = decode_bm2(
        include_bytes!("../../assets/graphics/turret.bm2"),
        Transparency::Black,
    )
    .expect("turret.bm2 should decode");
    let small = decode_bm2(
        include_bytes!("../../assets/graphics/turret2.bm2"),
        Transparency::Black,
    )
    .expect("turret2.bm2 should decode");

    assert_eq!(large.width, 320);
    assert_eq!(large.height, 200);
    assert_eq!(small.width, 320);
    assert_eq!(small.height, 32);
}

#[test]
fn decodes_original_tile_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/tiles.bm2"),
        Transparency::Black,
    )
    .expect("tiles.bm2 should decode");

    assert_eq!(decoded.width, 304);
    assert_eq!(decoded.height, 160);
}

#[test]
fn decodes_original_background_sheet_dimensions() {
    let sheets = [
        (
            include_bytes!("../../assets/graphics/bg01.bm2").as_slice(),
            64,
            64,
        ),
        (
            include_bytes!("../../assets/graphics/bg02.bm2").as_slice(),
            64,
            64,
        ),
        (
            include_bytes!("../../assets/graphics/bg03.bm2").as_slice(),
            64,
            64,
        ),
        (
            include_bytes!("../../assets/graphics/bg04.bm2").as_slice(),
            120,
            56,
        ),
        (
            include_bytes!("../../assets/graphics/bg05.bm2").as_slice(),
            64,
            64,
        ),
        (
            include_bytes!("../../assets/graphics/bg06.bm2").as_slice(),
            96,
            96,
        ),
        (
            include_bytes!("../../assets/graphics/bg07.bm2").as_slice(),
            96,
            104,
        ),
        (
            include_bytes!("../../assets/graphics/bg08.bm2").as_slice(),
            40,
            40,
        ),
        (
            include_bytes!("../../assets/graphics/bg09.bm2").as_slice(),
            110,
            88,
        ),
        (
            include_bytes!("../../assets/graphics/bg10.bm2").as_slice(),
            115,
            92,
        ),
        (
            include_bytes!("../../assets/graphics/bg11.bm2").as_slice(),
            152,
            152,
        ),
        (
            include_bytes!("../../assets/graphics/bg12.bm2").as_slice(),
            112,
            96,
        ),
        (
            include_bytes!("../../assets/graphics/bg13.bm2").as_slice(),
            78,
            81,
        ),
        (
            include_bytes!("../../assets/graphics/bg14.bm2").as_slice(),
            120,
            104,
        ),
    ];

    for (data, width, height) in sheets {
        let decoded = decode_bm2(data, Transparency::Opaque).expect("bg bm2 should decode");
        assert_eq!(decoded.width, width);
        assert_eq!(decoded.height, height);
    }
}

#[test]
fn decodes_original_bullet_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/bullets.bm2"),
        Transparency::Black,
    )
    .expect("bullets.bm2 should decode");

    assert_eq!(decoded.width, 20);
    assert_eq!(decoded.height, 50);
    let frame_count = (decoded.width / 5 * decoded.height / 5) as usize;
    assert!(BOUNCING_BULLET_FRAME_START + BOUNCING_BULLET_FRAMES <= frame_count);
    assert!(
        BOUNCING_BULLET_FRAME_START + WEAPON_LEVEL_MAX as usize * BULLET_FRAMES_PER_LEVEL
            <= frame_count
    );
}

#[test]
fn decodes_original_bullet_trail_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/gradient.bm2"),
        Transparency::Black,
    )
    .expect("gradient.bm2 should decode");

    assert_eq!(decoded.width, 14);
    assert_eq!(decoded.height, 8);
}

#[test]
fn decodes_original_exhaust_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/exhaust.bm2"),
        Transparency::Black,
    )
    .expect("exhaust.bm2 should decode");

    assert_eq!(decoded.width, 304);
    assert_eq!(decoded.height, 32);
}

#[test]
fn decodes_original_bullet_impact_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/explode0.bm2"),
        Transparency::Black,
    )
    .expect("explode0.bm2 should decode");

    assert_eq!(decoded.width, 112);
    assert_eq!(decoded.height, 16);
}

#[test]
fn decodes_original_ship_explosion_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/explode1.bm2"),
        Transparency::Black,
    )
    .expect("explode1.bm2 should decode");

    assert_eq!(decoded.width, 288);
    assert_eq!(decoded.height, 288);
}

#[test]
fn decodes_original_flag_and_goal_sheet_dimensions() {
    let flag = decode_bm2(
        include_bytes!("../../assets/graphics/flag.bm2"),
        Transparency::Black,
    )
    .expect("flag.bm2 should decode");
    let goal = decode_bm2(
        include_bytes!("../../assets/graphics/goal.bm2"),
        Transparency::Black,
    )
    .expect("goal.bm2 should decode");
    let dropped = decode_bm2(
        include_bytes!("../../assets/graphics/dropflag.bm2"),
        Transparency::Black,
    )
    .expect("dropflag.bm2 should decode");

    assert_eq!(flag.width, 160);
    assert_eq!(flag.height, 32);
    let flag_frame_count = (flag.width / 16 * flag.height / 16) as usize;
    assert_eq!(flag_frame_count, FLAG_FRAMES_PER_TEAM * 2);
    assert_eq!(FLAG_TEAM_FRAME_START, FLAG_FRAMES_PER_TEAM);
    assert_eq!(goal.width, 144);
    assert_eq!(goal.height, 32);
    let goal_frame_count = (goal.width / 16 * goal.height / 16) as usize;
    assert_eq!(goal_frame_count, GOAL_FRAMES_PER_TEAM * 2);
    assert_eq!(GOAL_SCORING_FRAME_START, GOAL_FRAMES_PER_TEAM);
    assert_eq!(dropped.width, 160);
    assert_eq!(dropped.height, 16);
}

#[test]
fn goal_radar_colors_match_reference_game_reference() {
    assert_eq!(
        radar_goal_tile_color(true),
        Color::from_rgba(33, 156, 173, 255)
    );
    assert_eq!(
        radar_goal_tile_color(false),
        Color::from_rgba(8, 57, 255, 255)
    );
    assert_eq!(
        radar_tile_color(GOAL_TILE_ID),
        Some(radar_goal_tile_color(false))
    );
}

#[test]
fn decodes_original_powerball_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/powerb.bm2"),
        Transparency::Black,
    )
    .expect("powerb.bm2 should decode");

    assert_eq!(decoded.width, 160);
    assert_eq!(decoded.height, 48);
    let frame_count = (decoded.width / 16 * decoded.height / 16) as usize;
    assert_eq!(frame_count, POWERBALL_FRAMES * 3);
    assert_eq!(POWERBALL_PHASED_FRAME_START, POWERBALL_FRAMES);
    assert_eq!(POWERBALL_TRAIL_FRAME_START, POWERBALL_FRAMES * 2);
}

#[test]
fn decodes_original_bomb_trail_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/trail.bm2"),
        Transparency::Black,
    )
    .expect("trail.bm2 should decode");

    assert_eq!(decoded.width, 160);
    assert_eq!(decoded.height, 80);
}

#[test]
fn decodes_original_bomb_flash_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/bombflsh.bm2"),
        Transparency::Black,
    )
    .expect("bombflsh.bm2 should decode");

    assert_eq!(decoded.width, 48);
    assert_eq!(decoded.height, 8);
}

#[test]
fn decodes_original_bomb_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/bombs.bm2"),
        Transparency::Black,
    )
    .expect("bombs.bm2 should decode");

    assert_eq!(decoded.width, 160);
    assert_eq!(decoded.height, 208);
    assert_eq!(EMP_BOMB_FRAME_START, 40);
    assert!(
        BOUNCEABLE_BOMB_FRAME_START + WEAPON_LEVEL_MAX as usize * BOMB_FRAMES_PER_LEVEL
            <= decoded.width as usize / 16 * decoded.height as usize / 16
    );
    assert!(
        THOR_FRAME_START + THOR_FRAMES
            <= decoded.width as usize / 16 * decoded.height as usize / 16
    );
}

#[test]
fn decodes_original_mine_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/mines.bm2"),
        Transparency::Black,
    )
    .expect("mines.bm2 should decode");

    assert_eq!(decoded.width, 160);
    assert_eq!(decoded.height, 128);
    assert_eq!(EMP_MINE_FRAME_START, 40);
    assert!(
        EMP_MINE_FRAME_START + WEAPON_LEVEL_MAX as usize * MINE_FRAMES_PER_LEVEL
            <= decoded.width as usize / 16 * decoded.height as usize / 16
    );
}

#[test]
fn decodes_original_shrapnel_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/shrapnel.bm2"),
        Transparency::Black,
    )
    .expect("shrapnel.bm2 should decode");

    assert_eq!(decoded.width, 80);
    assert_eq!(decoded.height, 48);
}

#[test]
fn decodes_original_star_sheet_dimensions() {
    let sheets = [
        (
            include_bytes!("../../assets/graphics/star01.bm2").as_slice(),
            21,
            21,
        ),
        (
            include_bytes!("../../assets/graphics/star02.bm2").as_slice(),
            9,
            9,
        ),
        (
            include_bytes!("../../assets/graphics/star03.bm2").as_slice(),
            7,
            7,
        ),
        (
            include_bytes!("../../assets/graphics/star04.bm2").as_slice(),
            9,
            9,
        ),
        (
            include_bytes!("../../assets/graphics/star05.bm2").as_slice(),
            7,
            7,
        ),
        (
            include_bytes!("../../assets/graphics/star06.bm2").as_slice(),
            14,
            14,
        ),
        (
            include_bytes!("../../assets/graphics/star07.bm2").as_slice(),
            20,
            20,
        ),
    ];

    for (data, width, height) in sheets {
        let decoded = decode_bm2(data, Transparency::Black).expect("star bm2 should decode");
        assert_eq!(decoded.width, width);
        assert_eq!(decoded.height, height);
    }
}

#[test]
fn decodes_original_map_object_sheet_dimensions() {
    let sheets = [
        (
            include_bytes!("../../assets/graphics/over1.bm2").as_slice(),
            240,
            32,
        ),
        (
            include_bytes!("../../assets/graphics/over2.bm2").as_slice(),
            320,
            96,
        ),
        (
            include_bytes!("../../assets/graphics/over3.bm2").as_slice(),
            240,
            32,
        ),
        (
            include_bytes!("../../assets/graphics/over4.bm2").as_slice(),
            480,
            192,
        ),
        (
            include_bytes!("../../assets/graphics/over5.bm2").as_slice(),
            320,
            480,
        ),
    ];

    for (data, width, height) in sheets {
        let decoded = decode_bm2(data, Transparency::Black).expect("over bm2 should decode");
        assert_eq!(decoded.width, width);
        assert_eq!(decoded.height, height);
    }
}

#[test]
fn decodes_original_radar_frame_sheet_dimensions() {
    let horizontal = decode_bm2(
        include_bytes!("../../assets/graphics/radarh.bm2"),
        Transparency::Black,
    )
    .expect("radarh.bm2 should decode");
    let vertical = decode_bm2(
        include_bytes!("../../assets/graphics/radarv.bm2"),
        Transparency::Black,
    )
    .expect("radarv.bm2 should decode");

    assert_eq!(horizontal.width, 75);
    assert_eq!(horizontal.height, 7);
    assert_eq!(vertical.width, 7);
    assert_eq!(vertical.height, 62);
}

#[test]
fn decodes_original_ui_effect_sheet_dimensions() {
    let spark = decode_bm2(
        include_bytes!("../../assets/graphics/spark.bm2"),
        Transparency::Black,
    )
    .expect("spark.bm2 should decode");
    let damage = decode_bm2(
        include_bytes!("../../assets/graphics/damage.bm2"),
        Transparency::Black,
    )
    .expect("damage.bm2 should decode");
    let health = decode_bm2(
        include_bytes!("../../assets/graphics/hlthbar.bm2"),
        Transparency::Black,
    )
    .expect("hlthbar.bm2 should decode");
    let spectate = decode_bm2(
        include_bytes!("../../assets/graphics/spectate.bm2"),
        Transparency::Black,
    )
    .expect("spectate.bm2 should decode");
    let icons = decode_bm2(
        include_bytes!("../../assets/graphics/Icons.bm2"),
        Transparency::Black,
    )
    .expect("Icons.bm2 should decode");
    let icon_counts = decode_bm2(
        include_bytes!("../../assets/graphics/icondoor.bm2"),
        Transparency::Black,
    )
    .expect("icondoor.bm2 should decode");
    let tall_font = decode_bm2(
        include_bytes!("../../assets/graphics/tallfont.bm2"),
        Transparency::Black,
    )
    .expect("tallfont.bm2 should decode");
    let energy_font = decode_bm2(
        include_bytes!("../../assets/graphics/engyfont.bm2"),
        Transparency::Black,
    )
    .expect("engyfont.bm2 should decode");

    assert_eq!(spark.width, 400);
    assert_eq!(spark.height, 40);
    assert_eq!(damage.width, 320);
    assert_eq!(damage.height, 32);
    assert_eq!(health.width, 232);
    assert_eq!(health.height, 28);
    assert_eq!(spectate.width, 88);
    assert_eq!(spectate.height, 8);
    assert_eq!(icons.width, 234);
    assert_eq!(icons.height, 144);
    assert_eq!(icons.width / 26, 9);
    assert_eq!(icons.height / 24, 6);
    assert_eq!(icon_counts.width, 132);
    assert_eq!(icon_counts.height, 26);
    assert_eq!(icon_counts.width / 11, 12);
    assert_eq!(icon_counts.height / 13, 2);
    assert_eq!(tall_font.width, 384);
    assert_eq!(tall_font.height, 192);
    assert_eq!(energy_font.width, 160);
    assert_eq!(energy_font.height, 24);
}

#[test]
fn repository_wa2_files_are_pcm_wave_assets() {
    let mut checked = 0;
    let entries = std::fs::read_dir("assets/sound").expect("sound asset directory should exist");

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("wa2") {
            continue;
        }

        let data = std::fs::read(&path).expect("sound asset should be readable");
        let format = wave_format(&data).unwrap_or_else(|| {
            panic!(
                "{} should be a RIFF/WAVE file with fmt and data chunks",
                path.display()
            )
        });

        assert_eq!(format.audio_format, 1, "{} should be PCM", path.display());
        assert_eq!(format.channels, 1, "{} should be mono", path.display());
        assert!(
            [11_025, 22_050].contains(&format.sample_rate),
            "{} should use an original Continuum sample rate",
            path.display()
        );
        assert_eq!(
            format.bits_per_sample,
            8,
            "{} should be 8-bit PCM",
            path.display()
        );
        assert!(
            format.has_data,
            "{} should contain audio data",
            path.display()
        );
        checked += 1;
    }

    assert!(checked >= 70, "expected the original Continuum sound set");
}

#[test]
fn small_radar_view_matches_reference_game_quantized_window() {
    let center = small_radar_view(1024, 1024, vec2(512.0, 512.0) * TILE_SIZE, 800.0, 600.0, 1);
    let zoomed_out = small_radar_view(1024, 1024, vec2(512.0, 512.0) * TILE_SIZE, 800.0, 600.0, 48);

    assert_eq!(center.dim_px, 132.0);
    assert!((center.tile_span.x - 21.12).abs() < 0.001);
    assert!((center.tile_span.y - 21.12).abs() < 0.001);
    assert!((center.min_tile.x - 501.44).abs() < 0.001);
    assert!((center.min_tile.y - 501.44).abs() < 0.001);
    assert!(zoomed_out.tile_span.x > center.tile_span.x);
    assert!((zoomed_out.tile_span.x - 150.0).abs() < 0.001);
    assert!((zoomed_out.tile_span.y - 112.5).abs() < 0.001);
    assert!((zoomed_out.min_tile.x - 437.0).abs() < 0.001);
    assert!((zoomed_out.min_tile.y - 455.75).abs() < 0.001);

    let origin = small_radar_view(1024, 1024, Vec2::ZERO, 800.0, 600.0, 1);
    assert_eq!(origin.min_tile, Vec2::ZERO);

    let edge = small_radar_view(
        1024,
        1024,
        vec2(1024.0, 1024.0) * TILE_SIZE,
        800.0,
        600.0,
        1,
    );
    assert!((edge.max_tile().x - 1024.0).abs() < 0.001);
    assert!((edge.max_tile().y - 1024.0).abs() < 0.001);
}

#[test]
fn small_radar_view_projects_world_positions() {
    let view = small_radar_view(1024, 1024, vec2(512.0, 512.0) * TILE_SIZE, 800.0, 600.0, 1);
    let origin = vec2(668.0, 12.0);
    let center = view.project_world(vec2(512.0, 512.0) * TILE_SIZE, origin.x, origin.y);
    let top_left = view.project_tile(view.min_tile.x, view.min_tile.y, origin.x, origin.y);
    let bottom_right = view.project_tile(view.max_tile().x, view.max_tile().y, origin.x, origin.y);

    assert!((center.x - (origin.x + 66.0)).abs() < 0.001);
    assert!((center.y - (origin.y + 66.0)).abs() < 0.001);
    assert!((top_left.x - origin.x).abs() < 0.001);
    assert!((top_left.y - origin.y).abs() < 0.001);
    assert!((bottom_right.x - (origin.x + view.dim_px)).abs() < 0.001);
    assert!((bottom_right.y - (origin.y + view.dim_px)).abs() < 0.001);
    assert!(view.contains_tile(512, 512));
    assert!(!view.contains_tile(200, 200));
}

#[test]
fn small_radar_origin_matches_reference_game_bottom_right_anchor() {
    let origin = small_radar_origin(800.0, 600.0, 132.0);

    assert_eq!(origin, vec2(662.0, 462.0));
}

#[test]
fn decodes_original_wall_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/wall.bm2"),
        Transparency::Black,
    )
    .expect("wall.bm2 should decode");

    assert_eq!(decoded.width, 160);
    assert_eq!(decoded.height, 32);
}

#[test]
fn decodes_original_repel_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/repel.bm2"),
        Transparency::Black,
    )
    .expect("repel.bm2 should decode");

    assert_eq!(decoded.width, 480);
    assert_eq!(decoded.height, 192);
}

#[test]
fn decodes_original_emp_burst_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/empburst.bm2"),
        Transparency::Black,
    )
    .expect("empburst.bm2 should decode");

    assert_eq!(decoded.width, 400);
    assert_eq!(decoded.height, 160);
}

#[test]
fn decodes_original_rocket_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/rocket.bm2"),
        Transparency::Black,
    )
    .expect("rocket.bm2 should decode");

    assert_eq!(decoded.width, 312);
    assert_eq!(decoded.height, 48);
}

#[test]
fn decodes_original_warp_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/warp.bm2"),
        Transparency::Black,
    )
    .expect("warp.bm2 should decode");

    assert_eq!(decoded.width, 288);
    assert_eq!(decoded.height, 144);
}

#[test]
fn decodes_original_portal_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/warppnt.bm2"),
        Transparency::Black,
    )
    .expect("warppnt.bm2 should decode");

    assert_eq!(decoded.width, 160);
    assert_eq!(decoded.height, 16);
}

#[test]
fn decodes_original_prize_sheet_dimensions() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/prizes.bm2"),
        Transparency::Black,
    )
    .expect("prizes.bm2 should decode");

    assert_eq!(decoded.width, 160);
    assert_eq!(decoded.height, 16);
}

#[test]
fn decodes_original_super_and_shield_sheet_dimensions() {
    let super_decoded = decode_bm2(
        include_bytes!("../../assets/graphics/super.bm2"),
        Transparency::Black,
    )
    .expect("super.bm2 should decode");
    let shield_decoded = decode_bm2(
        include_bytes!("../../assets/graphics/shield.bm2"),
        Transparency::Black,
    )
    .expect("shield.bm2 should decode");

    assert_eq!(super_decoded.width, 160);
    assert_eq!(super_decoded.height, 16);
    assert_eq!(shield_decoded.width, 160);
    assert_eq!(shield_decoded.height, 16);
}

#[test]
fn local_prize_kinds_use_existing_icon_frames() {
    let kinds = [
        PrizeKind::Energy,
        PrizeKind::FullCharge,
        PrizeKind::Rocket,
        PrizeKind::Repel,
        PrizeKind::Burst,
        PrizeKind::Brick,
        PrizeKind::Decoy,
        PrizeKind::Thor,
        PrizeKind::Portal,
        PrizeKind::Super,
        PrizeKind::Shield,
        PrizeKind::EngineShutdown,
        PrizeKind::BouncingBullets,
        PrizeKind::Stealth,
        PrizeKind::Cloak,
        PrizeKind::XRadar,
        PrizeKind::Warp,
        PrizeKind::Rotation,
        PrizeKind::Thruster,
        PrizeKind::TopSpeed,
        PrizeKind::Recharge,
        PrizeKind::Shrapnel,
        PrizeKind::Guns,
        PrizeKind::Bombs,
        PrizeKind::Multifire,
        PrizeKind::Proximity,
        PrizeKind::Antiwarp,
        PrizeKind::Multiprize,
    ];

    for kind in kinds {
        assert!(kind.frame_index() < 10);
    }
}

#[test]
fn local_prize_kinds_have_pickup_notifications() {
    let kinds = [
        PrizeKind::Energy,
        PrizeKind::FullCharge,
        PrizeKind::Rocket,
        PrizeKind::Repel,
        PrizeKind::Burst,
        PrizeKind::Brick,
        PrizeKind::Decoy,
        PrizeKind::Thor,
        PrizeKind::Portal,
        PrizeKind::Super,
        PrizeKind::Shield,
        PrizeKind::EngineShutdown,
        PrizeKind::BouncingBullets,
        PrizeKind::Stealth,
        PrizeKind::Cloak,
        PrizeKind::XRadar,
        PrizeKind::Warp,
        PrizeKind::Rotation,
        PrizeKind::Thruster,
        PrizeKind::TopSpeed,
        PrizeKind::Recharge,
        PrizeKind::Shrapnel,
        PrizeKind::Guns,
        PrizeKind::Bombs,
        PrizeKind::Multifire,
        PrizeKind::Proximity,
        PrizeKind::Antiwarp,
        PrizeKind::Multiprize,
    ];

    for kind in kinds {
        assert!(!prize_notification(kind, false).is_empty());
    }
}

struct WaveFormat {
    audio_format: u16,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    has_data: bool,
}

fn wave_format(data: &[u8]) -> Option<WaveFormat> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return None;
    }

    let mut offset = 12;
    let mut format = None;
    let mut has_data = false;
    while offset + 8 <= data.len() {
        let chunk_id = &data[offset..offset + 4];
        let chunk_len = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().ok()?) as usize;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start.checked_add(chunk_len)?;
        if chunk_end > data.len() {
            break;
        }

        if chunk_id == b"fmt " {
            if chunk_len < 16 {
                return None;
            }
            format = Some(WaveFormat {
                audio_format: u16::from_le_bytes(
                    data[chunk_start..chunk_start + 2].try_into().ok()?,
                ),
                channels: u16::from_le_bytes(
                    data[chunk_start + 2..chunk_start + 4].try_into().ok()?,
                ),
                sample_rate: u32::from_le_bytes(
                    data[chunk_start + 4..chunk_start + 8].try_into().ok()?,
                ),
                bits_per_sample: u16::from_le_bytes(
                    data[chunk_start + 14..chunk_start + 16].try_into().ok()?,
                ),
                has_data,
            });
        } else if chunk_id == b"data" {
            has_data = chunk_len > 0;
        }

        offset = chunk_end + (chunk_len & 1);
    }

    format.map(|mut format| {
        format.has_data = format.has_data || has_data;
        format
    })
}

use super::*;

pub(super) struct LoadedGraphics {
    pub(super) ships: Texture2D,
    pub(super) turrets: SpriteSheet,
    pub(super) tiles: Texture2D,
    pub(super) backgrounds: [Texture2D; 14],
    pub(super) stars: [Texture2D; 7],
    pub(super) radar_h: Texture2D,
    pub(super) radar_v: Texture2D,
    pub(super) health_bar: Texture2D,
    pub(super) tall_font: BitmapFont,
    pub(super) energy_font: SpriteSheet,
    pub(super) icons: SpriteSheet,
    pub(super) icon_counts: SpriteSheet,
    pub(super) bullets: SpriteSheet,
    pub(super) bullet_trail: SpriteSheet,
    pub(super) bullet_impact: SpriteSheet,
    pub(super) bombs: SpriteSheet,
    pub(super) bomb_flash: SpriteSheet,
    pub(super) bomb_trail: SpriteSheet,
    pub(super) mines: SpriteSheet,
    pub(super) shrapnel: SpriteSheet,
    pub(super) bricks: SpriteSheet,
    pub(super) repel: SpriteSheet,
    pub(super) portal: SpriteSheet,
    pub(super) warp: SpriteSheet,
    pub(super) ship_explosion: SpriteSheet,
    pub(super) bomb_explosion: SpriteSheet,
    pub(super) emp_burst: SpriteSheet,
    pub(super) emp_spark: SpriteSheet,
    pub(super) exhaust: SpriteSheet,
    pub(super) rocket: SpriteSheet,
    pub(super) super_effect: SpriteSheet,
    pub(super) shield_effect: SpriteSheet,
    pub(super) leader_badge: SpriteSheet,
    pub(super) powerball: SpriteSheet,
    pub(super) goal: SpriteSheet,
    pub(super) flag: SpriteSheet,
    pub(super) dropped_flag: SpriteSheet,
    pub(super) asteroid_small1: SpriteSheet,
    pub(super) asteroid_small2: SpriteSheet,
    pub(super) asteroid_large: SpriteSheet,
    pub(super) station: SpriteSheet,
    pub(super) wormhole: SpriteSheet,
    pub(super) prizes: SpriteSheet,
}

pub(super) fn load_original_graphics() -> LoadedGraphics {
    let ships = texture_from_bm2(
        include_bytes!("../../../assets/graphics/ships.bm2"),
        Transparency::Black,
    );
    let turrets = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/turret2.bm2"),
        vec2(16.0, 16.0),
        1.0,
    );
    let tiles = texture_from_bm2(
        include_bytes!("../../../assets/graphics/tiles.bm2"),
        Transparency::Black,
    );
    let backgrounds = [
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg01.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg02.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg03.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg04.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg05.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg06.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg07.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg08.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg09.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg10.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg11.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg12.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg13.bm2"),
            Transparency::Opaque,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/bg14.bm2"),
            Transparency::Opaque,
        ),
    ];
    let stars = [
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/star01.bm2"),
            Transparency::Black,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/star02.bm2"),
            Transparency::Black,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/star03.bm2"),
            Transparency::Black,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/star04.bm2"),
            Transparency::Black,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/star05.bm2"),
            Transparency::Black,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/star06.bm2"),
            Transparency::Black,
        ),
        texture_from_bm2(
            include_bytes!("../../../assets/graphics/star07.bm2"),
            Transparency::Black,
        ),
    ];
    let radar_h = texture_from_bm2(
        include_bytes!("../../../assets/graphics/radarh.bm2"),
        Transparency::Black,
    );
    let radar_v = texture_from_bm2(
        include_bytes!("../../../assets/graphics/radarv.bm2"),
        Transparency::Black,
    );
    let health_bar = texture_from_bm2(
        include_bytes!("../../../assets/graphics/hlthbar.bm2"),
        Transparency::Black,
    );
    let tall_font = BitmapFont::from_bm2(
        include_bytes!("../../../assets/graphics/tallfont.bm2"),
        vec2(8.0, 12.0),
        96,
    );
    let energy_font = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/engyfont.bm2"),
        vec2(16.0, 24.0),
        1.0,
    );
    let icons = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/Icons.bm2"),
        vec2(26.0, 24.0),
        1.0,
    );
    let icon_counts = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/icondoor.bm2"),
        vec2(11.0, 13.0),
        1.0,
    );
    let bullets = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/bullets.bm2"),
        vec2(5.0, 5.0),
        0.15,
    );
    let bullet_trail = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/gradient.bm2"),
        vec2(1.0, 1.0),
        BULLET_TRAIL_LIFETIME,
    );
    let bullet_impact = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/explode0.bm2"),
        vec2(16.0, 16.0),
        BULLET_IMPACT_LIFETIME,
    );
    let bombs = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/bombs.bm2"),
        vec2(16.0, 16.0),
        1.0,
    );
    let bomb_flash = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/bombflsh.bm2"),
        vec2(8.0, 8.0),
        BOMB_FLASH_LIFETIME,
    );
    let bomb_trail = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/trail.bm2"),
        vec2(16.0, 16.0),
        BOMB_TRAIL_LIFETIME,
    );
    let mines = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/mines.bm2"),
        vec2(16.0, 16.0),
        1.0,
    );
    let shrapnel = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/shrapnel.bm2"),
        vec2(8.0, 8.0),
        SHRAPNEL_ANIM_DURATION,
    );
    let bricks = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/wall.bm2"),
        vec2(16.0, 16.0),
        1.0,
    );
    let repel = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/repel.bm2"),
        vec2(96.0, 96.0),
        REPEL_LIFETIME,
    );
    let portal = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/warppnt.bm2"),
        vec2(16.0, 16.0),
        PORTAL_ANIM_DURATION,
    );
    let warp = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/warp.bm2"),
        vec2(48.0, 48.0),
        WARP_LIFETIME,
    );
    let ship_explosion = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/explode1.bm2"),
        vec2(48.0, 48.0),
        SHIP_EXPLOSION_LIFETIME,
    );
    let bomb_explosion = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/explode2.bm2"),
        vec2(80.0, 80.0),
        EXPLOSION_LIFETIME,
    );
    let emp_burst = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/empburst.bm2"),
        vec2(80.0, 80.0),
        BURST_EFFECT_LIFETIME,
    );
    let emp_spark = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/spark.bm2"),
        vec2(40.0, 40.0),
        EMP_SPARK_LIFETIME,
    );
    let exhaust = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/exhaust.bm2"),
        vec2(16.0, 16.0),
        EXHAUST_LIFETIME,
    );
    let rocket = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/rocket.bm2"),
        vec2(24.0, 24.0),
        ROCKET_PARTICLE_LIFETIME,
    );
    let super_effect = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/super.bm2"),
        vec2(16.0, 16.0),
        1.0,
    );
    let shield_effect = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/shield.bm2"),
        vec2(16.0, 16.0),
        1.0,
    );
    let leader_badge = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/ssshield.bm2"),
        vec2(16.0, 16.0),
        1.0,
    );
    let powerball = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/powerb.bm2"),
        vec2(16.0, 16.0),
        1.0,
    );
    let goal = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/goal.bm2"),
        vec2(16.0, 16.0),
        0.5,
    );
    let flag = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/flag.bm2"),
        vec2(16.0, 16.0),
        1.0,
    );
    let dropped_flag = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/dropflag.bm2"),
        vec2(16.0, 16.0),
        1.0,
    );
    let asteroid_small1 = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/over1.bm2"),
        vec2(16.0, 16.0),
        1.5,
    );
    let asteroid_small2 = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/over3.bm2"),
        vec2(16.0, 16.0),
        1.5,
    );
    let asteroid_large = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/over2.bm2"),
        vec2(32.0, 32.0),
        1.5,
    );
    let station = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/over4.bm2"),
        vec2(96.0, 96.0),
        1.0,
    );
    let wormhole = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/over5.bm2"),
        vec2(80.0, 80.0),
        2.5,
    );
    let prizes = SpriteSheet::from_bm2(
        include_bytes!("../../../assets/graphics/prizes.bm2"),
        vec2(16.0, 16.0),
        1.0,
    );

    for texture in [
        &ships,
        &tiles,
        &radar_h,
        &radar_v,
        &health_bar,
        &tall_font.texture,
    ] {
        texture.set_filter(FilterMode::Nearest);
    }
    for texture in &backgrounds {
        texture.set_filter(FilterMode::Nearest);
    }
    for texture in &stars {
        texture.set_filter(FilterMode::Nearest);
    }

    LoadedGraphics {
        ships,
        turrets,
        tiles,
        backgrounds,
        stars,
        radar_h,
        radar_v,
        health_bar,
        tall_font,
        energy_font,
        icons,
        icon_counts,
        bullets,
        bullet_trail,
        bullet_impact,
        bombs,
        bomb_flash,
        bomb_trail,
        mines,
        shrapnel,
        bricks,
        repel,
        portal,
        warp,
        ship_explosion,
        bomb_explosion,
        emp_burst,
        emp_spark,
        exhaust,
        rocket,
        super_effect,
        shield_effect,
        leader_badge,
        powerball,
        goal,
        flag,
        dropped_flag,
        asteroid_small1,
        asteroid_small2,
        asteroid_large,
        station,
        wormhole,
        prizes,
    }
}

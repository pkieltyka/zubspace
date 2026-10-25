use macroquad::audio::{Sound, load_sound_from_bytes};

pub(super) struct LoadedSounds {
    pub(super) thrust: Option<Sound>,
    pub(super) reverse_thrust: Option<Sound>,
    pub(super) rocket_sound: Option<Sound>,
    pub(super) rocket_sustain_sound: Option<Sound>,
    pub(super) gun_fire: [Option<Sound>; 4],
    pub(super) bounce_sound: Option<Sound>,
    pub(super) multifire_on: Option<Sound>,
    pub(super) multifire_off: Option<Sound>,
    pub(super) burst_sound: Option<Sound>,
    pub(super) stealth_sound: Option<Sound>,
    pub(super) cloak_sound: Option<Sound>,
    pub(super) xradar_sound: Option<Sound>,
    pub(super) antiwarp_sound: Option<Sound>,
    pub(super) toggle_off_sound: Option<Sound>,
    pub(super) bullet_impact_sound: Option<Sound>,
    pub(super) prize_sound: Option<Sound>,
    pub(super) energy_alarm_sound: Option<Sound>,
    pub(super) flag_sound: Option<Sound>,
    pub(super) goal_sound: Option<Sound>,
    pub(super) ball_bounce_sound: Option<Sound>,
    pub(super) ball_catch_sound: Option<Sound>,
    pub(super) ball_throw_sound: Option<Sound>,
    pub(super) bomb_fire: [Option<Sound>; 4],
    pub(super) emp_bomb_fire: [Option<Sound>; 4],
    pub(super) mine_fire: [Option<Sound>; 4],
    pub(super) decoy_sound: Option<Sound>,
    pub(super) thor_sound: Option<Sound>,
    pub(super) repel_sound: Option<Sound>,
    pub(super) wall_sound: Option<Sound>,
    pub(super) portal_sound: Option<Sound>,
    pub(super) warp_sound: Option<Sound>,
    pub(super) victory_music: Option<Sound>,
    pub(super) victory_music_long: Option<Sound>,
    pub(super) ship_explode: Option<Sound>,
    pub(super) bomb_explode: Option<Sound>,
    pub(super) emp_bomb_explode: Option<Sound>,
    pub(super) chat_sounds: [Option<Sound>; 26],
}

pub(super) async fn load_original_sounds() -> LoadedSounds {
    LoadedSounds {
        thrust: load_sound_from_bytes(include_bytes!("../../../assets/sound/thrust.wa2"))
            .await
            .ok(),
        reverse_thrust: load_sound_from_bytes(include_bytes!("../../../assets/sound/rev.wa2"))
            .await
            .ok(),
        rocket_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/rocket1.wa2"))
            .await
            .ok(),
        rocket_sustain_sound: load_sound_from_bytes(include_bytes!(
            "../../../assets/sound/rocket2.wa2"
        ))
        .await
        .ok(),
        gun_fire: [
            load_sound_from_bytes(include_bytes!("../../../assets/sound/gun1.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/gun2.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/gun3.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/gun4.wa2"))
                .await
                .ok(),
        ],
        bounce_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/bounce.wa2"))
            .await
            .ok(),
        multifire_on: load_sound_from_bytes(include_bytes!("../../../assets/sound/multion.wa2"))
            .await
            .ok(),
        multifire_off: load_sound_from_bytes(include_bytes!("../../../assets/sound/multioff.wa2"))
            .await
            .ok(),
        burst_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/burst.wa2"))
            .await
            .ok(),
        stealth_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/stealth.wa2"))
            .await
            .ok(),
        cloak_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/cloak.wa2"))
            .await
            .ok(),
        xradar_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/xradar.wa2"))
            .await
            .ok(),
        antiwarp_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/antiwarp.wa2"))
            .await
            .ok(),
        toggle_off_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/off.wa2"))
            .await
            .ok(),
        bullet_impact_sound: load_sound_from_bytes(include_bytes!(
            "../../../assets/sound/explode0.wa2"
        ))
        .await
        .ok(),
        prize_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/prize.wa2"))
            .await
            .ok(),
        energy_alarm_sound: load_sound_from_bytes(include_bytes!(
            "../../../assets/sound/alarm.wa2"
        ))
        .await
        .ok(),
        flag_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/flag.wa2"))
            .await
            .ok(),
        goal_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/goal.wa2"))
            .await
            .ok(),
        ball_bounce_sound: load_sound_from_bytes(include_bytes!(
            "../../../assets/sound/ballbnc.wa2"
        ))
        .await
        .ok(),
        ball_catch_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/catch.wa2"))
            .await
            .ok(),
        ball_throw_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/throw.wa2"))
            .await
            .ok(),
        bomb_fire: [
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bomb1.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bomb2.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bomb3.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bomb4.wa2"))
                .await
                .ok(),
        ],
        emp_bomb_fire: [
            load_sound_from_bytes(include_bytes!("../../../assets/sound/ebomb1.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/ebomb2.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/ebomb3.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/ebomb4.wa2"))
                .await
                .ok(),
        ],
        mine_fire: [
            load_sound_from_bytes(include_bytes!("../../../assets/sound/mine1.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/mine2.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/mine3.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/mine4.wa2"))
                .await
                .ok(),
        ],
        decoy_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/decoy.wa2"))
            .await
            .ok(),
        thor_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/thor.wa2"))
            .await
            .ok(),
        repel_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/repel.wa2"))
            .await
            .ok(),
        wall_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/wall.wa2"))
            .await
            .ok(),
        portal_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/warppnt.wa2"))
            .await
            .ok(),
        warp_sound: load_sound_from_bytes(include_bytes!("../../../assets/sound/warp.wa2"))
            .await
            .ok(),
        victory_music: load_sound_from_bytes(include_bytes!("../../../assets/sound/victory.wa2"))
            .await
            .ok(),
        victory_music_long: load_sound_from_bytes(include_bytes!(
            "../../../assets/sound/victoryl.wa2"
        ))
        .await
        .ok(),
        ship_explode: load_sound_from_bytes(include_bytes!("../../../assets/sound/explode1.wa2"))
            .await
            .ok(),
        bomb_explode: load_sound_from_bytes(include_bytes!("../../../assets/sound/explode2.wa2"))
            .await
            .ok(),
        emp_bomb_explode: load_sound_from_bytes(include_bytes!(
            "../../../assets/sound/ebombex.wa2"
        ))
        .await
        .ok(),
        chat_sounds: [
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong1.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong2.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong3.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong4.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong5.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong6.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong7.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong8.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong9.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong10.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong11.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong12.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong13.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong14.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong15.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong16.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong17.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong18.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong19.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong20.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong21.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong22.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong23.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong24.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong25.wa2"))
                .await
                .ok(),
            load_sound_from_bytes(include_bytes!("../../../assets/sound/bong26.wa2"))
                .await
                .ok(),
        ],
    }
}

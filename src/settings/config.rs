use super::*;

#[allow(dead_code)]
pub(crate) fn apply_subgame_settings_text(
    tuning: &mut ArenaTuning,
    text: &str,
) -> Result<(), String> {
    let mut section = String::new();

    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let line = strip_inline_comment(raw_line).trim();
        if line.is_empty() || line.starts_with("#include") {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            section = normalize_name(name);
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = normalize_name(key);
        let value = value.trim();

        if matches!(section.as_str(), "allships" | "ships") {
            for ship in &mut tuning.ships {
                apply_ship_setting(ship, &key, value, line_number)?;
            }
        } else if let Some(ship_index) = ship_section_index(&section) {
            apply_ship_setting(&mut tuning.ships[ship_index], &key, value, line_number)?;
        } else {
            apply_arena_setting(tuning, &section, &key, value, line_number)?;
        }
    }

    for ship in &mut tuning.ships {
        ship.energy_initial = ship.energy_initial.min(ship.energy_max);
    }
    tuning.prize_weight_total = tuning
        .prize_weights
        .iter()
        .map(|weight| *weight as u16)
        .sum();

    Ok(())
}

#[allow(dead_code)]
pub(crate) fn arena_tuning_from_subgame_settings_text(text: &str) -> Result<ArenaTuning, String> {
    let mut tuning = ArenaTuning::continuum_skirmish_default();
    apply_subgame_settings_text(&mut tuning, text)?;
    Ok(tuning)
}

fn apply_ship_setting(
    ship: &mut ShipTuning,
    key: &str,
    value: &str,
    line_number: usize,
) -> Result<(), String> {
    match key {
        "supertime" => ship.super_lifetime = ticks_to_seconds_u32(parse_u32(value, line_number)?),
        "shieldstime" => {
            ship.shield_lifetime = ticks_to_seconds_u32(parse_u32(value, line_number)?)
        }
        "gravity" => ship.wormhole_gravity = parse_i16(value, line_number)?,
        "gravitytopspeed" => {
            ship.wormhole_top_speed = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        "radius" => ship.radius_pixels = continuum_radius_pixels(parse_u8(value, line_number)?),
        "bulletfireenergy" => ship.bullet_energy = parse_nonnegative_f32(value, line_number)?,
        "multifireenergy" => ship.multifire_energy = parse_nonnegative_f32(value, line_number)?,
        "bombfireenergy" => ship.bomb_energy = parse_nonnegative_f32(value, line_number)?,
        "bombfireenergyupgrade" => {
            ship.bomb_energy_upgrade = parse_nonnegative_f32(value, line_number)?
        }
        "landminefireenergy" => ship.mine_energy = parse_nonnegative_f32(value, line_number)?,
        "landminefireenergyupgrade" => {
            ship.mine_energy_upgrade = parse_nonnegative_f32(value, line_number)?
        }
        "bulletspeed" => ship.bullet_speed = parse_nonnegative_f32(value, line_number)? / 10.0,
        "bombspeed" => {
            let speed = parse_nonnegative_f32(value, line_number)? / 10.0;
            ship.bomb_speed = speed;
            ship.thor_speed = speed;
        }
        "bulletfiredelay" => {
            ship.bullet_cooldown = ticks_to_seconds(parse_u16(value, line_number)?)
        }
        "multifiredelay" => {
            ship.multifire_cooldown = ticks_to_seconds(parse_u16(value, line_number)?)
        }
        "bombfiredelay" => {
            let cooldown = ticks_to_seconds(parse_u16(value, line_number)?);
            ship.bomb_cooldown = cooldown;
            ship.burst_cooldown = cooldown;
            ship.brick_cooldown = cooldown;
            ship.decoy_cooldown = cooldown;
            ship.thor_cooldown = cooldown;
        }
        "multifireangle" => {
            let angle = parse_u16(value, line_number)?;
            ship.multifire_angle = angle;
            ship.multifire_spread = angle as f32 / (111.0 * 360.0)
        }
        "cloakenergy" => {
            ship.cloak_energy_per_second = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        "stealthenergy" => {
            ship.stealth_energy_per_second = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        "antiwarpenergy" => {
            ship.antiwarp_energy_per_second = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        "xradarenergy" => {
            ship.xradar_energy_per_second = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        "maximumrotation" => {
            ship.rotation_max_per_second = parse_nonnegative_f32(value, line_number)? / 400.0
        }
        "maximumthrust" => {
            let thrust = parse_nonnegative_f32(value, line_number)? * 10.0;
            ship.thrust_max_acceleration = thrust;
            ship.afterburner_thrust_acceleration = thrust;
        }
        "maximumspeed" => {
            let speed = parse_nonnegative_f32(value, line_number)? / 10.0;
            ship.top_speed_max = speed;
            ship.afterburner_top_speed = speed;
        }
        "maximumrecharge" => {
            ship.energy_recharge_max = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        "maximumenergy" => ship.energy_max = parse_nonnegative_f32(value, line_number)?,
        "initialrotation" => {
            ship.rotation_per_second = parse_nonnegative_f32(value, line_number)? / 400.0
        }
        "initialthrust" => {
            ship.thrust_acceleration = parse_nonnegative_f32(value, line_number)? * 10.0
        }
        "initialspeed" => ship.top_speed = parse_nonnegative_f32(value, line_number)? / 10.0,
        "initialrecharge" => {
            ship.energy_recharge = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        "initialenergy" => ship.energy_initial = parse_nonnegative_f32(value, line_number)?,
        "upgraderotation" => {
            ship.rotation_upgrade_step = parse_nonnegative_f32(value, line_number)? / 400.0
        }
        "upgradethrust" => {
            ship.thrust_upgrade_step = parse_nonnegative_f32(value, line_number)? * 10.0
        }
        "upgradespeed" => {
            ship.top_speed_upgrade_step = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        "upgraderecharge" => {
            ship.recharge_upgrade_step = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        "upgradeenergy" => ship.energy_upgrade_step = parse_nonnegative_f32(value, line_number)?,
        "afterburnerenergy" => {
            ship.afterburner_energy_per_second = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        "bombthrust" => ship.bomb_recoil_speed = parse_nonnegative_f32(value, line_number)? / 10.0,
        "burstspeed" => ship.burst_speed = parse_nonnegative_f32(value, line_number)? / 10.0,
        "burstshrapnel" => ship.burst_bullet_count = parse_usize(value, line_number)?,
        "turretthrustpenalty" => {
            ship.turret_thrust_penalty = parse_nonnegative_f32(value, line_number)? * 10.0
        }
        "turretspeedpenalty" => {
            ship.turret_speed_penalty = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        "turretlimit" => ship.turret_limit = parse_u8(value, line_number)?,
        "rockettime" => ship.rocket_lifetime = ticks_to_seconds(parse_u16(value, line_number)?),
        "initialbounty" => ship.initial_bounty = parse_u32(value, line_number)?,
        "maxmines" => ship.max_mines = parse_usize(value, line_number)?,
        "repelmax" => ship.max_repels = parse_u32(value, line_number)?,
        "burstmax" => ship.max_bursts = parse_u32(value, line_number)?,
        "decoymax" => ship.max_decoys = parse_u32(value, line_number)?,
        "thormax" => ship.max_thors = parse_u32(value, line_number)?,
        "brickmax" => ship.max_bricks = parse_u32(value, line_number)?,
        "portalmax" => ship.max_portals = parse_u32(value, line_number)?,
        "rocketmax" => ship.max_rockets = parse_u32(value, line_number)?,
        "initialrepel" => ship.initial_repels = parse_u32(value, line_number)?,
        "initialburst" => ship.initial_bursts = parse_u32(value, line_number)?,
        "initialbrick" => ship.initial_bricks = parse_u32(value, line_number)?,
        "initialrocket" => ship.initial_rockets = parse_u32(value, line_number)?,
        "initialthor" => ship.initial_thors = parse_u32(value, line_number)?,
        "initialdecoy" => ship.initial_decoys = parse_u32(value, line_number)?,
        "initialportal" => ship.initial_portals = parse_u32(value, line_number)?,
        "bombbouncecount" => ship.bomb_bounces = parse_u8(value, line_number)?,
        "cloakstatus" => apply_status(
            parse_u8(value, line_number)?,
            &mut ship.cloak_available,
            &mut ship.initial_cloak,
        ),
        "stealthstatus" => apply_status(
            parse_u8(value, line_number)?,
            &mut ship.stealth_available,
            &mut ship.initial_stealth,
        ),
        "xradarstatus" => apply_status(
            parse_u8(value, line_number)?,
            &mut ship.xradar_available,
            &mut ship.initial_xradar,
        ),
        "antiwarpstatus" => apply_status(
            parse_u8(value, line_number)?,
            &mut ship.antiwarp_available,
            &mut ship.initial_antiwarp,
        ),
        "damagefactor" => ship.damage_factor = parse_u16(value, line_number)?,
        "prizesharelimit" => ship.prize_share_limit = parse_u32(value, line_number)?,
        "attachbounty" => ship.attach_bounty = parse_u32(value, line_number)?,
        "shrapnelmax" => ship.shrapnel_max = parse_u8(value, line_number)?,
        "shrapnelrate" => ship.shrapnel_rate = parse_u8(value, line_number)?,
        "maxguns" => {
            ship.max_gun_level = clamp_settings_weapon_level(parse_u8(value, line_number)?)
        }
        "maxbombs" => {
            ship.max_bomb_level = clamp_settings_weapon_level(parse_u8(value, line_number)?)
        }
        "initialguns" => {
            ship.initial_gun_level = clamp_settings_weapon_level(parse_u8(value, line_number)?)
        }
        "initialbombs" => {
            ship.initial_bomb_level = clamp_settings_weapon_level(parse_u8(value, line_number)?)
        }
        "disablefastshooting" => ship.disable_fast_shooting = parse_bool(value, line_number)?,
        "empbomb" => ship.emp_bomb = parse_bool(value, line_number)?,
        "seemines" => ship.see_mines = parse_bool(value, line_number)?,
        "seebomblevel" => ship.see_bomb_level = parse_u8(value, line_number)?,
        "doublebarrel" => ship.double_barrel = parse_bool(value, line_number)?,
        "soccerballfriction" => {
            ship.soccer_ball_friction = continuum_powerball_friction(parse_u16(value, line_number)?)
        }
        "soccerballproximity" => {
            ship.soccer_ball_proximity = parse_nonnegative_f32(value, line_number)?
        }
        "soccerthrowtime" | "soccerballthrowtimer" => {
            ship.soccer_ball_throw_timer = ticks_to_seconds(parse_u16(value, line_number)?)
        }
        "soccerballspeed" => {
            ship.soccer_ball_speed = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        _ => {}
    }

    Ok(())
}

fn apply_arena_setting(
    tuning: &mut ArenaTuning,
    section: &str,
    key: &str,
    value: &str,
    line_number: usize,
) -> Result<(), String> {
    match (section, key) {
        ("bullet", "bulletdamagelevel") => {
            tuning.bullet_damage = parse_nonnegative_f32(value, line_number)? / 1000.0
        }
        ("bullet", "bulletalivetime") => {
            tuning.bullet_lifetime = ticks_to_seconds_i32(parse_i32(value, line_number)?)
        }
        ("bullet", "bulletdamageupgrade") => {
            tuning.bullet_damage_upgrade = parse_nonnegative_f32(value, line_number)? / 1000.0
        }
        ("bomb", "bombdamagelevel") => {
            tuning.bomb_damage = parse_nonnegative_f32(value, line_number)? / 1000.0
        }
        ("bomb", "bombalivetime") => {
            tuning.bomb_lifetime = ticks_to_seconds_i32(parse_i32(value, line_number)?)
        }
        ("bomb", "bombexplodedelay") => {
            tuning.bomb_explode_delay_seconds =
                ticks_to_seconds_i16(parse_i16(value, line_number)?.max(0))
        }
        ("bomb", "bombexplodepixels") => {
            tuning.bomb_explode_pixels = parse_nonnegative_f32(value, line_number)?
        }
        ("bomb", "proximitydistance") => {
            tuning.proximity_distance = parse_nonnegative_f32(value, line_number)?
        }
        ("bomb", "bombsafety") => tuning.bomb_safety = parse_bool(value, line_number)?,
        ("bomb", "ebombshutdowntime") => {
            tuning.emp_bomb_shutdown_seconds =
                ticks_to_seconds_i16(parse_i16(value, line_number)?.max(0))
        }
        ("bomb", "ebombdamagepercent") => {
            tuning.emp_bomb_damage_multiplier = parse_nonnegative_f32(value, line_number)? / 1000.0
        }
        ("bomb", "bbombdamagepercent") => {
            tuning.bouncing_bomb_damage_multiplier =
                parse_nonnegative_f32(value, line_number)? / 1000.0
        }
        ("mine", "minealivetime") => {
            tuning.mine_lifetime = ticks_to_seconds_i32(parse_i32(value, line_number)?)
        }
        ("mine", "teammaxmines") => tuning.team_max_mines = parse_usize(value, line_number)?,
        ("shrapnel", "shrapnelspeed") => {
            tuning.shrapnel_speed = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        ("shrapnel", "inactiveshrapdamage") => {
            tuning.inactive_shrapnel_damage = parse_nonnegative_f32(value, line_number)? / 1000.0
        }
        ("shrapnel", "shrapneldamagepercent") => {
            tuning.shrapnel_damage_multiplier = parse_nonnegative_f32(value, line_number)? / 1000.0
        }
        ("shrapnel", "random") => tuning.shrapnel_random = parse_bool(value, line_number)?,
        ("burst", "burstdamagelevel") => {
            tuning.burst_damage = parse_nonnegative_f32(value, line_number)? / 1000.0
        }
        ("toggle", "antiwarppixels") => {
            tuning.antiwarp_radius = parse_nonnegative_f32(value, line_number)?
        }
        ("prize", "multiprizecount") => tuning.multiprize_count = parse_u16(value, line_number)?,
        ("prize", "prizefactor") => tuning.prize_factor = parse_u16(value, line_number)?,
        ("prize", "prizedelay") => {
            tuning.prize_spawn_interval =
                ticks_to_seconds_i16(parse_i16(value, line_number)?.max(1))
        }
        ("prize", "prizehidecount") => {
            tuning.prize_spawn_count = parse_u8(value, line_number)?.max(1)
        }
        ("prize", "minimumvirtual") => {
            tuning.prize_minimum_virtual = parse_u16(value, line_number)?
        }
        ("prize", "upgradevirtual") => {
            tuning.prize_upgrade_virtual = parse_u16(value, line_number)?
        }
        ("prize", "prizemaxexist") => {
            tuning.prize_lifetime_max = ticks_to_seconds_i16(parse_i16(value, line_number)?)
        }
        ("prize", "prizeminexist") => {
            tuning.prize_lifetime_min = ticks_to_seconds_i16(parse_i16(value, line_number)?)
        }
        ("prize", "prizenegativefactor") => {
            tuning.prize_negative_factor = parse_u16(value, line_number)?
        }
        ("prize", "deathprizetime") => {
            tuning.death_prize_lifetime = ticks_to_seconds_i16(parse_i16(value, line_number)?)
        }
        ("prize", "engineshutdowntime") => {
            for ship in &mut tuning.ships {
                ship.engine_shutdown_lifetime =
                    ticks_to_seconds_i16(parse_i16(value, line_number)?);
            }
        }
        ("prize", "takeprizereliable") | ("prize", "s2ctakeprizereliable") => {
            tuning.take_prize_reliable = parse_bool(value, line_number)?
        }
        ("flag", "flaggeronradar") => tuning.flagger_on_radar = parse_bool(value, line_number)?,
        ("radar", "radarmode") => tuning.radar_mode = parse_u8(value, line_number)?,
        ("radar", "mapzoomfactor") => {
            tuning.map_zoom_factor = parse_u16(value, line_number)?.max(1)
        }
        ("kill", "bountyincreaseforkill") => {
            tuning.bounty_increase_for_kill = parse_u32(value, line_number)?
        }
        ("kill", "enterdelay") => {
            tuning.enter_delay_seconds = ticks_to_seconds_i16(parse_i16(value, line_number)?.max(0))
        }
        ("repel", "repelspeed") => {
            tuning.repel_speed = parse_nonnegative_f32(value, line_number)? / 10.0
        }
        ("repel", "repeltime") => {
            tuning.repel_lifetime = ticks_to_seconds_i16(parse_i16(value, line_number)?.max(0))
        }
        ("repel", "repeldistance") => {
            tuning.repel_radius = parse_nonnegative_f32(value, line_number)?
        }
        ("wormhole", "gravitybombs") => tuning.gravity_bombs = parse_bool(value, line_number)?,
        ("brick", "bricktime") => {
            tuning.brick_lifetime = ticks_to_seconds_i16(parse_i16(value, line_number)?)
        }
        ("rocket", "rocketthrust") => {
            for ship in &mut tuning.ships {
                ship.rocket_thrust_acceleration = parse_nonnegative_f32(value, line_number)? * 10.0;
            }
        }
        ("rocket", "rocketspeed") => {
            for ship in &mut tuning.ships {
                ship.rocket_top_speed = parse_nonnegative_f32(value, line_number)? / 10.0;
            }
        }
        ("door", "doordelay") => {
            tuning.door_delay_seconds = ticks_to_seconds_i16(parse_i16(value, line_number)?.max(1))
        }
        ("door", "doormode") => tuning.door_mode = parse_i16(value, line_number)?,
        ("misc", "warppointdelay") => {
            tuning.portal_lifetime = ticks_to_seconds_i16(parse_i16(value, line_number)?)
        }
        ("misc", "decoyalivetime") => {
            tuning.decoy_lifetime = ticks_to_seconds_i32(parse_i32(value, line_number)?)
        }
        ("misc", "bouncefactor") => {
            tuning.bounce_factor = continuum_bounce_factor(parse_i16(value, line_number)?)
        }
        ("misc", "warpradiuslimit") => {
            // The reference game doesn't clamp WarpRadiusLimit; preserve the parsed
            // value (1024 = anywhere per ArenaSettings).
            tuning.warp_radius_limit = parse_u16(value, line_number)?
        }
        ("misc", "victorymusic") => tuning.victory_music = parse_bool(value, line_number)?,
        ("misc" | "team", "maxfrequency") => {
            tuning.max_frequency = parse_u16(value, line_number)?.max(1)
        }
        ("misc" | "soccer", "allowbombs") => tuning.allow_bombs = parse_bool(value, line_number)?,
        ("misc" | "soccer", "allowguns") => tuning.allow_guns = parse_bool(value, line_number)?,
        ("prizeweight", _) => {
            if let Some(index) = prize_weight_index(key) {
                tuning.prize_weights[index] = parse_u8(value, line_number)?;
            }
        }
        _ => {}
    }

    Ok(())
}

fn apply_status(value: u8, available: &mut bool, initial: &mut bool) {
    *available = value != 0;
    *initial = value == 2;
}

fn clamp_settings_weapon_level(level: u8) -> u8 {
    crate::continuum_weapon::settings_weapon_level_to_bits(level)
}

fn ship_section_index(section: &str) -> Option<usize> {
    match section {
        "warbird" => Some(0),
        "javelin" => Some(1),
        "spider" => Some(2),
        "leviathan" => Some(3),
        "terrier" => Some(4),
        "weasel" => Some(5),
        "lancaster" => Some(6),
        "shark" => Some(7),
        _ => None,
    }
}

fn prize_weight_index(key: &str) -> Option<usize> {
    match key {
        "quickcharge" => Some(0),
        "energy" => Some(1),
        "rotation" => Some(2),
        "stealth" => Some(3),
        "cloak" => Some(4),
        "xradar" => Some(5),
        "warp" => Some(6),
        "gun" => Some(7),
        "bomb" => Some(8),
        "bouncingbullets" => Some(9),
        "thruster" => Some(10),
        "topspeed" => Some(11),
        "recharge" => Some(12),
        "glue" => Some(13),
        "multifire" => Some(14),
        "proximity" => Some(15),
        "allweapons" => Some(16),
        "shields" => Some(17),
        "shrapnel" => Some(18),
        "antiwarp" => Some(19),
        "repel" => Some(20),
        "burst" => Some(21),
        "decoy" => Some(22),
        "thor" => Some(23),
        "multiprize" => Some(24),
        "brick" => Some(25),
        "rocket" => Some(26),
        "portal" => Some(27),
        _ => None,
    }
}

fn strip_inline_comment(line: &str) -> &str {
    line.split_once(';').map(|(line, _)| line).unwrap_or(line)
}

fn normalize_name(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| !matches!(ch, ' ' | '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_bool(value: &str, line_number: usize) -> Result<bool, String> {
    match normalize_name(value).as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!(
            "invalid boolean setting on line {line_number}: {value}"
        )),
    }
}

fn parse_i16(value: &str, line_number: usize) -> Result<i16, String> {
    value
        .trim()
        .parse::<i16>()
        .map_err(|_| format!("invalid i16 setting on line {line_number}: {value}"))
}

fn parse_i32(value: &str, line_number: usize) -> Result<i32, String> {
    value
        .trim()
        .parse::<i32>()
        .map_err(|_| format!("invalid i32 setting on line {line_number}: {value}"))
}

fn parse_u8(value: &str, line_number: usize) -> Result<u8, String> {
    value
        .trim()
        .parse::<u8>()
        .map_err(|_| format!("invalid u8 setting on line {line_number}: {value}"))
}

fn parse_u16(value: &str, line_number: usize) -> Result<u16, String> {
    value
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("invalid u16 setting on line {line_number}: {value}"))
}

fn parse_u32(value: &str, line_number: usize) -> Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("invalid u32 setting on line {line_number}: {value}"))
}

fn parse_usize(value: &str, line_number: usize) -> Result<usize, String> {
    value
        .trim()
        .parse::<usize>()
        .map_err(|_| format!("invalid usize setting on line {line_number}: {value}"))
}

fn parse_nonnegative_f32(value: &str, line_number: usize) -> Result<f32, String> {
    let parsed = value
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("invalid numeric setting on line {line_number}: {value}"))?;
    if parsed < 0.0 {
        return Err(format!(
            "expected nonnegative setting on line {line_number}: {value}"
        ));
    }
    Ok(parsed)
}

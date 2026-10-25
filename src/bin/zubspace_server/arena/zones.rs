use crate::arena_settings::activate_settings_profile;
use crate::state::ServerState;
use crate::zone::zone_id_for_arena_name;

const ARENA_LOGIN_NAME_OFFSET: usize = 10;
const ARENA_LOGIN_NAME_LEN: usize = 16;

pub(crate) fn select_arena_zone_for_login_in_state(
    state: &mut ServerState,
    payload: &[u8],
) -> Result<(), &'static str> {
    let Some(zone_id) = requested_zone_id_from_arena_login(payload)? else {
        return Ok(());
    };

    if state.config.default_zone == zone_id {
        return Ok(());
    }
    if state.clients.values().any(|client| client.joined) {
        return Err("ZUBSPACE_ARENA_BUSY");
    }

    let zone = crate::zone::zone_map_for_id(zone_id).ok_or("ZUBSPACE_ARENA_UNKNOWN")?;
    let mut staged_objectives = state.objectives.clone();
    staged_objectives
        .reset(zone.objective_profile.clone())
        .and_then(|()| {
            crate::objectives::initialize_objectives(
                &mut staged_objectives,
                0,
                |x, y| {
                    zone.tiles
                        .get(usize::from(y) * crate::zone::MAP_TILES + usize::from(x))
                        .copied()
                        .is_none_or(|tile| tile != 0)
                },
                |x, y| {
                    crate::objectives::FlagDropTile::from_map_tile(
                        zone.tiles
                            .get(usize::from(y) * crate::zone::MAP_TILES + usize::from(x))
                            .copied()
                            .unwrap_or(20),
                        zone.no_flag_drop_tiles.contains(x, y),
                    )
                },
            )
            .map(|_| ())
        })
        .map_err(|_| "ZUBSPACE_ARENA_RESET_UNAVAILABLE")?;

    activate_settings_profile(zone_id).map_err(|_| "ZUBSPACE_ARENA_UNKNOWN")?;
    state.config.default_zone = zone_id.to_string();
    clear_transient_arena_state(state, staged_objectives);
    state.initialize_configured_bots();
    Ok(())
}

fn requested_zone_id_from_arena_login(
    payload: &[u8],
) -> Result<Option<&'static str>, &'static str> {
    let Some(name_bytes) = payload.get(
        ARENA_LOGIN_NAME_OFFSET
            ..payload
                .len()
                .min(ARENA_LOGIN_NAME_OFFSET + ARENA_LOGIN_NAME_LEN),
    ) else {
        return Ok(None);
    };
    let name_end = name_bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(name_bytes.len());
    let name = std::str::from_utf8(&name_bytes[..name_end])
        .map_err(|_| "ZUBSPACE_ARENA_INVALID")?
        .trim();
    if name.is_empty() {
        return Ok(None);
    }

    zone_id_for_arena_name(name)
        .map(Some)
        .ok_or("ZUBSPACE_ARENA_UNKNOWN")
}

fn clear_transient_arena_state(
    state: &mut ServerState,
    staged_objectives: crate::objectives::ObjectiveRegistry,
) {
    state.turret_links.clear();
    state.active_bricks.clear();
    state.active_projectiles.clear();
    state.active_decoys.clear();
    state.pending_owner_weapon_cancellations.clear();
    state.owner_weapon_cancellation_watermarks.clear();
    state.latest_owner_weapon_groups.clear();
    state.pending_damage_transactions.clear();
    state.retired_projectile_owners.clear();
    state.active_hidden_prizes.clear();
    state.next_hidden_prize_spawn_at_tick = None;
    state.claimed_prizes.clear();
    state.pending_position_inputs.clear();
    state.weapon_admission_deferred.clear();
    state.input_recovery_deferred_at.clear();
    state.arena_tick = 0;
    state.last_tick_at = None;
    state.objectives = staged_objectives;
}

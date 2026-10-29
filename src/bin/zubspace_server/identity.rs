use std::sync::{Arc, Mutex};

use super::state::{ClientHandle, PlayerStats, ServerState};
use super::wire::read_u32_le;

const LOGIN_NAME_OFFSET: usize = 2;
const LOGIN_NAME_LEN: usize = 32;
pub(crate) const LOGIN_ID_LOW_OFFSET: usize = 66;
pub(crate) const LOGIN_ID_HIGH_OFFSET: usize = 89;
const LOGIN_ID_MAX: u64 = 999_999_999_999;
const MAX_PLAYER_NAME_LEN: usize = 14;

pub(crate) fn player_login_accepted(state: &Arc<Mutex<ServerState>>, player_id: u16) -> bool {
    state
        .lock()
        .expect("server state should not be poisoned")
        .clients
        .get(&player_id)
        .is_some_and(|client| client.player_identity.is_some())
}

pub(crate) fn player_identity_in_use(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    identity: &str,
) -> bool {
    state
        .lock()
        .expect("server state should not be poisoned")
        .clients
        .iter()
        .any(|(&id, client)| id != player_id && client.player_identity.as_deref() == Some(identity))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoginAdmissionError {
    InvalidIdentity,
    InvalidName,
    IdentityInUse,
    NameInUse,
    Full,
    QualityCapacity,
}

impl LoginAdmissionError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::InvalidIdentity => "ZUBSPACE_ID_INVALID",
            Self::InvalidName => "ZUBSPACE_NAME_INVALID",
            Self::IdentityInUse => "ZUBSPACE_ID_IN_USE",
            Self::NameInUse => "ZUBSPACE_NAME_IN_USE",
            Self::Full | Self::QualityCapacity => "ZUBSPACE_SERVER_FULL",
        }
    }
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::InvalidIdentity => "invalid_identity",
            Self::InvalidName => "invalid_name",
            Self::IdentityInUse => "identity_in_use",
            Self::NameInUse => "name_in_use",
            Self::Full => "server_full",
            Self::QualityCapacity => "quality_hold_capacity",
        }
    }
}

pub(crate) fn name_key(name: &str) -> String {
    name.trim_matches(player_name_edge_whitespace)
        .to_ascii_lowercase()
}

pub(crate) fn arena_player_limit(state: &ServerState) -> usize {
    state
        .config
        .max_players
        .min(crate::limits::MAX_ARENA_PLAYERS)
}

pub(crate) fn reserved_human_count(state: &ServerState, excluding: Option<u16>) -> usize {
    state
        .clients
        .iter()
        .filter(|(id, client)| {
            Some(**id) != excluding && (client.joined || client.arena_admission_reserved)
        })
        .count()
}

pub(crate) fn server_at_capacity(state: &Arc<Mutex<ServerState>>, player_id: u16) -> bool {
    let state = state.lock().expect("server state should not be poisoned");
    reserved_human_count(&state, Some(player_id)) + state.bots.len() >= arena_player_limit(&state)
}

/// Caller holds the sole membership/name transaction lock. Never count an
/// existing reservation twice, including the later joined-state transition.
pub(crate) fn reserve_arena_slot_in_state(
    state: &mut ServerState,
    player_id: u16,
    name: &str,
) -> Result<(), LoginAdmissionError> {
    let client = state
        .clients
        .get(&player_id)
        .ok_or(LoginAdmissionError::InvalidIdentity)?;
    if (client.joined || client.arena_admission_reserved)
        && name_key(&client.player.name) == name_key(name)
    {
        return Ok(());
    }
    let key = name_key(name);
    if state.clients.iter().any(|(&id, client)| {
        id != player_id
            && (client.joined || client.arena_admission_reserved)
            && name_key(&client.player.name) == key
    }) || state
        .bots
        .iter()
        .any(|bot| name_key(&bot.player.name) == key)
    {
        return Err(LoginAdmissionError::NameInUse);
    }
    if reserved_human_count(state, Some(player_id)) + state.bots.len() >= arena_player_limit(state)
    {
        return Err(LoginAdmissionError::Full);
    }
    state
        .clients
        .get_mut(&player_id)
        .unwrap()
        .arena_admission_reserved = true;
    Ok(())
}

/// Identity uniqueness, name/slot ownership and quality-store capacity must
/// commit under one lock. Replayed accepted login never resets live statistics.
pub(crate) fn admit_player_login(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    identity: String,
    name: String,
) -> Result<(), LoginAdmissionError> {
    let name = normalized_login_name(&name).ok_or(LoginAdmissionError::InvalidName)?;
    let mut state = state.lock().expect("server state should not be poisoned");
    let client = state
        .clients
        .get(&player_id)
        .ok_or(LoginAdmissionError::InvalidIdentity)?;
    if let Some(existing) = &client.player_identity {
        if existing != &identity {
            return Err(LoginAdmissionError::InvalidIdentity);
        }
        if client.joined || client.arena_admission_reserved {
            return if name_key(&client.player.name) == name_key(&name) {
                Ok(())
            } else {
                Err(LoginAdmissionError::InvalidIdentity)
            };
        }
    }
    if state.clients.iter().any(|(&id, client)| {
        id != player_id && client.player_identity.as_deref() == Some(&identity)
    }) {
        return Err(LoginAdmissionError::IdentityInUse);
    }
    let other_unheld = state
        .clients
        .iter()
        .filter(|(id, client)| {
            **id != player_id
                && client
                    .player_identity
                    .as_deref()
                    .is_some_and(|identity| state.quality_holds.get(identity).is_none())
        })
        .count();
    if !state.quality_holds.can_admit(&identity, other_unheld) {
        return Err(LoginAdmissionError::QualityCapacity);
    }
    reserve_arena_slot_in_state(&mut state, player_id, &name)?;
    let stats = state
        .identity_stats
        .get(&identity)
        .copied()
        .unwrap_or_default();
    let client = state.clients.get_mut(&player_id).unwrap();
    if client.player_identity.is_none() {
        client.player.assists = stats.assists;
        client.player.kill_points = stats.kill_points;
        client.player.flag_points = stats.flag_points;
        client.player.wins = stats.wins;
        client.player.losses = stats.losses;
        client
            .session_summary
            .scores_restored(stats.wins, stats.losses);
    }
    client.player_identity = Some(identity);
    client.player.name = name;
    Ok(())
}

pub(crate) fn set_player_identity_and_name(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    identity: String,
    name: String,
) -> bool {
    admit_player_login(state, player_id, identity, name).is_ok()
}

pub(crate) fn persist_stats_for_client(state: &mut ServerState, player_id: u16) {
    let update = state.clients.get(&player_id).and_then(|client| {
        client
            .player_identity
            .clone()
            .map(|identity| (identity, client_stats(client)))
    });
    if let Some((identity, stats)) = update {
        state.identity_stats.insert(identity, stats);
    }
}

pub(crate) fn persist_stats_for_departure(state: &mut ServerState, player_id: u16) {
    let has_statistics = state.clients.get(&player_id).is_some_and(|client| {
        client.player_identity.as_ref().is_some_and(|identity| {
            state.identity_stats.contains_key(identity)
                || client_stats(client) != PlayerStats::default()
        })
    });
    // A zero-score anonymous connect/disconnect need not allocate a cache row.
    if has_statistics {
        persist_stats_for_client(state, player_id);
    }
}

fn client_stats(client: &ClientHandle) -> PlayerStats {
    PlayerStats {
        assists: client.player.assists,
        kill_points: client.player.kill_points,
        flag_points: client.player.flag_points,
        wins: client.player.wins,
        losses: client.player.losses,
    }
}

pub(crate) fn login_name(payload: &[u8]) -> Option<String> {
    let name = payload.get(LOGIN_NAME_OFFSET..LOGIN_NAME_OFFSET + LOGIN_NAME_LEN)?;
    let end = name
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(name.len());
    let name = String::from_utf8_lossy(&name[..end]).trim().to_string();
    (!name.is_empty()).then_some(name)
}

pub(crate) fn login_player_identity(payload: &[u8]) -> Option<String> {
    let low = read_u32_le(payload, LOGIN_ID_LOW_OFFSET)? as u64;
    let high = read_u32_le(payload, LOGIN_ID_HIGH_OFFSET)? as u64;
    let identity = (high << 32) | low;
    (identity <= LOGIN_ID_MAX).then(|| format!("{identity:012}"))
}

pub(crate) fn normalized_login_name(name: &str) -> Option<String> {
    let name = name.trim_matches(player_name_edge_whitespace);
    (!name.is_empty()
        && name.len() <= MAX_PLAYER_NAME_LEN
        && name
            .split(' ')
            .all(|part| !part.is_empty() && part.chars().all(valid_login_name_char)))
    .then(|| name.to_owned())
}

fn valid_login_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '!' | '*' | '-' | '_')
}

fn player_name_edge_whitespace(ch: char) -> bool {
    // Match browser normalization, including common invisible separators
    // outside Unicode White_Space. Internal instances remain invalid.
    ch.is_whitespace() || matches!(ch, '\u{180e}' | '\u{200b}' | '\u{2060}' | '\u{feff}')
}

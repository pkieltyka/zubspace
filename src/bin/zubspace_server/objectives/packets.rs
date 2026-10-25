use super::super::zone::ZoneFlag;
use super::registry::{FlagSnapshot, FlagState, PowerballSnapshot, PowerballState};

pub(crate) const BRICK_DROPPED: u8 = 0x21;
pub(crate) const POWERBALL_POSITION: u8 = 0x2e;

const CREATE_TURRET_LINK: u8 = 0x0e;
const DESTROY_TURRET_LINK: u8 = 0x15;
pub(crate) const FLAG_POSITION: u8 = 0x12;
const FLAG_CLAIM: u8 = 0x13;
const DROP_FLAG: u8 = 0x16;
const CREATE_TURRET_LINK_LEN: usize = 5;
const DESTROY_TURRET_LINK_LEN: usize = 3;
const FLAG_POSITION_LEN: usize = 9;
const FLAG_CLAIM_LEN: usize = 5;
const DROP_FLAG_LEN: usize = 3;
const BRICK_DROPPED_LEN: usize = 17;
const POWERBALL_POSITION_LEN: usize = 16;
const INVALID_PLAYER_ID: u16 = 0xffff;

pub(crate) fn create_turret_link(requester_id: u16, destination_id: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(CREATE_TURRET_LINK_LEN);
    payload.push(CREATE_TURRET_LINK);
    payload.extend(requester_id.to_le_bytes());
    payload.extend(destination_id.to_le_bytes());
    payload
}

pub(crate) fn destroy_turret_link(player_id: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(DESTROY_TURRET_LINK_LEN);
    payload.push(DESTROY_TURRET_LINK);
    payload.extend(player_id.to_le_bytes());
    payload
}

pub(crate) fn flag_position_packet(flag: ZoneFlag) -> Vec<u8> {
    let mut payload = Vec::with_capacity(FLAG_POSITION_LEN);
    payload.push(FLAG_POSITION);
    payload.extend(flag.id.to_le_bytes());
    payload.extend(flag.x.to_le_bytes());
    payload.extend(flag.y.to_le_bytes());
    payload.extend(flag.owner.to_le_bytes());
    payload
}

pub(crate) fn flag_claim_packet(flag_id: u16, player_id: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(FLAG_CLAIM_LEN);
    payload.push(FLAG_CLAIM);
    payload.extend(flag_id.to_le_bytes());
    payload.extend(player_id.to_le_bytes());
    payload
}

pub(crate) fn drop_flag_packet(player_id: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(DROP_FLAG_LEN);
    payload.push(DROP_FLAG);
    payload.extend(player_id.to_le_bytes());
    payload
}

pub(crate) fn clear_bricks_packet() -> Vec<u8> {
    vec![BRICK_DROPPED]
}

pub(crate) fn brick_dropped_packet(
    start: (u16, u16),
    end: (u16, u16),
    team: u16,
    brick_id: u16,
    timestamp: u32,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(BRICK_DROPPED_LEN);
    payload.push(BRICK_DROPPED);
    payload.extend(start.0.to_le_bytes());
    payload.extend(start.1.to_le_bytes());
    payload.extend(end.0.to_le_bytes());
    payload.extend(end.1.to_le_bytes());
    payload.extend(team.to_le_bytes());
    payload.extend(brick_id.to_le_bytes());
    payload.extend(timestamp.to_le_bytes());
    payload
}

pub(crate) fn powerball_removal_packet(ball_id: u8) -> Vec<u8> {
    powerball_position_packet(ball_id, 0, 0, 0, 0, None, 0)
}

pub(crate) fn powerball_position_packet(
    ball_id: u8,
    x: u16,
    y: u16,
    velocity_x: i16,
    velocity_y: i16,
    owner: Option<u16>,
    version: u32,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(POWERBALL_POSITION_LEN);
    payload.push(POWERBALL_POSITION);
    payload.push(ball_id);
    payload.extend(x.to_le_bytes());
    payload.extend(y.to_le_bytes());
    payload.extend(velocity_x.to_le_bytes());
    payload.extend(velocity_y.to_le_bytes());
    payload.extend(owner.unwrap_or(INVALID_PLAYER_ID).to_le_bytes());
    payload.extend((version & crate::continuum::CONTINUUM_TICK_MASK).to_le_bytes());
    payload
}

#[track_caller]
pub(crate) fn flag_state_packet(
    flag: FlagSnapshot,
    server_tick: u32,
    action_id: u32,
    outcome: u8,
) -> Vec<u8> {
    // One choke point for every authoritative flag state that can reach a client.
    // The registry's own `objective flag transition` line says what the server
    // believes; this says what a client is handed, and by whom - the flag defect
    // presents an on-map unowned flag while the registry still holds `Carried`.
    tracing::debug!(
        flag = flag.identity.id,
        generation = flag.identity.generation,
        version = flag.version.get(),
        state = ?flag.state,
        caller = %std::panic::Location::caller(),
        "authoritative flag state packet emitted",
    );
    let (state, x, y, owner_id, owner_frequency) = match flag.state {
        FlagState::Waiting { .. } => (
            crate::objective_protocol::ObjectiveStateUpdate::STATE_WAITING,
            0,
            0,
            None,
            None,
        ),
        FlagState::OnMap { x, y, owner } => (
            crate::objective_protocol::ObjectiveStateUpdate::STATE_ON_MAP,
            x,
            y,
            None,
            owner,
        ),
        FlagState::Carried {
            carrier,
            x,
            y,
            owner,
        } => (
            crate::objective_protocol::ObjectiveStateUpdate::STATE_CARRIED,
            x,
            y,
            Some(carrier.player_id()),
            owner,
        ),
    };
    crate::objective_protocol::ObjectiveStateUpdate {
        kind: crate::objective_protocol::ObjectiveStateUpdate::KIND_FLAG,
        state,
        objective_id: flag.identity.id,
        generation: flag.identity.generation,
        version: flag.version.get(),
        x,
        y,
        velocity_x: 0,
        velocity_y: 0,
        owner_id,
        owner_frequency,
        server_tick,
        pickup_unlock_tick: None,
        killer_id: None,
        killer_pickup_unlock_tick: None,
        action_id,
        outcome,
    }
    .encode()
}

pub(crate) fn objective_score_packet(
    generation: u64,
    version: super::registry::ObjectiveVersion,
    frequency: u16,
    score: u32,
    server_tick: u32,
    outcome: u8,
) -> Vec<u8> {
    crate::objective_protocol::ObjectiveScoreUpdate {
        generation,
        version: version.get(),
        frequency,
        score,
        server_tick,
        outcome,
    }
    .encode()
}

pub(crate) fn powerball_state_packet(
    ball: PowerballSnapshot,
    lineage: Option<(u16, u16)>,
    killer_pickup: Option<(u16, u32)>,
    server_tick: u32,
    action_id: u32,
    outcome: u8,
) -> Vec<u8> {
    let (state, x, y, velocity_x, velocity_y, owner_id, owner_frequency, pickup_unlock_tick) =
        match ball.state {
            PowerballState::Waiting { .. } => (
                crate::objective_protocol::ObjectiveStateUpdate::STATE_WAITING,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
            ),
            PowerballState::OnMap {
                x,
                y,
                velocity_x,
                velocity_y,
                pickup_unlock_tick,
            } => (
                crate::objective_protocol::ObjectiveStateUpdate::STATE_ON_MAP,
                x,
                y,
                velocity_x,
                velocity_y,
                lineage.map(|(player_id, _)| player_id),
                lineage.map(|(_, frequency)| frequency),
                pickup_unlock_tick,
            ),
            PowerballState::Carried { carrier } => (
                crate::objective_protocol::ObjectiveStateUpdate::STATE_CARRIED,
                0,
                0,
                0,
                0,
                Some(carrier.player_id()),
                lineage.map(|(_, frequency)| frequency),
                None,
            ),
        };
    crate::objective_protocol::ObjectiveStateUpdate {
        kind: crate::objective_protocol::ObjectiveStateUpdate::KIND_POWERBALL,
        state,
        objective_id: u16::from(ball.identity.id),
        generation: ball.identity.generation,
        version: ball.version.get(),
        x,
        y,
        velocity_x,
        velocity_y,
        owner_id,
        owner_frequency,
        server_tick,
        pickup_unlock_tick,
        killer_id: killer_pickup.map(|(player_id, _)| player_id),
        killer_pickup_unlock_tick: killer_pickup.map(|(_, deadline)| deadline),
        action_id,
        outcome,
    }
    .encode()
}

pub(crate) fn flag_victory_packet(frequency: u16, jackpot_points: i64) -> Vec<u8> {
    let mut packet = Vec::with_capacity(7);
    packet.push(0x14);
    packet.extend(frequency.to_le_bytes());
    packet.extend((jackpot_points.clamp(0, i64::from(u32::MAX)) as u32).to_le_bytes());
    packet
}

pub(crate) fn soccer_goal_packet(frequency: u16, reward_points: i64) -> Vec<u8> {
    let mut packet = Vec::with_capacity(7);
    packet.push(0x0b);
    packet.extend(frequency.to_le_bytes());
    packet.extend((reward_points.clamp(0, i64::from(u32::MAX)) as u32).to_le_bytes());
    packet
}

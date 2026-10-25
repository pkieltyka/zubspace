#![allow(dead_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectiveActionKind {
    FlagClaim = 1,
    FlagDrop = 2,
    BallPickup = 3,
    BallFire = 4,
    BallGoal = 5,
}

impl ObjectiveActionKind {
    fn parse(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::FlagClaim,
            2 => Self::FlagDrop,
            3 => Self::BallPickup,
            4 => Self::BallFire,
            5 => Self::BallGoal,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectiveActionRequest {
    pub kind: ObjectiveActionKind,
    pub server_session_id: u64,
    pub arena_stream_id: u64,
    pub player_id: u16,
    pub life_seq: u32,
    pub action_id: u32,
    pub position_sequence: u16,
    pub objective_id: u16,
    pub objective_generation: u64,
    pub expected_version: u32,
}

impl ObjectiveActionRequest {
    pub const TYPE: u8 = 0x67;
    pub const VERSION: u8 = 1;
    pub const PACKET_LEN: usize = 45;

    pub fn encode(self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::PACKET_LEN);
        packet.extend([Self::TYPE, Self::VERSION, self.kind as u8]);
        packet.extend(self.server_session_id.to_le_bytes());
        packet.extend(self.arena_stream_id.to_le_bytes());
        packet.extend(self.player_id.to_le_bytes());
        packet.extend(self.life_seq.to_le_bytes());
        packet.extend(self.action_id.to_le_bytes());
        packet.extend(self.position_sequence.to_le_bytes());
        packet.extend(self.objective_id.to_le_bytes());
        packet.extend(self.objective_generation.to_le_bytes());
        packet.extend(self.expected_version.to_le_bytes());
        packet
    }

    pub fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN || packet[0] != Self::TYPE || packet[1] != Self::VERSION
        {
            return Err("invalid objective action framing".to_string());
        }
        let request = Self {
            kind: ObjectiveActionKind::parse(packet[2])
                .ok_or_else(|| "invalid objective action kind".to_string())?,
            server_session_id: u64::from_le_bytes(packet[3..11].try_into().unwrap()),
            arena_stream_id: u64::from_le_bytes(packet[11..19].try_into().unwrap()),
            player_id: u16::from_le_bytes(packet[19..21].try_into().unwrap()),
            life_seq: u32::from_le_bytes(packet[21..25].try_into().unwrap()),
            action_id: u32::from_le_bytes(packet[25..29].try_into().unwrap()),
            position_sequence: u16::from_le_bytes(packet[29..31].try_into().unwrap()),
            objective_id: u16::from_le_bytes(packet[31..33].try_into().unwrap()),
            objective_generation: u64::from_le_bytes(packet[33..41].try_into().unwrap()),
            expected_version: u32::from_le_bytes(packet[41..45].try_into().unwrap()),
        };
        if request.server_session_id == 0
            || request.arena_stream_id == 0
            || request.life_seq == 0
            || request.action_id == 0
            || request.objective_generation == 0
            || request.expected_version == 0
            || request.expected_version > crate::continuum::CONTINUUM_TICK_MASK
            || matches!(
                request.kind,
                ObjectiveActionKind::BallPickup
                    | ObjectiveActionKind::BallFire
                    | ObjectiveActionKind::BallGoal
            ) && request.objective_id > u16::from(u8::MAX)
        {
            return Err("invalid objective action identity".to_string());
        }
        Ok(request)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectiveStateUpdate {
    pub kind: u8,
    pub state: u8,
    pub objective_id: u16,
    pub generation: u64,
    pub version: u32,
    pub x: u16,
    pub y: u16,
    pub velocity_x: i16,
    pub velocity_y: i16,
    pub owner_id: Option<u16>,
    pub owner_frequency: Option<u16>,
    pub server_tick: u32,
    pub pickup_unlock_tick: Option<u32>,
    pub killer_id: Option<u16>,
    pub killer_pickup_unlock_tick: Option<u32>,
    pub action_id: u32,
    pub outcome: u8,
}

impl ObjectiveStateUpdate {
    pub const TYPE: u8 = 0x68;
    pub const VERSION: u8 = 2;
    pub const PACKET_LEN: usize = 49;
    pub const KIND_FLAG: u8 = 1;
    pub const KIND_POWERBALL: u8 = 2;
    pub const STATE_WAITING: u8 = 0;
    pub const STATE_ON_MAP: u8 = 1;
    pub const STATE_CARRIED: u8 = 2;
    pub const OUTCOME_REPLAY: u8 = 0;
    pub const OUTCOME_APPLIED: u8 = 1;
    pub const OUTCOME_REJECTED: u8 = 2;
    pub const INVALID_ID: u16 = u16::MAX;

    pub fn encode(self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::PACKET_LEN);
        packet.extend([Self::TYPE, Self::VERSION, self.kind, self.state]);
        packet.extend(self.objective_id.to_le_bytes());
        packet.extend(self.generation.to_le_bytes());
        packet.extend(self.version.to_le_bytes());
        packet.extend(self.x.to_le_bytes());
        packet.extend(self.y.to_le_bytes());
        packet.extend(self.velocity_x.to_le_bytes());
        packet.extend(self.velocity_y.to_le_bytes());
        packet.extend(self.owner_id.unwrap_or(Self::INVALID_ID).to_le_bytes());
        packet.extend(
            self.owner_frequency
                .unwrap_or(Self::INVALID_ID)
                .to_le_bytes(),
        );
        packet.extend((self.server_tick & crate::continuum::CONTINUUM_TICK_MASK).to_le_bytes());
        packet.extend(self.pickup_unlock_tick.unwrap_or(u32::MAX).to_le_bytes());
        packet.extend(self.killer_id.unwrap_or(Self::INVALID_ID).to_le_bytes());
        packet.extend(
            self.killer_pickup_unlock_tick
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        packet.extend(self.action_id.to_le_bytes());
        packet.push(self.outcome);
        packet
    }

    pub fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN
            || packet[0] != Self::TYPE
            || packet[1] != Self::VERSION
            || !matches!(packet[2], Self::KIND_FLAG | Self::KIND_POWERBALL)
            || !matches!(
                packet[3],
                Self::STATE_WAITING | Self::STATE_ON_MAP | Self::STATE_CARRIED
            )
            || !matches!(
                packet[48],
                Self::OUTCOME_REPLAY | Self::OUTCOME_APPLIED | Self::OUTCOME_REJECTED
            )
        {
            return Err("invalid objective state framing".to_string());
        }
        let owner_id = u16::from_le_bytes(packet[26..28].try_into().unwrap());
        let owner_frequency = u16::from_le_bytes(packet[28..30].try_into().unwrap());
        let killer_id = u16::from_le_bytes(packet[38..40].try_into().unwrap());
        let update = Self {
            kind: packet[2],
            state: packet[3],
            objective_id: u16::from_le_bytes(packet[4..6].try_into().unwrap()),
            generation: u64::from_le_bytes(packet[6..14].try_into().unwrap()),
            version: u32::from_le_bytes(packet[14..18].try_into().unwrap()),
            x: u16::from_le_bytes(packet[18..20].try_into().unwrap()),
            y: u16::from_le_bytes(packet[20..22].try_into().unwrap()),
            velocity_x: i16::from_le_bytes(packet[22..24].try_into().unwrap()),
            velocity_y: i16::from_le_bytes(packet[24..26].try_into().unwrap()),
            owner_id: (owner_id != Self::INVALID_ID).then_some(owner_id),
            owner_frequency: (owner_frequency != Self::INVALID_ID).then_some(owner_frequency),
            server_tick: u32::from_le_bytes(packet[30..34].try_into().unwrap()),
            pickup_unlock_tick: match u32::from_le_bytes(packet[34..38].try_into().unwrap()) {
                u32::MAX => None,
                tick => Some(tick),
            },
            killer_id: (killer_id != Self::INVALID_ID).then_some(killer_id),
            killer_pickup_unlock_tick: match u32::from_le_bytes(packet[40..44].try_into().unwrap())
            {
                u32::MAX => None,
                tick => Some(tick),
            },
            action_id: u32::from_le_bytes(packet[44..48].try_into().unwrap()),
            outcome: packet[48],
        };
        if update.generation == 0
            || update.version == 0
            || update.version > crate::continuum::CONTINUUM_TICK_MASK
            || update.server_tick > crate::continuum::CONTINUUM_TICK_MASK
            || update
                .pickup_unlock_tick
                .is_some_and(|tick| tick > crate::continuum::CONTINUUM_TICK_MASK)
            || update
                .killer_pickup_unlock_tick
                .is_some_and(|tick| tick > crate::continuum::CONTINUUM_TICK_MASK)
            || update.killer_id.is_some() != update.killer_pickup_unlock_tick.is_some()
        {
            return Err("invalid objective state identity".to_string());
        }
        Ok(update)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectiveScoreUpdate {
    pub generation: u64,
    pub version: u32,
    pub frequency: u16,
    pub score: u32,
    pub server_tick: u32,
    pub outcome: u8,
}

impl ObjectiveScoreUpdate {
    pub const TYPE: u8 = 0x69;
    pub const VERSION: u8 = 1;
    pub const PACKET_LEN: usize = 25;
    pub const OUTCOME_REPLAY: u8 = 0;
    pub const OUTCOME_APPLIED: u8 = 1;

    pub fn encode(self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::PACKET_LEN);
        packet.extend([Self::TYPE, Self::VERSION]);
        packet.extend(self.generation.to_le_bytes());
        packet.extend(self.version.to_le_bytes());
        packet.extend(self.frequency.to_le_bytes());
        packet.extend(self.score.to_le_bytes());
        packet.extend((self.server_tick & crate::continuum::CONTINUUM_TICK_MASK).to_le_bytes());
        packet.push(self.outcome);
        packet
    }

    pub fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN
            || packet[0] != Self::TYPE
            || packet[1] != Self::VERSION
            || !matches!(packet[24], Self::OUTCOME_REPLAY | Self::OUTCOME_APPLIED)
        {
            return Err("invalid objective score framing".to_string());
        }
        let update = Self {
            generation: u64::from_le_bytes(packet[2..10].try_into().unwrap()),
            version: u32::from_le_bytes(packet[10..14].try_into().unwrap()),
            frequency: u16::from_le_bytes(packet[14..16].try_into().unwrap()),
            score: u32::from_le_bytes(packet[16..20].try_into().unwrap()),
            server_tick: u32::from_le_bytes(packet[20..24].try_into().unwrap()),
            outcome: packet[24],
        };
        if update.generation == 0
            || update.version == 0
            || update.version > crate::continuum::CONTINUUM_TICK_MASK
            || update.server_tick > crate::continuum::CONTINUUM_TICK_MASK
        {
            return Err("invalid objective score identity".to_string());
        }
        Ok(update)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_state_and_score_round_trip_full_identities() {
        let action = ObjectiveActionRequest {
            kind: ObjectiveActionKind::BallFire,
            server_session_id: u64::MAX,
            arena_stream_id: u64::MAX - 1,
            player_id: 7,
            life_seq: u32::MAX,
            action_id: u32::MAX,
            position_sequence: u16::MAX,
            objective_id: u16::from(u8::MAX),
            objective_generation: u64::MAX,
            expected_version: crate::continuum::CONTINUUM_TICK_MASK,
        };
        assert_eq!(ObjectiveActionRequest::parse(&action.encode()), Ok(action));

        let update = ObjectiveStateUpdate {
            kind: ObjectiveStateUpdate::KIND_POWERBALL,
            state: ObjectiveStateUpdate::STATE_CARRIED,
            objective_id: 1,
            generation: 2,
            version: 3,
            x: 4,
            y: 5,
            velocity_x: -6,
            velocity_y: 7,
            owner_id: Some(8),
            owner_frequency: Some(9),
            server_tick: 10,
            pickup_unlock_tick: Some(12),
            killer_id: Some(13),
            killer_pickup_unlock_tick: Some(11),
            action_id: 11,
            outcome: ObjectiveStateUpdate::OUTCOME_APPLIED,
        };
        assert_eq!(ObjectiveStateUpdate::parse(&update.encode()), Ok(update));
        let mut invalid_unlock = update.encode();
        invalid_unlock[34..38]
            .copy_from_slice(&(crate::continuum::CONTINUUM_TICK_MASK + 1).to_le_bytes());
        assert!(ObjectiveStateUpdate::parse(&invalid_unlock).is_err());

        let score = ObjectiveScoreUpdate {
            generation: u64::MAX,
            version: crate::continuum::CONTINUUM_TICK_MASK,
            frequency: u16::MAX,
            score: u32::MAX,
            server_tick: crate::continuum::CONTINUUM_TICK_MASK,
            outcome: ObjectiveScoreUpdate::OUTCOME_APPLIED,
        };
        assert_eq!(ObjectiveScoreUpdate::parse(&score.encode()), Ok(score));
    }
}

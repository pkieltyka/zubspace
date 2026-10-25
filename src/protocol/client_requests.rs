use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrequencyChangeRequest {
    pub(crate) frequency: u16,
    /// First input/position sequence that may belong to the new frequency
    /// generation. The client pauses realtime emission at this boundary until
    /// the reliable roster change and baseline complete.
    pub(crate) position_sequence_base: u16,
    /// Weapon-free pre-cutover position submitted on the reliable request.
    /// The server validates it under the old runtime identity, captures the
    /// canonical accepted fields, and echoes those fields in its private
    /// cutover acknowledgement.
    pub(crate) baseline: Vec<u8>,
}

impl FrequencyChangeRequest {
    pub(crate) const TYPE: u8 = 0x0f;

    #[allow(dead_code)]
    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(5 + self.baseline.len());
        push_u8(&mut bytes, Self::TYPE);
        push_u16(&mut bytes, self.frequency);
        push_u16(&mut bytes, self.position_sequence_base);
        bytes.extend_from_slice(&self.baseline);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpectateRequest {
    pub(crate) player_id: u16,
}

impl SpectateRequest {
    pub(crate) const TYPE: u8 = 0x08;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(3);
        push_u8(&mut bytes, Self::TYPE);
        push_u16(&mut bytes, self.player_id);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttachRequest {
    pub(crate) player_id: u16,
}

impl AttachRequest {
    pub(crate) const TYPE: u8 = 0x10;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(3);
        push_u8(&mut bytes, Self::TYPE);
        push_u16(&mut bytes, self.player_id);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagRequest {
    pub(crate) flag_id: u16,
}

impl FlagRequest {
    pub(crate) const TYPE: u8 = 0x13;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(3);
        push_u8(&mut bytes, Self::TYPE);
        push_u16(&mut bytes, self.flag_id);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeathRequest {
    pub(crate) killer_id: u16,
    pub(crate) bounty: u16,
}

impl DeathRequest {
    pub(crate) const TYPE: u8 = 0x05;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(5);
        push_u8(&mut bytes, Self::TYPE);
        push_u16(&mut bytes, self.killer_id);
        push_u16(&mut bytes, self.bounty);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaveArenaRequest;

impl LeaveArenaRequest {
    pub(crate) const TYPE: u8 = 0x02;

    pub(crate) fn encode_payload(self) -> Vec<u8> {
        vec![Self::TYPE]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreDisconnectRequest;

impl CoreDisconnectRequest {
    pub(crate) const CORE_BYTE: u8 = 0x00;
    pub(crate) const TYPE: u8 = 0x07;

    pub(crate) fn encode(self) -> Vec<u8> {
        vec![Self::CORE_BYTE, Self::TYPE]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceUploadRequest {
    pub(crate) index: u8,
    pub(crate) player_id: u16,
    pub(crate) wav_data: Vec<u8>,
}

impl VoiceUploadRequest {
    pub(crate) const TYPE: u8 = 0x0e;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(4 + self.wav_data.len());
        push_u8(&mut bytes, Self::TYPE);
        push_u8(&mut bytes, self.index);
        push_u16(&mut bytes, self.player_id);
        bytes.extend_from_slice(&self.wav_data);
        bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientFileTransferRequest {
    pub(crate) filename: String,
    pub(crate) data: Vec<u8>,
}

impl ClientFileTransferRequest {
    pub(crate) const TYPE: u8 = 0x16;
    pub(crate) const FILENAME_LEN: usize = 16;

    #[cfg(any(test, target_arch = "wasm32"))]
    pub(crate) fn empty_response_to(request: &ClientFileRequest) -> Self {
        let filename = if request.remote_filename.is_empty() {
            request.local_filename.clone()
        } else {
            request.remote_filename.clone()
        };

        Self {
            filename,
            data: Vec::new(),
        }
    }

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1 + Self::FILENAME_LEN + self.data.len());
        push_u8(&mut bytes, Self::TYPE);
        push_fixed_string(&mut bytes, self.filename.as_bytes(), Self::FILENAME_LEN);
        bytes.extend_from_slice(&self.data);
        bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationFormResponse {
    pub(crate) real_name: String,
    pub(crate) email: String,
    pub(crate) city: String,
    pub(crate) state: String,
    pub(crate) sex: u8,
    pub(crate) age: u8,
    pub(crate) connection_home: bool,
    pub(crate) connection_work: bool,
    pub(crate) connection_school: bool,
    pub(crate) processor_type: u32,
    pub(crate) unknown: u32,
    pub(crate) windows_registration_name: String,
    pub(crate) windows_registration_organization: String,
    pub(crate) driver_descriptions: [String; 13],
}

impl RegistrationFormResponse {
    pub(crate) const TYPE: u8 = 0x17;
    pub(crate) const REAL_NAME_LEN: usize = 32;
    pub(crate) const EMAIL_LEN: usize = 64;
    pub(crate) const CITY_LEN: usize = 32;
    pub(crate) const STATE_LEN: usize = 24;
    pub(crate) const WINDOWS_REGISTRATION_NAME_LEN: usize = 40;
    pub(crate) const WINDOWS_REGISTRATION_ORGANIZATION_LEN: usize = 40;
    pub(crate) const DRIVER_DESCRIPTION_LEN: usize = 40;

    #[cfg(any(test, target_arch = "wasm32"))]
    pub(crate) fn minimal() -> Self {
        Self {
            real_name: String::new(),
            email: "zubspace@example.invalid".to_string(),
            city: String::new(),
            state: String::new(),
            sex: b'M',
            age: 0,
            connection_home: true,
            connection_work: false,
            connection_school: false,
            processor_type: 0,
            unknown: 0,
            windows_registration_name: String::new(),
            windows_registration_organization: String::new(),
            driver_descriptions: std::array::from_fn(|_| String::new()),
        }
    }

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(766);
        push_u8(&mut bytes, Self::TYPE);
        push_fixed_string(&mut bytes, self.real_name.as_bytes(), Self::REAL_NAME_LEN);
        push_fixed_string(&mut bytes, self.email.as_bytes(), Self::EMAIL_LEN);
        push_fixed_string(&mut bytes, self.city.as_bytes(), Self::CITY_LEN);
        push_fixed_string(&mut bytes, self.state.as_bytes(), Self::STATE_LEN);
        push_u8(&mut bytes, self.sex);
        push_u8(&mut bytes, self.age);
        push_u8(&mut bytes, u8::from(self.connection_home));
        push_u8(&mut bytes, u8::from(self.connection_work));
        push_u8(&mut bytes, u8::from(self.connection_school));
        push_u32(&mut bytes, self.processor_type);
        push_u32(&mut bytes, self.unknown);
        push_fixed_string(
            &mut bytes,
            self.windows_registration_name.as_bytes(),
            Self::WINDOWS_REGISTRATION_NAME_LEN,
        );
        push_fixed_string(
            &mut bytes,
            self.windows_registration_organization.as_bytes(),
            Self::WINDOWS_REGISTRATION_ORGANIZATION_LEN,
        );
        for description in &self.driver_descriptions {
            push_fixed_string(
                &mut bytes,
                description.as_bytes(),
                Self::DRIVER_DESCRIPTION_LEN,
            );
        }
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExeRequest;

impl ExeRequest {
    pub(crate) const TYPE: u8 = 0x0b;

    pub(crate) fn encode_payload(self) -> Vec<u8> {
        vec![Self::TYPE]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewsRequest;

impl NewsRequest {
    pub(crate) const TYPE: u8 = 0x0d;

    pub(crate) fn encode_payload(self) -> Vec<u8> {
        vec![Self::TYPE]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecurityViolationReport {
    pub(crate) violation_id: u8,
}

impl SecurityViolationReport {
    pub(crate) const TYPE: u8 = 0x1b;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        vec![Self::TYPE, self.violation_id]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecurityViolationChecksumReport {
    pub(crate) unknown: u32,
    pub(crate) arena_settings_checksum: u32,
    pub(crate) code_checksum_1: u32,
    pub(crate) code_checksum_2: u32,
    pub(crate) violation_id: u8,
}

impl SecurityViolationChecksumReport {
    pub(crate) const TYPE: u8 = 0x22;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(18);
        push_u8(&mut bytes, Self::TYPE);
        push_u32(&mut bytes, self.unknown);
        push_u32(&mut bytes, self.arena_settings_checksum);
        push_u32(&mut bytes, self.code_checksum_1);
        push_u32(&mut bytes, self.code_checksum_2);
        push_u8(&mut bytes, self.violation_id);
        bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArenaSettingsChangeRequest {
    pub(crate) changes: Vec<String>,
}

impl ArenaSettingsChangeRequest {
    pub(crate) const TYPE: u8 = 0x1d;

    #[allow(dead_code)]
    pub(crate) fn new(changes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            changes: changes.into_iter().map(Into::into).collect(),
        }
    }

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let payload_len = self
            .changes
            .iter()
            .map(|change| change.len() + 1)
            .sum::<usize>()
            + 1;
        let mut bytes = Vec::with_capacity(1 + payload_len);
        push_u8(&mut bytes, Self::TYPE);
        for change in &self.changes {
            bytes.extend_from_slice(change.as_bytes());
            push_u8(&mut bytes, 0);
        }
        push_u8(&mut bytes, 0);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropAttachedRequest;

impl DropAttachedRequest {
    pub(crate) const TYPE: u8 = 0x14;

    pub(crate) fn encode_payload(self) -> Vec<u8> {
        vec![Self::TYPE]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropFlagsRequest;

impl DropFlagsRequest {
    pub(crate) const TYPE: u8 = 0x15;

    pub(crate) fn encode_payload(self) -> Vec<u8> {
        vec![Self::TYPE]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndKothTimerRequest;

impl EndKothTimerRequest {
    pub(crate) const TYPE: u8 = 0x1e;

    pub(crate) fn encode_payload(self) -> Vec<u8> {
        vec![Self::TYPE]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShipChangeRequest {
    pub(crate) ship: u8,
    /// First input/position sequence that belongs to the requested ship life.
    /// Zubspace clients pause realtime emission until the reliable roster
    /// acknowledgement, so every earlier unordered packet is unambiguously
    /// stale once the server commits this request.
    pub(crate) position_sequence_base: Option<u16>,
}

impl ShipChangeRequest {
    pub(crate) const TYPE: u8 = 0x18;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(if self.position_sequence_base.is_some() {
            4
        } else {
            2
        });
        push_u8(&mut bytes, Self::TYPE);
        push_u8(&mut bytes, self.ship);
        if let Some(sequence) = self.position_sequence_base {
            push_u16(&mut bytes, sequence);
        }
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetBannerRequest {
    pub(crate) banner: [u8; PLAYER_BANNER_BYTES],
}

impl SetBannerRequest {
    pub(crate) const TYPE: u8 = 0x19;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(1 + PLAYER_BANNER_BYTES);
        push_u8(&mut bytes, Self::TYPE);
        bytes.extend_from_slice(&self.banner);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TakePrizeRequest {
    /// Server-issued, generation-safe identity of the collectible being
    /// claimed. This occupies Continuum's legacy timestamp slot, but is never
    /// authored by the client and is not interpreted as time.
    pub(crate) collectible_id: u32,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) prize_id: i16,
}

impl TakePrizeRequest {
    pub(crate) const TYPE: u8 = 0x07;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(11);
        push_u8(&mut bytes, Self::TYPE);
        push_u32(&mut bytes, self.collectible_id);
        push_u16(&mut bytes, self.x);
        push_u16(&mut bytes, self.y);
        push_u16(&mut bytes, self.prize_id as u16);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropBrickRequest {
    pub(crate) x: u16,
    pub(crate) y: u16,
}

impl DropBrickRequest {
    pub(crate) const TYPE: u8 = 0x1c;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(5);
        push_u8(&mut bytes, Self::TYPE);
        push_u16(&mut bytes, self.x);
        push_u16(&mut bytes, self.y);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BallPickupRequest {
    pub(crate) ball_id: u8,
    /// Server-issued object version for the ball being picked up, not a
    /// client-authored timestamp.
    pub(crate) timestamp: u32,
}

impl BallPickupRequest {
    pub(crate) const TYPE: u8 = 0x20;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(6);
        push_u8(&mut bytes, Self::TYPE);
        push_u8(&mut bytes, self.ball_id);
        push_u32(&mut bytes, self.timestamp);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BallFireRequest {
    pub(crate) ball_id: u8,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) velocity_x: i16,
    pub(crate) velocity_y: i16,
    pub(crate) player_id: u16,
    /// Legacy Continuum compatibility slot. Zubspace gameplay builders send
    /// zero and the production server stamps fired balls with server time.
    pub(crate) timestamp: u32,
}

impl BallFireRequest {
    pub(crate) const TYPE: u8 = 0x1f;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16);
        push_u8(&mut bytes, Self::TYPE);
        push_u8(&mut bytes, self.ball_id);
        push_u16(&mut bytes, self.x);
        push_u16(&mut bytes, self.y);
        push_u16(&mut bytes, self.velocity_x as u16);
        push_u16(&mut bytes, self.velocity_y as u16);
        push_u16(&mut bytes, self.player_id);
        push_u32(&mut bytes, self.timestamp);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BallGoalRequest {
    pub(crate) ball_id: u8,
    pub(crate) x: i16,
    pub(crate) y: i16,
}

impl BallGoalRequest {
    pub(crate) const TYPE: u8 = 0x21;

    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(6);
        push_u8(&mut bytes, Self::TYPE);
        push_u8(&mut bytes, self.ball_id);
        push_u16(&mut bytes, self.x as u16);
        push_u16(&mut bytes, self.y as u16);
        bytes
    }
}

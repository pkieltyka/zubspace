use super::*;

fn read_time_sync_u64(packet: &[u8], offset: usize) -> Result<u64, String> {
    let bytes = packet
        .get(offset..offset + 8)
        .ok_or_else(|| "time sync response identity truncated".to_string())?;
    Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
        "invalid time sync response identity".to_string()
    })?))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArenaLoginRequest {
    pub(crate) ship: u8,
    pub(crate) audio: u16,
    pub(crate) xres: u16,
    pub(crate) yres: u16,
    pub(crate) arena_number: u16,
    pub(crate) arena_name: String,
    pub(crate) download_lvz: bool,
}

impl ArenaLoginRequest {
    pub(crate) const TYPE: u8 = 0x01;
    pub(crate) const ARENA_NAME_LEN: usize = 16;

    #[allow(dead_code)]
    pub(crate) fn new(
        ship: u8,
        audio: u16,
        xres: u16,
        yres: u16,
        arena_number: u16,
        arena_name: impl Into<String>,
        download_lvz: bool,
    ) -> Self {
        Self {
            ship,
            audio,
            xres,
            yres,
            arena_number,
            arena_name: arena_name.into(),
            download_lvz,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn spectator_default() -> Self {
        Self::new(8, 0, 1920, 1080, 0xffff, "", true)
    }

    /// Appended after the 27-byte Continuum body: the newest client protocol
    /// revision this build understands. Servers read it from byte 27 and
    /// treat a missing byte as revision 0; older servers ignore it.
    pub(crate) const PROTOCOL_REVISION: u8 = CLIENT_PROTOCOL_POSITION_OFFSETS;
    pub(crate) const PROTOCOL_REVISION_OFFSET: usize = 27;

    #[allow(dead_code)]
    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::PROTOCOL_REVISION_OFFSET + 1);
        push_u8(&mut bytes, Self::TYPE);
        push_u8(&mut bytes, self.ship);
        push_u16(&mut bytes, self.audio);
        push_u16(&mut bytes, self.xres);
        push_u16(&mut bytes, self.yres);
        push_u16(&mut bytes, self.arena_number);
        push_fixed_string(&mut bytes, self.arena_name.as_bytes(), Self::ARENA_NAME_LEN);
        push_u8(&mut bytes, u8::from(self.download_lvz));
        debug_assert_eq!(bytes.len(), Self::PROTOCOL_REVISION_OFFSET);
        push_u8(&mut bytes, Self::PROTOCOL_REVISION);
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum LoginProtocol {
    Subspace,
    Continuum,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeSyncRequest {
    /// Legacy Continuum timestamp slot. Zubspace treats this client-sent
    /// value as an opaque nonce echoed by the server, never as authoritative
    /// time.
    pub(crate) nonce: u32,
    pub(crate) total_sent: u32,
    pub(crate) total_received: u32,
}

impl TimeSyncRequest {
    pub(crate) const CORE_BYTE: u8 = 0x00;
    pub(crate) const TYPE: u8 = 0x05;

    pub(crate) fn encode(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(14);
        push_u8(&mut bytes, Self::CORE_BYTE);
        push_u8(&mut bytes, Self::TYPE);
        push_u32(&mut bytes, self.nonce);
        push_u32(&mut bytes, self.total_sent);
        push_u32(&mut bytes, self.total_received);
        bytes
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeSyncResponse {
    pub(crate) received_timestamp: u32,
    pub(crate) local_timestamp: u32,
    pub(crate) server_session_id: Option<u64>,
    pub(crate) arena_stream_id: Option<u64>,
}

impl TimeSyncResponse {
    pub(crate) const CORE_BYTE: u8 = 0x00;
    pub(crate) const REQUEST_TYPE: u8 = 0x05;
    pub(crate) const RESPONSE_TYPE: u8 = 0x06;
    pub(crate) const REQUEST_LEN: usize = 6;

    pub(crate) fn parse_request(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::REQUEST_LEN
            || packet[0] != Self::CORE_BYTE
            || packet[1] != Self::REQUEST_TYPE
        {
            return Err("invalid time sync request packet".to_string());
        }
        Ok(Self {
            received_timestamp: read_u32(packet, 2)?,
            local_timestamp: 0,
            server_session_id: None,
            arena_stream_id: None,
        })
    }

    pub(crate) fn response_to_request(packet: &[u8], local_timestamp: u32) -> Result<Self, String> {
        let mut response = Self::parse_request(packet)?;
        response.local_timestamp = local_timestamp;
        Ok(response)
    }

    pub(crate) fn parse_response(packet: &[u8]) -> Result<Self, String> {
        if !matches!(packet.len(), 10 | 26)
            || packet[0] != Self::CORE_BYTE
            || packet[1] != Self::RESPONSE_TYPE
        {
            return Err("invalid time sync response packet".to_string());
        }
        let scoped = packet.len() == 26;
        Ok(Self {
            received_timestamp: read_u32(packet, 2)?,
            local_timestamp: read_u32(packet, 6)?,
            server_session_id: scoped.then(|| read_time_sync_u64(packet, 10)).transpose()?,
            arena_stream_id: scoped.then(|| read_time_sync_u64(packet, 18)).transpose()?,
        })
    }

    pub(crate) fn encode(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(if self.server_session_id.is_some() {
            26
        } else {
            10
        });
        push_u8(&mut bytes, Self::CORE_BYTE);
        push_u8(&mut bytes, Self::RESPONSE_TYPE);
        push_u32(&mut bytes, self.received_timestamp);
        push_u32(&mut bytes, self.local_timestamp);
        if let (Some(server_session_id), Some(arena_stream_id)) =
            (self.server_session_id, self.arena_stream_id)
        {
            bytes.extend(server_session_id.to_le_bytes());
            bytes.extend(arena_stream_id.to_le_bytes());
        }
        bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasswordLoginRequest {
    pub(crate) protocol: LoginProtocol,
    pub(crate) registration: bool,
    pub(crate) name: String,
    pub(crate) password: String,
    pub(crate) machine_id: u32,
    pub(crate) timezone_bias: u16,
    pub(crate) remote_ip: u32,
    pub(crate) local_port: u16,
}

impl PasswordLoginRequest {
    pub(crate) const SUBSPACE_TYPE: u8 = 0x09;
    pub(crate) const CONTINUUM_TYPE: u8 = 0x24;
    pub(crate) const CONNECT_TYPE: u8 = 0x04;
    pub(crate) const SUBSPACE_VERSION: u16 = 0x86;
    pub(crate) const CONTINUUM_VERSION: u16 = 40;
    pub(crate) const CONTINUUM_BUILD: u16 = 444;
    pub(crate) const NAME_LEN: usize = 32;
    pub(crate) const PASSWORD_LEN: usize = 32;
    pub(crate) const WATCH_DAMAGE: u16 = 1 << 0;
    pub(crate) const BATCH_POSITIONS: u16 = 1 << 1;
    pub(crate) const WARP_TO: u16 = 1 << 2;
    pub(crate) const LVZ: u16 = 1 << 3;
    pub(crate) const DEFAULT_FEATURES: u16 =
        Self::WATCH_DAMAGE | Self::BATCH_POSITIONS | Self::WARP_TO | Self::LVZ;
    pub(crate) const CONTINUUM_KEY_VALUES: [u32; 16] = [
        0x5a56_42a8,
        0x714c_a252,
        0x4730_da93,
        0x8b2b_a1b9,
        0x7e9e_f009,
        0x5507_282d,
        0xbcc1_8114,
        0x4050_8748,
        0x8827_df38,
        0x4622_33e9,
        0x1f53_998c,
        0xad34_83a6,
        0xb7e7_2494,
        0xfacf_2267,
        0x1fe8_0deb,
        0x63d1_1557,
    ];

    #[allow(dead_code)]
    pub(crate) fn new(
        protocol: LoginProtocol,
        registration: bool,
        name: impl Into<String>,
        password: impl Into<String>,
        machine_id: u32,
        timezone_bias: u16,
        remote_ip: u32,
        local_port: u16,
    ) -> Self {
        Self {
            protocol,
            registration,
            name: name.into(),
            password: password.into(),
            machine_id,
            timezone_bias,
            remote_ip,
            local_port,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn encode_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(match self.protocol {
            LoginProtocol::Subspace => 101,
            LoginProtocol::Continuum => 165,
        });
        let (packet_type, version) = match self.protocol {
            LoginProtocol::Subspace => (Self::SUBSPACE_TYPE, Self::SUBSPACE_VERSION),
            LoginProtocol::Continuum => (Self::CONTINUUM_TYPE, Self::CONTINUUM_VERSION),
        };

        push_u8(&mut bytes, packet_type);
        push_u8(&mut bytes, u8::from(self.registration));
        push_fixed_string(&mut bytes, self.name.as_bytes(), Self::NAME_LEN);
        push_fixed_string(&mut bytes, self.password.as_bytes(), Self::PASSWORD_LEN);
        push_u32(&mut bytes, self.machine_id);
        push_u8(&mut bytes, Self::CONNECT_TYPE);
        push_u16(&mut bytes, self.timezone_bias);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, version);
        push_u16(&mut bytes, Self::CONTINUUM_BUILD);
        push_u16(&mut bytes, Self::DEFAULT_FEATURES);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, self.remote_ip);
        push_u32(&mut bytes, self.local_port as u32);
        push_u32(&mut bytes, 0);

        if self.protocol == LoginProtocol::Continuum {
            for value in Self::CONTINUUM_KEY_VALUES {
                push_u32(&mut bytes, value);
            }
        }

        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswordResponse {
    pub(crate) code: u8,
    pub(crate) registration_form_requested: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordResponseAction {
    SendArenaLogin,
    RequestRegistration,
    WaitForRegistrationForm,
    Failed(u8),
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) const LOGIN_RESPONSE_MESSAGES: [&str; 21] = [
    "Ok",
    "Unregistered player",
    "Bad password",
    "Arena is full",
    "Locked out of zone",
    "Permission only arena",
    "Permission to spectate only",
    "Too many points to play here",
    "Connection is too slow",
    "Permission only arena",
    "Server is full",
    "Invalid name",
    "Offensive name",
    "No active biller",
    "Server busy, try later",
    "Not enough usage to play here",
    "Restricted zone",
    "Demo version detected",
    "Too many demo users",
    "Demo versions not allowed",
    "Restricted zone, mod access required",
];

#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) fn login_response_message(code: u8) -> Option<&'static str> {
    LOGIN_RESPONSE_MESSAGES.get(code as usize).copied()
}

impl PasswordResponse {
    pub(crate) const TYPE: u8 = 0x0a;
    pub(crate) const MIN_PACKET_LEN: usize = 20;
    pub(crate) const RESPONSE_OFFSET: usize = 1;
    pub(crate) const REGISTRATION_REQUEST_OFFSET: usize = 19;

    #[allow(dead_code)]
    pub(crate) fn parse_packet(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::MIN_PACKET_LEN {
            return Err(format!(
                "password response packet too short: {} < {}",
                packet.len(),
                Self::MIN_PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected password response packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            code: packet[Self::RESPONSE_OFFSET],
            registration_form_requested: packet[Self::REGISTRATION_REQUEST_OFFSET] != 0,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn action(self) -> PasswordResponseAction {
        if self.registration_form_requested {
            return PasswordResponseAction::WaitForRegistrationForm;
        }

        match self.code {
            0x00 | 0x0d => PasswordResponseAction::SendArenaLogin,
            0x01 => PasswordResponseAction::RequestRegistration,
            code => PasswordResponseAction::Failed(code),
        }
    }
}

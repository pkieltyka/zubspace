#![allow(dead_code)]

use crate::authority::PositionStatusBits;

pub(crate) const CLIENT_POSITION_TYPE: u8 = 0x03;
pub(crate) const CLIENT_POSITION_MIN_LEN: usize = 22;
pub(crate) const CLIENT_POSITION_EXTRA_LEN: usize = 32;
/// Zubspace current-protocol exact-state extension.  The legacy integer wire
/// fields remain first for Continuum-compatible relays; this mandatory,
/// versioned block binds the precise shared-simulation f32 tail which produced
/// them.  Embedded input history and one-shot markers follow it.
pub(crate) const CLIENT_POSITION_CANONICAL_STATE_TYPE: u8 = 0x78;
pub(crate) const CLIENT_POSITION_CANONICAL_STATE_VERSION: u8 = 1;
pub(crate) const CLIENT_POSITION_CANONICAL_STATE_PAYLOAD_LEN: u8 = 20;
pub(crate) const CLIENT_POSITION_CANONICAL_STATE_OFFSET: usize = CLIENT_POSITION_EXTRA_LEN;
pub(crate) const CLIENT_POSITION_CANONICAL_STATE_LEN: usize = 23;
pub(crate) const CLIENT_POSITION_EMBEDDED_INPUT_OFFSET: usize =
    CLIENT_POSITION_CANONICAL_STATE_OFFSET + CLIENT_POSITION_CANONICAL_STATE_LEN;
pub(crate) const CLIENT_POSITION_EMBEDDED_INPUT_TYPE: u8 = 0x62;
pub(crate) const CLIENT_POSITION_EMBEDDED_INPUT_HEADER_LEN: usize = 4;
pub(crate) const CLIENT_POSITION_EMBEDDED_INPUT_ENTRY_LEN: usize =
    4 + crate::combat_timeline::CombatViewReceiptV1::ENCODED_LEN;
pub(crate) const CLIENT_POSITION_SINGLE_INPUT_RECORD_LEN: usize =
    CLIENT_POSITION_EMBEDDED_INPUT_HEADER_LEN + CLIENT_POSITION_EMBEDDED_INPUT_ENTRY_LEN;
pub(crate) const CLIENT_POSITION_REPEL_MARKER_MAGIC: u16 = 0x5250;
/// Current-protocol exact application boundary for a delivered hostile Repel.
/// The legacy extra fields still identify the snapshot; this trailer also
/// binds the logical input tail after whose movement the browser applied the
/// server-owned velocity replacement.
pub(crate) const CLIENT_POSITION_REPEL_APPLICATION_MARKER_TYPE: u8 = 0x79;
pub(crate) const CLIENT_POSITION_REPEL_APPLICATION_MARKER_VERSION: u8 = 3;
pub(crate) const CLIENT_POSITION_REPEL_APPLICATION_MARKER_LEN: usize = 10;
pub(crate) const CLIENT_POSITION_WARP_MARKER_TYPE: u8 = 0x76;
pub(crate) const CLIENT_POSITION_WARP_MARKER_LEN: usize = 5;
pub(crate) const ZUBSPACE_POSITION_SEQUENCE_MAGIC: u32 = 0x5a5a_0000;
const ZUBSPACE_POSITION_SEQUENCE_MASK: u32 = 0xffff_0000;

/// Raw IEEE-754 identity of one current-protocol client movement tail.  Bits
/// are retained rather than decoded values so `-0`, NaN payloads, and every
/// ordinary finite f32 remain unambiguous at the security boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClientPositionCanonicalState {
    pub(crate) position_x_bits: u32,
    pub(crate) position_y_bits: u32,
    pub(crate) velocity_x_bits: u32,
    pub(crate) velocity_y_bits: u32,
    pub(crate) orientation_bits: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClientRepelApplicationMarker {
    pub(crate) snapshot_id: u16,
    pub(crate) application_sequence: u16,
    pub(crate) delivery_generation: u32,
}

impl ClientPositionCanonicalState {
    pub(crate) fn from_f32(
        position_x: f32,
        position_y: f32,
        velocity_x: f32,
        velocity_y: f32,
        orientation: f32,
    ) -> Self {
        Self {
            position_x_bits: position_x.to_bits(),
            position_y_bits: position_y.to_bits(),
            velocity_x_bits: velocity_x.to_bits(),
            velocity_y_bits: velocity_y.to_bits(),
            orientation_bits: orientation.to_bits(),
        }
    }

    pub(crate) fn values(self) -> (f32, f32, f32, f32, f32) {
        (
            f32::from_bits(self.position_x_bits),
            f32::from_bits(self.position_y_bits),
            f32::from_bits(self.velocity_x_bits),
            f32::from_bits(self.velocity_y_bits),
            f32::from_bits(self.orientation_bits),
        )
    }

    /// Test-fixture convenience only. Production clients always capture the
    /// already-existing simulation floats and never infer them from wire
    /// fields.
    #[cfg(test)]
    pub(crate) fn from_legacy_wire(
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> Self {
        Self::from_f32(
            x as f32,
            y as f32,
            protocol_velocity_pixels_per_second(velocity_x),
            protocol_velocity_pixels_per_second(velocity_y),
            protocol_direction_to_orientation(direction),
        )
    }

    pub(crate) fn is_finite_current_state(self) -> bool {
        let (x, y, velocity_x, velocity_y, orientation) = self.values();
        x.is_finite()
            && y.is_finite()
            && velocity_x.is_finite()
            && velocity_y.is_finite()
            && orientation.is_finite()
            && x >= 0.0
            && y >= 0.0
            // The reference game's ordered Left branch adds `1.0f` after a negative
            // subtraction.  When the residue is tiny that f32 addition can
            // round to exactly 1.0.  It is a valid raw canonical state whose
            // wrapped legacy direction is zero, not a malformed sub-bin.
            && (0.0..=1.0).contains(&orientation)
    }

    pub(crate) fn matches_legacy_wire(
        self,
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) -> bool {
        let (exact_x, exact_y, exact_velocity_x, exact_velocity_y, exact_orientation) =
            self.values();
        self.is_finite_current_state()
            && protocol_position_coord(exact_x) == x
            && protocol_position_coord(exact_y) == y
            && protocol_velocity_component(exact_velocity_x) == velocity_x
            && protocol_velocity_component(exact_velocity_y) == velocity_y
            && protocol_direction(exact_orientation) == direction
    }

    pub(crate) fn encode(self, bytes: &mut Vec<u8>) {
        bytes.push(CLIENT_POSITION_CANONICAL_STATE_TYPE);
        bytes.push(CLIENT_POSITION_CANONICAL_STATE_VERSION);
        bytes.push(CLIENT_POSITION_CANONICAL_STATE_PAYLOAD_LEN);
        bytes.extend_from_slice(&self.position_x_bits.to_le_bytes());
        bytes.extend_from_slice(&self.position_y_bits.to_le_bytes());
        bytes.extend_from_slice(&self.velocity_x_bits.to_le_bytes());
        bytes.extend_from_slice(&self.velocity_y_bits.to_le_bytes());
        bytes.extend_from_slice(&self.orientation_bits.to_le_bytes());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClientPositionSnapshot {
    pub(crate) direction: u8,
    /// Legacy Continuum timestamp field. Zubspace clients do not use this as
    /// time; authoritative timing is server-owned.
    pub(crate) timestamp: u32,
    /// Zubspace logical input/position sequence carried in the optional
    /// extended position tail. Used only to reject duplicates/replays.
    pub(crate) client_sequence: Option<u16>,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) velocity_x: i16,
    pub(crate) velocity_y: i16,
    pub(crate) togglables: u8,
    pub(crate) bounty: u16,
    pub(crate) energy: u16,
    pub(crate) weapon: u16,
    /// Mandatory current-protocol exact f32 identity.  Admission never
    /// reconstructs hidden floats from the integer wire fields.
    pub(crate) canonical_state: ClientPositionCanonicalState,
    /// Exact server snapshot whose repel weapon the client has applied. The
    /// existing extended position latency/flag fields carry the id and magic
    /// marker so this observation arrives atomically with the resulting
    /// position and velocity.
    pub(crate) repel_snapshot_id: Option<u16>,
    /// Exact logical tail after whose movement the delivered Repel was
    /// applied. It is required whenever `repel_snapshot_id` is present.
    pub(crate) repel_application_sequence: Option<u16>,
    /// Session-scoped monotonic identity of the target-private Repel
    /// delivery. Snapshot ids alone wrap and may be reused.
    pub(crate) repel_delivery_generation: Option<u32>,
    /// Exact server collectible ACK whose positive Warp relocation produced
    /// this report. It is an explicit one-shot origin marker, never inferred
    /// from spawn coordinates or saturated bounty/energy fields.
    pub(crate) warp_collectible_id: Option<u32>,
}

pub(crate) fn parse_client_position_snapshot(packet: &[u8]) -> Option<ClientPositionSnapshot> {
    if packet.len() < CLIENT_POSITION_EMBEDDED_INPUT_OFFSET || packet[0] != CLIENT_POSITION_TYPE {
        return None;
    }
    if !client_position_trailer_is_well_formed(packet) {
        return None;
    }

    let extra_items = if packet.len() >= CLIENT_POSITION_EXTRA_LEN {
        Some(u32::from_le_bytes(packet[28..32].try_into().ok()?))
    } else {
        None
    };
    let legacy_repel_snapshot_id = if packet.len() >= CLIENT_POSITION_EXTRA_LEN
        && u16::from_le_bytes(packet[26..28].try_into().ok()?) == CLIENT_POSITION_REPEL_MARKER_MAGIC
    {
        Some(u16::from_le_bytes(packet[24..26].try_into().ok()?))
    } else {
        None
    };
    let repel_application = client_position_repel_application_marker(packet);
    if repel_application.map(|marker| marker.snapshot_id) != legacy_repel_snapshot_id {
        return None;
    }
    let repel_snapshot_id = repel_application.map(|marker| marker.snapshot_id);
    let repel_application_sequence = repel_application.map(|marker| marker.application_sequence);
    let repel_delivery_generation = repel_application.map(|marker| marker.delivery_generation);
    let warp_collectible_id = client_position_warp_marker(packet);
    let canonical_state = client_position_canonical_state(packet)?;

    let direction = packet[1];
    let velocity_x = i16::from_le_bytes(packet[6..8].try_into().ok()?);
    let y = u16::from_le_bytes(packet[8..10].try_into().ok()?);
    let x = u16::from_le_bytes(packet[12..14].try_into().ok()?);
    let velocity_y = i16::from_le_bytes(packet[14..16].try_into().ok()?);
    if !canonical_state.matches_legacy_wire(direction, x, y, velocity_x, velocity_y) {
        return None;
    }

    Some(ClientPositionSnapshot {
        direction,
        timestamp: u32::from_le_bytes(packet[2..6].try_into().ok()?),
        client_sequence: extra_items.and_then(zubspace_position_sequence_from_items),
        velocity_x,
        y,
        togglables: packet[11],
        x,
        velocity_y,
        bounty: u16::from_le_bytes(packet[16..18].try_into().ok()?),
        energy: u16::from_le_bytes(packet[18..20].try_into().ok()?),
        weapon: u16::from_le_bytes(packet[20..22].try_into().ok()?),
        canonical_state,
        repel_snapshot_id,
        repel_application_sequence,
        repel_delivery_generation,
        warp_collectible_id,
    })
}

pub(crate) fn client_position_trailer_is_well_formed(packet: &[u8]) -> bool {
    if packet.len() < CLIENT_POSITION_EMBEDDED_INPUT_OFFSET
        || client_position_canonical_state(packet).is_none()
    {
        return false;
    }
    client_position_marker_tail(packet).is_some()
}

pub(crate) fn client_position_canonical_state(
    packet: &[u8],
) -> Option<ClientPositionCanonicalState> {
    client_position_canonical_state_at(packet, CLIENT_POSITION_CANONICAL_STATE_OFFSET)
}

pub(crate) fn client_position_canonical_state_at(
    packet: &[u8],
    offset: usize,
) -> Option<ClientPositionCanonicalState> {
    let end = offset.checked_add(CLIENT_POSITION_CANONICAL_STATE_LEN)?;
    if packet.len() < end
        || packet.get(offset).copied() != Some(CLIENT_POSITION_CANONICAL_STATE_TYPE)
        || packet.get(offset + 1).copied() != Some(CLIENT_POSITION_CANONICAL_STATE_VERSION)
        || packet.get(offset + 2).copied() != Some(CLIENT_POSITION_CANONICAL_STATE_PAYLOAD_LEN)
    {
        return None;
    }
    let read = |start: usize| {
        packet
            .get(start..start + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let state = ClientPositionCanonicalState {
        position_x_bits: read(offset + 3)?,
        position_y_bits: read(offset + 7)?,
        velocity_x_bits: read(offset + 11)?,
        velocity_y_bits: read(offset + 15)?,
        orientation_bits: read(offset + 19)?,
    };
    state.is_finite_current_state().then_some(state)
}

pub(crate) fn embedded_client_input_record(packet: &[u8]) -> Option<&[u8]> {
    let offset = CLIENT_POSITION_EMBEDDED_INPUT_OFFSET;
    if packet.len() <= offset || packet[offset] != CLIENT_POSITION_EMBEDDED_INPUT_TYPE {
        return None;
    }
    let count = usize::from(*packet.get(offset + 3)?);
    let input_len = CLIENT_POSITION_EMBEDDED_INPUT_HEADER_LEN
        .checked_add(count.checked_mul(CLIENT_POSITION_EMBEDDED_INPUT_ENTRY_LEN)?)?;
    let input_end = offset.checked_add(input_len)?;
    (input_end <= packet.len()).then_some(&packet[offset..input_end])
}

fn client_position_marker_tail(
    packet: &[u8],
) -> Option<(Option<ClientRepelApplicationMarker>, Option<u32>)> {
    let base = CLIENT_POSITION_EMBEDDED_INPUT_OFFSET;
    let mut cursor = base;
    if packet.get(cursor).copied() == Some(CLIENT_POSITION_EMBEDDED_INPUT_TYPE) {
        let count = usize::from(*packet.get(cursor + 3)?);
        cursor = cursor.checked_add(
            CLIENT_POSITION_EMBEDDED_INPUT_HEADER_LEN
                + count.checked_mul(CLIENT_POSITION_EMBEDDED_INPUT_ENTRY_LEN)?,
        )?;
        if cursor > packet.len() {
            return None;
        }
    }

    let mut repel = None;
    if packet.get(cursor).copied() == Some(CLIENT_POSITION_REPEL_APPLICATION_MARKER_TYPE) {
        let end = cursor.checked_add(CLIENT_POSITION_REPEL_APPLICATION_MARKER_LEN)?;
        if packet.get(cursor + 1).copied() != Some(CLIENT_POSITION_REPEL_APPLICATION_MARKER_VERSION)
        {
            return None;
        }
        let snapshot_id = u16::from_le_bytes(packet.get(cursor + 2..cursor + 4)?.try_into().ok()?);
        let application_sequence =
            u16::from_le_bytes(packet.get(cursor + 4..cursor + 6)?.try_into().ok()?);
        let delivery_generation =
            u32::from_le_bytes(packet.get(cursor + 6..cursor + 10)?.try_into().ok()?);
        if delivery_generation == 0 {
            return None;
        }
        repel = Some(ClientRepelApplicationMarker {
            snapshot_id,
            application_sequence,
            delivery_generation,
        });
        cursor = end;
    }

    let mut warp = None;
    if packet.get(cursor).copied() == Some(CLIENT_POSITION_WARP_MARKER_TYPE) {
        let end = cursor.checked_add(CLIENT_POSITION_WARP_MARKER_LEN)?;
        let collectible_id = u32::from_le_bytes(packet.get(cursor + 1..end)?.try_into().ok()?);
        if collectible_id == 0 {
            return None;
        }
        warp = Some(collectible_id);
        cursor = end;
    }

    (cursor == packet.len()).then_some((repel, warp))
}

pub(crate) fn client_position_repel_application_marker(
    packet: &[u8],
) -> Option<ClientRepelApplicationMarker> {
    client_position_marker_tail(packet)?.0
}

pub(crate) fn client_position_warp_marker(packet: &[u8]) -> Option<u32> {
    client_position_marker_tail(packet)?.1
}

pub(crate) fn zubspace_position_sequence_items(sequence: u16) -> u32 {
    ZUBSPACE_POSITION_SEQUENCE_MAGIC | u32::from(sequence)
}

pub(crate) fn zubspace_position_sequence_from_items(items: u32) -> Option<u16> {
    ((items & ZUBSPACE_POSITION_SEQUENCE_MASK) == ZUBSPACE_POSITION_SEQUENCE_MAGIC)
        .then_some((items & 0xffff) as u16)
}

pub(crate) fn large_position_world_coordinates(packet: &[u8]) -> Option<(u16, u16)> {
    if packet.len() < 17 {
        return None;
    }
    Some((
        u16::from_le_bytes(packet[4..6].try_into().ok()?),
        u16::from_le_bytes(packet[15..17].try_into().ok()?),
    ))
}

pub(crate) fn world_coordinates_in_bounds(
    x: u16,
    y: u16,
    map_tiles: usize,
    tile_size: f32,
) -> bool {
    let max_world = map_tiles as f32 * tile_size;
    (x as f32) < max_world && (y as f32) < max_world
}

/// Continuum stores velocity components on the wire as `i16` in tenths of a
/// pixel/second. This decoder and its inverse `protocol_velocity_component`
/// live together so encoder/decoder can never use different scales.
pub(crate) fn protocol_velocity_pixels_per_second(component: i16) -> f32 {
    component as f32 / 10.0
}

/// Encode an exact shared-simulation position component into the legacy
/// Continuum rounded u16 coordinate carried beside the raw f32 provenance.
pub(crate) fn protocol_position_coord(value: f32) -> u16 {
    value.round().clamp(0.0, u16::MAX as f32) as u16
}

/// Encode an orientation turn fraction into Continuum's 0..39 simulation and
/// wire direction. The reference game casts `orientation * 40.0f` to `u8`; valid live
/// orientation is already wrapped, and normalizing first gives deterministic
/// wrap-safe behavior for diagnostic/untrusted callers without changing that
/// truncating 40-bin contract.
#[allow(dead_code)]
pub(crate) fn protocol_direction(orientation: f32) -> u8 {
    let scaled = orientation.rem_euclid(1.0) * 40.0;
    (scaled.trunc() as u8) % 40
}

/// Decode a 0..39 wire direction byte back into a 0..1 orientation fraction.
#[allow(dead_code)]
pub(crate) fn protocol_direction_to_orientation(direction: u8) -> f32 {
    (direction as f32 / 40.0).rem_euclid(1.0)
}

/// The reference game's `OrientationToHeading(u8)` verbatim. The slightly shortened
/// `3.14159f` constant is intentional and its f32 output is simulation state;
/// replacing it with `PI` changes thrust, recoil, and projectile wire tails.
/// Keep the reference-produced f32 bit table instead of evaluating libm at
/// runtime: native and WASM `sin`/`cos` are not required to round identically,
/// while these values cross the client/server authority boundary.
pub(crate) fn protocol_direction_heading_components(direction: u8) -> (f32, f32) {
    const REFERENCE_GAME_HEADING_BITS: [(u32, u32); 40] = [
        (0x359a2217, 0xbf800000),
        (0x3e2030a4, 0xbf7cd922),
        (0x3e9e3797, 0xbf73786c),
        (0x3ee8718b, 0xbf6418fa),
        (0x3f167921, 0xbf4f1bb6),
        (0x3f3504fa, 0xbf3504ec),
        (0x3f4f1bc1, 0xbf167912),
        (0x3f641904, 0xbee87167),
        (0x3f737872, 0xbe9e3772),
        (0x3f7cd925, 0xbe203054),
        (0x3f800000, 0x80000000),
        (0x3f7cd925, 0x3e203054),
        (0x3f737872, 0x3e9e3772),
        (0x3f641904, 0x3ee87167),
        (0x3f4f1bc1, 0x3f167912),
        (0x3f3504fa, 0x3f3504ec),
        (0x3f167921, 0x3f4f1bb6),
        (0x3ee8718b, 0x3f6418fa),
        (0x3e9e3797, 0x3f73786c),
        (0x3e2030a4, 0x3f7cd922),
        (0x359a2217, 0x3f800000),
        (0xbe203004, 0x3f7cd928),
        (0xbe9e374e, 0x3f737878),
        (0xbee87143, 0x3f64190d),
        (0xbf167901, 0x3f4f1bce),
        (0xbf3504df, 0x3f350507),
        (0xbf4f1bab, 0x3f167931),
        (0xbf6418f2, 0x3ee871ad),
        (0xbf737865, 0x3e9e37c0),
        (0xbf7cd91e, 0x3e2030f8),
        (0xbf800000, 0x361a2217),
        (0xbf7cd92b, 0xbe202fb7),
        (0xbf73787e, 0xbe9e3725),
        (0xbf641916, 0xbee8711d),
        (0xbf4f1bd8, 0xbf1678f3),
        (0xbf350516, 0xbf3504d0),
        (0xbf167942, 0xbf4f1b9e),
        (0xbee871cc, 0xbf6418ea),
        (0xbe9e37e8, 0xbf73785f),
        (0xbe20313c, 0xbf7cd91c),
    ];
    let (x, y) = REFERENCE_GAME_HEADING_BITS[usize::from(direction % 40)];
    (f32::from_bits(x), f32::from_bits(y))
}

/// Canonical simulation heading for a continuous presentation orientation.
pub(crate) fn canonical_discrete_heading_components(orientation: f32) -> (f32, f32) {
    protocol_direction_heading_components(protocol_direction(orientation))
}

/// The reference game's canonical carried-ball anchor. Player position is first
/// pixel-rounded (a truncation in pixel space), then the ball is moved along
/// the discrete ship heading by the ship radius less one quarter tile.
pub(crate) fn canonical_carried_powerball_position(
    position_x: f32,
    position_y: f32,
    orientation: f32,
    ship_radius_pixels: f32,
) -> (f32, f32) {
    let (heading_x, heading_y) = canonical_discrete_heading_components(orientation);
    let extension = (ship_radius_pixels - 4.0).max(0.0);
    (
        position_x.trunc() + heading_x * extension,
        position_y.trunc() + heading_y * extension,
    )
}

/// Map a 0..1 orientation onto unit-vector heading components. Returns
/// `(x, y)` separately so callers without `macroquad::Vec2` (e.g. the
/// server) can use the same math. `math::orientation_to_heading` wraps
/// this into a `Vec2`.
pub(crate) fn orientation_to_heading_components(orientation: f32) -> (f32, f32) {
    let angle = orientation * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
    (angle.cos(), angle.sin())
}

/// Outward-radius spiral search for a spawn tile. The caller supplies a
/// closure returning `true` if a tile (or its 8-neighbour clearance) is
/// blocked. Identical search order on client and server prevents spawn
/// position desync between zubspace and Continuum and the reference game's expectations.
///
/// Iterates rings of increasing radius around `(center_x, center_y)`.
/// Each ring inspects only its border tiles. Returns the first
/// non-blocked `(x, y)` found, or `None` if the entire `max_radius`
/// region is blocked.
#[allow(dead_code)]
pub(crate) fn find_spawn_tile<F>(
    center_x: i32,
    center_y: i32,
    max_radius: i32,
    mut is_blocked: F,
) -> Option<(i32, i32)>
where
    F: FnMut(i32, i32) -> bool,
{
    for radius in 0..=max_radius {
        for y in center_y - radius..=center_y + radius {
            for x in center_x - radius..=center_x + radius {
                if radius > 0
                    && x != center_x - radius
                    && x != center_x + radius
                    && y != center_y - radius
                    && y != center_y + radius
                {
                    // Only consider the border tiles of each ring; interior
                    // tiles were checked at smaller radii.
                    continue;
                }
                if !is_blocked(x, y) {
                    return Some((x, y));
                }
            }
        }
    }
    None
}

/// Inverse of `protocol_velocity_pixels_per_second`: encode a px/s velocity
/// into the i16 wire field with proper rounding and saturation.
#[allow(dead_code)]
pub(crate) fn protocol_velocity_component(value: f32) -> i16 {
    (value * 10.0)
        .round()
        .clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

pub(crate) fn reported_velocity_within_limit(
    snapshot: ClientPositionSnapshot,
    max_speed_px_per_second: f32,
    allowance_px_per_second: f32,
) -> bool {
    let velocity_x = protocol_velocity_pixels_per_second(snapshot.velocity_x);
    let velocity_y = protocol_velocity_pixels_per_second(snapshot.velocity_y);
    velocity_x.hypot(velocity_y) <= max_speed_px_per_second + allowance_px_per_second
}

pub(crate) fn reported_velocity_delta_within_limit(
    previous_velocity_x: i16,
    previous_velocity_y: i16,
    snapshot: ClientPositionSnapshot,
    elapsed_ticks: u32,
    max_acceleration_px_per_second_squared: f32,
    grace_px_per_second: f32,
) -> bool {
    let elapsed_seconds = crate::continuum::ticks_to_seconds_u32(elapsed_ticks.max(1));
    let previous_x = protocol_velocity_pixels_per_second(previous_velocity_x);
    let previous_y = protocol_velocity_pixels_per_second(previous_velocity_y);
    let velocity_x = protocol_velocity_pixels_per_second(snapshot.velocity_x);
    let velocity_y = protocol_velocity_pixels_per_second(snapshot.velocity_y);
    let max_delta = max_acceleration_px_per_second_squared.max(0.0) * elapsed_seconds
        + grace_px_per_second.max(0.0);
    (velocity_x - previous_x).hypot(velocity_y - previous_y) <= max_delta
}

pub(crate) fn reported_energy_within_limit(
    snapshot: ClientPositionSnapshot,
    max_energy: u16,
    authoritative_energy: u16,
) -> bool {
    snapshot.energy <= max_energy.max(authoritative_energy)
}

pub(crate) fn reported_position_delta_within_limit(
    previous_x: u16,
    previous_y: u16,
    snapshot: ClientPositionSnapshot,
    elapsed_ticks: u32,
    max_speed_px_per_second: f32,
    allowance_px_per_second: f32,
    grace_px: f32,
) -> bool {
    let elapsed_seconds = crate::continuum::ticks_to_seconds_u32(elapsed_ticks.max(1));
    let max_distance =
        (max_speed_px_per_second + allowance_px_per_second) * elapsed_seconds + grace_px.max(0.0);
    let dx = snapshot.x as f32 - previous_x as f32;
    let dy = snapshot.y as f32 - previous_y as f32;
    dx.hypot(dy) <= max_distance
}

pub(crate) fn reported_position_path_crosses_blocked_tile(
    start_x: u16,
    start_y: u16,
    end_x: u16,
    end_y: u16,
    tile_size: f32,
    mut blocked_at_world: impl FnMut(u16, u16) -> bool,
) -> bool {
    let dx = end_x as f32 - start_x as f32;
    let dy = end_y as f32 - start_y as f32;
    let distance = dx.hypot(dy);
    if tile_size <= 0.0 || distance <= tile_size * 0.5 {
        return false;
    }

    let steps = (distance / (tile_size * 0.5)).ceil() as u16;
    for step in 1..steps {
        let t = step as f32 / steps as f32;
        let sample_x = (start_x as f32 + dx * t)
            .round()
            .clamp(0.0, u16::MAX as f32) as u16;
        let sample_y = (start_y as f32 + dy * t)
            .round()
            .clamp(0.0, u16::MAX as f32) as u16;
        if blocked_at_world(sample_x, sample_y) {
            return true;
        }
    }
    false
}

pub(crate) fn reported_position_path_hull_overlaps_blocked_tile(
    start_x: u16,
    start_y: u16,
    end_x: u16,
    end_y: u16,
    radius_px: f32,
    map_tiles: usize,
    tile_size: f32,
    mut blocked_tile: impl FnMut(usize, usize) -> bool,
) -> bool {
    let dx = end_x as f32 - start_x as f32;
    let dy = end_y as f32 - start_y as f32;
    let distance = dx.hypot(dy);
    if tile_size <= 0.0 || distance <= tile_size * 0.5 {
        return false;
    }

    let steps = (distance / (tile_size * 0.5)).ceil() as u16;
    for step in 1..steps {
        let t = step as f32 / steps as f32;
        let sample_x = (start_x as f32 + dx * t)
            .round()
            .clamp(0.0, u16::MAX as f32) as u16;
        let sample_y = (start_y as f32 + dy * t)
            .round()
            .clamp(0.0, u16::MAX as f32) as u16;
        if reported_position_hull_overlaps_blocked_tile(
            sample_x,
            sample_y,
            radius_px,
            map_tiles,
            tile_size,
            |tile_x, tile_y| blocked_tile(tile_x, tile_y),
        ) {
            return true;
        }
    }
    false
}

pub(crate) fn reported_position_hull_overlaps_blocked_tile(
    x: u16,
    y: u16,
    radius_px: f32,
    map_tiles: usize,
    tile_size: f32,
    mut blocked_tile: impl FnMut(usize, usize) -> bool,
) -> bool {
    if radius_px <= 0.0 || tile_size <= 0.0 || map_tiles == 0 {
        return false;
    }

    let arena_px = map_tiles as f32 * tile_size;
    let center_x = x as f32;
    let center_y = y as f32;
    let ship_min_x = center_x - radius_px;
    let ship_min_y = center_y - radius_px;
    let ship_max_x = center_x + radius_px;
    let ship_max_y = center_y + radius_px;
    if ship_min_x < 0.0 || ship_min_y < 0.0 || ship_max_x >= arena_px || ship_max_y >= arena_px {
        return true;
    }

    let min_tile_x = (ship_min_x / tile_size).floor().max(0.0) as usize;
    let min_tile_y = (ship_min_y / tile_size).floor().max(0.0) as usize;
    let max_tile_x = (ship_max_x / tile_size).floor().min((map_tiles - 1) as f32) as usize;
    let max_tile_y = (ship_max_y / tile_size).floor().min((map_tiles - 1) as f32) as usize;

    for tile_y in min_tile_y..=max_tile_y {
        for tile_x in min_tile_x..=max_tile_x {
            if !blocked_tile(tile_x, tile_y) {
                continue;
            }
            let tile_min_x = tile_x as f32 * tile_size;
            let tile_min_y = tile_y as f32 * tile_size;
            let tile_max_x = tile_min_x + tile_size;
            let tile_max_y = tile_min_y + tile_size;
            let closest_x = center_x.clamp(tile_min_x, tile_max_x);
            let closest_y = center_y.clamp(tile_min_y, tile_max_y);
            let dx = center_x - closest_x;
            let dy = center_y - closest_y;
            if dx * dx + dy * dy <= radius_px * radius_px {
                return true;
            }
        }
    }

    false
}

pub(crate) fn sanitized_position_togglables(reported: u8, in_safe_tile: bool) -> u8 {
    PositionStatusBits::from_reported_position(reported, in_safe_tile).bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_game_direction_uses_truncating_wrap_safe_f32_bins() {
        assert_eq!(protocol_direction(0.0), 0);
        assert_eq!(protocol_direction(0.024), 0);
        assert_eq!(protocol_direction(0.025), 1);
        assert_eq!(protocol_direction(0.999), 39);
        assert_eq!(protocol_direction(1.0), 0);
        assert_eq!(protocol_direction(-0.001), 39);

        // A one-ULP-lower f32 can still multiply back onto the integer bin
        // boundary. This table is the reference game's direct float-cast result, not an
        // ideal-real-number assumption about `next_down`.
        let below_golden = [
            0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 17, 17, 18, 19, 20, 21, 22,
            23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 34, 34, 35, 37, 37, 39,
        ];
        for direction in 0_u8..40 {
            let boundary = direction as f32 / 40.0;
            assert_eq!(
                protocol_direction(boundary),
                direction,
                "boundary {direction}"
            );
            if direction > 0 {
                let below = f32::from_bits(boundary.to_bits() - 1);
                assert_eq!(
                    protocol_direction(below),
                    below_golden[direction as usize],
                    "one f32 ULP below boundary {direction}",
                );
            }
        }
        assert_eq!(protocol_direction_to_orientation(39), 0.975);
        assert_eq!(protocol_direction(1.0 + 39.0 / 40.0), 39);
        assert_eq!(protocol_direction(-1.0 / 40.0), 39);
        assert_eq!(protocol_direction(39.0 / 40.0), 39);
        assert_eq!(protocol_direction(40.0 / 40.0), 0, "39 -> 0 wrap");
    }

    #[test]
    fn reference_game_orientation_to_heading_golden_table_is_bit_exact() {
        // Generated once from the reference game's Math `OrientationToHeading` using
        // the reference compiler/libm. This is independent of the Rust helper
        // and pins the intentional 3.14159f constant for all 40 headings.
        let golden: [(u32, u32); 40] = [
            (0x359a2217, 0xbf800000),
            (0x3e2030a4, 0xbf7cd922),
            (0x3e9e3797, 0xbf73786c),
            (0x3ee8718b, 0xbf6418fa),
            (0x3f167921, 0xbf4f1bb6),
            (0x3f3504fa, 0xbf3504ec),
            (0x3f4f1bc1, 0xbf167912),
            (0x3f641904, 0xbee87167),
            (0x3f737872, 0xbe9e3772),
            (0x3f7cd925, 0xbe203054),
            (0x3f800000, 0x80000000),
            (0x3f7cd925, 0x3e203054),
            (0x3f737872, 0x3e9e3772),
            (0x3f641904, 0x3ee87167),
            (0x3f4f1bc1, 0x3f167912),
            (0x3f3504fa, 0x3f3504ec),
            (0x3f167921, 0x3f4f1bb6),
            (0x3ee8718b, 0x3f6418fa),
            (0x3e9e3797, 0x3f73786c),
            (0x3e2030a4, 0x3f7cd922),
            (0x359a2217, 0x3f800000),
            (0xbe203004, 0x3f7cd928),
            (0xbe9e374e, 0x3f737878),
            (0xbee87143, 0x3f64190d),
            (0xbf167901, 0x3f4f1bce),
            (0xbf3504df, 0x3f350507),
            (0xbf4f1bab, 0x3f167931),
            (0xbf6418f2, 0x3ee871ad),
            (0xbf737865, 0x3e9e37c0),
            (0xbf7cd91e, 0x3e2030f8),
            (0xbf800000, 0x361a2217),
            (0xbf7cd92b, 0xbe202fb7),
            (0xbf73787e, 0xbe9e3725),
            (0xbf641916, 0xbee8711d),
            (0xbf4f1bd8, 0xbf1678f3),
            (0xbf350516, 0xbf3504d0),
            (0xbf167942, 0xbf4f1b9e),
            (0xbee871cc, 0xbf6418ea),
            (0xbe9e37e8, 0xbf73785f),
            (0xbe20313c, 0xbf7cd91c),
        ];
        for (direction, expected) in golden.into_iter().enumerate() {
            let (x, y) = protocol_direction_heading_components(direction as u8);
            assert_eq!(
                (x.to_bits(), y.to_bits()),
                expected,
                "direction {direction}"
            );
            let orientation = protocol_direction_to_orientation(direction as u8);
            assert_eq!(
                canonical_discrete_heading_components(orientation),
                (x, y),
                "orientation adapter {direction}",
            );
        }
    }
    use crate::authority::{
        POSITION_STATUS_RELAY_MASK, POSITION_STATUS_SAFETY, POSITION_STATUS_STEALTH,
    };

    fn append_test_canonical_state(
        packet: &mut Vec<u8>,
        direction: u8,
        x: u16,
        y: u16,
        velocity_x: i16,
        velocity_y: i16,
    ) {
        packet.resize(CLIENT_POSITION_EXTRA_LEN, 0);
        ClientPositionCanonicalState::from_legacy_wire(direction, x, y, velocity_x, velocity_y)
            .encode(packet);
    }

    #[test]
    fn client_position_snapshot_parses_continuum_layout() {
        let mut packet = vec![0; CLIENT_POSITION_MIN_LEN];
        packet[0] = CLIENT_POSITION_TYPE;
        packet[1] = 7;
        packet[2..6].copy_from_slice(&0x1122_3344_u32.to_le_bytes());
        packet[6..8].copy_from_slice(&(-32_i16).to_le_bytes());
        packet[8..10].copy_from_slice(&700_u16.to_le_bytes());
        packet[11] = 0x55;
        packet[12..14].copy_from_slice(&500_u16.to_le_bytes());
        packet[14..16].copy_from_slice(&64_i16.to_le_bytes());
        packet[16..18].copy_from_slice(&3_u16.to_le_bytes());
        packet[18..20].copy_from_slice(&900_u16.to_le_bytes());
        packet[20..22].copy_from_slice(&0x83_u16.to_le_bytes());
        append_test_canonical_state(&mut packet, 7, 500, 700, -32, 64);

        let snapshot = parse_client_position_snapshot(&packet).expect("position should parse");

        assert_eq!(
            snapshot,
            ClientPositionSnapshot {
                direction: 7,
                timestamp: 0x1122_3344,
                client_sequence: None,
                x: 500,
                y: 700,
                velocity_x: -32,
                velocity_y: 64,
                togglables: 0x55,
                bounty: 3,
                energy: 900,
                weapon: 0x83,
                canonical_state: ClientPositionCanonicalState::from_legacy_wire(
                    7, 500, 700, -32, 64,
                ),
                repel_snapshot_id: None,
                repel_application_sequence: None,
                repel_delivery_generation: None,
                warp_collectible_id: None,
            }
        );
    }

    #[test]
    fn current_position_protocol_requires_well_formed_finite_exact_state_identity() {
        let mut packet = vec![0; CLIENT_POSITION_EXTRA_LEN];
        packet[0] = CLIENT_POSITION_TYPE;
        packet[1] = 4;
        packet[6..8].copy_from_slice(&(-120_i16).to_le_bytes());
        packet[8..10].copy_from_slice(&700_u16.to_le_bytes());
        packet[12..14].copy_from_slice(&500_u16.to_le_bytes());
        packet[14..16].copy_from_slice(&240_i16.to_le_bytes());
        let exact = ClientPositionCanonicalState::from_f32(500.25, 699.75, -12.0, 24.0, 0.1);
        exact.encode(&mut packet);
        assert_eq!(
            parse_client_position_snapshot(&packet)
                .expect("matching exact block")
                .canonical_state,
            exact,
        );

        assert!(parse_client_position_snapshot(&packet[..CLIENT_POSITION_EXTRA_LEN]).is_none());
        for (index, value) in [
            (CLIENT_POSITION_CANONICAL_STATE_OFFSET, 0xff),
            (CLIENT_POSITION_CANONICAL_STATE_OFFSET + 1, 2),
            (CLIENT_POSITION_CANONICAL_STATE_OFFSET + 2, 19),
        ] {
            let mut malformed = packet.clone();
            malformed[index] = value;
            assert!(parse_client_position_snapshot(&malformed).is_none());
        }

        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut malformed = packet.clone();
            malformed[CLIENT_POSITION_CANONICAL_STATE_OFFSET + 3
                ..CLIENT_POSITION_CANONICAL_STATE_OFFSET + 7]
                .copy_from_slice(&invalid.to_bits().to_le_bytes());
            assert!(parse_client_position_snapshot(&malformed).is_none());
        }
        let mut mismatched = packet.clone();
        mismatched[CLIENT_POSITION_CANONICAL_STATE_OFFSET + 3
            ..CLIENT_POSITION_CANONICAL_STATE_OFFSET + 7]
            .copy_from_slice(&500.75_f32.to_bits().to_le_bytes());
        assert!(parse_client_position_snapshot(&mismatched).is_none());

        let mut negative_zero = vec![0; CLIENT_POSITION_EXTRA_LEN];
        negative_zero[0] = CLIENT_POSITION_TYPE;
        let signed_zero = ClientPositionCanonicalState::from_f32(-0.0, 0.0, -0.0, 0.0, -0.0);
        signed_zero.encode(&mut negative_zero);
        assert_eq!(
            parse_client_position_snapshot(&negative_zero)
                .expect("signed zero is finite and its raw identity is preserved")
                .canonical_state,
            signed_zero,
        );

        let mut left_wrap_one = vec![0; CLIENT_POSITION_EXTRA_LEN];
        left_wrap_one[0] = CLIENT_POSITION_TYPE;
        let exact_one = ClientPositionCanonicalState::from_f32(0.0, 0.0, 0.0, 0.0, 1.0);
        exact_one.encode(&mut left_wrap_one);
        assert_eq!(
            parse_client_position_snapshot(&left_wrap_one)
                .expect("the reference game's Left can produce exact 1.0 with wrapped wire direction zero")
                .canonical_state,
            exact_one,
        );
        for invalid_orientation in [f32::from_bits(1.0_f32.to_bits() + 1), -f32::EPSILON] {
            let mut malformed = left_wrap_one.clone();
            malformed[CLIENT_POSITION_CANONICAL_STATE_OFFSET + 19
                ..CLIENT_POSITION_CANONICAL_STATE_OFFSET + 23]
                .copy_from_slice(&invalid_orientation.to_bits().to_le_bytes());
            assert!(parse_client_position_snapshot(&malformed).is_none());
        }

        let mut trailing = packet.clone();
        trailing.push(0xff);
        assert!(parse_client_position_snapshot(&trailing).is_none());
    }

    #[test]
    fn client_position_snapshot_parses_zubspace_logical_sequence_from_extra_tail() {
        let mut packet = vec![0; CLIENT_POSITION_EXTRA_LEN];
        packet[0] = CLIENT_POSITION_TYPE;
        packet[28..32].copy_from_slice(&zubspace_position_sequence_items(0x1234).to_le_bytes());
        append_test_canonical_state(&mut packet, 0, 0, 0, 0, 0);

        let snapshot = parse_client_position_snapshot(&packet).expect("position should parse");

        assert_eq!(snapshot.timestamp, 0);
        assert_eq!(snapshot.client_sequence, Some(0x1234));
    }

    #[test]
    fn client_position_snapshot_parses_only_exact_repel_marker_magic() {
        let mut packet = vec![0; CLIENT_POSITION_EXTRA_LEN];
        packet[0] = CLIENT_POSITION_TYPE;
        packet[24..26].copy_from_slice(&u16::MAX.to_le_bytes());
        append_test_canonical_state(&mut packet, 0, 0, 0, 0, 0);

        let without_magic = parse_client_position_snapshot(&packet).expect("position should parse");
        assert_eq!(without_magic.repel_snapshot_id, None);

        packet[26..28].copy_from_slice(&CLIENT_POSITION_REPEL_MARKER_MAGIC.to_le_bytes());
        packet.push(CLIENT_POSITION_REPEL_APPLICATION_MARKER_TYPE);
        packet.push(CLIENT_POSITION_REPEL_APPLICATION_MARKER_VERSION);
        packet.extend_from_slice(&u16::MAX.to_le_bytes());
        packet.extend_from_slice(&0x1234_u16.to_le_bytes());
        packet.extend_from_slice(&7_u32.to_le_bytes());
        let with_magic = parse_client_position_snapshot(&packet).expect("position should parse");
        assert_eq!(with_magic.repel_snapshot_id, Some(u16::MAX));
        assert_eq!(with_magic.repel_application_sequence, Some(0x1234));
        assert_eq!(with_magic.repel_delivery_generation, Some(7));

        let mut missing_application = packet.clone();
        missing_application.truncate(CLIENT_POSITION_EMBEDDED_INPUT_OFFSET);
        assert!(parse_client_position_snapshot(&missing_application).is_none());

        let mut mismatched_snapshot = packet.clone();
        mismatched_snapshot
            [CLIENT_POSITION_EMBEDDED_INPUT_OFFSET + 2..CLIENT_POSITION_EMBEDDED_INPUT_OFFSET + 4]
            .copy_from_slice(&(u16::MAX - 1).to_le_bytes());
        assert!(parse_client_position_snapshot(&mismatched_snapshot).is_none());

        let mut reordered = packet.clone();
        reordered[CLIENT_POSITION_EMBEDDED_INPUT_OFFSET] = CLIENT_POSITION_WARP_MARKER_TYPE;
        assert!(parse_client_position_snapshot(&reordered).is_none());

        let mut trailing = packet.clone();
        trailing.push(0);
        assert!(parse_client_position_snapshot(&trailing).is_none());

        let mut client_timed_v2 = packet.clone();
        client_timed_v2[CLIENT_POSITION_EMBEDDED_INPUT_OFFSET + 1] = 2;
        client_timed_v2.extend_from_slice(&331_u32.to_le_bytes());
        client_timed_v2.extend_from_slice(&135_u16.to_le_bytes());
        assert!(
            parse_client_position_snapshot(&client_timed_v2).is_none(),
            "the removed client application-tick/full-duration marker must fail closed"
        );

        let mut wrapped_snapshot_id = packet.clone();
        wrapped_snapshot_id[24..26].copy_from_slice(&0_u16.to_le_bytes());
        wrapped_snapshot_id
            [CLIENT_POSITION_EMBEDDED_INPUT_OFFSET + 2..CLIENT_POSITION_EMBEDDED_INPUT_OFFSET + 4]
            .copy_from_slice(&0_u16.to_le_bytes());
        let wrapped = parse_client_position_snapshot(&wrapped_snapshot_id)
            .expect("snapshot id zero is a valid u16 wrap boundary");
        assert_eq!(wrapped.repel_snapshot_id, Some(0));
        assert_eq!(wrapped.repel_application_sequence, Some(0x1234));

        packet[26..28].copy_from_slice(
            &CLIENT_POSITION_REPEL_MARKER_MAGIC
                .wrapping_add(1)
                .to_le_bytes(),
        );
        assert!(parse_client_position_snapshot(&packet).is_none());
    }

    #[test]
    fn world_bounds_rejects_coordinates_at_or_beyond_world_edge() {
        assert!(world_coordinates_in_bounds(16_383, 16_383, 1024, 16.0));
        assert!(!world_coordinates_in_bounds(16_384, 16_383, 1024, 16.0));
        assert!(!world_coordinates_in_bounds(16_383, 16_384, 1024, 16.0));
    }

    #[test]
    fn reported_velocity_limit_uses_protocol_velocity_scale() {
        let mut snapshot = ClientPositionSnapshot {
            direction: 0,
            timestamp: 0,
            client_sequence: None,
            x: 0,
            y: 0,
            velocity_x: 3_000,
            velocity_y: 4_000,
            togglables: 0,
            bounty: 0,
            energy: 0,
            weapon: 0,
            canonical_state: ClientPositionCanonicalState::from_legacy_wire(0, 0, 0, 3_000, 4_000),
            repel_snapshot_id: None,
            repel_application_sequence: None,
            repel_delivery_generation: None,
            warp_collectible_id: None,
        };
        assert_eq!(
            protocol_velocity_pixels_per_second(snapshot.velocity_x),
            300.0
        );
        assert!(reported_velocity_within_limit(snapshot, 375.0, 150.0));

        snapshot.velocity_y = 20_000;
        assert!(!reported_velocity_within_limit(snapshot, 375.0, 150.0));
    }

    #[test]
    fn reported_velocity_delta_limit_uses_elapsed_ticks_and_grace() {
        let mut snapshot = ClientPositionSnapshot {
            direction: 0,
            timestamp: 0,
            client_sequence: None,
            x: 0,
            y: 0,
            velocity_x: 100,
            velocity_y: 0,
            togglables: 0,
            bounty: 0,
            energy: 0,
            weapon: 0,
            canonical_state: ClientPositionCanonicalState::from_legacy_wire(0, 0, 0, 100, 0),
            repel_snapshot_id: None,
            repel_application_sequence: None,
            repel_delivery_generation: None,
            warp_collectible_id: None,
        };
        assert!(reported_velocity_delta_within_limit(
            0, 0, snapshot, 10, 50.0, 5.0,
        ));

        snapshot.velocity_x = 111;
        assert!(!reported_velocity_delta_within_limit(
            0, 0, snapshot, 10, 50.0, 5.0,
        ));
    }

    #[test]
    fn reported_energy_limit_rejects_impossible_energy() {
        let mut snapshot = ClientPositionSnapshot {
            direction: 0,
            timestamp: 0,
            client_sequence: None,
            x: 0,
            y: 0,
            velocity_x: 0,
            velocity_y: 0,
            togglables: 0,
            bounty: 0,
            energy: 1_700,
            weapon: 0,
            canonical_state: ClientPositionCanonicalState::from_legacy_wire(0, 0, 0, 0, 0),
            repel_snapshot_id: None,
            repel_application_sequence: None,
            repel_delivery_generation: None,
            warp_collectible_id: None,
        };
        assert!(reported_energy_within_limit(snapshot, 1_700, 1_700));

        snapshot.energy = 1_701;
        assert!(!reported_energy_within_limit(snapshot, 1_700, 1_700));
        assert!(reported_energy_within_limit(snapshot, 1_700, 1_701));
    }

    #[test]
    fn reported_position_delta_limit_rejects_obvious_teleports() {
        let mut snapshot = ClientPositionSnapshot {
            direction: 0,
            timestamp: 0,
            client_sequence: None,
            x: 540,
            y: 700,
            velocity_x: 0,
            velocity_y: 0,
            togglables: 0,
            bounty: 0,
            energy: 0,
            weapon: 0,
            canonical_state: ClientPositionCanonicalState::from_legacy_wire(0, 540, 700, 0, 0),
            repel_snapshot_id: None,
            repel_application_sequence: None,
            repel_delivery_generation: None,
            warp_collectible_id: None,
        };
        assert!(reported_position_delta_within_limit(
            500, 700, snapshot, 2, 325.0, 1_200.0, 256.0,
        ));

        snapshot.x = 5_000;
        assert!(!reported_position_delta_within_limit(
            500, 700, snapshot, 2, 325.0, 1_200.0, 256.0,
        ));
    }

    #[test]
    fn reported_position_path_samples_between_endpoints_for_blocked_tiles() {
        let wall_tile_x = 3_u16;
        let blocked = |x: u16, _y: u16| x / 16 == wall_tile_x;

        assert!(reported_position_path_crosses_blocked_tile(
            40, 24, 72, 24, 16.0, blocked,
        ));
        assert!(!reported_position_path_crosses_blocked_tile(
            40, 24, 47, 24, 16.0, blocked,
        ));
        assert!(!reported_position_path_crosses_blocked_tile(
            72, 24, 88, 24, 16.0, blocked,
        ));
    }

    #[test]
    fn reported_position_path_hull_catches_swept_radius_overlaps() {
        let blocked = |tile_x: usize, tile_y: usize| tile_x == 3 && tile_y == 1;

        assert!(!reported_position_path_crosses_blocked_tile(
            24,
            40,
            88,
            40,
            16.0,
            |x, y| blocked((x / 16) as usize, (y / 16) as usize),
        ));
        assert!(!reported_position_hull_overlaps_blocked_tile(
            24, 40, 10.0, 8, 16.0, blocked,
        ));
        assert!(!reported_position_hull_overlaps_blocked_tile(
            88, 40, 10.0, 8, 16.0, blocked,
        ));
        assert!(reported_position_path_hull_overlaps_blocked_tile(
            24, 40, 88, 40, 10.0, 8, 16.0, blocked,
        ));
        assert!(!reported_position_path_hull_overlaps_blocked_tile(
            24, 40, 88, 40, 4.0, 8, 16.0, blocked,
        ));
    }

    #[test]
    fn reported_position_hull_uses_radius_and_world_bounds() {
        let blocked = |tile_x: usize, tile_y: usize| tile_x == 3 && tile_y == 1;

        assert!(!reported_position_hull_overlaps_blocked_tile(
            40, 24, 1.0, 8, 16.0, blocked,
        ));
        assert!(reported_position_hull_overlaps_blocked_tile(
            40, 24, 14.0, 8, 16.0, blocked,
        ));
        assert!(reported_position_hull_overlaps_blocked_tile(
            4,
            24,
            14.0,
            8,
            16.0,
            |_, _| false,
        ));
    }

    #[test]
    fn sanitized_position_togglables_masks_high_bits_and_owns_safety() {
        assert_eq!(
            sanitized_position_togglables(0xff, false),
            POSITION_STATUS_RELAY_MASK & !POSITION_STATUS_SAFETY
        );
        assert_eq!(
            sanitized_position_togglables(POSITION_STATUS_STEALTH, true),
            POSITION_STATUS_STEALTH | POSITION_STATUS_SAFETY
        );
    }
}

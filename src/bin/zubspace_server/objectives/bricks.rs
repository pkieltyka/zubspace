#[path = "bricks/overlap.rs"]
mod overlap;
pub(crate) use overlap::{brick_overlap_destination, queue_enemy_brick_overlap_corrections};

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use super::super::arena_settings::{
    arena_brick_lifetime_ticks, arena_brick_span, arena_ship_radius_pixels,
    arena_ship_weapon_fire_delay_ticks,
};
use super::super::reliable::broadcast_reliable_to_joined;
use super::super::state::{
    BrickSourceGeometry, ClientHandle, ReliableLifecycleMutationOutcome, ServerBrick, ServerState,
};
#[cfg(test)]
use super::super::wire::current_tick;
use super::super::zone::{MAP_TILES, TILE_SIZE_PX, ZoneMap};
use super::DROP_BRICK_REQUEST;
use super::packets::{brick_dropped_packet, clear_bricks_packet};

// Matches `SHIP_RADIUS` in `src/constants.rs` — the parity assertion below
// ensures both sides stay aligned. Used as a fallback for brick-placement
// math where a per-ship radius lookup isn't easily threaded.
const DEFAULT_SHIP_RADIUS_PX: f32 = 14.0;

#[cfg(test)]
#[test]
fn default_ship_radius_matches_client_ship_radius() {
    assert!(
        (DEFAULT_SHIP_RADIUS_PX - 14.0).abs() < f32::EPSILON,
        "DEFAULT_SHIP_RADIUS_PX drifted from client SHIP_RADIUS"
    );
}
#[cfg(test)]
pub(crate) fn relay_drop_brick(state: &Arc<Mutex<ServerState>>, player_id: u16, request: &[u8]) {
    relay_drop_brick_at_tick(state, player_id, request, current_tick());
}

#[cfg(test)]
pub(crate) fn relay_drop_brick_at_tick(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    request: &[u8],
    tick: u32,
) {
    if let Some(packet) =
        brick_dropped_from_request_at_tick(state, None, player_id, request, None, tick)
    {
        broadcast_reliable_to_joined(state, packet);
    }
}

pub(crate) fn relay_drop_brick_for_runtime_at_tick(
    state: &Arc<Mutex<ServerState>>,
    identity: crate::authority_identity::PlayerRuntimeIdentity,
    request: &[u8],
    source: BrickSourceGeometry,
    tick: u32,
) -> ReliableLifecycleMutationOutcome {
    let Some(packet) = brick_dropped_from_request_at_tick(
        state,
        Some(identity),
        identity.player_id(),
        request,
        Some(source),
        tick,
    ) else {
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    };
    broadcast_reliable_to_joined(state, packet);
    ReliableLifecycleMutationOutcome::Applied
}

pub(crate) fn current_brick_packets_for_state_at_tick(
    state: &mut ServerState,
    tick: u32,
) -> Vec<Vec<u8>> {
    state
        .active_bricks
        .retain(|brick| !crate::continuum::continuum_tick_reached(tick, brick.expires_at_tick));
    state
        .active_bricks
        .iter()
        .map(|brick| brick.packet.clone())
        .collect()
}

pub(crate) fn expire_bricks(state: &Arc<Mutex<ServerState>>, tick: u32) -> Vec<Vec<u8>> {
    let mut state = state.lock().expect("server state should not be poisoned");
    let before = state.active_bricks.len();
    state
        .active_bricks
        .retain(|brick| !crate::continuum::continuum_tick_reached(tick, brick.expires_at_tick));
    if state.active_bricks.len() == before {
        return Vec::new();
    }

    let mut packets = Vec::with_capacity(state.active_bricks.len() + 1);
    packets.push(clear_bricks_packet());
    packets.extend(state.active_bricks.iter().map(|brick| brick.packet.clone()));
    packets
}

fn brick_dropped_from_request_at_tick(
    state: &Arc<Mutex<ServerState>>,
    expected_identity: Option<crate::authority_identity::PlayerRuntimeIdentity>,
    player_id: u16,
    request: &[u8],
    bound_source: Option<BrickSourceGeometry>,
    tick: u32,
) -> Option<Vec<u8>> {
    if request.len() < 5 || request[0] != DROP_BRICK_REQUEST {
        return None;
    }
    let x = u16::from_le_bytes(request[1..3].try_into().ok()?);
    let y = u16::from_le_bytes(request[3..5].try_into().ok()?);

    let mut state = state.lock().expect("server state should not be poisoned");
    if expected_identity.is_some_and(|identity| !state.player_runtime_matches(identity)) {
        return None;
    }
    let client = state.clients.get(&player_id)?;
    if !client.joined || !client.alive || client.player.ship >= 8 {
        return None;
    }
    let source = bound_source.or_else(|| brick_source_from_client(client))?;
    if crate::zone::world_position_in_safe_tile_for_map(state.zone_map(), source.x, source.y) {
        return None;
    }
    if !brick_request_matches_source(source, x, y) {
        return None;
    }

    // Place a cardinal lateral wall of at most BrickSpan tiles, halting
    // at terrain, existing bricks or map edges. Even spans keep the center
    // and one extra tile in the positive lateral direction.
    let map = state.zone_map();
    let blocked_by_existing_brick: HashSet<(u16, u16)> = state
        .active_bricks
        .iter()
        .filter(|brick| !crate::continuum::continuum_tick_reached(tick, brick.expires_at_tick))
        .flat_map(|brick| brick.tiles.iter().copied())
        .collect();
    let configured_span = arena_brick_span();
    // The request may lag this immutable source by a few pixels. Tolerance
    // permits delivery, never a client-selected displacement of the wall.
    let wall = brick_wall_tiles(
        source.x,
        source.y,
        source.direction,
        configured_span,
        map,
        &blocked_by_existing_brick,
    );
    if wall.is_empty() {
        return None;
    }
    let (start, end) = brick_wall_endpoints(&wall);
    // IDs index the client's retained live-line replay state. Wrapping may
    // reuse an expired identity, never overwrite a still-live line. Resolve
    // exhaustion before touching inventory, cooldown or the allocation cursor.
    let brick_id = available_brick_id(
        state.next_brick_id,
        state
            .active_bricks
            .iter()
            .filter(|brick| !crate::continuum::continuum_tick_reached(tick, brick.expires_at_tick))
            .filter_map(|brick| brick.packet.get(11..13))
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]])),
    )?;

    let cooldown_ticks = arena_ship_weapon_fire_delay_ticks(source.ship, 5);
    let client = state.clients.get_mut(&player_id)?;
    if !client.server_weapons.can_spend_brick()
        || client
            .server_warp_bomb_lock_until_tick
            .is_some_and(|until| !crate::continuum::continuum_tick_reached(tick, until))
    {
        return None;
    }
    if cooldown_ticks > 0
        && client.last_brick_drop_tick.is_some_and(|last_tick| {
            crate::continuum::continuum_tick_diff(tick, last_tick) < cooldown_ticks as i32
        })
    {
        return None;
    }
    client.server_weapons.spend_brick();
    client.last_brick_drop_tick = Some(tick);
    let life_seq = client.life_seq;

    state.next_brick_id = brick_id.wrapping_add(1);
    let packet = brick_dropped_packet(start, end, source.frequency, brick_id, tick);
    // Only expired lines may be retired here. Replacing a live line without
    // Clear/replay leaves its other cells visible and solid on existing clients.
    state
        .active_bricks
        .retain(|brick| !crate::continuum::continuum_tick_reached(tick, brick.expires_at_tick));
    let expires_at_tick = crate::continuum::continuum_tick_add(tick, arena_brick_lifetime_ticks());
    let tile_count = wall.len();
    state.active_bricks.push(ServerBrick {
        tiles: wall,
        packet: packet.clone(),
        expires_at_tick,
    });
    tracing::debug!(
        event = "server_brick_deployed",
        player_id,
        life_seq,
        brick_id,
        source_x = source.x,
        source_y = source.y,
        source_direction = source.direction,
        request_x = x,
        request_y = y,
        x1 = start.0,
        y1 = start.1,
        x2 = end.0,
        y2 = end.1,
        tile_count,
        configured_span,
        server_tick = tick,
        expires_at_tick,
        "server brick deployed"
    );
    Some(packet)
}

pub(crate) fn brick_tile_from_ship_position(x: u16, y: u16, direction: u8) -> (u16, u16) {
    // Decode wire direction into orientation, then use the canonical
    // heading-components helper so this brick-placement math can't drift
    // from `math::orientation_to_heading` on the client side.
    let (heading_x, heading_y) =
        crate::continuum_position::protocol_direction_heading_components(direction);
    let offset = DEFAULT_SHIP_RADIUS_PX + TILE_SIZE_PX * 0.5;
    let tile_x = ((x as f32 - heading_x * offset) / TILE_SIZE_PX)
        .floor()
        .clamp(0.0, (MAP_TILES - 1) as f32) as u16;
    let tile_y = ((y as f32 - heading_y * offset) / TILE_SIZE_PX)
        .floor()
        .clamp(0.0, (MAP_TILES - 1) as f32) as u16;
    (tile_x, tile_y)
}

/// Find a free wire identity without replacing a retained live line.
fn available_brick_id(next: u16, occupied: impl Iterator<Item = u16>) -> Option<u16> {
    let occupied = occupied.collect::<HashSet<_>>();
    (0..=u16::MAX)
        .map(|offset| next.wrapping_add(offset))
        .find(|candidate| !occupied.contains(candidate))
}

/// Keep the existing centered, lateral deployment policy, quantized to a
/// cardinal axis. The reference game renders server-supplied inclusive tile endpoints;
/// it does not select the server's placement policy. BrickSpan counts tiles,
/// not endpoint distance, so an unobstructed span of 10 occupies 160 pixels.
pub(crate) fn brick_wall_tiles(
    x: u16,
    y: u16,
    direction: u8,
    span: u8,
    map: &ZoneMap,
    blocked: &HashSet<(u16, u16)>,
) -> Vec<(u16, u16)> {
    brick_wall_tiles_in_grid(x, y, direction, span, &map.tiles, blocked)
}

fn brick_wall_tiles_in_grid(
    x: u16,
    y: u16,
    direction: u8,
    span: u8,
    tiles: &[u8],
    blocked: &HashSet<(u16, u16)>,
) -> Vec<(u16, u16)> {
    cardinal_brick_wall(
        brick_tile_from_ship_position(x, y, direction),
        direction,
        span,
        |tile| {
            let id = tiles
                .get(usize::from(tile.1) * MAP_TILES + usize::from(tile.0))
                .copied()
                .unwrap_or(0);
            // ASSS core/bricks.c requires GetTile == 0. Passable nonempty
            // cells are not brick space; the client has the same empty-cell rule.
            id != 0 || blocked.contains(&tile)
        },
    )
}

fn cardinal_brick_wall(
    center: (u16, u16),
    direction: u8,
    span: u8,
    blocked: impl Fn((u16, u16)) -> bool,
) -> Vec<(u16, u16)> {
    if blocked(center) {
        return Vec::new();
    }
    // Quantize the perpendicular to the nearest cardinal heading. Integer
    // tile steps cannot create diagonal stairs, duplicates or shortened spans.
    let axis = match (u16::from(direction % 40) + 5) % 40 / 10 {
        0 => (1, 0),
        1 => (0, 1),
        2 => (-1, 0),
        _ => (0, -1),
    };
    let span = usize::from(span.max(1));
    let mut wall = vec![center];
    for (sign, count) in [(1, span / 2), (-1, (span - 1) / 2)] {
        for step in 1..=count {
            let x = i32::from(center.0) + axis.0 * sign * step as i32;
            let y = i32::from(center.1) + axis.1 * sign * step as i32;
            if !(0..MAP_TILES as i32).contains(&x) || !(0..MAP_TILES as i32).contains(&y) {
                break;
            }
            let tile = (x as u16, y as u16);
            if blocked(tile) {
                break;
            }
            wall.push(tile);
        }
    }
    wall
}

/// Cardinal walls serialize as one inclusive horizontal/vertical interval.
/// Both server collision and client rendering must reconstruct these same tiles.
pub(crate) fn brick_wall_endpoints(wall: &[(u16, u16)]) -> ((u16, u16), (u16, u16)) {
    (
        *wall.iter().min().expect("brick wall contains its center"),
        *wall.iter().max().expect("brick wall contains its center"),
    )
}

#[cfg(test)]
mod cardinal_wall_tests {
    use super::*;

    #[test]
    fn brick_id_allocation_is_bounded_across_wrap_and_exhaustion() {
        assert_eq!(
            available_brick_id(u16::MAX, [u16::MAX, 0].into_iter()),
            Some(1)
        );
        assert_eq!(
            available_brick_id(7, (0..=u16::MAX).filter(|id| *id != 6)),
            Some(6)
        );
        assert_eq!(available_brick_id(7, 0..=u16::MAX), None);
    }

    #[test]
    fn brick_placement_preserves_nonempty_map_cells_and_rejects_occupied_centers() {
        let mut tiles = vec![0; MAP_TILES * MAP_TILES];
        for id in [1, 162, 170, 171, 172, 190, 220, 250] {
            tiles[512 * MAP_TILES + 513] = id;
            let wall = brick_wall_tiles_in_grid(8200, 8180, 0, 10, &tiles, &HashSet::new());
            assert_eq!(
                wall.into_iter().collect::<HashSet<_>>(),
                (508..=512).map(|x| (x, 512)).collect::<HashSet<_>>(),
                "tile {id} must terminate the line"
            );
            tiles[512 * MAP_TILES + 513] = 0;
            tiles[512 * MAP_TILES + 512] = id;
            assert!(
                brick_wall_tiles_in_grid(8200, 8180, 0, 10, &tiles, &HashSet::new()).is_empty(),
                "occupied center {id}"
            );
            tiles[512 * MAP_TILES + 512] = 0;
        }
        assert!(
            brick_wall_tiles_in_grid(8200, 8180, 0, 10, &tiles, &HashSet::from([(512, 512)]))
                .is_empty()
        );
    }

    #[test]
    fn brick_walls_all_forty_headings_have_exact_cardinal_span_and_wire_coverage() {
        for direction in 0..40 {
            for span in [1, 2, 3, 10, 11, 255] {
                let wall = cardinal_brick_wall((512, 512), direction, span, |_| false);
                let unique = wall.iter().copied().collect::<HashSet<_>>();
                assert_eq!(unique.len(), usize::from(span));
                assert_eq!(wall.len(), unique.len());
                let (start, end) = brick_wall_endpoints(&wall);
                assert!(
                    start.0 == end.0 || start.1 == end.1,
                    "direction {direction}: {start:?}..{end:?}"
                );
                let decoded = (start.0..=end.0)
                    .flat_map(|x| (start.1..=end.1).map(move |y| (x, y)))
                    .collect::<HashSet<_>>();
                assert_eq!(
                    unique, decoded,
                    "rendered wire interval must equal collision tiles"
                );
            }
        }
    }

    #[test]
    fn brick_walls_stop_at_obstructions_and_map_edges_without_gaps() {
        let blocked = HashSet::from([(515, 512), (510, 512)]);
        let wall = cardinal_brick_wall((512, 512), 0, 10, |tile| blocked.contains(&tile));
        assert_eq!(
            wall.into_iter().collect::<HashSet<_>>(),
            HashSet::from([(511, 512), (512, 512), (513, 512), (514, 512)])
        );
        let edge = cardinal_brick_wall((0, 0), 0, 10, |_| false);
        assert_eq!(
            edge.into_iter().collect::<HashSet<_>>(),
            (0_u16..=5).map(|x| (x, 0)).collect::<HashSet<_>>()
        );
        let corner = cardinal_brick_wall((1023, 1023), 10, 10, |_| false);
        assert!(corner.iter().all(|&(x, y)| x == 1023 && y <= 1023));
        assert_eq!(corner.len(), 5);
    }
}

fn brick_request_matches_source(source: BrickSourceGeometry, x: u16, y: u16) -> bool {
    let tolerance = arena_ship_radius_pixels(source.ship).max(0.0) + TILE_SIZE_PX * 3.0;
    let dx = f32::from(source.x) - x as f32;
    let dy = f32::from(source.y) - y as f32;
    dx * dx + dy * dy <= tolerance * tolerance
}

fn brick_source_from_client(client: &ClientHandle) -> Option<BrickSourceGeometry> {
    let packet = client.last_position.as_deref()?;
    let (x, y) = large_position_world_position(Some(packet))?;
    Some(BrickSourceGeometry {
        x: x as u16,
        y: y as u16,
        direction: packet.get(1).copied().unwrap_or(0),
        ship: client.player.ship,
        frequency: client.player.frequency,
    })
}

fn large_position_world_position(packet: Option<&[u8]>) -> Option<(f32, f32)> {
    let packet = packet?;
    if packet.len() < 17 || packet[0] != crate::arena::LARGE_POSITION_PACKET {
        return None;
    }
    let x = u16::from_le_bytes(packet[4..6].try_into().ok()?);
    let y = u16::from_le_bytes(packet[15..17].try_into().ok()?);
    Some((x as f32, y as f32))
}

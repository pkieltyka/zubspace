use std::collections::{BTreeSet, HashMap};

// 16 px per tile × 64 tiles per quadrant = 1024 px per quadrant. The
// 1024×1024 tile arena maps to a 16×16 = 256-quadrant grid. Spec
// `docs/authoritative_server.md` §5.
pub(crate) const QUADRANT_SIZE_PX: u16 = 1_024;

// AOI tier radii from spec §5. Near = 4×4 quadrants (~16 quadrants
// after circular trim), full per-tick state. Mid = 5×5 quadrants
// (~25 after trim), low-frequency position pings. Beyond mid = far
// (zero bytes).
pub(crate) const NEAR_AOI_RADIUS_PX: f32 = 2_048.0;
pub(crate) const MID_AOI_RADIUS_PX: f32 = 2_560.0;

/// Single-tier fanout radius retained as a test-only constant so the
/// regression tests for the cell-grid + radius math still have a
/// concrete radius to assert on. Production fan-out uses `NEAR_AOI` +
/// `MID_AOI` directly.
#[cfg(test)]
pub(crate) const POSITION_FANOUT_RADIUS_PX: f32 = 4_096.0;

/// Per-recipient AOI tier classification. Drives whether a recipient
/// receives full per-tick state, throttled position pings, or nothing
/// for a given sender. Spec `docs/authoritative_server.md` §5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AoiTier {
    Near,
    Mid,
    Far,
}

impl AoiTier {
    pub(crate) fn is_near(self) -> bool {
        matches!(self, AoiTier::Near)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct VisibilityCell {
    x: u16,
    y: u16,
}

#[cfg(test)]
pub(crate) fn position_visible_to_recipient(
    sender_position: &[u8],
    recipient_position: Option<&[u8]>,
    radius_px: f32,
) -> bool {
    let Some(recipient_position) = recipient_position else {
        return true;
    };
    let (Some(sender), Some(recipient)) = (
        PositionedPacket::from_large_position(sender_position),
        PositionedPacket::from_large_position(recipient_position),
    ) else {
        return true;
    };
    if !cells_may_overlap_radius(sender.cell, recipient.cell, radius_px) {
        return false;
    }
    let radius_squared = radius_px.max(0.0) * radius_px.max(0.0);
    let dx = sender.x - recipient.x;
    let dy = sender.y - recipient.y;
    dx * dx + dy * dy <= radius_squared
}

/// Classify a recipient relative to a sender by their large-position
/// packets. Returns `Far` when either packet is malformed enough that
/// a position can't be derived; the existing fanout policy keeps
/// malformed-position recipients via a separate failsafe path, so
/// returning `Far` here is safe — the caller decides whether to gate
/// on tier or on a malformed-position bypass.
pub(crate) fn aoi_tier_between_positions(
    sender_position: &[u8],
    recipient_position: &[u8],
    near_radius_px: f32,
    mid_radius_px: f32,
) -> AoiTier {
    let (Some(sender), Some(recipient)) = (
        PositionedPacket::from_large_position(sender_position),
        PositionedPacket::from_large_position(recipient_position),
    ) else {
        return AoiTier::Far;
    };
    let dx = sender.x - recipient.x;
    let dy = sender.y - recipient.y;
    let distance_squared = dx * dx + dy * dy;
    aoi_tier_for_distance_squared(distance_squared, near_radius_px, mid_radius_px)
}

pub(crate) fn aoi_tier_for_distance_squared(
    distance_squared: f32,
    near_radius_px: f32,
    mid_radius_px: f32,
) -> AoiTier {
    let near_sq = near_radius_px.max(0.0) * near_radius_px.max(0.0);
    if distance_squared <= near_sq {
        return AoiTier::Near;
    }
    let mid_sq = mid_radius_px.max(0.0) * mid_radius_px.max(0.0);
    if distance_squared <= mid_sq {
        return AoiTier::Mid;
    }
    AoiTier::Far
}

/// Phase B AOI hysteresis: how far past a tier's radius an entity already
/// in that tier must travel before it's demoted. Keeps a ship hovering on
/// the near/mid boundary from flickering tiers (and forcing client render
/// re-snaps) frame to frame. Promotion uses the plain radius, so entering
/// the screen stays instant.
pub(crate) const AOI_HYSTERESIS_MARGIN_PX: f32 = 256.0;

/// Phase C predictive AOI: how far ahead (seconds) to project a closing
/// candidate so it enters AOI a frame or two before it physically crosses
/// the radius — removing the "pop-in" of a fast ship arriving at the
/// screen edge.
pub(crate) const AOI_PREDICTIVE_LEAD_SECONDS: f32 = 0.2;

/// Cap on the predictive lead distance (one quadrant) so a very fast ship
/// can't be pulled into AOI from absurdly far, bounding the extra fanout.
pub(crate) const AOI_PREDICTIVE_LEAD_MAX_PX: f32 = QUADRANT_SIZE_PX as f32;

/// Predictive variant of `aoi_tier_between_positions` (Phase C). Pulls a
/// *closing* candidate into a higher tier early: the candidate's velocity
/// component toward the viewer is projected `lead_seconds` ahead and
/// subtracted from the gap, so an approacher enters near/mid before it
/// physically crosses the radius. Receding or perpendicular-crossing
/// candidates are unaffected (closing speed clamped to >= 0), and the lead
/// is capped at `AOI_PREDICTIVE_LEAD_MAX_PX`. Reduces to the plain
/// distance classification when the candidate is stationary.
pub(crate) fn aoi_tier_between_positions_predictive(
    viewer_position: &[u8],
    candidate_position: &[u8],
    near_radius_px: f32,
    mid_radius_px: f32,
    lead_seconds: f32,
) -> AoiTier {
    let (Some(viewer), Some(candidate)) = (
        PositionedPacket::from_large_position(viewer_position),
        PositionedPacket::from_large_position(candidate_position),
    ) else {
        return AoiTier::Far;
    };
    let dx = candidate.x - viewer.x;
    let dy = candidate.y - viewer.y;
    let distance = (dx * dx + dy * dy).sqrt();
    let lead = if distance > 1.0 {
        // Unit vector from the candidate toward the viewer.
        let toward_x = -dx / distance;
        let toward_y = -dy / distance;
        let closing = candidate.velocity_x * toward_x + candidate.velocity_y * toward_y;
        (closing.max(0.0) * lead_seconds).min(AOI_PREDICTIVE_LEAD_MAX_PX)
    } else {
        0.0
    };
    let effective = (distance - lead).max(0.0);
    aoi_tier_for_distance_squared(effective * effective, near_radius_px, mid_radius_px)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct VisibilityIndex {
    cells: HashMap<VisibilityCell, BTreeSet<u16>>,
}

impl VisibilityIndex {
    pub(crate) fn set_player_cell(
        &mut self,
        player_id: u16,
        old_cell: Option<VisibilityCell>,
        new_cell: Option<VisibilityCell>,
    ) {
        if old_cell == new_cell {
            return;
        }
        if let Some(old_cell) = old_cell {
            self.remove_player_from_cell(player_id, old_cell);
        }
        if let Some(new_cell) = new_cell {
            self.cells.entry(new_cell).or_default().insert(player_id);
        }
    }

    pub(crate) fn remove_player(&mut self, player_id: u16, old_cell: Option<VisibilityCell>) {
        if let Some(old_cell) = old_cell {
            self.remove_player_from_cell(player_id, old_cell);
        }
    }

    pub(crate) fn candidate_player_ids(
        &self,
        sender_position: &[u8],
        radius_px: f32,
    ) -> BTreeSet<u16> {
        let Some(sender) = PositionedPacket::from_large_position(sender_position) else {
            return self
                .cells
                .values()
                .flat_map(|player_ids| player_ids.iter().copied())
                .collect();
        };
        let mut candidates = BTreeSet::new();
        for cell in cells_near(sender.cell, cell_radius_for_radius(radius_px)) {
            if let Some(player_ids) = self.cells.get(&cell) {
                candidates.extend(player_ids.iter().copied());
            }
        }
        candidates
    }

    fn remove_player_from_cell(&mut self, player_id: u16, cell: VisibilityCell) {
        let Some(player_ids) = self.cells.get_mut(&cell) else {
            return;
        };
        player_ids.remove(&player_id);
        if player_ids.is_empty() {
            self.cells.remove(&cell);
        }
    }
}

#[cfg(test)]
pub(crate) fn visible_position_recipient_ids<'a>(
    sender_position: &[u8],
    recipients: impl IntoIterator<Item = (u16, Option<&'a [u8]>)>,
    radius_px: f32,
) -> Vec<u16> {
    let Some(sender) = PositionedPacket::from_large_position(sender_position) else {
        return recipients
            .into_iter()
            .map(|(player_id, _)| player_id)
            .collect();
    };
    let mut visible = Vec::new();
    let mut positioned_by_cell: HashMap<VisibilityCell, Vec<PositionedRecipient>> = HashMap::new();
    for (player_id, position) in recipients {
        let Some(position) = position.and_then(PositionedPacket::from_large_position) else {
            visible.push(player_id);
            continue;
        };
        positioned_by_cell
            .entry(position.cell)
            .or_default()
            .push(PositionedRecipient {
                player_id,
                position,
            });
    }

    let cell_radius = cell_radius_for_radius(radius_px);
    for cell in cells_near(sender.cell, cell_radius) {
        if let Some(recipients) = positioned_by_cell.get(&cell) {
            for recipient in recipients {
                if positioned_packets_within_radius(sender, recipient.position, radius_px) {
                    visible.push(recipient.player_id);
                }
            }
        }
    }
    visible
}

#[cfg(test)]
pub(crate) fn large_position_packet_cell(packet: &[u8]) -> Option<VisibilityCell> {
    PositionedPacket::from_large_position(packet).map(|packet| packet.cell)
}

#[cfg(test)]
fn cells_may_overlap_radius(
    sender: VisibilityCell,
    recipient: VisibilityCell,
    radius_px: f32,
) -> bool {
    let cell_radius = cell_radius_for_radius(radius_px);
    sender.x.abs_diff(recipient.x) <= cell_radius && sender.y.abs_diff(recipient.y) <= cell_radius
}

fn cell_radius_for_radius(radius_px: f32) -> u16 {
    ((radius_px.max(0.0) / QUADRANT_SIZE_PX as f32).ceil()) as u16
}

fn cells_near(center: VisibilityCell, radius: u16) -> impl Iterator<Item = VisibilityCell> {
    let min_x = center.x.saturating_sub(radius);
    let max_x = center.x.saturating_add(radius);
    let min_y = center.y.saturating_sub(radius);
    let max_y = center.y.saturating_add(radius);
    (min_x..=max_x).flat_map(move |x| (min_y..=max_y).map(move |y| VisibilityCell { x, y }))
}

#[cfg(test)]
fn positioned_packets_within_radius(
    sender: PositionedPacket,
    recipient: PositionedPacket,
    radius_px: f32,
) -> bool {
    let radius_squared = radius_px.max(0.0) * radius_px.max(0.0);
    let dx = sender.x - recipient.x;
    let dy = sender.y - recipient.y;
    dx * dx + dy * dy <= radius_squared
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct PositionedRecipient {
    player_id: u16,
    position: PositionedPacket,
}

#[derive(Clone, Copy)]
struct PositionedPacket {
    x: f32,
    y: f32,
    /// Reported velocity in px/s (decoded from the tenths-of-px/s wire
    /// value). Used by the Phase C predictive AOI classification.
    velocity_x: f32,
    velocity_y: f32,
    cell: VisibilityCell,
}

impl PositionedPacket {
    fn from_large_position(packet: &[u8]) -> Option<Self> {
        let (x, y) = large_position_packet_position(packet)?;
        // Velocity is tenths-of-px/s, i16 LE: x at [10..12], y at [6..8]
        // (matching `large_position_velocity` in arena/positions.rs).
        // `large_position_packet_position` already verified len >= 17.
        let velocity_x = i16::from_le_bytes(packet[10..12].try_into().ok()?) as f32 / 10.0;
        let velocity_y = i16::from_le_bytes(packet[6..8].try_into().ok()?) as f32 / 10.0;
        Some(Self {
            x,
            y,
            velocity_x,
            velocity_y,
            cell: VisibilityCell {
                x: (x as u16) / QUADRANT_SIZE_PX,
                y: (y as u16) / QUADRANT_SIZE_PX,
            },
        })
    }
}

pub(crate) fn visibility_cell_for_large_position(packet: &[u8]) -> Option<VisibilityCell> {
    PositionedPacket::from_large_position(packet).map(|packet| packet.cell)
}

fn large_position_packet_position(packet: &[u8]) -> Option<(f32, f32)> {
    if packet.len() < 17 {
        return None;
    }
    let x = u16::from_le_bytes(packet[4..6].try_into().ok()?) as f32;
    let y = u16::from_le_bytes(packet[15..17].try_into().ok()?) as f32;
    Some((x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn large_position(x: u16, y: u16) -> Vec<u8> {
        let mut packet = vec![0; 23];
        packet[0] = 0x05;
        packet[4..6].copy_from_slice(&x.to_le_bytes());
        packet[15..17].copy_from_slice(&y.to_le_bytes());
        packet
    }

    fn large_position_with_velocity(x: u16, y: u16, vx_px_s: i16, vy_px_s: i16) -> Vec<u8> {
        let mut packet = large_position(x, y);
        // Wire velocity is tenths-of-px/s: x at [10..12], y at [6..8].
        packet[10..12].copy_from_slice(&vx_px_s.saturating_mul(10).to_le_bytes());
        packet[6..8].copy_from_slice(&vy_px_s.saturating_mul(10).to_le_bytes());
        packet
    }

    #[test]
    fn predictive_promotes_a_closing_candidate_into_near() {
        let viewer = large_position(8_000, 8_000);
        // 2200 px to the right of the viewer: outside near (2048), inside
        // mid (2560) — normally Mid.
        let stationary = large_position(10_200, 8_000);
        assert_eq!(
            aoi_tier_between_positions(&viewer, &stationary, NEAR_AOI_RADIUS_PX, MID_AOI_RADIUS_PX),
            AoiTier::Mid,
        );
        // Same spot, closing at 1000 px/s toward the viewer (-x). Lead =
        // 1000 * 0.2 = 200 px ⇒ effective 2000 px < 2048 ⇒ promoted to Near.
        let closing = large_position_with_velocity(10_200, 8_000, -1_000, 0);
        assert_eq!(
            aoi_tier_between_positions_predictive(
                &viewer,
                &closing,
                NEAR_AOI_RADIUS_PX,
                MID_AOI_RADIUS_PX,
                AOI_PREDICTIVE_LEAD_SECONDS,
            ),
            AoiTier::Near,
            "a fast approacher should enter near AOI early",
        );
    }

    #[test]
    fn predictive_leaves_receding_and_crossing_candidates_unchanged() {
        let viewer = large_position(8_000, 8_000);
        // Receding (+x, away from viewer) — closing speed clamped to 0.
        let receding = large_position_with_velocity(10_200, 8_000, 1_000, 0);
        assert_eq!(
            aoi_tier_between_positions_predictive(
                &viewer,
                &receding,
                NEAR_AOI_RADIUS_PX,
                MID_AOI_RADIUS_PX,
                AOI_PREDICTIVE_LEAD_SECONDS,
            ),
            AoiTier::Mid,
            "a receding candidate must not be promoted",
        );
        // Crossing perpendicular (+y) — no radial closing component.
        let crossing = large_position_with_velocity(10_200, 8_000, 0, 2_000);
        assert_eq!(
            aoi_tier_between_positions_predictive(
                &viewer,
                &crossing,
                NEAR_AOI_RADIUS_PX,
                MID_AOI_RADIUS_PX,
                AOI_PREDICTIVE_LEAD_SECONDS,
            ),
            AoiTier::Mid,
            "a perpendicular-crossing candidate has no closing speed",
        );
    }

    #[test]
    fn predictive_matches_plain_classification_when_stationary() {
        let viewer = large_position(8_000, 8_000);
        for (x, y) in [(8_500u16, 8_000u16), (10_200, 8_000), (12_000, 8_000)] {
            let candidate = large_position(x, y);
            assert_eq!(
                aoi_tier_between_positions_predictive(
                    &viewer,
                    &candidate,
                    NEAR_AOI_RADIUS_PX,
                    MID_AOI_RADIUS_PX,
                    AOI_PREDICTIVE_LEAD_SECONDS,
                ),
                aoi_tier_between_positions(
                    &viewer,
                    &candidate,
                    NEAR_AOI_RADIUS_PX,
                    MID_AOI_RADIUS_PX
                ),
                "stationary candidate must classify identically to the plain path",
            );
        }
    }

    #[test]
    fn visibility_allows_recipients_without_position_or_bad_packets() {
        let sender = large_position(500, 700);

        assert!(position_visible_to_recipient(
            &sender,
            None,
            POSITION_FANOUT_RADIUS_PX
        ));
        assert!(position_visible_to_recipient(
            &sender,
            Some(&[0x05]),
            POSITION_FANOUT_RADIUS_PX
        ));
    }

    #[test]
    fn visibility_filters_recipients_outside_radius() {
        let sender = large_position(500, 700);
        let near = large_position(4_000, 700);
        let far = large_position(8_000, 8_000);

        assert!(position_visible_to_recipient(
            &sender,
            Some(&near),
            POSITION_FANOUT_RADIUS_PX
        ));
        assert!(!position_visible_to_recipient(
            &sender,
            Some(&far),
            POSITION_FANOUT_RADIUS_PX
        ));
    }

    #[test]
    fn quadrant_size_matches_64_tile_spec() {
        assert_eq!(QUADRANT_SIZE_PX, 64 * 16);
    }

    #[test]
    fn near_radius_smaller_than_mid_radius() {
        assert!(NEAR_AOI_RADIUS_PX < MID_AOI_RADIUS_PX);
    }

    #[test]
    fn visibility_cell_uses_quadrant_aligned_coordinates() {
        // (4_200, 6_300) lies in quadrant (4, 6) at 1024-px quadrants.
        assert_eq!(
            large_position_packet_cell(&large_position(4_200, 6_300)),
            Some(VisibilityCell { x: 4, y: 6 })
        );
        assert_eq!(large_position_packet_cell(&[0x05]), None);
    }

    #[test]
    fn visibility_cell_prefilter_is_conservative_before_exact_radius() {
        let sender = large_position(2_000, 2_000);
        let same_cell = large_position(2_047, 2_047);
        let nearby_inside_radius = large_position(5_000, 2_000);
        let nearby_outside_exact_radius = large_position(6_200, 2_000);
        let far_cell = large_position(9_000, 2_000);

        assert!(position_visible_to_recipient(
            &sender,
            Some(&same_cell),
            POSITION_FANOUT_RADIUS_PX
        ));
        assert!(position_visible_to_recipient(
            &sender,
            Some(&nearby_inside_radius),
            POSITION_FANOUT_RADIUS_PX
        ));
        assert!(!position_visible_to_recipient(
            &sender,
            Some(&nearby_outside_exact_radius),
            POSITION_FANOUT_RADIUS_PX
        ));
        assert!(!position_visible_to_recipient(
            &sender,
            Some(&far_cell),
            POSITION_FANOUT_RADIUS_PX
        ));
    }

    #[test]
    fn visibility_recipient_index_returns_unpositioned_and_nearby_cells_only() {
        let sender = large_position(2_000, 2_000);
        let near = large_position(4_000, 2_000);
        let far = large_position(9_000, 9_000);
        let just_outside_exact_radius = large_position(6_200, 2_000);

        let recipients = visible_position_recipient_ids(
            &sender,
            [
                (1, Some(near.as_slice())),
                (2, Some(far.as_slice())),
                (3, None),
                (4, Some(just_outside_exact_radius.as_slice())),
                (5, Some(&[0x05][..])),
            ],
            POSITION_FANOUT_RADIUS_PX,
        );

        assert_eq!(recipients, vec![3, 5, 1]);
    }

    #[test]
    fn maintained_visibility_index_tracks_player_cell_membership() {
        let mut index = VisibilityIndex::default();
        let old_cell = visibility_cell_for_large_position(&large_position(2_000, 2_000));
        let new_cell = visibility_cell_for_large_position(&large_position(6_000, 2_000));
        let sender = large_position(2_100, 2_100);

        index.set_player_cell(7, None, old_cell);
        assert!(
            index
                .candidate_player_ids(&sender, POSITION_FANOUT_RADIUS_PX)
                .contains(&7)
        );

        index.set_player_cell(7, old_cell, new_cell);
        assert!(
            index
                .candidate_player_ids(&sender, POSITION_FANOUT_RADIUS_PX)
                .contains(&7)
        );

        index.remove_player(7, new_cell);
        assert!(
            !index
                .candidate_player_ids(&sender, POSITION_FANOUT_RADIUS_PX)
                .contains(&7)
        );
    }

    #[test]
    fn aoi_tier_classifies_near_mid_far_concentric_rings() {
        let sender = large_position(8_000, 8_000);

        let same = large_position(8_000, 8_000);
        assert_eq!(
            aoi_tier_between_positions(&sender, &same, NEAR_AOI_RADIUS_PX, MID_AOI_RADIUS_PX),
            AoiTier::Near
        );

        // 1500 px on x — inside near (≤ 2048).
        let inside_near = large_position(8_000 + 1_500, 8_000);
        assert_eq!(
            aoi_tier_between_positions(
                &sender,
                &inside_near,
                NEAR_AOI_RADIUS_PX,
                MID_AOI_RADIUS_PX,
            ),
            AoiTier::Near
        );

        // 2300 px on x — outside near (>2048), inside mid (≤ 2560).
        let inside_mid = large_position(8_000 + 2_300, 8_000);
        assert_eq!(
            aoi_tier_between_positions(&sender, &inside_mid, NEAR_AOI_RADIUS_PX, MID_AOI_RADIUS_PX,),
            AoiTier::Mid
        );

        // 3000 px on x — beyond mid.
        let in_far = large_position(8_000 + 3_000, 8_000);
        assert_eq!(
            aoi_tier_between_positions(&sender, &in_far, NEAR_AOI_RADIUS_PX, MID_AOI_RADIUS_PX),
            AoiTier::Far
        );
    }

    #[test]
    fn aoi_tier_treats_malformed_position_as_far() {
        let sender = large_position(8_000, 8_000);
        let bad: &[u8] = &[0x05];
        assert_eq!(
            aoi_tier_between_positions(&sender, bad, NEAR_AOI_RADIUS_PX, MID_AOI_RADIUS_PX),
            AoiTier::Far
        );
    }

    #[test]
    fn aoi_tier_near_predicate_matches_only_near() {
        assert!(AoiTier::Near.is_near());
        assert!(!AoiTier::Mid.is_near());
        assert!(!AoiTier::Far.is_near());
    }

    #[test]
    fn aoi_tier_radius_boundary_is_inclusive_on_near_and_mid() {
        let sender = large_position(8_000, 8_000);

        // Exactly NEAR_AOI_RADIUS_PX on x.
        let on_near = large_position(8_000 + NEAR_AOI_RADIUS_PX as u16, 8_000);
        assert_eq!(
            aoi_tier_between_positions(&sender, &on_near, NEAR_AOI_RADIUS_PX, MID_AOI_RADIUS_PX),
            AoiTier::Near
        );

        // Exactly MID_AOI_RADIUS_PX on x.
        let on_mid = large_position(8_000 + MID_AOI_RADIUS_PX as u16, 8_000);
        assert_eq!(
            aoi_tier_between_positions(&sender, &on_mid, NEAR_AOI_RADIUS_PX, MID_AOI_RADIUS_PX),
            AoiTier::Mid
        );
    }
}

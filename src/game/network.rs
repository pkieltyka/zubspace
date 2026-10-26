use super::*;

mod browser;
mod outbound;
mod owner_cancellation;
mod remote;
#[cfg(test)]
mod tests;

const SELF_CORRECTION_RESPAWN_AUTHORITY: u8 = 1;

/// [REVIEW M10] How far the predicted self-energy may sit ABOVE the
/// server's snapshot value before the server value is adopted. Recharge
/// runs ~115/s and the self snapshot is ~1 RTT stale, so honest prediction
/// legitimately leads by a couple dozen energy; a larger gap means a
/// mispredicted drain (or missed damage) and the server wins immediately.
pub(in crate::game) const SELF_ENERGY_PREDICTION_TOLERANCE: f32 = 25.0;

/// Blend factor for converging the prediction upward when the server
/// reports MORE energy than predicted (e.g. a server-applied recharge
/// upgrade) — converge smoothly instead of snapping.
const SELF_ENERGY_UPWARD_BLEND: f32 = 0.25;

/// Mirrors the server's complete input repair/retention window. A Rocket pulse
/// edge that is absent below this distance may still arrive in overlapping
/// history and must remain predicted.
const ROCKET_INPUT_RECOVERY_WINDOW: u16 = 64;
#[cfg(target_arch = "wasm32")]
const MAX_PENDING_WARP_MARKER_SEQUENCES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::game) enum LifecycleAction {
    Ignore,
    Observe,
    StartDeath,
}

fn apply_exact_movement_correction_terminal(ship: &mut Ship, receipt: MovementCorrectionAccepted) {
    ship.pos = vec2(receipt.position_x, receipt.position_y);
    ship.vel = vec2(receipt.velocity_x, receipt.velocity_y);
    ship.orientation = receipt.orientation;
}

pub(in crate::game) fn prize_pickup_ack_matches_current_runtime(
    server_session_id: Option<u64>,
    arena_stream_id: Option<u64>,
    self_player_id: Option<u16>,
    self_life_seq: Option<u32>,
    self_alive: Option<bool>,
    lifecycle_cutover_pending: bool,
    scoped: ScopedPrizePickupAck,
) -> bool {
    !lifecycle_cutover_pending
        && self_alive == Some(true)
        && scoped.server_session_id.is_some()
        && scoped.player_id.is_some()
        && scoped.arena_stream_id.is_some()
        && server_session_id == scoped.server_session_id
        && arena_stream_id == scoped.arena_stream_id
        && self_player_id == scoped.player_id
        && self_life_seq == Some(scoped.ack.life_seq)
        && scoped.ack.server_session_id == scoped.server_session_id.unwrap_or(0)
        && scoped.ack.arena_stream_id == scoped.arena_stream_id.unwrap_or(0)
        && Some(scoped.ack.player_id) == scoped.player_id
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::game) struct AuthorityAdmissionSeed {
    pub(in crate::game) scope: AuthorityScopeIdentity,
    pub(in crate::game) readiness: AuthorityReadiness,
    pub(in crate::game) segment: u64,
}

fn private_admission_for_scope(
    origin: AuthorityScopeIdentity,
    origin_segment: u64,
    cursor: AuthorityScopeIdentity,
    segment: u64,
    server_session_id: Option<u64>,
    arena_stream_id: Option<u64>,
    player_id: Option<u16>,
    life_seq: Option<u32>,
    alive: Option<bool>,
) -> PrivateAuthorityAdmission {
    let candidate = AuthorityScopeIdentity {
        server_session_id,
        arena_stream_id,
        player_id,
        life_seq,
        alive,
    };
    if candidate.server_session_id.is_none()
        || candidate.arena_stream_id.is_none()
        || candidate.player_id.is_none()
        || candidate.life_seq.is_none()
        || candidate.alive.is_none()
        || candidate != cursor
    {
        PrivateAuthorityAdmission::Reject
    } else if candidate == origin && segment == origin_segment {
        PrivateAuthorityAdmission::Current(candidate)
    } else {
        PrivateAuthorityAdmission::Replacement(candidate)
    }
}

fn authority_action_is_explicitly_neutral(event: &ClientProtocolEvent) -> bool {
    matches!(
        event,
        ClientProtocolEvent::KeepAlive
            | ClientProtocolEvent::UnknownPacket(_)
            | ClientProtocolEvent::UnknownCorePacket(_)
    )
}

fn authority_action_requires_pre_session_scope(event: &ClientProtocolEvent) -> bool {
    matches!(
        event,
        ClientProtocolEvent::PasswordAccepted
            | ClientProtocolEvent::RegistrationRequired
            | ClientProtocolEvent::RegistrationFormRequested
            | ClientProtocolEvent::LoginFailed(_)
            | ClientProtocolEvent::LoginFailureMessage(_)
            | ClientProtocolEvent::ServerVersion(_)
    )
}

fn advance_scope_for_soft_boundary(
    mut scope: AuthorityScopeIdentity,
    event: &ClientProtocolEvent,
) -> AuthorityScopeIdentity {
    match event {
        ClientProtocolEvent::PlayerDeath(death) if scope.player_id == Some(death.killed_id) => {
            scope.life_seq = Some(death.life_seq);
            scope.alive = Some(false);
        }
        ClientProtocolEvent::SnapshotReceived(frame) => {
            if let Some(lifecycle) = frame.events.iter().find_map(|event| match event {
                SnapshotEvent::LifecycleState(lifecycle) => Some(*lifecycle),
                _ => None,
            }) {
                scope.life_seq = Some(lifecycle.life_seq);
                scope.alive = Some(lifecycle.alive);
            }
        }
        ClientProtocolEvent::ShipFrequencyChanged(change)
            if scope.player_id == Some(change.player_id)
                && change.life_seq.is_some_and(|life_seq| life_seq != 0) =>
        {
            scope.life_seq = change.life_seq;
            scope.alive = Some(change.ship < 8);
        }
        _ => {}
    }
    scope
}

fn exact_soft_lifecycle_target_matches_current_scope(
    event: &ClientProtocolEvent,
    observed: AuthorityScopeIdentity,
    current: AuthorityScopeIdentity,
) -> bool {
    if observed.server_session_id.is_none()
        || observed.server_session_id != current.server_session_id
        || observed.arena_stream_id.is_none()
        || observed.arena_stream_id != current.arena_stream_id
        || observed.player_id.is_none()
        || observed.player_id != current.player_id
    {
        return false;
    }
    let target = match event {
        ClientProtocolEvent::PlayerDeath(death) if current.player_id == Some(death.killed_id) => {
            Some((death.life_seq, false))
        }
        ClientProtocolEvent::SnapshotReceived(frame)
            if current.arena_stream_id == Some(frame.header.arena_stream_id) =>
        {
            frame.events.iter().find_map(|event| match event {
                SnapshotEvent::LifecycleState(lifecycle) => {
                    Some((lifecycle.life_seq, lifecycle.alive))
                }
                _ => None,
            })
        }
        ClientProtocolEvent::ShipFrequencyChanged(change)
            if current.player_id == Some(change.player_id) =>
        {
            change
                .life_seq
                .filter(|life_seq| *life_seq != 0)
                .map(|life_seq| (life_seq, change.ship < 8))
        }
        _ => None,
    };
    target.is_some_and(|(life_seq, alive)| {
        current.life_seq == Some(life_seq) && current.alive == Some(alive)
    })
}

/// Revalidate immutable admissions against the authority that exists at final
/// application. This is deliberately ordered and generation-based: a held
/// fact cannot be rebound by wrapped/reused numeric network identity.
fn admitted_authority_actions(
    actions: &ClientProtocolActions,
    admissions: &[AuthorityEventAdmission],
    mut segment: u64,
    mut scope: AuthorityScopeIdentity,
) -> Vec<bool> {
    actions
        .events
        .iter()
        .zip(admissions)
        .map(|(event, admission)| {
            if matches!(event, ClientProtocolEvent::PreGameplayAuthorityReset) {
                return true;
            }
            if let AuthorityBoundaryAdmission::Hard {
                segment: replacement_segment,
                replacement,
            } = admission.boundary
            {
                if replacement_segment != segment.wrapping_add(1) {
                    return false;
                }
                segment = replacement_segment;
                scope = replacement.unwrap_or_default();
                return true;
            }
            let admitted = match admission.action {
                AuthorityActionAdmission::Neutral => true,
                AuthorityActionAdmission::Bound(observed) => {
                    admission.segment == segment
                        && if admission.readiness_fact != AuthorityReadinessAdmission::Irrelevant {
                            // Readiness is arena-generation authority. A
                            // private snapshot later in this same poll may
                            // discover or advance the current life, but it
                            // must not retire the one-shot settings/SECURITY/
                            // map carrier which precedes it.
                            observed.server_session_id == scope.server_session_id
                                && observed.arena_stream_id == scope.arena_stream_id
                                && observed.player_id == scope.player_id
                        } else if let ClientProtocolEvent::SpawnIdleSpectator(fact) = event {
                            observed.server_session_id == Some(fact.server_session_id)
                                && observed.arena_stream_id == Some(fact.arena_stream_id)
                                && observed.player_id == Some(fact.player_id)
                                && scope.server_session_id == Some(fact.server_session_id)
                                && scope.arena_stream_id == Some(fact.arena_stream_id)
                                && scope.player_id == Some(fact.player_id)
                        } else if let ClientProtocolEvent::ConnectionQualityDirective(fact) = event
                        {
                            // These reliable facts carry their own exact life
                            // and hold tuple. An unordered snapshot can advance
                            // the poll's planning cursor before the reliable
                            // Clear is consumed. Preserve immutable segment and
                            // server/arena/player scope here; the sequencer
                            // checks the hold and predecessor/successor lives.
                            observed.server_session_id == Some(fact.server_session_id)
                                && observed.arena_stream_id == Some(fact.arena_stream_id)
                                && observed.player_id == Some(fact.player_id)
                                && scope.server_session_id == Some(fact.server_session_id)
                                && scope.arena_stream_id == Some(fact.arena_stream_id)
                                && scope.player_id == Some(fact.player_id)
                        } else if let ClientProtocolEvent::LifecycleBaselineAccepted(receipt) =
                            event
                        {
                            // A 0x75 is the application receipt which closes
                            // the exact dead -> alive lifecycle cutover. The
                            // session decoder may already have observed the
                            // new-life live snapshot while Game is still
                            // intentionally held at the same life's dead
                            // boundary. Bind every reusable identity and the
                            // persistent authority segment exactly, but do
                            // not require the transitional `alive` bit to
                            // agree. The handler additionally requires the
                            // exact delivery/life/position tuple before it
                            // can release gameplay.
                            observed.server_session_id.is_some()
                                && observed.server_session_id == scope.server_session_id
                                && observed.arena_stream_id.is_some()
                                && observed.arena_stream_id == scope.arena_stream_id
                                && observed.player_id.is_some()
                                && observed.player_id == scope.player_id
                                && observed.life_seq == Some(receipt.life_seq)
                                && scope.life_seq == Some(receipt.life_seq)
                        } else if admission.boundary == AuthorityBoundaryAdmission::Soft
                            && exact_soft_lifecycle_target_matches_current_scope(
                                event, observed, scope,
                            )
                        {
                            // Browser callbacks from different DataChannels
                            // may be partitioned and appended before Game
                            // consumes the prior deferred snapshot. If that
                            // prior immutable action has already advanced the
                            // evaluator to this event's exact lifecycle
                            // target, admit the transition once against the
                            // same persistent segment and complete
                            // session/stream/player identity. This is not a
                            // neutral rebind: stale generations, other
                            // players, and different target lives remain
                            // rejected.
                            true
                        } else {
                            observed == scope
                        }
                }
                AuthorityActionAdmission::Reject => false,
            };
            if admitted && admission.boundary == AuthorityBoundaryAdmission::Soft {
                scope = advance_scope_for_soft_boundary(scope, event);
            }
            admitted
        })
        .collect()
}

/// Fold one merged browser poll exactly once against the pre-mutation runtime.
/// Each event receives an immutable admission decision aligned by index. The
/// ordered cursor advances only on explicit authority/lifecycle boundaries;
/// consumers never reinterpret an earlier fact after Game or NetworkWorld has
/// already mutated.
pub(in crate::game) fn build_authority_admission_plan(
    actions: &ClientProtocolActions,
    seed: AuthorityAdmissionSeed,
) -> Vec<AuthorityEventAdmission> {
    let origin = seed.scope;
    let mut cursor = origin;
    let mut readiness = seed.readiness;
    let origin_segment = seed.segment;
    let mut segment = origin_segment;
    let mut plan = Vec::with_capacity(actions.events.len());
    for event in &actions.events {
        let mut admission = AuthorityEventAdmission::default();
        let observed_scope = cursor;
        admission.action = if authority_action_is_explicitly_neutral(event) {
            AuthorityActionAdmission::Neutral
        } else if authority_action_requires_pre_session_scope(event)
            && observed_scope.server_session_id.is_some()
        {
            AuthorityActionAdmission::Reject
        } else {
            AuthorityActionAdmission::Bound(observed_scope)
        };
        match event {
            ClientProtocolEvent::Disconnected => {
                segment = segment.wrapping_add(1);
                cursor = AuthorityScopeIdentity::default();
                readiness = AuthorityReadiness::default();
                admission.boundary = AuthorityBoundaryAdmission::Hard {
                    segment,
                    replacement: None,
                };
            }
            ClientProtocolEvent::PlayerIdAssigned(player_id) => {
                segment = segment.wrapping_add(1);
                cursor.player_id = Some(*player_id);
                cursor.life_seq = None;
                cursor.alive = None;
                // A player-id assignment is a hard authority generation even
                // when the numeric id is reused. No settings, SECURITY, or map
                // readiness observed in its predecessor segment may survive;
                // the following exact arena join rebuilds the complete tuple.
                readiness = AuthorityReadiness::default();
                admission.boundary = AuthorityBoundaryAdmission::Hard {
                    segment,
                    replacement: Some(cursor),
                };
            }
            ClientProtocolEvent::JoinedArena(arena_epoch) => {
                segment = segment.wrapping_add(1);
                cursor.life_seq = None;
                cursor.alive = None;
                readiness.joined = true;
                readiness.arena_epoch = Some(*arena_epoch);
                readiness.settings = false;
                readiness.security = false;
                readiness.map = false;
                admission.boundary = AuthorityBoundaryAdmission::Hard {
                    segment,
                    replacement: Some(cursor),
                };
            }
            ClientProtocolEvent::PlayerLeft(player_id) if cursor.player_id == Some(*player_id) => {
                segment = segment.wrapping_add(1);
                cursor.player_id = None;
                cursor.life_seq = None;
                cursor.alive = None;
                readiness = AuthorityReadiness::default();
                admission.boundary = AuthorityBoundaryAdmission::Hard {
                    segment,
                    replacement: Some(cursor),
                };
            }
            ClientProtocolEvent::ServerClockBootstrapped(bootstrap)
                if cursor.server_session_id != Some(bootstrap.server_session_id)
                    || cursor.arena_stream_id != Some(bootstrap.arena_stream_id) =>
            {
                segment = segment.wrapping_add(1);
                cursor = AuthorityScopeIdentity {
                    server_session_id: Some(bootstrap.server_session_id),
                    arena_stream_id: Some(bootstrap.arena_stream_id),
                    ..AuthorityScopeIdentity::default()
                };
                readiness = AuthorityReadiness::default();
                admission.boundary = AuthorityBoundaryAdmission::Hard {
                    segment,
                    replacement: Some(cursor),
                };
            }
            ClientProtocolEvent::ArenaSettingsUpdated(_) => readiness.settings = true,
            ClientProtocolEvent::SecurityChallengeReceived {
                server_session_id,
                arena_epoch,
                ..
            } if *server_session_id == cursor.server_session_id
                && Some(*arena_epoch) == readiness.arena_epoch =>
            {
                readiness.security = true;
            }
            ClientProtocolEvent::ZoneMapPublished => readiness.map = true,
            ClientProtocolEvent::ZoneFilesPending => readiness.map = false,
            ClientProtocolEvent::PlayerDeath(death)
                if cursor.player_id == Some(death.killed_id) =>
            {
                cursor.life_seq = Some(death.life_seq);
                cursor.alive = Some(false);
                admission.boundary = AuthorityBoundaryAdmission::Soft;
            }
            ClientProtocolEvent::ShipFrequencyChanged(change)
                if cursor.player_id == Some(change.player_id)
                    && change.life_seq.is_some_and(|life_seq| life_seq != 0) =>
            {
                cursor.life_seq = change.life_seq;
                cursor.alive = Some(change.ship < 8);
                admission.boundary = AuthorityBoundaryAdmission::Soft;
            }
            _ => {}
        }

        if matches!(admission.boundary, AuthorityBoundaryAdmission::Hard { .. }) {
            admission.action = AuthorityActionAdmission::Bound(cursor);
        }

        if let ClientProtocolEvent::SnapshotReceived(frame) = event {
            let stream_matches = cursor.arena_stream_id == Some(frame.header.arena_stream_id);
            if stream_matches
                && let Some(lifecycle) = frame.events.iter().find_map(|event| match event {
                    SnapshotEvent::LifecycleState(lifecycle) => Some(*lifecycle),
                    _ => None,
                })
                && authoritative_lifecycle_action(cursor.life_seq, cursor.alive, lifecycle)
                    != LifecycleAction::Ignore
            {
                cursor.life_seq = Some(lifecycle.life_seq);
                cursor.alive = Some(lifecycle.alive);
                admission.boundary = AuthorityBoundaryAdmission::Soft;
            }
        }
        admission.segment = segment;
        admission.readiness_after = readiness;
        plan.push(admission);
    }
    // Close only the final ordered generation. Facts observed in an earlier
    // segment can never be rebound merely because wrapped numeric scope ids
    // happen to equal the final cursor. Within one segment, using its final
    // lifecycle cursor lets an ACK that shares a poll with its exact private
    // lifecycle baseline remain ordered without crossing a hard boundary.
    let final_segment = segment;
    let mut snapshot_position_admission = PrivateAuthorityAdmission::Irrelevant;
    for (event, admission) in actions.events.iter().zip(&mut plan) {
        let readiness_carrier = matches!(
            event,
            ClientProtocolEvent::ArenaSettingsUpdated(_)
                | ClientProtocolEvent::SecurityChallengeReceived { .. }
                | ClientProtocolEvent::ZoneFilesPending
                | ClientProtocolEvent::MapDownloaded(_)
                | ClientProtocolEvent::ZoneMapPublished
                | ClientProtocolEvent::ZoneFilesPublished
        );
        if admission.segment != final_segment {
            admission.readiness_fact = if readiness_carrier {
                AuthorityReadinessAdmission::Reject
            } else {
                AuthorityReadinessAdmission::Irrelevant
            };
            admission.private_fact = if matches!(
                event,
                ClientProtocolEvent::SnapshotReceived(_)
                    | ClientProtocolEvent::PreGameplayPrizeState { .. }
                    | ClientProtocolEvent::PrizePickupAck(_)
                    | ClientProtocolEvent::PrizeEffectAccepted(_)
                    | ClientProtocolEvent::PlayerPosition(_)
                    | ClientProtocolEvent::SnapshotPlayerPosition { .. }
            ) {
                PrivateAuthorityAdmission::Reject
            } else {
                PrivateAuthorityAdmission::Irrelevant
            };
            if admission.boundary == AuthorityBoundaryAdmission::None
                && admission.action != AuthorityActionAdmission::Neutral
            {
                admission.action = AuthorityActionAdmission::Reject;
            }
            snapshot_position_admission = PrivateAuthorityAdmission::Irrelevant;
            continue;
        }
        admission.readiness_fact = if readiness_carrier {
            let exact_security_scope = match event {
                ClientProtocolEvent::SecurityChallengeReceived {
                    server_session_id,
                    arena_epoch,
                    ..
                } => {
                    *server_session_id == cursor.server_session_id
                        && Some(*arena_epoch) == readiness.arena_epoch
                }
                _ => true,
            };
            if !exact_security_scope {
                AuthorityReadinessAdmission::Reject
            } else if final_segment == origin_segment {
                AuthorityReadinessAdmission::Current
            } else {
                AuthorityReadinessAdmission::Replacement
            }
        } else {
            AuthorityReadinessAdmission::Irrelevant
        };
        admission.private_fact = match event {
            ClientProtocolEvent::SnapshotReceived(frame) => {
                let snapshot_life = frame.events.iter().find_map(|event| match event {
                    SnapshotEvent::PrizeState(prize) => Some(prize.life_seq),
                    SnapshotEvent::LifecycleState(lifecycle) => Some(lifecycle.life_seq),
                    SnapshotEvent::RocketState(rocket) => Some(rocket.life_seq),
                    _ => None,
                });
                let private = private_admission_for_scope(
                    origin,
                    origin_segment,
                    cursor,
                    final_segment,
                    cursor.server_session_id,
                    Some(frame.header.arena_stream_id),
                    cursor.player_id,
                    snapshot_life.or(cursor.life_seq),
                    frame
                        .events
                        .iter()
                        .find_map(|event| match event {
                            SnapshotEvent::LifecycleState(lifecycle) => Some(lifecycle.alive),
                            _ => None,
                        })
                        .or(cursor.alive),
                );
                snapshot_position_admission = private;
                private
            }
            // The session synthesizes every near-entity position immediately
            // after its SnapshotReceived carrier. All of them inherit the
            // carrier generation, not only the self entity: otherwise a
            // rejected old snapshot could repopulate remote world/weapon
            // presentation after a hard reset.
            ClientProtocolEvent::PlayerPosition(_)
            | ClientProtocolEvent::SnapshotPlayerPosition { .. } => snapshot_position_admission,
            ClientProtocolEvent::PreGameplayPrizeState { state, .. } => {
                private_admission_for_scope(
                    origin,
                    origin_segment,
                    cursor,
                    final_segment,
                    cursor.server_session_id,
                    cursor.arena_stream_id,
                    cursor.player_id,
                    Some(state.life_seq),
                    Some(true),
                )
            }
            ClientProtocolEvent::PrizePickupAck(scoped) => private_admission_for_scope(
                origin,
                origin_segment,
                cursor,
                final_segment,
                scoped.server_session_id,
                scoped.arena_stream_id,
                scoped.player_id,
                Some(scoped.ack.life_seq),
                Some(true),
            ),
            ClientProtocolEvent::PrizeEffectAccepted(receipt) => private_admission_for_scope(
                origin,
                origin_segment,
                cursor,
                final_segment,
                Some(receipt.server_session_id),
                cursor.arena_stream_id,
                Some(receipt.player_id),
                Some(receipt.life_seq),
                Some(true),
            ),
            ClientProtocolEvent::PortalPlacementAccepted(receipt) => private_admission_for_scope(
                origin,
                origin_segment,
                cursor,
                final_segment,
                Some(receipt.server_session_id),
                Some(receipt.arena_stream_id),
                Some(receipt.player_id),
                Some(receipt.life_seq),
                Some(true),
            ),
            ClientProtocolEvent::PortalWarpAccepted(receipt) => private_admission_for_scope(
                origin,
                origin_segment,
                cursor,
                final_segment,
                Some(receipt.server_session_id),
                Some(receipt.arena_stream_id),
                Some(receipt.player_id),
                Some(receipt.life_seq),
                Some(true),
            ),
            _ => {
                snapshot_position_admission = PrivateAuthorityAdmission::Irrelevant;
                PrivateAuthorityAdmission::Irrelevant
            }
        };
        if admission.private_fact != PrivateAuthorityAdmission::Irrelevant {
            admission.readiness_after = readiness;
        }
    }
    // Snapshot WATCH_DAMAGE is decoded into a presentation action immediately
    // before its carrier frame. Bind that action back to the exact carrier's
    // immutable admission; reliable 0x7d reports have no source snapshot id
    // and deliberately remain ordinary current-stream events.
    for index in 0..actions.events.len() {
        let ClientProtocolEvent::DamageWatched(report) = &actions.events[index] else {
            continue;
        };
        let Some(snapshot_id) = report
            .entries
            .last()
            .and_then(|entry| entry.source_snapshot_id)
        else {
            continue;
        };
        let carrier_index = actions.events[index + 1..]
            .iter()
            .position(|event| {
                matches!(
                    event,
                    ClientProtocolEvent::SnapshotReceived(frame)
                        if frame.header.snapshot_id == snapshot_id
                )
            })
            .map(|offset| index + 1 + offset);
        let Some(carrier_index) = carrier_index else {
            plan[index].private_fact = PrivateAuthorityAdmission::Reject;
            continue;
        };
        plan[index].private_fact = if plan[index].segment == plan[carrier_index].segment {
            plan[carrier_index].private_fact
        } else {
            PrivateAuthorityAdmission::Reject
        };
        plan[index].readiness_after = plan[carrier_index].readiness_after;
    }
    plan
}

#[cfg(test)]
fn prize_pickup_ack_is_overtaken_in_action_batch(
    actions: &ClientProtocolActions,
    self_player_id: Option<u16>,
    current_life_seq: Option<u32>,
    current_alive: Option<bool>,
    ack: PrizePickupAck,
) -> bool {
    build_authority_admission_plan(
        actions,
        AuthorityAdmissionSeed {
            scope: AuthorityScopeIdentity {
                server_session_id: Some(ack.server_session_id),
                arena_stream_id: Some(ack.arena_stream_id),
                player_id: self_player_id,
                life_seq: current_life_seq,
                alive: current_alive,
            },
            readiness: AuthorityReadiness::default(),
            segment: 0,
        },
    )
    .iter()
    .any(|admission| admission.boundary != AuthorityBoundaryAdmission::None)
}

fn authoritative_engine_shutdown_deadline_by_revision<I>(
    snapshot: Option<(u32, u32, Option<u32>, bool)>,
    accepted: Option<AcceptedPrizeEngineShutdown>,
    pending: I,
) -> Option<u32>
where
    I: IntoIterator<Item = (u32, u32, Option<u32>)>,
{
    let authoritative_life = snapshot.map(|(life_seq, _, _, _)| life_seq);
    // A pending snapshot source proves the claim survived an unrelated
    // private-state revision, but does not activate Shutdown before the exact
    // pickup ACK/application boundary. While it remains pending, an ACK or
    // accepted terminal for that life is allowed to supply the older source
    // revision which the newer aggregate snapshot intentionally carries.
    let snapshot_application_pending = snapshot.is_some_and(|(_, _, _, pending)| pending);
    let mut selected = snapshot
        .filter(|(_, _, _, pending)| !pending)
        .map(|(life, revision, deadline, _)| (life, revision, deadline));
    let mut consider = |candidate: (u32, u32, Option<u32>)| {
        if authoritative_life.is_some_and(|life_seq| life_seq != candidate.0) {
            return;
        }
        if snapshot_application_pending
            || selected.is_none_or(|(_, revision, _)| {
                let advance = candidate.1.wrapping_sub(revision);
                advance != 0 && advance < (1 << 31)
            })
        {
            selected = Some(candidate);
        }
    };
    if let Some(accepted) = accepted {
        consider((
            accepted.life_seq,
            accepted.prize_state_revision,
            accepted.until_tick,
        ));
    }
    for candidate in pending {
        consider(candidate);
    }
    selected.and_then(|(_, _, deadline)| deadline)
}

fn damage_prize_notification_is_new(
    previous_life_seq: Option<u32>,
    previous_damage_seq: Option<u32>,
    previous_prize_id: Option<i16>,
    life_seq: u32,
    damage_seq: u32,
    prize_id: i16,
) -> bool {
    (previous_life_seq, previous_damage_seq, previous_prize_id)
        != (Some(life_seq), Some(damage_seq), Some(prize_id))
}

fn newer_same_life_boundary_requires_correction_cancellation(
    unpublished_boundary_position: bool,
    pending_warp_relocation_collectible_id: Option<u32>,
    pending_repel_application: Option<crate::continuum_position::ClientRepelApplicationMarker>,
) -> bool {
    unpublished_boundary_position
        || pending_warp_relocation_collectible_id.is_some()
        || pending_repel_application.is_some()
}

fn apply_frequency_baseline_rebase(
    ship: &mut Ship,
    player_bounty: &mut u32,
    last_self_server_energy: &mut Option<f32>,
    baseline: &ClientPositionUpdate,
) {
    // The application receipt acknowledges the server's mandatory exact-f32
    // baseline, not merely its legacy integer projection. Reconstructing from
    // x/y/v/direction here would discard subpixel and sub-bin state, so the
    // very next authenticated record could not replay bit-for-bit from the
    // witness the server committed.
    let (position_x, position_y, velocity_x, velocity_y, orientation) =
        baseline.canonical_state.values();
    ship.pos = vec2(position_x, position_y);
    ship.vel = vec2(velocity_x, velocity_y);
    ship.orientation = orientation;
    ship.stealth = baseline.togglables & STATUS_STEALTH != 0;
    ship.cloak = baseline.togglables & STATUS_CLOAK != 0;
    ship.xradar = baseline.togglables & STATUS_XRADAR != 0;
    ship.antiwarp = baseline.togglables & STATUS_ANTIWARP != 0;
    ship.flash = baseline.togglables & STATUS_FLASH != 0;
    *player_bounty = u32::from(baseline.bounty);
    ship.energy = f32::from(baseline.energy);
    *last_self_server_energy = Some(ship.energy);
}

fn warp_relocation_receipt_matches(
    pending_collectible_id: Option<u32>,
    pending_life_seq: Option<u32>,
    emitted_sequences: &std::collections::VecDeque<u16>,
    receipt: WarpRelocationAccepted,
) -> bool {
    receipt.life_seq != 0
        && pending_collectible_id == Some(receipt.collectible_id)
        && pending_life_seq == Some(receipt.life_seq)
        && emitted_sequences.contains(&receipt.position_sequence)
}

fn repel_application_receipt_matches(
    server_session_id: Option<u64>,
    life_seq: Option<u32>,
    pending: Option<crate::continuum_position::ClientRepelApplicationMarker>,
    receipt: RepelApplicationAccepted,
) -> bool {
    pending.is_some_and(|pending| {
        receipt.server_session_id != 0
            && receipt.life_seq != 0
            && server_session_id == Some(receipt.server_session_id)
            && life_seq == Some(receipt.life_seq)
            && pending.delivery_generation == receipt.delivery_generation
            && pending.snapshot_id == receipt.snapshot_id
            && pending.application_sequence == receipt.application_sequence
            && receipt.active_steps != 0
            && receipt.accepted_end_sequence
                == pending
                    .application_sequence
                    .wrapping_add(receipt.active_steps)
    })
}

fn expired_repel_application_receipt_index(
    server_session_id: Option<u64>,
    life_seq: Option<u32>,
    expired: &std::collections::VecDeque<crate::continuum_position::ClientRepelApplicationMarker>,
    receipt: RepelApplicationAccepted,
) -> Option<usize> {
    expired.iter().position(|marker| {
        repel_application_receipt_matches(server_session_id, life_seq, Some(*marker), receipt)
    })
}

fn private_repel_provisional_steps(
    snapshot_tick: u32,
    expires_at_tick: u32,
    offered_active_steps: u16,
    observed_server_tick: u32,
) -> Option<u16> {
    if offered_active_steps == 0
        || crate::continuum::continuum_tick_reached(snapshot_tick, expires_at_tick)
    {
        return None;
    }
    // The frame itself proves that application cannot precede its authored
    // snapshot tick. Browser clock extrapolation may lag a newly received
    // snapshot by one render poll, so clamp that stale estimate forward to
    // the frame tick. Never clamp a forward estimate backward: time already
    // elapsed still shortens the immutable suffix.
    let application_tick =
        if crate::continuum::continuum_tick_diff(observed_server_tick, snapshot_tick) >= 0 {
            observed_server_tick
        } else {
            snapshot_tick
        };
    if crate::continuum::continuum_tick_reached(application_tick, expires_at_tick) {
        return None;
    }
    let remaining_ticks =
        crate::continuum::continuum_tick_diff(expires_at_tick, application_tick).max(0) as u32;
    let active_steps = u16::try_from(crate::gameplay_rules::repel_active_steps_for_server_ticks(
        remaining_ticks,
    ))
    .ok()?;
    (active_steps > 0 && active_steps <= offered_active_steps).then_some(active_steps)
}

fn repel_receipt_remaining_steps(next_input_sequence: u16, accepted_end_sequence: u16) -> u16 {
    let end_exclusive = accepted_end_sequence.wrapping_add(1);
    let remaining = end_exclusive.wrapping_sub(next_input_sequence);
    (remaining < (1 << 15)).then_some(remaining).unwrap_or(0)
}

fn repel_receipt_movement_window(
    next_input_sequence: u16,
    application_sequence: u16,
    accepted_end_sequence: u16,
    active_steps: u16,
) -> Option<(u16, u16)> {
    (repel_receipt_remaining_steps(next_input_sequence, accepted_end_sequence) > 0).then(|| {
        crate::gameplay_rules::repel_sequence_window_from_active_steps(
            application_sequence,
            u32::from(active_steps),
        )
    })
}

fn input_sequence_precedes(candidate: u16, later: u16) -> bool {
    let distance = later.wrapping_sub(candidate);
    distance != 0 && distance < (1_u16 << 15)
}

#[cfg(any(test, target_arch = "wasm32"))]
fn browser_server_tick_across_owner_poll(
    live_owner_tick: Option<u32>,
    retained_tick: Option<u32>,
) -> Option<u32> {
    live_owner_tick.or(retained_tick)
}

pub(in crate::game) fn authoritative_lifecycle_action(
    previous_life_seq: Option<u32>,
    previous_alive: Option<bool>,
    candidate: SnapshotLifecycleState,
) -> LifecycleAction {
    match previous_life_seq {
        None => {
            if candidate.alive {
                LifecycleAction::Observe
            } else {
                LifecycleAction::StartDeath
            }
        }
        Some(previous) if candidate.life_seq == previous => match (previous_alive, candidate.alive)
        {
            (Some(false), true) | (None, _) => LifecycleAction::Observe,
            _ => LifecycleAction::Ignore,
        },
        Some(previous) if candidate.life_seq == previous.wrapping_add(1).max(1) => {
            if candidate.alive {
                LifecycleAction::Observe
            } else {
                LifecycleAction::StartDeath
            }
        }
        Some(_) => LifecycleAction::Ignore,
    }
}

/// [REVIEW M10] Reconcile predicted self-energy against the latest server
/// snapshot. Within the recharge-lag tolerance, prediction leads (the
/// energy used for fire/warp gating is no longer dragged to a stale
/// value); real downward divergence adopts the server immediately; an
/// above-prediction server converges smoothly. `visible_hud_energy` separately
/// ensures the display never outruns the server-fed value while preserving
/// an authoritative negative-Energy transient above the new capacity.
fn reconciled_self_energy(predicted: f32, server: f32) -> f32 {
    if server + SELF_ENERGY_PREDICTION_TOLERANCE < predicted {
        server
    } else if server > predicted {
        predicted + (server - predicted) * SELF_ENERGY_UPWARD_BLEND
    } else {
        predicted
    }
}

impl Game {
    pub(in crate::game) fn expire_pending_repel_application_at_tick(&mut self, server_tick: u32) {
        if self
            .pending_repel_application_expiry_tick
            .is_some_and(|deadline| crate::continuum::continuum_tick_reached(server_tick, deadline))
        {
            if let Some(expired) = self.pending_repel_application.take() {
                self.expired_repel_applications.push_back(expired);
                while self.expired_repel_applications.len() > 256 {
                    self.expired_repel_applications.pop_front();
                }
            }
            self.pending_repel_application_expiry_tick = None;
            // The immutable wall deadline retires receipt waiting and blocks
            // reapplication. An impulse already applied to input history owns
            // a bounded sequence window, just as on the server. Slow frames
            // can leave steps in that window after wall expiry; clearing it
            // here clamps velocity before the canonical server replay does.
            if self.repel_movement_sequence_window.is_none() {
                self.ship.repel_timer = 0.0;
            }
            self.repel_delivery_expired_application_cancellations = self
                .repel_delivery_expired_application_cancellations
                .saturating_add(1);
            self.apply_deferred_warp_pickup_acks();
        }
    }

    pub(in crate::game) fn prize_reconciliation_cutover_pending(&self) -> bool {
        let authority = self.client_authority.view();
        // The reliable ship ACK establishes the successor identity. Its own
        // private loadout must be installable before the neutral baseline:
        // authored initial energy can differ from the server's maxed policy.
        // A request alone, frequency/correction cutovers and dead respawn
        // snapshots still cannot reconcile private runtime here.
        let confirmed_ship_successor = authority.transition_kind
            == Some(ClientAuthorityTransitionKind::Ship)
            && authority.scope.is_complete_live()
            && (authority.neutral_ship_baseline_ready()
                || authority.phase == ClientAuthorityPhase::AwaitingApplicationReceipt);
        prize_state_transition_blocks_reconciliation(
            authority.transition_kind,
            authority.scope.alive,
        ) && !confirmed_ship_successor
    }

    pub(in crate::game) fn current_life_loadout_baseline_ready(&self) -> bool {
        let scope = self.client_authority.view().scope;
        scope.alive == Some(true)
            && scope.life_seq == self.last_self_server_prize_life_seq
            && self.last_self_server_prize_revision.is_some()
            && self.last_self_server_energy.is_some()
    }

    pub(in crate::game) fn normalize_ship_life_reset_pose(&mut self) {
        // SetCoordinates follows the legacy half-pixel presentation rule, but
        // a zubspace ship-life baseline is an exact authority transaction at
        // the canonical map spawn. Re-anchor both local prediction and the
        // raw-f32 provenance before constructing that reliable baseline so
        // `(spawn + 0.5).round()` cannot turn 8200 into 8201 on the wire.
        self.ship.pos = self.player_spawn_position();
        self.ship.vel = Vec2::ZERO;
        self.ship.orientation = 0.0;
    }

    fn apply_accepted_ship_life_pose(&mut self, accepted_position: Option<(u16, u16)>) {
        // Reset already ran at the ship ACK / respawn eligibility boundary.
        // The receipt commits its exact pose, not a second grant of arena
        // defaults that could overwrite an installed private maximum loadout.
        self.normalize_ship_life_reset_pose();
        if let Some((x, y)) = accepted_position {
            // Applied only after the sequencer matches delivery, life and
            // input sequence. Prediction stays held until this transaction;
            // duplicate or stale receipts never enter this consumer.
            self.ship.pos = vec2(f32::from(x), f32::from(y));
        }
        let life = self.client_authority.view().scope.life_seq;
        if life != self.last_self_server_prize_life_seq
            || self.last_self_server_prize_revision.is_none()
        {
            // The reliable receipt can overtake the replacement private
            // snapshot. Neither canonical movement nor outbound records may
            // integrate reset tuning while the server already uses upgrades.
            self.network_prize_baseline_pending = true;
        }
    }
}

pub(in crate::game) fn self_runtime_snapshot_is_admissible(
    authoritative_alive: Option<bool>,
    respawn_baseline_pending: bool,
) -> bool {
    // The server intentionally keeps the replacement life dead until its
    // exact neutral baseline is admitted. A snapshot built from the final
    // dead position can therefore arrive after the local respawn timer has
    // reset the ship but before that baseline is accepted. Letting the old
    // zero energy overwrite the armed reset would make the baseline identify
    // itself as non-respawn and deadlock the lifecycle receipt barrier.
    !(authoritative_alive == Some(false) && respawn_baseline_pending)
}

fn life_sequence_is_current_or_forward(previous: Option<u32>, candidate: u32) -> bool {
    previous.map_or(true, |last| {
        candidate == last || candidate.wrapping_sub(last) < (1_u32 << 31)
    })
}

fn rocket_life_sequence_is_admissible(
    previous: Option<u32>,
    candidate: u32,
    require_strictly_forward: bool,
) -> bool {
    match previous {
        None => true,
        Some(last) if require_strictly_forward => {
            // Server life generations advance exactly once per lifecycle
            // boundary and reserve zero. Do not let an arbitrary forward
            // snapshot skip the tracked reset boundary and unlatch it.
            candidate == last.wrapping_add(1).max(1)
        }
        Some(_) => life_sequence_is_current_or_forward(previous, candidate),
    }
}

fn rocket_state_is_admissible_across_lifecycle_cutover(
    lifecycle_cutover_pending: bool,
    previous_life_seq: Option<u32>,
    candidate_life_seq: u32,
    require_strictly_forward: bool,
) -> bool {
    !lifecycle_cutover_pending
        && rocket_life_sequence_is_admissible(
            previous_life_seq,
            candidate_life_seq,
            require_strictly_forward,
        )
}

pub(in crate::game) fn prize_state_is_admissible_across_lifecycle_cutover(
    lifecycle_cutover_pending: bool,
    authoritative_life_seq: Option<u32>,
    candidate_life_seq: u32,
) -> bool {
    !lifecycle_cutover_pending && authoritative_life_seq == Some(candidate_life_seq)
}

pub(in crate::game) fn prize_state_transition_blocks_reconciliation(
    transition_kind: Option<ClientAuthorityTransitionKind>,
    authoritative_alive: Option<bool>,
) -> bool {
    matches!(
        transition_kind,
        Some(
            ClientAuthorityTransitionKind::Ship
                | ClientAuthorityTransitionKind::Frequency
                | ClientAuthorityTransitionKind::Correction
        )
    ) || (transition_kind == Some(ClientAuthorityTransitionKind::Respawn)
        && authoritative_alive != Some(true))
}

fn authoritative_rocket_timer_seconds(
    rocket: SnapshotRocketState,
    current_server_tick: u32,
) -> f32 {
    if !rocket.active {
        return 0.0;
    }
    let Some(until_tick) = rocket.until_tick else {
        return 0.0;
    };
    crate::continuum::ticks_to_seconds_i32(
        crate::continuum::continuum_tick_diff(until_tick, current_server_tick).max(0),
    )
}

fn reconciled_self_rocket_timer_seconds(
    predicted_seconds: f32,
    rocket_lifetime_seconds: f32,
    rocket: SnapshotRocketState,
    current_server_tick: u32,
    next_input_sequence: u16,
) -> f32 {
    crate::gameplay_rules::reconciled_rocket_timer_seconds(
        predicted_seconds,
        rocket_lifetime_seconds,
        crate::fixed_step::GAMEPLAY_STEP_SECONDS,
        rocket.activation_sequence,
        next_input_sequence.wrapping_sub(1),
        authoritative_rocket_timer_seconds(rocket, current_server_tick),
        rocket.active,
    )
}

fn current_rocket_reconciliation_tick(frame_tick: u32, synchronized_tick: Option<u32>) -> u32 {
    synchronized_tick
        .filter(|tick| crate::continuum::continuum_tick_diff(*tick, frame_tick) >= 0)
        .unwrap_or(frame_tick)
}

fn authoritative_respawn_delay_seconds(respawn_at_tick: Option<u32>, current_tick: u32) -> f32 {
    respawn_at_tick.map_or(0.0, |deadline| {
        crate::continuum::ticks_to_seconds_i32(
            crate::continuum::continuum_tick_diff(deadline, current_tick).max(0),
        )
    })
}

pub(super) fn authoritative_prize_timer_seconds(deadline: Option<u32>, current_tick: u32) -> f32 {
    deadline.map_or(0.0, |deadline| {
        crate::continuum::ticks_to_seconds_i32(
            crate::continuum::continuum_tick_diff(deadline, current_tick).max(0),
        )
    })
}

fn prize_revision_is_admissible(previous: Option<u32>, candidate: u32) -> bool {
    candidate != 0
        && previous.is_none_or(|previous| {
            candidate == previous || {
                let delta = candidate.wrapping_sub(previous);
                delta != 0 && delta < (1_u32 << 31)
            }
        })
}

pub(in crate::game) fn rocket_state_resolves_pending_activation(
    pending_sequence: Option<u16>,
    rocket: SnapshotRocketState,
) -> bool {
    let Some(pending_sequence) = pending_sequence else {
        return true;
    };
    rocket.activation_sequence == Some(pending_sequence)
        || rocket.observed_input_sequence == Some(pending_sequence)
        || rocket.input_sequence_watermark.is_some_and(|watermark| {
            let distance = watermark.wrapping_sub(pending_sequence);
            distance < (1_u16 << 15) && distance >= ROCKET_INPUT_RECOVERY_WINDOW
        })
}

#[allow(clippy::too_many_arguments)]
fn commit_accepted_lifecycle_baseline_rocket_lifecycle(
    life_seq: u32,
    last_life_seq: &mut Option<u32>,
    require_forward_life: &mut bool,
    last_activation_sequence: &mut Option<u16>,
    last_until_tick: &mut Option<u32>,
    pending_activation_sequence: &mut Option<u16>,
    rocket_timer: &mut f32,
    rocket_cooldown: &mut f32,
    last_input: &mut PlayerInput,
) {
    // A matched cutover ACK is the reliable authority boundary itself. Seed
    // reconciliation with its new life immediately so a delayed snapshot
    // from the old frequency cannot restore RocketTime across u32 wrap.
    *last_life_seq = Some(life_seq);
    *require_forward_life = false;
    *last_activation_sequence = None;
    *last_until_tick = None;
    *pending_activation_sequence = None;
    *rocket_timer = 0.0;
    *rocket_cooldown = 0.0;
    last_input.rocket_activation_pulse = false;
}

fn json_opt_u16(value: Option<u16>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn json_opt_u32(value: Option<u32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn json_opt_f32(value: Option<f32>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "null".to_string())
}

fn json_opt_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "null",
    }
}

impl Game {
    fn remember_network_damage(&mut self, segment: u64, report: &ServerDamageReport) -> bool {
        let Some(entry) = report.entries.last() else {
            return true;
        };
        let (Some(victim_life_seq), Some(damage_seq)) = (entry.victim_life_seq, entry.damage_seq)
        else {
            return true;
        };
        let identity = NetworkDamageIdentity {
            authority_segment: segment,
            victim_player_id: report.player_id,
            victim_life_seq,
            damage_seq,
        };
        if self.recent_network_damage.contains(&identity) {
            return false;
        }

        const CAPACITY: usize = crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS;
        if let Some((latest_segment, latest)) = self.latest_network_damage_sequence {
            if latest_segment == segment {
                let advance = damage_seq.wrapping_sub(latest);
                if advance != 0 && advance < (1 << 31) {
                    self.latest_network_damage_sequence = Some((segment, damage_seq));
                } else if latest.wrapping_sub(damage_seq) as usize >= CAPACITY {
                    return false;
                }
            } else {
                self.latest_network_damage_sequence = Some((segment, damage_seq));
            }
        } else {
            self.latest_network_damage_sequence = Some((segment, damage_seq));
        }

        self.recent_network_damage.push_back(identity);
        while self.recent_network_damage.len() > CAPACITY {
            self.recent_network_damage.pop_front();
        }
        true
    }

    fn remember_network_death(&mut self, segment: u64, death: PlayerDeath) -> bool {
        let identity = NetworkDeathIdentity {
            authority_segment: segment,
            killed_id: death.killed_id,
            life_seq: death.life_seq,
            green_collectible_id: death.green_collectible_id,
            respawn_at_tick: death.respawn_at_tick,
        };
        if self.recent_network_deaths.contains(&identity) {
            return false;
        }
        self.recent_network_deaths.push_back(identity);
        while self.recent_network_deaths.len() > 256 {
            self.recent_network_deaths.pop_front();
        }
        true
    }

    fn network_authority_clock_seconds(&self) -> f64 {
        #[cfg(test)]
        if self.assets.0.is_none() {
            return 0.0;
        }
        get_time()
    }

    pub(in crate::game) fn clear_prize_effect_application_state(&mut self) {
        self.client_authority
            .clear_prize_effect_application_barriers();
        self.accepted_prize_engine_shutdown = None;
        self.last_self_server_engine_shutdown_until_tick = None;
        self.last_self_server_emp_until_tick = None;
        self.last_self_server_engine_shutdown_application_pending = false;
        self.ship.engine_shutdown_timer = 0.0;
        self.ship.emp_shutdown_timer = 0.0;
    }

    pub(in crate::game) fn clear_portal_placement_state(&mut self) {
        self.pending_portal_placements.clear();
        self.client_authority.clear_portal_warp_barrier();
        self.last_self_server_portals = None;
        self.portal = None;
        self.portal_endpoint_authority = None;
    }

    fn invalidate_authoritative_prize_runtime(&mut self) {
        self.clear_prize_effect_application_state();
        self.clear_portal_placement_state();
        self.applied_collectible_acks.clear();
        self.last_self_server_prize_life_seq = None;
        self.last_self_server_prize_revision = None;
        self.server_maxed_spawn_loadout = None;
        self.last_self_server_rocket_life_seq = None;
        self.last_self_server_rocket_activation_sequence = None;
        self.last_self_server_rocket_until_tick = None;
        self.require_forward_self_server_rocket_life = false;
        self.last_self_server_damage_prize_loss_seq = None;
        self.last_self_server_damage_prize_loss_id = None;
        self.last_self_server_damage_prize_loss_life_seq = None;
        self.last_self_server_damage_prize_loss_revision = None;
        self.player_bounty = 0;
        self.ship.energy = 0.0;
        self.ship.max_energy = 0.0;
        self.last_self_server_energy = None;
        // Movement tuning is the one thing this window must not destroy. `top_speed`
        // is `effective_top_speed()` verbatim (`state.rs:89`) and feeds
        // `MovementParams.base_top_speed` into the physics clamp, so writing `0.0` here
        // immobilised the ship for the whole baseline wait - which the frame step used
        // to hide by not running at all. Reset to the arena base tuning instead, the
        // same source `reset_ship` uses (`actions/lifecycle.rs:151-154`); the first
        // authoritative snapshot overwrites all four values from server prize bits
        // below (`apply_self_server_prize_state`, `network.rs:1468`). Prize-derived
        // capability flags and every weapon, energy and prize counter above stay
        // zeroed, so a stepped ship can fly and nothing else: it cannot fire, collect,
        // cloak or spend, and outbound input stays blocked independently by
        // `can_send_browser_gameplay_action`.
        let tuning = self.ship_tuning();
        self.ship.rotation_per_second = tuning.rotation_per_second;
        self.ship.thrust_acceleration = tuning.thrust_acceleration;
        self.ship.top_speed = tuning.top_speed;
        self.ship.recharge_per_second = tuning.energy_recharge;
        self.ship.bouncing_bullets = false;
        self.ship.multifire_capable = false;
        self.ship.multifire = false;
        self.ship.proximity_bombs = false;
        self.ship.stealth_capable = false;
        self.ship.stealth = false;
        self.ship.cloak_capable = false;
        self.ship.cloak = false;
        self.ship.xradar_capable = false;
        self.ship.xradar = false;
        self.ship.antiwarp_capable = false;
        self.ship.antiwarp = false;
        self.ship.gun_level = 0;
        self.ship.bomb_level = 0;
        self.ship.shrapnel_upgrades = 0;
        self.ship.repels = 0;
        self.ship.bursts = 0;
        self.ship.decoys = 0;
        self.ship.thors = 0;
        self.ship.bricks = 0;
        self.ship.portals = 0;
        self.ship.rockets = 0;
        self.ship.super_timer = 0.0;
        self.ship.shield_timer = 0.0;
        self.ship.rocket_timer = 0.0;
        self.ship.rocket_cooldown = 0.0;
        self.pending_rocket_activation_sequence = None;
        self.pending_warp_relocation_collectible_id = None;
        self.pending_warp_relocation_life_seq = None;
        self.pending_warp_relocation_sequences.clear();
        self.client_authority.clear_deferred_warp_pickup_acks();
        self.pending_weapon = 0;
        self.cancel_pending_weapon_prediction();
        self.network_prize_baseline_pending = true;
    }

    #[cfg(test)]
    pub(in crate::game) fn install_network_test_player_identity(&mut self, player_id: u16) {
        let authority = self.client_authority.view();
        self.client_authority.install_test_scope(
            authority.segment,
            AuthorityScopeIdentity {
                player_id: Some(player_id),
                ..authority.scope
            },
            authority.readiness,
        );
        self.network_world
            .install_test_viewer_player_id(Some(player_id));
    }

    #[cfg(test)]
    fn install_network_authority_test_scope(
        &mut self,
        scope: AuthorityScopeIdentity,
        readiness: AuthorityReadiness,
        arena_epoch: u32,
    ) {
        self.install_network_authority_test_scope_at_segment(0, scope, readiness, arena_epoch);
    }

    #[cfg(test)]
    fn install_network_authority_test_scope_at_segment(
        &mut self,
        segment: u64,
        scope: AuthorityScopeIdentity,
        readiness: AuthorityReadiness,
        arena_epoch: u32,
    ) {
        self.networked_play = true;
        self.client_authority.install_test_scope(
            segment,
            scope,
            ClientAuthorityReadiness {
                joined: readiness.joined,
                arena_epoch: readiness.arena_epoch,
                settings: readiness.settings,
                security: readiness.security,
                map: readiness.map,
                files: readiness.map,
            },
        );
        self.network_world.reset_presentation_scope(scope.player_id);
        if readiness.joined {
            self.network_world
                .apply_planned_action(&ClientProtocolEvent::JoinedArena(arena_epoch), None);
        }
        self.network_settings_received = readiness.settings;
        self.network_security_synchronized = readiness.security;
        self.network_zone_map_ready = readiness.map;
        self.network_security_generation =
            scope
                .server_session_id
                .map(|server_session_id| NetworkSecurityGeneration {
                    server_session_id,
                    arena_epoch,
                });
        self.network_security_challenge =
            readiness.security.then_some(SecurityChallenge::default());
        self.network_security_challenge_generation = readiness
            .security
            .then_some(self.network_security_generation)
            .flatten();
        self.door_clock_fresh = readiness.security;
        self.network_prize_baseline_pending = false;
    }

    /// Reconcile Shutdown by authoritative source revision, never by the
    /// longest duration. A newer positive claim may intentionally replace a
    /// longer negative claim, while a pending same-revision marker still
    /// pauses gameplay until its exact server terminal arrives.
    pub(in crate::game) fn reconcile_authoritative_engine_shutdown(&mut self) {
        self.reconcile_authoritative_engine_shutdown_at_tick(
            self.browser_server_tick().unwrap_or(0),
        );
    }

    fn reconcile_authoritative_engine_shutdown_at_tick(&mut self, current_server_tick: u32) {
        let snapshot = self
            .last_self_server_prize_life_seq
            .zip(self.last_self_server_prize_revision)
            .map(|(life_seq, revision)| {
                (
                    life_seq,
                    revision,
                    self.last_self_server_engine_shutdown_until_tick,
                    self.last_self_server_engine_shutdown_application_pending,
                )
            });
        let deadline = authoritative_engine_shutdown_deadline_by_revision(
            snapshot,
            self.accepted_prize_engine_shutdown,
            self.client_authority.pending_prize_engine_shutdowns(),
        );
        self.ship.engine_shutdown_timer =
            authoritative_prize_timer_seconds(deadline, current_server_tick);
        // EMP gates recharge only; the independent prize deadline above owns
        // engine Shutdown and its movement/weapon restrictions.
        self.ship.emp_shutdown_timer = authoritative_prize_timer_seconds(
            self.last_self_server_emp_until_tick,
            current_server_tick,
        );
    }

    fn finish_network_prize_baseline_if_complete(&mut self) {
        if self.current_life_loadout_baseline_ready() {
            self.network_prize_baseline_pending = false;
            self.clear_initial_join_wait_notice();
        }
    }

    /// Apply one already-authenticated recipient-private PrizeState. Browser
    /// polling extracts this exact event before the fixed step; native/direct
    /// action application reaches the same helper from SnapshotReceived.
    /// Returning whether it was admitted lets ordering tests prove that the
    /// stripped post-step snapshot cannot apply it a second time.
    fn apply_authoritative_prize_state(
        &mut self,
        prize: SnapshotPrizeState,
        snapshot_tick: u32,
    ) -> bool {
        let authority = self.client_authority.view();
        if !self_runtime_snapshot_is_admissible(authority.scope.alive, authority.transition_pending)
        {
            return false;
        }
        let current_life = authority.scope.life_seq;
        let lifecycle_cutover_pending = self.prize_reconciliation_cutover_pending();
        if !prize_state_is_admissible_across_lifecycle_cutover(
            lifecycle_cutover_pending,
            current_life,
            prize.life_seq,
        ) {
            return false;
        }
        let same_life = self.last_self_server_prize_life_seq == Some(prize.life_seq);
        if same_life
            && !prize_revision_is_admissible(self.last_self_server_prize_revision, prize.revision)
        {
            return false;
        }
        if !same_life {
            self.last_self_server_damage_prize_loss_seq = None;
            self.last_self_server_damage_prize_loss_id = None;
            self.last_self_server_damage_prize_loss_life_seq = None;
            self.last_self_server_damage_prize_loss_revision = None;
        }
        self.last_self_server_prize_life_seq = Some(prize.life_seq);
        self.last_self_server_prize_revision = Some(prize.revision);
        self.server_maxed_spawn_loadout =
            Some(prize.flags & SnapshotPrizeState::MAXED_SPAWN_LOADOUT != 0);
        self.last_self_server_engine_shutdown_until_tick = prize.engine_shutdown_until_tick;
        self.last_self_server_emp_until_tick = prize.emp_until_tick;
        self.last_self_server_engine_shutdown_application_pending = prize.flags & (1 << 11) != 0;
        self.player_bounty = u32::from(prize.bounty);
        self.ship.max_energy = f32::from(prize.max_energy);
        // Current authoritative energy, when this body carries it. This per-recipient prize/state carrier is the
        // only repeated self-authority the client has, and a wormhole relocation is a server-side energy change the
        // client did not cause and gets no other event for: the server retains 20% and told nobody, so the local
        // prediction kept its pre-warp value until an unrelated damage/prize/life event happened to resync it.
        // Adopt what the server sent instead of predicting through it; `last_self_server_energy` moves with it so
        // the HUD blend agrees.
        if let Some(energy) = prize.energy {
            self.ship.energy = f32::from(energy);
            self.last_self_server_energy = Some(f32::from(energy));
        }
        // Bit 12 means the server has committed this revision but has not yet
        // accepted the client's exact first-input boundary. Keep simulating
        // with predecessor tuning until the reliable collectible ACK creates
        // that marker; applying these fields early recreates a cross-channel
        // one-frame authority split under ordinary RTC delay.
        if prize.flags & (1 << 12) == 0 {
            self.ship.rotation_per_second = f32::from_bits(prize.rotation_bits);
            self.ship.thrust_acceleration = f32::from_bits(prize.thrust_bits);
            self.ship.top_speed = f32::from_bits(prize.top_speed_bits);
            self.ship.recharge_per_second = f32::from_bits(prize.recharge_bits);
        }
        self.ship.bouncing_bullets = prize.flags & (1 << 0) != 0;
        self.ship.multifire_capable = prize.flags & (1 << 1) != 0;
        self.ship.multifire &= self.ship.multifire_capable;
        self.ship.proximity_bombs = prize.flags & (1 << 2) != 0;
        self.ship.stealth_capable = prize.flags & (1 << 3) != 0;
        self.ship.stealth &= self.ship.stealth_capable;
        self.ship.cloak_capable = prize.flags & (1 << 4) != 0;
        self.ship.cloak &= self.ship.cloak_capable;
        self.ship.xradar_capable = prize.flags & (1 << 5) != 0;
        self.ship.xradar &= self.ship.xradar_capable;
        self.ship.antiwarp_capable = prize.flags & (1 << 6) != 0;
        self.ship.antiwarp &= self.ship.antiwarp_capable;
        self.ship.gun_level = prize.gun_level;
        self.ship.bomb_level = prize.bomb_level;
        self.ship.shrapnel_upgrades = prize.shrapnel;
        self.ship.repels = u32::from(prize.repels);
        self.ship.bursts = u32::from(prize.bursts);
        self.ship.decoys = u32::from(prize.decoys);
        self.ship.thors = u32::from(prize.thors);
        self.ship.bricks = u32::from(prize.bricks);
        self.last_self_server_portals = Some((prize.life_seq, prize.revision, prize.portals));
        if self.pending_portal_placements.is_empty() {
            self.ship.portals = u32::from(prize.portals);
        }
        let current_server_tick =
            current_rocket_reconciliation_tick(snapshot_tick, self.browser_server_tick());
        if self.super_visual_deadline != prize.super_until_tick {
            self.super_animation_age = 0.0;
            self.super_visual_deadline = prize.super_until_tick;
        }
        if self.shield_visual_deadline != prize.shield_until_tick {
            self.shield_animation_age = 0.0;
            self.shield_visual_deadline = prize.shield_until_tick;
        }
        self.ship.super_timer =
            authoritative_prize_timer_seconds(prize.super_until_tick, current_server_tick);
        self.ship.shield_timer =
            authoritative_prize_timer_seconds(prize.shield_until_tick, current_server_tick);
        if self.accepted_prize_engine_shutdown.is_some_and(|accepted| {
            accepted.life_seq != prize.life_seq
                || accepted.until_tick.is_some_and(|until_tick| {
                    crate::continuum::continuum_tick_reached(current_server_tick, until_tick)
                })
                || !self.last_self_server_engine_shutdown_application_pending
                    && prize.revision.wrapping_sub(accepted.prize_state_revision) < (1 << 31)
        }) {
            self.accepted_prize_engine_shutdown = None;
        }
        // A non-pending private snapshot is the applied prize authority at
        // this revision. EMP has an independent recharge-only deadline.
        // Retire accepted continuity before this frame's fixed step.
        self.reconcile_authoritative_engine_shutdown_at_tick(current_server_tick);
        if let (Some(seq), Some(id)) = (prize.damage_loss_seq, prize.damage_loss_prize_id)
            && damage_prize_notification_is_new(
                self.last_self_server_damage_prize_loss_life_seq,
                self.last_self_server_damage_prize_loss_seq,
                self.last_self_server_damage_prize_loss_id,
                prize.life_seq,
                seq,
                id,
            )
        {
            self.last_self_server_damage_prize_loss_seq = Some(seq);
            self.last_self_server_damage_prize_loss_id = Some(id);
            self.last_self_server_damage_prize_loss_life_seq = Some(prize.life_seq);
            self.last_self_server_damage_prize_loss_revision = Some(prize.revision);
            if let Some((kind, _)) = PrizeKind::from_protocol_id(id) {
                let notification = crate::game::events::PrizeNotification::new(kind, true);
                let message =
                    crate::game::events::prize_notification_text_for_entry(notification, true);
                self.push_damage_prize_notification(notification);
                self.damage_prize_notification_count =
                    self.damage_prize_notification_count.wrapping_add(1);
                self.last_damage_prize_notification = Some(message);
            }
        }
        self.finish_network_prize_baseline_if_complete();
        true
    }

    fn current_network_security_generation(&self) -> Option<NetworkSecurityGeneration> {
        let authority = self.client_authority.view();
        self.network_security_generation.filter(|generation| {
            Some(generation.server_session_id) == authority.scope.server_session_id
                && Some(generation.arena_epoch) == authority.readiness.arena_epoch
        })
    }

    fn invalidate_network_security_generation(&mut self) {
        self.network_security_challenge = None;
        self.network_security_challenge_generation = None;
        self.network_security_synchronized = false;
        self.door_clock_server_tick = None;
        self.door_clock_observed_server_tick = None;
        self.door_clock_fresh = false;
        self.outbound_input_ring.clear();
    }

    fn synchronize_current_network_security(&mut self, challenge: SecurityChallenge) {
        let server_tick = security_door_sync_tick(self.browser_server_tick(), challenge.timestamp);
        self.door_timer = synchronize_map_doors_from_security(
            &mut self.maps[self.map_index],
            challenge,
            self.tuning.door_mode,
            self.tuning.door_delay_seconds,
            server_tick,
        );
        let now = self.network_authority_clock_seconds();
        self.network_security_synchronized = true;
        self.client_authority.observe_security_ready();
        self.door_clock_server_tick = Some(server_tick);
        self.door_clock_observed_server_tick = Some(server_tick);
        self.door_clock_anchor_seconds = now;
        // A SECURITY packet can sit behind a large reliable map transfer.
        // Its timestamp (and even the latest tick observed before it drains)
        // does not prove the server is still in that door phase at application
        // time. Keep gameplay closed until a strictly-forward snapshot/time
        // sample arrives after this bind; `synchronized_door_clock_step` then
        // catches the map up before the first new input record can be emitted.
        self.door_clock_last_observed_seconds = f64::NEG_INFINITY;
        self.door_clock_fresh = false;
        self.prize_rng_seed = challenge.prize_seed as i32;
    }

    /// Rebase the deterministic door stream at an accepted lifecycle or
    /// movement-correction boundary. The server installs that boundary against
    /// its current SECURITY-derived door phase. If a browser extrapolated ahead while
    /// the server or reliable lane was stalled, retaining that prediction
    /// would make every successor-life input fail strict phase validation.
    ///
    /// The application receipt is admitted by the lifecycle sequencer before
    /// this method is called, so an old-life or mismatched receipt cannot
    /// rewind the current map. A newer exact snapshot tick may advance the
    /// receipt tick, but an older cached sample cannot rewind it.
    fn rebase_network_doors_after_authority_receipt(&mut self, accepted_server_tick: u32) -> bool {
        let Some(challenge) = self.network_security_challenge else {
            self.network_security_synchronized = false;
            self.door_clock_fresh = false;
            self.client_authority.observe_security_pending();
            return false;
        };
        if self.network_security_challenge_generation != self.current_network_security_generation()
        {
            self.network_security_synchronized = false;
            self.door_clock_fresh = false;
            self.client_authority.observe_security_pending();
            return false;
        }

        let server_tick = security_door_sync_tick(self.browser_server_tick(), accepted_server_tick);
        self.door_timer = synchronize_map_doors_from_security(
            &mut self.maps[self.map_index],
            challenge,
            self.tuning.door_mode,
            self.tuning.door_delay_seconds,
            server_tick,
        );
        let now = self.network_authority_clock_seconds();
        self.door_clock_server_tick = Some(server_tick);
        self.door_clock_observed_server_tick = Some(server_tick);
        self.door_clock_anchor_seconds = now;
        self.door_clock_last_observed_seconds = now;
        self.door_clock_fresh = true;
        self.outbound_input_ring.clear();
        true
    }

    fn replay_current_network_security(&mut self) -> bool {
        let Some(challenge) = self.network_security_challenge else {
            return false;
        };
        if self.network_security_challenge_generation != self.current_network_security_generation()
        {
            return false;
        }
        self.synchronize_current_network_security(challenge);
        true
    }

    #[allow(dead_code)]
    pub(crate) fn apply_network_actions(&mut self, actions: &ClientProtocolActions) -> Vec<bool> {
        // Final application must revalidate against the same sole authority
        // owner that produced the immutable browser-poll plan. The legacy
        // Game/NetworkWorld mirrors are downstream presentation consumers and
        // can intentionally lag this scope by one poll while a replacement
        // snapshot is deferred. Consulting them here would reject that valid
        // snapshot before the sequencer could observe its private lifecycle.
        let authority_entry = self.client_authority.view();
        let computed_admission;
        let authority_admission = if actions.authority_admission.len() == actions.events.len() {
            &actions.authority_admission
        } else {
            computed_admission = build_authority_admission_plan(
                actions,
                AuthorityAdmissionSeed {
                    scope: authority_entry.scope,
                    readiness: AuthorityReadiness {
                        joined: authority_entry.readiness.joined,
                        arena_epoch: authority_entry.readiness.arena_epoch,
                        settings: authority_entry.readiness.settings,
                        security: authority_entry.readiness.security,
                        map: authority_entry.readiness.map,
                    },
                    segment: authority_entry.segment,
                },
            );
            &computed_admission
        };
        let current_scope = authority_entry.scope;
        let mut admitted_actions = admitted_authority_actions(
            actions,
            authority_admission,
            authority_entry.segment,
            current_scope,
        );
        let replacement_baseline_already_installed = authority_admission
            .iter()
            .zip(&admitted_actions)
            .any(|(admission, admitted)| {
                *admitted && {
                    matches!(
                        admission.private_fact,
                        PrivateAuthorityAdmission::Replacement(scope) if scope == current_scope
                    )
                }
            });
        if !actions
            .events
            .iter()
            .any(|event| matches!(event, ClientProtocolEvent::PreGameplayAuthorityReset))
            && !self.network_prize_baseline_pending
            && !replacement_baseline_already_installed
            && authority_admission
                .iter()
                .zip(&admitted_actions)
                .any(|(admission, admitted)| {
                    *admitted
                        && matches!(admission.boundary, AuthorityBoundaryAdmission::Hard { .. })
                })
        {
            self.invalidate_authoritative_prize_runtime();
        }
        let mut death_prizes = Vec::new();
        let mut death_explosions = Vec::new();
        let mut departing_player_names = vec![None; actions.events.len()];
        let mut new_roster_entries = vec![false; actions.events.len()];
        let mut admitted_turf_update = false;
        for index in 0..actions.events.len() {
            if !admitted_actions[index] {
                continue;
            }
            let event = &actions.events[index];
            let admission = &authority_admission[index];
            if let ClientProtocolEvent::PlayerDeath(death) = event
                && !self.remember_network_death(admission.segment, *death)
            {
                admitted_actions[index] = false;
                continue;
            }
            if admission.private_fact != PrivateAuthorityAdmission::Reject
                && let ClientProtocolEvent::DamageWatched(report) = event
                && !self.remember_network_damage(admission.segment, report)
            {
                admitted_actions[index] = false;
                continue;
            }
            // Resolve death presentation against the world produced by every
            // preceding admitted action in this exact authority generation.
            // This sees replacement roster/position facts, but an old death
            // rejected by the immutable plan never reaches presentation.
            let death_prize = match event {
                ClientProtocolEvent::PlayerDeath(death) => {
                    death_prize_from_player_death(&self.network_world, *death)
                }
                _ => None,
            };
            let death_explosion = match event {
                ClientProtocolEvent::PlayerDeath(death)
                    if self.client_authority.view().scope.player_id != Some(death.killed_id) =>
                {
                    ship_explosion_position_from_player_death(&self.network_world, *death).map(
                        |position| {
                            (
                                NetworkDeathIdentity {
                                    authority_segment: admission.segment,
                                    killed_id: death.killed_id,
                                    life_seq: death.life_seq,
                                    green_collectible_id: death.green_collectible_id,
                                    respawn_at_tick: death.respawn_at_tick,
                                },
                                position,
                            )
                        },
                    )
                }
                _ => None,
            };
            if let ClientProtocolEvent::PlayerEntered(entry) = event {
                new_roster_entries[index] =
                    !self.network_world.players.contains_key(&entry.player_id);
            }
            // Capture the admitted roster at this event, before removal. A
            // later entry in the same poll may reuse the departing player's ID.
            if let ClientProtocolEvent::PlayerLeft(player_id) = event {
                departing_player_names[index] = self
                    .network_world
                    .players
                    .get(player_id)
                    .map(|player| player.name.clone());
            }
            if self
                .network_world
                .apply_planned_action(event, Some(admission))
            {
                death_prizes.extend(death_prize);
                death_explosions.extend(death_explosion);
                admitted_turf_update |= matches!(event, ClientProtocolEvent::TurfFlagUpdate(_));
            }
        }
        if admitted_turf_update {
            let positions = self.map().object_positions(FLAG_TILE_ID);
            self.network_world.apply_turf_flag_positions(&positions);
        }
        for pos in self.network_world.drain_pending_warp_effects() {
            self.warps.push(WarpEffect { pos, age: 0.0 });
        }
        for (death, pos) in death_explosions {
            self.add_remote_ship_explosion(pos, death);
        }
        for (collectible_id, pos, kind) in death_prizes {
            self.spawn_death_prize_at(collectible_id, pos, kind);
        }

        // Snapshot and realtime event carriers are independent. Release any
        // immutable weapon facts that overtook their exact source actor only
        // after every admitted world action in this batch has installed its
        // presentation generation and position.
        self.drain_pending_remote_weapon_presentations();

        for (index, ((event, authority_admission), admitted)) in actions
            .events
            .iter()
            .zip(authority_admission)
            .zip(&admitted_actions)
            .enumerate()
        {
            if !*admitted {
                continue;
            }
            if !matches!(event, ClientProtocolEvent::PreGameplayAuthorityReset)
                && let AuthorityBoundaryAdmission::Hard {
                    segment,
                    replacement,
                } = authority_admission.boundary
            {
                let prior_scope = self.client_authority.view().scope;
                let sequencer_disposition = self.client_authority.observe_hard_replacement(
                    ClientAuthoritySource::Reliable,
                    segment,
                    replacement,
                );
                debug_assert_eq!(
                    sequencer_disposition,
                    ClientAuthorityDisposition::Replacement,
                    "admitted planner hard boundary must advance the sole lifecycle owner"
                );
                self.observer_combat_timeline.reset();
                self.combat_view_ship_positions.clear();
                let replacement = replacement.unwrap_or_default();
                let session_replaced = prior_scope.server_session_id
                    != replacement.server_session_id
                    || prior_scope.arena_stream_id != replacement.arena_stream_id;
                self.clear_remote_weapon_presentation();
                self.clear_active_weapons();
                self.explosions.clear();
                self.pending_same_life_movement_boundary_position = false;
                self.clear_prize_effect_application_state();
                self.clear_portal_placement_state();
                self.pending_repel_application = None;
                self.pending_repel_application_expiry_tick = None;
                self.expired_repel_applications.clear();
                self.consumed_repel_delivery_generations.clear();
                self.client_authority.clear_deferred_warp_pickup_acks();
                self.repel_movement_sequence_window = None;
                self.pending_warp_relocation_collectible_id = None;
                self.pending_warp_relocation_life_seq = None;
                self.pending_warp_relocation_sequences.clear();
                self.outbound_input_ring.clear();
                // No readiness fact crosses a hard authority generation,
                // even when every numeric network identifier is reused.
                self.network_settings_received = false;
                self.network_zone_files_ready = false;
                self.network_zone_map_ready = false;
                self.invalidate_network_security_generation();
                self.network_security_generation = None;
                if session_replaced {
                    self.last_observed_browser_server_tick = None;
                    self.door_clock_server_tick = None;
                    self.door_clock_observed_server_tick = None;
                    self.door_clock_fresh = false;
                }
            }
            if authority_admission.readiness_fact == AuthorityReadinessAdmission::Reject {
                // Settings, SECURITY, and map/file readiness are bound to the
                // immutable batch segment in which they were observed. An old
                // carrier cannot mutate a numerically reused replacement
                // generation or open its gameplay barrier.
                continue;
            }
            // The admission roster is state, not a sequence of arrivals. Only
            // an already-entered match announces subsequent roster changes;
            // death, correction and menu transitions remain part of that match.
            let announce_roster_changes = matches!(
                self.client_authority.view().phase,
                ClientAuthorityPhase::Live
                    | ClientAuthorityPhase::Spectating
                    | ClientAuthorityPhase::DeadRespawning
                    | ClientAuthorityPhase::CorrectionHeld
                    | ClientAuthorityPhase::ShipFrequencyTransition
            );
            match event {
                ClientProtocolEvent::PreGameplayAuthorityReset => {
                    self.invalidate_authoritative_prize_runtime();
                }
                ClientProtocolEvent::Disconnected => {
                    self.clear_bricks();
                    self.observer_combat_timeline.reset();
                    self.combat_view_ship_positions.clear();
                    self.clear_remote_weapon_presentation();
                    self.clear_active_weapons();
                    self.explosions.clear();
                    self.pending_same_life_movement_boundary_position = false;
                    self.clear_prize_effect_application_state();
                    self.clear_portal_placement_state();
                    self.last_observed_browser_server_tick = None;
                    self.invalidate_network_security_generation();
                    self.network_security_generation = None;
                    self.network_zone_files_ready = false;
                    self.network_zone_map_ready = false;
                    self.pending_repel_application = None;
                    self.pending_repel_application_expiry_tick = None;
                    self.expired_repel_applications.clear();
                    self.consumed_repel_delivery_generations.clear();
                    self.client_authority.clear_deferred_warp_pickup_acks();
                    self.repel_movement_sequence_window = None;
                }
                ClientProtocolEvent::ServerClockBootstrapped(_) => {}
                ClientProtocolEvent::QaTraceContextReceived(_) => {}
                ClientProtocolEvent::PlayerIdAssigned(_) => {
                    self.clear_bricks();
                    self.clear_remote_weapon_presentation();
                    self.clear_active_weapons();
                    self.explosions.clear();
                    self.pending_same_life_movement_boundary_position = false;
                    self.clear_prize_effect_application_state();
                    self.clear_portal_placement_state();
                    self.invalidate_authoritative_prize_runtime();
                }
                ClientProtocolEvent::JoinedArena(arena_epoch) => {
                    self.clear_bricks();
                    self.clear_remote_weapon_presentation();
                    self.clear_active_weapons();
                    self.explosions.clear();
                    self.pending_same_life_movement_boundary_position = false;
                    self.clear_prize_effect_application_state();
                    // Arena entry is a new authority generation even when the
                    // RTC session and reusable Game instance are unchanged.
                    self.invalidate_network_security_generation();
                    self.network_security_generation =
                        self.client_authority.view().scope.server_session_id.map(
                            |server_session_id| NetworkSecurityGeneration {
                                server_session_id,
                                arena_epoch: *arena_epoch,
                            },
                        );
                    self.network_zone_files_ready = false;
                    self.network_zone_map_ready = false;
                    self.pending_repel_application = None;
                    self.pending_repel_application_expiry_tick = None;
                    self.expired_repel_applications.clear();
                    self.consumed_repel_delivery_generations.clear();
                    self.client_authority.clear_deferred_warp_pickup_acks();
                    self.repel_movement_sequence_window = None;
                    // The server reserves the clock-bootstrap input sequence
                    // as this arena life's exact neutral position boundary.
                    // Arm it at the admitted join itself: a browser may load
                    // quickly enough that an early numeric ship key is a
                    // server no-op, and must not depend on a later ship-change
                    // ACK to start publishing authoritative positions.
                    let disposition = self.client_authority.observe_joined_arena(*arena_epoch);
                    debug_assert!(matches!(
                        disposition,
                        ClientAuthorityDisposition::Applied | ClientAuthorityDisposition::Duplicate
                    ));
                }
                ClientProtocolEvent::ArenaSettingsUpdated(settings) => {
                    self.tuning = ArenaTuning::from_continuum_settings(settings);
                    let tuning = self.ship_tuning();
                    self.ship.max_energy = max_energy_after_settings_update(
                        self.ship.max_energy,
                        tuning,
                        !self.network_settings_received,
                    );
                    self.ship.energy = self.ship.energy.min(self.ship.max_energy);
                    self.network_settings_received = true;
                    self.client_authority.observe_settings_ready();
                    self.arena_enter_time = self.network_authority_clock_seconds() as f32;
                    self.door_timer = 0.0;
                    self.network_security_synchronized = false;
                    self.client_authority.observe_security_pending();
                    self.door_clock_server_tick = None;
                    self.door_clock_observed_server_tick = None;
                    self.door_clock_fresh = false;
                    // Rolling history is phase-authenticated. No record made
                    // under the superseded door tuning may be replayed after
                    // this settings boundary, even if SECURITY and settings
                    // were delivered on different RTC channels.
                    self.outbound_input_ring.clear();
                    // SECURITY can arrive on the realtime stream before the
                    // reliable arena settings drain. Settings change the door
                    // mode/delay and deliberately invalidate the old bind;
                    // immediately replay the cached authenticated challenge
                    // with the new tuning so the first rolling input history
                    // cannot retain a pre-settings door phase.
                    self.replay_current_network_security();
                }
                ClientProtocolEvent::SecurityChallengeReceived {
                    challenge,
                    server_session_id,
                    arena_epoch,
                } => {
                    let Some(generation) = self.current_network_security_generation() else {
                        // SECURITY without an exact RTC+arena generation is
                        // stale/unscoped and cannot open gameplay.
                        continue;
                    };
                    if !generation.admits_challenge(
                        *server_session_id,
                        *arena_epoch,
                        *challenge,
                        self.network_security_challenge,
                    ) {
                        // The protocol session captured a different RTC or
                        // arena generation when this packet was decoded. It
                        // may still be valid for that old arena, but it can
                        // never seed this one.
                        continue;
                    }
                    if self.network_security_challenge_generation == Some(generation)
                        && self.network_security_challenge == Some(*challenge)
                    {
                        // Exact duplicates are transport retries, not a new
                        // door/prize RNG application. Replaying here would
                        // rewind RNG progress after the first admitted bind.
                        continue;
                    }
                    self.network_security_challenge = Some(*challenge);
                    self.network_security_challenge_generation = Some(generation);
                    self.synchronize_current_network_security(*challenge);
                }
                ClientProtocolEvent::ZoneFilesPending => {
                    // The session may parse the replacement map before later
                    // LVZ files finish. Keep all gameplay and input history
                    // closed until the ordered publication batch is applied,
                    // or collision/safe-tile decisions would use the old map.
                    self.network_zone_files_ready = false;
                    self.network_zone_map_ready = false;
                    // Downloaded LVZ archives in this window defer their whole-cache resolve/band pass
                    // to `ZoneFilesPublished`, so a fourteen-archive join does one band pass, not fourteen.
                    self.zone_files_batch_open = true;
                    self.client_authority.observe_files_pending();
                    self.client_authority.observe_map_pending();
                    self.outbound_input_ring.clear();
                }
                ClientProtocolEvent::MapDownloaded(download) => {
                    let installs_level = download
                        .filename()
                        .rsplit_once('.')
                        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("lvl"));
                    // A downloaded level belongs to the arena generation that
                    // requested it. `install_downloaded_file` uses the same
                    // map-switch helper as a user-initiated zone change, which
                    // deliberately clears security scope, so retain only the
                    // currently validated generation/bind across this exact
                    // authoritative install.
                    let retained_generation = installs_level
                        .then(|| self.current_network_security_generation())
                        .flatten();
                    let retained_challenge = retained_generation.and_then(|generation| {
                        (self.network_security_challenge_generation == Some(generation))
                            .then_some(self.network_security_challenge)
                            .flatten()
                    });
                    self.install_downloaded_file(download);
                    // The reliable file transfer can finish after the security
                    // challenge. Installing a level constructs a fresh map
                    // with its default door RNG, so immediately replay the
                    // authenticated challenge against the replacement map.
                    // Otherwise ordinary input can carry a divergent seed
                    // even though the gameplay-ready barrier remains open.
                    if installs_level {
                        self.network_security_generation = retained_generation;
                        self.network_security_challenge = retained_challenge;
                        self.network_security_challenge_generation =
                            retained_challenge.and(retained_generation);
                        self.network_security_synchronized = false;
                        self.door_clock_fresh = false;
                        self.replay_current_network_security();
                    }
                }
                ClientProtocolEvent::ZoneMapPublished => {
                    self.network_zone_map_ready = true;
                    self.client_authority.observe_map_ready();
                    self.rebuild_network_brick_tiles();
                }
                ClientProtocolEvent::ZoneFilesPublished => {
                    // Resolve a zone that fits one chunk inline; defer a larger batch (one Chaos LVZ
                    // carries ~27k of the zone's 28.9k objects, a measured ~54 ms resolve) to
                    // `advance_deferred_lvz_merge`, which spends a bounded chunk per frame and marks
                    // files ready when the resolve completes.
                    self.zone_files_batch_open = false;
                    if self.lvz_render_cache.finalize_deferred_merges_budgeted(
                        crate::lvz::LVZ_RESOLVE_OBJECTS_PER_FRAME,
                    ) {
                        self.network_zone_files_ready = true;
                        self.client_authority.observe_files_ready();
                    } else {
                        self.zone_files_resolve_pending = true;
                    }
                }
                ClientProtocolEvent::PlayerPrize(prize) => {
                    self.apply_network_player_prize(*prize);
                }
                ClientProtocolEvent::PrizePickupAck(scoped) => {
                    let authority = self.client_authority.view();
                    if !matches!(
                        authority_admission.private_fact,
                        PrivateAuthorityAdmission::Current(_)
                            | PrivateAuthorityAdmission::Replacement(_)
                    ) || !authority_admission.readiness_after.complete()
                        || !prize_pickup_ack_matches_current_runtime(
                            authority.scope.server_session_id,
                            authority.scope.arena_stream_id,
                            authority.scope.player_id,
                            authority.scope.life_seq,
                            authority.scope.alive,
                            matches!(
                                authority.transition_kind,
                                Some(
                                    ClientAuthorityTransitionKind::Ship
                                        | ClientAuthorityTransitionKind::Frequency
                                        | ClientAuthorityTransitionKind::Respawn
                                        | ClientAuthorityTransitionKind::Correction
                                )
                            ),
                            *scoped,
                        )
                    {
                        continue;
                    }
                    let ack = scoped.ack;
                    let application = ack
                        .engine_shutdown_application_required
                        .then(|| {
                            Some(PrizeEffectApplied {
                                server_session_id: authority.scope.server_session_id?,
                                player_id: authority.scope.player_id?,
                                life_seq: ack.life_seq,
                                collectible_id: ack.collectible_id,
                                prize_id: ack.prize_id,
                                prize_state_revision: ack.prize_state_revision,
                                application_sequence: self.next_input_seq,
                            })
                        })
                        .flatten();
                    if ack.engine_shutdown_application_required && application.is_none() {
                        continue;
                    }
                    let newly_applied = self.apply_network_prize_pickup_ack(*scoped);
                    if newly_applied
                        && let Some(application) = application
                        && authority.scope.life_seq == Some(application.life_seq)
                    {
                        let disposition = self.client_authority.arm_prize_effect_application(
                            application,
                            ack.engine_shutdown_until_tick,
                        );
                        if matches!(disposition, ClientAuthorityDisposition::Rejected(_)) {
                            self.push_notification("Prize effect authority barrier rejected.", RED);
                            continue;
                        }
                        self.reconcile_authoritative_engine_shutdown();
                    }
                }
                ClientProtocolEvent::PrizeEffectAccepted(receipt) => {
                    if !matches!(
                        authority_admission.private_fact,
                        PrivateAuthorityAdmission::Current(_)
                            | PrivateAuthorityAdmission::Replacement(_)
                    ) || !authority_admission.readiness_after.complete()
                    {
                        continue;
                    }
                    match self
                        .client_authority
                        .observe_prize_effect_terminal(*receipt)
                    {
                        PrizeEffectReceiptOutcome::Applied(pending) => {
                            // This terminal commits the exact first sequence
                            // generated under the new movement semantics.
                            // Input generation is paused while the reliable
                            // application barrier is outstanding, so the
                            // rolling realtime ring can otherwise retain
                            // predecessor records for arbitrarily longer than
                            // their sequence distance suggests. Retire them at
                            // the accepted boundary: replaying those records
                            // afterward can neither recover new-semantics
                            // movement nor satisfy the server's current door
                            // phase window.
                            self.outbound_input_ring.clear();
                            if let Some(until_tick) = pending.engine_shutdown_until_tick {
                                self.accepted_prize_engine_shutdown =
                                    Some(AcceptedPrizeEngineShutdown {
                                        life_seq: pending.application.life_seq,
                                        prize_state_revision: pending
                                            .application
                                            .prize_state_revision,
                                        until_tick: Some(until_tick),
                                    });
                            }
                            self.reconcile_authoritative_engine_shutdown();
                        }
                        PrizeEffectReceiptOutcome::Rebased
                        | PrizeEffectReceiptOutcome::Cancelled(_) => {
                            self.reconcile_authoritative_engine_shutdown();
                        }
                        PrizeEffectReceiptOutcome::Duplicate
                        | PrizeEffectReceiptOutcome::Rejected(_) => {}
                    }
                }
                ClientProtocolEvent::HiddenPrizeSpawn(spawn) => {
                    self.apply_network_hidden_prize_spawn(*spawn);
                }
                ClientProtocolEvent::CollectedPrize(prize) => {
                    self.apply_network_collected_prize(*prize);
                }
                ClientProtocolEvent::PlayerPosition(update)
                | ClientProtocolEvent::SnapshotPlayerPosition { update, .. } => {
                    if authority_admission.private_fact == PrivateAuthorityAdmission::Reject {
                        continue;
                    }
                    let private_baseline_position = matches!(
                        authority_admission.private_fact,
                        PrivateAuthorityAdmission::Current(_)
                            | PrivateAuthorityAdmission::Replacement(_)
                    );
                    if self.client_authority.view().scope.player_id == Some(update.player_id)
                        && (!self.network_prize_baseline_pending || private_baseline_position)
                    {
                        self.apply_authoritative_self_snapshot(update);
                        if private_baseline_position {
                            self.finish_network_prize_baseline_if_complete();
                        }
                    }
                    let _ = self.apply_remote_weapon_visual(update.player_id, update.weapon);
                }
                ClientProtocolEvent::RealtimeWeaponFired {
                    server_session_id,
                    arena_stream_id,
                    event_id,
                    server_tick,
                    source,
                } => {
                    let _ = self.apply_realtime_weapon_visual(
                        *server_session_id,
                        *arena_stream_id,
                        *event_id,
                        *server_tick,
                        *source,
                    );
                }
                ClientProtocolEvent::RealtimeOwnerWeaponsCancelled {
                    server_session_id,
                    arena_stream_id,
                    event_id,
                    server_tick: _,
                    cancellation,
                } => {
                    self.apply_owner_weapon_cancellation(
                        *server_session_id,
                        *arena_stream_id,
                        *event_id,
                        *cancellation,
                    );
                }
                ClientProtocolEvent::RealtimeProjectileState {
                    server_session_id,
                    arena_stream_id,
                    event_id,
                    server_tick,
                    state,
                } => {
                    let _ = self.apply_realtime_projectile_state(
                        *server_session_id,
                        *arena_stream_id,
                        *event_id,
                        *server_tick,
                        *state,
                    );
                }
                ClientProtocolEvent::RealtimeRepelMutations {
                    server_session_id,
                    arena_stream_id,
                    event_id,
                    server_tick,
                    batch,
                } => {
                    self.apply_realtime_repel_mutations(
                        *server_session_id,
                        *arena_stream_id,
                        *event_id,
                        *server_tick,
                        *batch,
                    );
                }
                ClientProtocolEvent::BrickDropped(brick) => {
                    self.apply_network_brick_dropped(*brick);
                }
                ClientProtocolEvent::ShipReset => {
                    self.reset_ship();
                }
                ClientProtocolEvent::SetCoordinates(coordinates) => {
                    let radius = self.ship_tuning().radius_pixels;
                    let warp_pos = apply_set_coordinates_to_ship(
                        &self.maps[self.map_index],
                        &mut self.ship.pos,
                        &mut self.ship.vel,
                        *coordinates,
                        radius,
                    );
                    self.ship.flash = true;
                    self.warps.push(WarpEffect {
                        pos: warp_pos,
                        age: 0.0,
                    });
                    self.camera.target = self.ship.pos;
                }
                ClientProtocolEvent::PlayerDeath(death) => {
                    self.apply_authoritative_player_death(*death);
                }
                ClientProtocolEvent::PlayerEntered(entry) => {
                    if Some(entry.player_id) == self.client_authority.view().scope.player_id {
                        self.client_authority.observe_initial_ship(entry.ship);
                        self.spectator_mode = entry.ship == SPECTATOR_SHIP;
                    } else if announce_roster_changes && new_roster_entries[index] {
                        self.push_notification(&player_entered_notification(entry), ORANGE);
                    }
                }
                ClientProtocolEvent::PlayerLeft(player_id) => {
                    if announce_roster_changes
                        && departing_player_names[index].is_some()
                        && !matches!(
                            authority_admission.boundary,
                            AuthorityBoundaryAdmission::Hard { .. }
                        )
                        && Some(*player_id) != self.client_authority.view().scope.player_id
                    {
                        self.push_notification(
                            &player_left_notification(departing_player_names[index].as_deref()),
                            ORANGE,
                        );
                    }
                }
                ClientProtocolEvent::FrequencyChanged(change) => {
                    let name = self
                        .network_world
                        .players
                        .get(&change.player_id)
                        .map(|player| player.name.as_str());
                    self.push_notification(&frequency_change_notification(change, name), ORANGE);
                }
                ClientProtocolEvent::ShipFrequencyChanged(change) => {
                    if self.client_authority.view().scope.player_id == Some(change.player_id) {
                        let sequencer_disposition = self
                            .client_authority
                            .observe_ship_ack(change.ship, change.life_seq);
                        let acknowledged_pending_change =
                            sequencer_disposition == ClientAuthorityDisposition::Replacement;
                        if acknowledged_pending_change {
                            if change.ship == SPECTATOR_SHIP {
                                self.apply_admitted_spectator_mode();
                                self.outbound_input_ring.clear();
                            } else {
                                self.spectator_mode = false;
                                self.spectating_player_id = None;
                            }
                        }
                        if acknowledged_pending_change && change.ship < 8 {
                            self.pending_same_life_movement_boundary_position = false;
                            self.clear_prize_effect_application_state();
                            self.clear_portal_placement_state();
                            self.pending_warp_relocation_collectible_id = None;
                            self.pending_repel_application = None;
                            self.pending_repel_application_expiry_tick = None;
                            self.expired_repel_applications.clear();
                            self.consumed_repel_delivery_generations.clear();
                            self.client_authority.clear_deferred_warp_pickup_acks();
                            self.repel_movement_sequence_window = None;
                            self.pending_warp_relocation_life_seq = None;
                            self.pending_warp_relocation_sequences.clear();
                            self.ship.ship_type = change.ship as usize;
                            self.reset_ship();
                            self.last_input = PlayerInput::default();
                            self.outbound_input_ring.clear();
                            // The sequencer retained the request boundary and
                            // now owns the exact neutral baseline submission.
                            // No parallel Game flag may re-arm it after the
                            // application receipt.
                        } else if self.ship.ship_type != change.ship as usize && change.ship < 8 {
                            self.ship.ship_type = change.ship as usize;
                            self.reset_ship();
                            self.last_input = PlayerInput::default();
                            self.outbound_input_ring.clear();
                        }
                    }
                    let name = self
                        .network_world
                        .players
                        .get(&change.player_id)
                        .map(|player| player.name.as_str());
                    self.push_notification(
                        &ship_frequency_change_notification(change, name),
                        ORANGE,
                    );
                }
                ClientProtocolEvent::FrequencyCutoverAcknowledged(ack) => {
                    let sequencer_disposition = self.client_authority.observe_frequency_ack(*ack);
                    if sequencer_disposition != ClientAuthorityDisposition::Replacement {
                        continue;
                    }
                    self.pending_same_life_movement_boundary_position = false;
                    self.clear_prize_effect_application_state();
                    self.clear_portal_placement_state();
                    let Some(baseline) = self.client_authority.lifecycle_baseline_to_submit()
                    else {
                        continue;
                    };
                    apply_frequency_baseline_rebase(
                        &mut self.ship,
                        &mut self.player_bounty,
                        &mut self.last_self_server_energy,
                        &baseline,
                    );
                    commit_accepted_lifecycle_baseline_rocket_lifecycle(
                        ack.life_seq,
                        &mut self.last_self_server_rocket_life_seq,
                        &mut self.require_forward_self_server_rocket_life,
                        &mut self.last_self_server_rocket_activation_sequence,
                        &mut self.last_self_server_rocket_until_tick,
                        &mut self.pending_rocket_activation_sequence,
                        &mut self.ship.rocket_timer,
                        &mut self.ship.rocket_cooldown,
                        &mut self.last_input,
                    );
                    self.pending_weapon = 0;
                    self.cancel_pending_weapon_prediction();
                    self.pending_warp_relocation_collectible_id = None;
                    self.pending_repel_application = None;
                    self.pending_repel_application_expiry_tick = None;
                    self.expired_repel_applications.clear();
                    self.consumed_repel_delivery_generations.clear();
                    self.client_authority.clear_deferred_warp_pickup_acks();
                    self.repel_movement_sequence_window = None;
                    self.pending_warp_relocation_life_seq = None;
                    self.pending_warp_relocation_sequences.clear();
                }
                ClientProtocolEvent::ShipChangeRejected(receipt) => {
                    if self.client_authority.observe_ship_rejection(*receipt)
                        == ClientAuthorityDisposition::Applied
                    {
                        self.spectator_mode = false;
                        self.push_notification(
                            "Must have full energy to change ships or spectate.",
                            ORANGE,
                        );
                    }
                }
                ClientProtocolEvent::SpawnIdleSpectator(fact) => {
                    if self.client_authority.observe_spawn_idle_spectator(*fact)
                        != ClientAuthorityDisposition::Replacement
                    {
                        continue;
                    }
                    self.spectator_mode = true;
                    self.spectating_player_id = None;
                    self.ship.vel = Vec2::ZERO;
                    self.last_input = PlayerInput::default();
                    self.outbound_input_ring.clear();
                    self.pending_weapon = 0;
                    self.cancel_pending_weapon_prediction();
                    self.pending_same_life_movement_boundary_position = false;
                    self.clear_prize_effect_application_state();
                    self.clear_portal_placement_state();
                    self.pending_warp_relocation_collectible_id = None;
                    self.pending_warp_relocation_life_seq = None;
                    self.pending_warp_relocation_sequences.clear();
                    self.pending_repel_application = None;
                    self.pending_repel_application_expiry_tick = None;
                    self.expired_repel_applications.clear();
                    self.consumed_repel_delivery_generations.clear();
                    self.repel_movement_sequence_window = None;
                    self.clear_local_weapons();
                    self.player_respawn_timer = 0.0;
                    self.push_notification(
                        "Moved to spectator: no movement or firing after spawning.",
                        ORANGE,
                    );
                }
                ClientProtocolEvent::ConnectionQualityDirective(fact) => {
                    let was_spectator = self.spectator_mode;
                    let result = self
                        .client_authority
                        .observe_quality_directive(*fact, macroquad::miniquad::date::now());
                    if matches!(
                        result,
                        ClientAuthorityDisposition::Rejected(_)
                            | ClientAuthorityDisposition::Duplicate
                    ) {
                        continue;
                    }
                    if self.client_authority.view().phase == ClientAuthorityPhase::Spectating {
                        self.spectator_mode = true;
                        if !was_spectator || result == ClientAuthorityDisposition::Replacement {
                            self.spectating_player_id = None;
                            self.ship.vel = Vec2::ZERO;
                            self.last_input = PlayerInput::default();
                            self.outbound_input_ring.clear();
                        }
                    }
                    if fact.rejected_request.is_some() && fact.remaining_ms == 0 {
                        #[cfg(any(test, target_arch = "wasm32"))]
                        if self.quality_return_recovery.is_none() {
                            self.quality_return_recovery =
                                fact.rejected_request.and_then(|(ship, _)| {
                                    quality_return::QualityReturnRecovery::new(
                                        ship,
                                        macroquad::miniquad::date::now(),
                                    )
                                });
                            self.push_notification("Re-establishing your connection. You will stay in spectator until you select a ship again.", ORANGE);
                        } else if self
                            .quality_return_recovery
                            .as_ref()
                            .is_some_and(|attempt| !attempt.is_stopped())
                        {
                            self.push_notification("Your connection is still being checked. Stay in spectator, then use ESC and select a ship to try again shortly.", ORANGE);
                        }
                    }
                    #[cfg(any(test, target_arch = "wasm32"))]
                    if fact.remaining_ms > 0 {
                        // A current server hold always wins over a pending retry.
                        // Neither a local timer nor a reconnect clears it.
                        self.quality_return_recovery = None;
                    }
                    if result == ClientAuthorityDisposition::Replacement {
                        self.pending_weapon = 0;
                        self.cancel_pending_weapon_prediction();
                        self.pending_same_life_movement_boundary_position = false;
                        self.clear_prize_effect_application_state();
                        self.clear_portal_placement_state();
                        self.pending_warp_relocation_collectible_id = None;
                        self.pending_warp_relocation_life_seq = None;
                        self.pending_warp_relocation_sequences.clear();
                        self.pending_repel_application = None;
                        self.pending_repel_application_expiry_tick = None;
                        self.expired_repel_applications.clear();
                        self.consumed_repel_delivery_generations.clear();
                        self.repel_movement_sequence_window = None;
                        self.clear_local_weapons();
                        self.player_respawn_timer = 0.0;
                        self.push_notification("Sorry, your connection is having difficulty. You've been moved to spectator mode for one minute.", ORANGE);
                    }
                }
                ClientProtocolEvent::LifecycleBaselineAccepted(receipt) => {
                    match self.client_authority.observe_initial_receipt(*receipt) {
                        InitialReceiptOutcome::Applied => {
                            // The sequencer atomically retired both the unsent
                            // intent and durable delivery before this consumer
                            // applies the accepted canonical state. A same-life
                            // snapshot can no longer re-arm the transaction.
                            if !self.rebase_network_doors_after_authority_receipt(
                                receipt.accepted_server_tick,
                            ) {
                                self.suppress_gameplay_step_after_network_rebase = true;
                                continue;
                            }
                            self.apply_accepted_ship_life_pose(receipt.accepted_position);
                            commit_accepted_lifecycle_baseline_rocket_lifecycle(
                                receipt.life_seq,
                                &mut self.last_self_server_rocket_life_seq,
                                &mut self.require_forward_self_server_rocket_life,
                                &mut self.last_self_server_rocket_activation_sequence,
                                &mut self.last_self_server_rocket_until_tick,
                                &mut self.pending_rocket_activation_sequence,
                                &mut self.ship.rocket_timer,
                                &mut self.ship.rocket_cooldown,
                                &mut self.last_input,
                            );
                            self.last_input = PlayerInput::default();
                            self.outbound_input_ring.clear();
                            self.pending_weapon = 0;
                            self.cancel_pending_weapon_prediction();
                            self.suppress_gameplay_step_after_network_rebase = true;
                            continue;
                        }
                        InitialReceiptOutcome::Duplicate | InitialReceiptOutcome::Rejected(_) => {
                            continue;
                        }
                        InitialReceiptOutcome::NotInitial => {}
                    }
                    match self.client_authority.observe_lifecycle_receipt(*receipt) {
                        LifecycleReceiptOutcome::Applied {
                            kind:
                                ClientAuthorityTransitionKind::Ship
                                | ClientAuthorityTransitionKind::Respawn,
                            ..
                        } => {
                            if !self.rebase_network_doors_after_authority_receipt(
                                receipt.accepted_server_tick,
                            ) {
                                self.suppress_gameplay_step_after_network_rebase = true;
                                continue;
                            }
                            self.apply_accepted_ship_life_pose(receipt.accepted_position);
                            commit_accepted_lifecycle_baseline_rocket_lifecycle(
                                receipt.life_seq,
                                &mut self.last_self_server_rocket_life_seq,
                                &mut self.require_forward_self_server_rocket_life,
                                &mut self.last_self_server_rocket_activation_sequence,
                                &mut self.last_self_server_rocket_until_tick,
                                &mut self.pending_rocket_activation_sequence,
                                &mut self.ship.rocket_timer,
                                &mut self.ship.rocket_cooldown,
                                &mut self.last_input,
                            );
                            self.last_input = PlayerInput::default();
                            self.outbound_input_ring.clear();
                            self.pending_weapon = 0;
                            self.cancel_pending_weapon_prediction();
                            self.suppress_gameplay_step_after_network_rebase = true;
                            continue;
                        }
                        LifecycleReceiptOutcome::Applied {
                            kind: ClientAuthorityTransitionKind::Frequency,
                            accepted_baseline: Some(baseline),
                        } => {
                            apply_frequency_baseline_rebase(
                                &mut self.ship,
                                &mut self.player_bounty,
                                &mut self.last_self_server_energy,
                                &baseline,
                            );
                            self.last_input = PlayerInput::default();
                            self.outbound_input_ring.clear();
                            self.pending_weapon = 0;
                            self.cancel_pending_weapon_prediction();
                            self.pending_warp_relocation_collectible_id = None;
                            self.pending_repel_application = None;
                            self.pending_repel_application_expiry_tick = None;
                            self.expired_repel_applications.clear();
                            self.consumed_repel_delivery_generations.clear();
                            self.client_authority.clear_deferred_warp_pickup_acks();
                            self.repel_movement_sequence_window = None;
                            self.pending_warp_relocation_life_seq = None;
                            self.pending_warp_relocation_sequences.clear();
                            self.suppress_gameplay_step_after_network_rebase = true;
                            continue;
                        }
                        LifecycleReceiptOutcome::Applied { .. }
                        | LifecycleReceiptOutcome::Duplicate
                        | LifecycleReceiptOutcome::Rejected(_) => continue,
                        LifecycleReceiptOutcome::NotLifecycle => continue,
                    }
                }
                ClientProtocolEvent::PortalPlacementAccepted(receipt) => {
                    // A Portal terminal confirms a prediction from the exact
                    // currently alive life; it can never establish a
                    // replacement authority scope. In particular, a delayed
                    // old-life rejection must not refund the respawned ship.
                    if !matches!(
                        authority_admission.private_fact,
                        PrivateAuthorityAdmission::Current(_)
                    ) || !authority_admission.readiness_after.complete()
                    {
                        continue;
                    }
                    let Some(index) = self.pending_portal_placements.iter().position(|pending| {
                        let request = pending.request;
                        request.server_session_id == receipt.server_session_id
                            && request.arena_stream_id == receipt.arena_stream_id
                            && request.player_id == receipt.player_id
                            && request.life_seq == receipt.life_seq
                            && request.action_id == receipt.action_id
                            && request.position_sequence == receipt.position_sequence
                    }) else {
                        continue;
                    };
                    self.pending_portal_placements.remove(index);
                    let retained_revision = self
                        .last_self_server_portals
                        .filter(|(life_seq, _, _)| *life_seq == receipt.life_seq)
                        .map(|(_, revision, _)| revision);
                    let revision_admissible = prize_revision_is_admissible(
                        retained_revision,
                        receipt.prize_state_revision,
                    );
                    if receipt.outcome == PortalPlacementAccepted::OUTCOME_REJECTED {
                        self.ship.portals = self.ship.portals.saturating_add(1);
                        // Network placement has no provisional endpoint. A
                        // rejected replacement preserves the server's prior
                        // active endpoint exactly.
                    } else {
                        // The matching terminal is the only private fact that
                        // carries the endpoint. A newer PrizeState may have
                        // overtaken it, so endpoint action ordering is
                        // deliberately independent from inventory revision
                        // reconciliation.
                        let (position_x, position_y) = receipt.position();
                        self.install_authoritative_portal(
                            receipt.action_id,
                            vec2(position_x, position_y),
                            receipt.placed_at_tick,
                            receipt.expires_at_tick,
                        );
                    }
                    if revision_admissible {
                        self.last_self_server_portals = Some((
                            receipt.life_seq,
                            receipt.prize_state_revision,
                            receipt.portals,
                        ));
                    }
                    if self.last_self_server_prize_life_seq == Some(receipt.life_seq)
                        && prize_revision_is_admissible(
                            self.last_self_server_prize_revision,
                            receipt.prize_state_revision,
                        )
                    {
                        self.last_self_server_prize_revision = Some(receipt.prize_state_revision);
                    }
                    if self.pending_portal_placements.is_empty()
                        && let Some((life_seq, _, portals)) = self.last_self_server_portals
                        && self.client_authority.view().scope.life_seq == Some(life_seq)
                    {
                        self.ship.portals = u32::from(portals);
                    }
                }
                ClientProtocolEvent::PortalWarpAccepted(receipt) => {
                    if !matches!(
                        authority_admission.private_fact,
                        PrivateAuthorityAdmission::Current(_)
                    ) || !authority_admission.readiness_after.complete()
                    {
                        continue;
                    }
                    match self.client_authority.observe_portal_warp_terminal(*receipt) {
                        PortalWarpReceiptOutcome::Applied => {}
                        PortalWarpReceiptOutcome::Rejected
                        | PortalWarpReceiptOutcome::Duplicate
                        | PortalWarpReceiptOutcome::RejectedAuthority(_) => continue,
                    }
                    let from = self.ship.pos;
                    let (position_x, position_y, velocity_x, velocity_y, orientation) =
                        receipt.canonical_state();
                    self.warps.push(WarpEffect {
                        pos: from,
                        age: 0.0,
                    });
                    self.ship.pos = vec2(position_x, position_y);
                    self.ship.vel = vec2(velocity_x, velocity_y);
                    self.ship.orientation = orientation;
                    self.ship.energy = f32::from(receipt.energy);
                    self.last_self_server_energy = Some(self.ship.energy);
                    self.ship.flash = true;
                    self.ship.bomb_cooldown = manual_warp_bomb_cooldown_after(
                        self.ship.bomb_cooldown,
                        REPEL_REUSE_COOLDOWN,
                    );
                    if self.map().tile_at_world(self.ship.pos) != WORMHOLE_TILE_ID {
                        self.ship.wormhole_cooldown = WORMHOLE_COOLDOWN;
                    }
                    self.ship.fake_antiwarp_timer = self.tuning.antiwarp_settle_seconds;
                    if receipt.used_portal {
                        self.consume_authoritative_portal(receipt.action_id);
                    }
                    self.warps.push(WarpEffect {
                        pos: self.ship.pos,
                        age: 0.0,
                    });
                    self.pending_same_life_movement_boundary_position = true;
                    self.suppress_gameplay_step_after_network_rebase = true;
                    #[cfg(not(test))]
                    let warp_sound = self.assets.warp_sound.as_ref();
                    #[cfg(test)]
                    let warp_sound = self
                        .assets
                        .0
                        .as_ref()
                        .and_then(|assets| assets.warp_sound.as_ref());
                    if let Some(sound) = warp_sound {
                        play_sound(
                            sound,
                            PlaySoundParams {
                                looped: false,
                                volume: self.effect_volume(0.36),
                            },
                        );
                    }
                }
                ClientProtocolEvent::WarpRelocationAccepted(receipt) => {
                    if warp_relocation_receipt_matches(
                        self.pending_warp_relocation_collectible_id,
                        self.pending_warp_relocation_life_seq,
                        &self.pending_warp_relocation_sequences,
                        *receipt,
                    ) {
                        self.pending_warp_relocation_collectible_id = None;
                        self.pending_warp_relocation_life_seq = None;
                        self.pending_warp_relocation_sequences.clear();
                    }
                }
                ClientProtocolEvent::RepelApplicationAccepted(receipt) => {
                    let authority = self.client_authority.view();
                    let active_match = repel_application_receipt_matches(
                        authority.scope.server_session_id,
                        authority.scope.life_seq,
                        self.pending_repel_application,
                        *receipt,
                    );
                    let expired_index = expired_repel_application_receipt_index(
                        authority.scope.server_session_id,
                        authority.scope.life_seq,
                        &self.expired_repel_applications,
                        *receipt,
                    );
                    if active_match || expired_index.is_some() {
                        let matching_live_window = expired_index.is_some()
                            && self
                                .repel_movement_sequence_window
                                .is_some_and(|window| window.0 == receipt.application_sequence);
                        if active_match || matching_live_window {
                            self.repel_movement_sequence_window = repel_receipt_movement_window(
                                self.next_input_seq,
                                receipt.application_sequence,
                                receipt.accepted_end_sequence,
                                receipt.active_steps,
                            );
                            self.ship.repel_timer = f32::from(repel_receipt_remaining_steps(
                                self.next_input_seq,
                                receipt.accepted_end_sequence,
                            )) * crate::fixed_step::GAMEPLAY_STEP_SECONDS;
                        }
                        if active_match {
                            self.pending_repel_application = None;
                            self.pending_repel_application_expiry_tick = None;
                        } else if let Some(index) = expired_index {
                            self.expired_repel_applications.remove(index);
                            self.repel_delivery_expired_receipts_recovered = self
                                .repel_delivery_expired_receipts_recovered
                                .saturating_add(1);
                        }
                        self.repel_delivery_last_accepted_end_sequence =
                            Some(receipt.accepted_end_sequence);
                        self.repel_delivery_last_accepted_steps = Some(receipt.active_steps);
                        if !self
                            .consumed_repel_delivery_generations
                            .contains(&receipt.delivery_generation)
                        {
                            self.consumed_repel_delivery_generations
                                .push_back(receipt.delivery_generation);
                            while self.consumed_repel_delivery_generations.len() > 256 {
                                self.consumed_repel_delivery_generations.pop_front();
                            }
                        }
                        // A matching receipt narrows an existing logical
                        // window even after wall expiry. Once that window is
                        // retired it can never be re-armed by a late receipt,
                        // nor can an older receipt change a newer impulse.
                        self.apply_deferred_warp_pickup_acks();
                    }
                }
                ClientProtocolEvent::MovementCorrected(correction) => {
                    let pending_repel = self.pending_repel_application.filter(|marker| {
                        correction.reason != MovementCorrection::REASON_REPEL_WINDOW
                            || correction
                                .repel_window
                                .is_none_or(|(start, _, generation)| {
                                    marker.application_sequence != start
                                        || marker.delivery_generation != generation
                                })
                    });
                    let newer_boundary_pending =
                        newer_same_life_boundary_requires_correction_cancellation(
                            self.pending_same_life_movement_boundary_position,
                            self.pending_warp_relocation_collectible_id,
                            pending_repel,
                        );
                    // Keep the existing local lineage untouched until the
                    // server serializes either an exact current-tail rebase or
                    // cancellation. This makes a proposal invalidated by an
                    // overtaking Warp/door/Repel safe even if the proposal was
                    // already delivered. Any edge sampled while the barrier is
                    // live is retained separately until that terminal arrives.
                    let _ = self.client_authority.observe_movement_correction(
                        *correction,
                        self.next_input_seq,
                        newer_boundary_pending,
                    );
                }
                ClientProtocolEvent::MovementCorrectionAccepted(receipt) => {
                    if let CorrectionReceiptOutcome::Applied {
                        receipt,
                        proposal_reason,
                        repel_window,
                    } = self.client_authority.observe_correction_terminal(*receipt)
                    {
                        let from = self.ship.pos;
                        apply_exact_movement_correction_terminal(&mut self.ship, receipt);
                        if proposal_reason == MovementCorrection::REASON_REPEL_WINDOW {
                            // The reliable correction restores the sealed
                            // movement interval without consuming the held
                            // Repel application receipt or advancing its queue.
                            self.repel_movement_sequence_window = repel_window
                                .map(|(start, end, _)| (start, end))
                                .filter(|window| {
                                    !crate::gameplay_rules::repel_sequence_window_end_reached(
                                        *window,
                                        self.next_input_seq,
                                    )
                                });
                            self.ship.repel_timer = 0.0;
                        }
                        if proposal_reason == MovementCorrection::REASON_BRICK_OVERLAP {
                            self.ship.flash = true;
                            self.warps.push(WarpEffect {
                                pos: from,
                                age: 0.0,
                            });
                            self.warps.push(WarpEffect {
                                pos: self.ship.pos,
                                age: 0.0,
                            });
                        }
                        if !self.rebase_network_doors_after_authority_receipt(receipt.server_tick) {
                            self.suppress_gameplay_step_after_network_rebase = true;
                        }
                        // The terminal state is the new exact lineage at
                        // application_sequence. Old rolling input redundancy
                        // must not cross that boundary.
                        self.outbound_input_ring.clear();
                        self.self_correction_count = self.self_correction_count.wrapping_add(1);
                        self.last_self_correction_reason = proposal_reason;
                        self.camera.target = self.ship.pos;
                    }
                }
                ClientProtocolEvent::ServerVersion(packet) => {
                    if crate::browser_metrics::debug_mode() {
                        self.push_notification(&server_version_notification(packet), ORANGE);
                    }
                }
                ClientProtocolEvent::ArenaDirectory(listing) => {
                    if crate::browser_metrics::debug_mode() {
                        self.push_notification(&arena_directory_notification(listing), ORANGE);
                    }
                }
                // Zone advertisement packets are decoded for compatibility, but
                // do not create a gameplay popup or notification.
                ClientProtocolEvent::ZoneBannerAdvertised(_) => {}
                ClientProtocolEvent::SoccerGoal(goal) => {
                    self.push_notification(&soccer_goal_notification(goal), GREEN);
                }
                ClientProtocolEvent::UfoToggled(packet) => {
                    self.push_notification(ufo_toggle_notification(packet), ORANGE);
                }
                ClientProtocolEvent::PersonalKothTimerSet(packet) => {
                    self.push_notification(&personal_koth_timer_notification(packet), ORANGE);
                }
                ClientProtocolEvent::KothGameReset(packet) => {
                    self.push_notification(&koth_reset_notification(packet), ORANGE);
                }
                ClientProtocolEvent::KothTimeAdded(packet) => {
                    self.push_notification(&koth_time_added_notification(packet), ORANGE);
                }
                ClientProtocolEvent::FlagVictory(victory) => {
                    if self.tuning.victory_music {
                        self.play_victory_music_once();
                    }
                    self.push_notification(&flag_victory_notification(victory), GREEN);
                }
                ClientProtocolEvent::FlagReward(reward) => {
                    self.push_notification(&flag_reward_notification(reward), GREEN);
                }
                ClientProtocolEvent::SpeedGameOver(result) => {
                    self.push_notification(&speed_game_over_notification(result), ORANGE);
                }
                ClientProtocolEvent::SendDamageToggled(_) => {}
                ClientProtocolEvent::DamageWatched(report) => {
                    if authority_admission.private_fact == PrivateAuthorityAdmission::Reject {
                        continue;
                    }
                    if let Some(entry) = report.entries.last() {
                        let energy_before = entry
                            .energy_before
                            .unwrap_or_else(|| entry.energy.saturating_add(entry.damage));
                        self.network_damage_event_seq =
                            self.network_damage_event_seq.wrapping_add(1);
                        self.last_network_damage_player_id = Some(report.player_id);
                        self.last_network_damage_attacker_id = Some(entry.attacker_id);
                        self.last_network_damage_energy = Some(entry.energy as f32);
                        self.last_network_damage_energy_before = Some(energy_before as f32);
                        self.last_network_damage_amount = Some(entry.damage);
                        self.last_network_damage_weapon = Some(entry.weapon_data);
                        self.last_network_damage_server_tick = Some(report.timestamp);
                        self.last_network_damage_server_seq = entry.damage_seq;
                        self.last_network_damage_victim_life_seq = entry.victim_life_seq;
                        self.last_network_damage_shooter_life_seq = entry.shooter_life_seq;
                        self.last_network_damage_for_self =
                            self.client_authority.view().scope.player_id == Some(report.player_id);
                        if self.last_network_damage_for_self {
                            self.self_damage_event_seq = self.self_damage_event_seq.wrapping_add(1);
                            self.last_self_damage_attacker_id = Some(entry.attacker_id);
                            self.last_self_damage_energy = Some(entry.energy as f32);
                            self.last_self_damage_energy_before = Some(energy_before as f32);
                            self.last_self_damage_amount = Some(entry.damage);
                            self.last_self_damage_weapon = Some(entry.weapon_data);
                            self.last_self_damage_server_tick = Some(report.timestamp);
                            self.last_self_damage_server_seq = entry.damage_seq;
                            self.last_self_damage_victim_life_seq = entry.victim_life_seq;
                            self.last_self_damage_shooter_life_seq = entry.shooter_life_seq;
                            // WATCH_DAMAGE is a visual/diagnostic fact, not a
                            // state mutation. The same snapshot carries the
                            // authoritative self entity energy and reconciles
                            // it below. Applying this event by reusable numeric
                            // id allowed an event decoded around a lifecycle
                            // transition to overwrite the newer life's energy.
                        }
                        self.publish_network_damage_trace(report, entry, energy_before);
                    }
                    if crate::browser_metrics::debug_mode() {
                        self.push_notification(&watched_damage_notification(report), ORANGE);
                    }
                }
                ClientProtocolEvent::FileTransferred(file) => {
                    if crate::browser_metrics::debug_mode() {
                        self.push_notification(&server_file_transfer_notification(file), GREEN);
                    }
                }
                ClientProtocolEvent::ClientFileRequested(request) => {
                    if crate::browser_metrics::debug_mode() {
                        self.push_notification(&client_file_request_notification(request), ORANGE);
                    }
                }
                ClientProtocolEvent::PlayerVoice(voice) => {
                    #[cfg(target_arch = "wasm32")]
                    self.queue_voice_playback(voice);
                    if crate::browser_metrics::debug_mode() {
                        self.push_notification(&player_voice_notification(voice), ORANGE);
                    }
                }
                ClientProtocolEvent::Chat(chat) => {
                    let sender_name = self
                        .network_world
                        .players
                        .get(&chat.sender_id)
                        .map(|player| player.name.as_str());
                    self.chat.push_message(
                        format_network_chat_message(
                            chat.chat_type,
                            sender_name,
                            chat.sender_id,
                            &chat.message,
                        ),
                        match chat.chat_type {
                            2 => TextColor::Blue,
                            3 => TextColor::Yellow,
                            _ => TextColor::Green,
                        },
                    );
                    if !self.handle_original_special_chat_sound(chat.sound) {
                        if let Some(sound) = self.chat_sound(chat.sound) {
                            play_sound(
                                sound,
                                PlaySoundParams {
                                    looped: false,
                                    volume: self.chat_volume(0.28),
                                },
                            );
                        }
                    }
                }
                ClientProtocolEvent::SnapshotReceived(frame) => {
                    // A snapshot rejected by the immutable pre-batch plan is
                    // wholly inert. In particular, it cannot restore private
                    // lifecycle/Rocket/Repel/position/WATCH_DAMAGE/weapon
                    // state after the pre-gameplay hard reset.
                    if authority_admission.private_fact == PrivateAuthorityAdmission::Reject {
                        continue;
                    }
                    // Only new frames reach here, so this follows the server,
                    // including a rollback that moves it back. A frame without
                    // it (no input accepted yet this life) restores whole windows.
                    self.server_input_applied_through =
                        frame.events.iter().find_map(|event| match event {
                            SnapshotEvent::InputApplied(applied) => Some(applied.through_sequence),
                            _ => None,
                        });
                    let observer_timeline_directive = frame.events.iter().find_map(|event| {
                        let SnapshotEvent::ObserverTimeline(directive) = event else {
                            return None;
                        };
                        Some(*directive)
                    });
                    let private_repel_delivery = frame.events.iter().find_map(|event| {
                        let SnapshotEvent::LifecycleState(lifecycle) = event else {
                            return None;
                        };
                        lifecycle.repel_delivery.and_then(|delivery| {
                            lifecycle.repel_until_tick.map(|expires_at_tick| {
                                (
                                    frame.header.snapshot_id,
                                    frame.header.tick,
                                    lifecycle.life_seq,
                                    expires_at_tick,
                                    delivery,
                                )
                            })
                        })
                    });
                    let current_server_tick = current_rocket_reconciliation_tick(
                        frame.header.tick,
                        self.browser_server_tick(),
                    );
                    for event in &frame.events {
                        let SnapshotEvent::LifecycleState(lifecycle) = event else {
                            continue;
                        };
                        let action = self.apply_authoritative_lifecycle_state(*lifecycle);
                        // RepelTime starts only when the exact weapon-bearing
                        // snapshot is applied. Bind it to that immutable frame
                        // tick, not a later synchronized/poll tick or a repeated
                        // lifecycle frame, so client and canonical replay count
                        // the same logical movement records under RepelSpeed.
                        if action == LifecycleAction::StartDeath {
                            let death_pos = self.ship.pos;
                            self.drop_carried_flag_at(death_pos);
                            self.start_player_respawn_with_delay(
                                death_pos,
                                authoritative_respawn_delay_seconds(
                                    lifecycle.respawn_at_tick,
                                    current_server_tick,
                                ),
                            );
                        }
                    }
                    for event in &frame.events {
                        let SnapshotEvent::PrizeState(prize) = event else {
                            continue;
                        };
                        if !matches!(
                            authority_admission.private_fact,
                            PrivateAuthorityAdmission::Current(_)
                                | PrivateAuthorityAdmission::Replacement(_)
                        ) || !authority_admission.readiness_after.complete()
                        {
                            continue;
                        }
                        self.apply_authoritative_prize_state(*prize, frame.header.tick);
                    }
                    if let Some((snapshot_id, snapshot_tick, life_seq, expires_at_tick, delivery)) =
                        private_repel_delivery
                    {
                        self.repel_delivery_frames_seen =
                            self.repel_delivery_frames_seen.saturating_add(1);
                        self.repel_delivery_last_snapshot_tick = Some(snapshot_tick);
                        self.repel_delivery_last_expiry_tick = Some(expires_at_tick);
                        let observed_server_tick = self.browser_server_tick();
                        self.repel_delivery_last_observed_tick = observed_server_tick;
                        let provisional_steps =
                            observed_server_tick.and_then(|observed_server_tick| {
                                private_repel_provisional_steps(
                                    snapshot_tick,
                                    expires_at_tick,
                                    delivery.active_steps,
                                    observed_server_tick,
                                )
                            });
                        let authority_scope = self.client_authority.view().scope;
                        let life_matches = authority_scope.alive == Some(true)
                            && authority_scope.life_seq == Some(life_seq);
                        if self
                            .consumed_repel_delivery_generations
                            .contains(&delivery.delivery_generation)
                            || self.expired_repel_applications.iter().any(|marker| {
                                marker.delivery_generation == delivery.delivery_generation
                            })
                        {
                            // An unordered retry may arrive after its reliable
                            // receipt or after the immutable wall deadline.
                            // Neither may apply the target effects again.
                            self.repel_delivery_consumed_retries =
                                self.repel_delivery_consumed_retries.saturating_add(1);
                        } else if self.pending_repel_application.is_some_and(|pending| {
                            pending.delivery_generation == delivery.delivery_generation
                        }) {
                            // Retries cannot rewrite the immutable application.
                            self.repel_delivery_pending_retries =
                                self.repel_delivery_pending_retries.saturating_add(1);
                        } else if !life_matches {
                            self.repel_delivery_life_rejects =
                                self.repel_delivery_life_rejects.saturating_add(1);
                        } else if provisional_steps.is_none() {
                            self.repel_delivery_timing_rejects =
                                self.repel_delivery_timing_rejects.saturating_add(1);
                        } else if self.pending_repel_application.is_some() {
                            self.repel_delivery_application_rejects =
                                self.repel_delivery_application_rejects.saturating_add(1);
                        } else if self.apply_private_remote_repel(
                            delivery.source_player_id,
                            vec2(f32::from(delivery.origin_x), f32::from(delivery.origin_y)),
                            delivery.source_frequency,
                            vec2(
                                f32::from_bits(delivery.expected_velocity_x_bits),
                                f32::from_bits(delivery.expected_velocity_y_bits),
                            ),
                        ) {
                            let active_steps = provisional_steps
                                .expect("predeadline Repel has an exact remaining suffix");
                            // Bind the diagnostic to the immutable offer that
                            // actually armed this generation. Later unordered
                            // retries may carry a shorter wall-time remainder
                            // and must not rewrite the acceptance comparison.
                            self.repel_delivery_last_offered_steps = Some(delivery.active_steps);
                            let marker = crate::continuum_position::ClientRepelApplicationMarker {
                                snapshot_id,
                                application_sequence: self.next_input_seq,
                                delivery_generation: delivery.delivery_generation,
                            };
                            self.pending_repel_application = Some(marker);
                            self.pending_repel_application_expiry_tick = Some(expires_at_tick);
                            self.ship.repel_timer =
                                f32::from(active_steps) * crate::fixed_step::GAMEPLAY_STEP_SECONDS;
                            self.repel_movement_sequence_window = Some(
                                crate::gameplay_rules::repel_sequence_window_from_active_steps(
                                    marker.application_sequence,
                                    u32::from(active_steps),
                                ),
                            );
                            self.repel_delivery_frames_applied =
                                self.repel_delivery_frames_applied.saturating_add(1);
                        } else {
                            self.repel_delivery_application_rejects =
                                self.repel_delivery_application_rejects.saturating_add(1);
                        }
                    }
                    for event in &frame.events {
                        let SnapshotEvent::RocketState(rocket) = event else {
                            continue;
                        };
                        if !rocket_state_is_admissible_across_lifecycle_cutover(
                            self.client_authority.view().lifecycle_intent.is_some(),
                            self.last_self_server_rocket_life_seq,
                            rocket.life_seq,
                            self.require_forward_self_server_rocket_life,
                        ) {
                            continue;
                        }
                        if !rocket_state_resolves_pending_activation(
                            self.pending_rocket_activation_sequence,
                            *rocket,
                        ) {
                            // This frame predates the just-issued pulse;
                            // treating it as rejection would cancel valid F3
                            // before the server had a chance to observe it.
                            continue;
                        }
                        let rocket_lifetime_seconds = self.ship_tuning().rocket_lifetime;
                        self.pending_rocket_activation_sequence = None;
                        self.require_forward_self_server_rocket_life = false;
                        self.last_self_server_rocket_life_seq = Some(rocket.life_seq);
                        self.last_self_server_rocket_activation_sequence =
                            rocket.activation_sequence;
                        self.last_self_server_rocket_until_tick = rocket.until_tick;
                        self.ship.rockets = u32::from(rocket.rockets);
                        self.ship.rocket_timer = reconciled_self_rocket_timer_seconds(
                            self.ship.rocket_timer,
                            rocket_lifetime_seconds,
                            *rocket,
                            current_server_tick,
                            self.next_input_seq,
                        );
                    }
                    // Protocol ACK means only that the snapshot codec/base was
                    // accepted. Publish a combat-view watermark only after
                    // this exact frame crossed NetworkWorld and the Game-side
                    // private lifecycle/application path above. The repeated
                    // directive is mandatory for this new proof; legacy or
                    // malformed carriers remain fail-closed.
                    let world_applied = self.network_world.authoritative_snapshot_id
                        == Some(frame.header.snapshot_id)
                        && self.network_world.authoritative_snapshot_server_tick
                            == Some(frame.header.tick);
                    if world_applied && let Some(directive) = observer_timeline_directive {
                        use crate::combat_timeline::ClientObserverTimelineDisposition;
                        let disposition = self
                            .observer_combat_timeline
                            .observe_directive(directive, self.next_input_seq);
                        if matches!(
                            disposition,
                            ClientObserverTimelineDisposition::Applied
                                | ClientObserverTimelineDisposition::Pending
                                | ClientObserverTimelineDisposition::Duplicate
                        ) {
                            let applied =
                                self.observer_combat_timeline.observe_game_applied_snapshot(
                                    frame.header.snapshot_id,
                                    frame.header.tick,
                                );
                            if applied
                                == crate::combat_timeline::ClientAppliedSnapshotDisposition::Applied
                            {
                                self.combat_view_ship_positions
                                    .record(frame.header.snapshot_id, &self.network_world);
                            }
                        }
                    }
                }
                ClientProtocolEvent::PreGameplayPrizeState {
                    snapshot_tick,
                    state,
                    ..
                } => {
                    if matches!(
                        authority_admission.private_fact,
                        PrivateAuthorityAdmission::Current(_)
                            | PrivateAuthorityAdmission::Replacement(_)
                    ) && authority_admission.readiness_after.complete()
                    {
                        self.apply_authoritative_prize_state(*state, *snapshot_tick);
                    }
                }
                _ => {}
            }
        }
        admitted_actions
    }

    fn apply_authoritative_player_death(&mut self, death: PlayerDeath) {
        let self_player_id = self.client_authority.view().scope.player_id;
        if let Some(viewer_id) = self_player_id
            && viewer_id != death.killed_id
        {
            let killed_name = self
                .network_world
                .players
                .get(&death.killed_id)
                .map(|player| player.name.as_str())
                .unwrap_or("Player");
            let killer_name = if viewer_id == death.killer_id {
                self.local_player_name()
            } else {
                self.network_world
                    .players
                    .get(&death.killer_id)
                    .map(|player| player.name.as_str())
                    .unwrap_or("Player")
            };
            let message =
                player_killed_by_notification(killed_name, u32::from(death.bounty), killer_name);
            let color = if viewer_id == death.killer_id {
                Color::from_rgba(255, 220, 95, 255)
            } else {
                Color::from_rgba(120, 220, 150, 255)
            };
            self.combat_kill_notification_count =
                self.combat_kill_notification_count.wrapping_add(1);
            self.last_combat_kill_notification = Some(message.clone());
            self.push_notification(&message, color);
        }
        // This global notice identifies the victim life, not the firing
        // life. A retained old-life projectile can earn cumulative credit
        // without awarding bounty to its owner's successor. Recipient-private
        // PrizeState owns the current life's bounty, including normal kills;
        // predicting it here also double-counts when its snapshot arrives first.

        if self_player_id != Some(death.killed_id) {
            return;
        }

        let lifecycle = SnapshotLifecycleState {
            life_seq: death.life_seq,
            alive: false,
            respawn_at_tick: Some(death.respawn_at_tick),
            repel_until_tick: None,
            repel_delivery: None,
        };
        if self.apply_authoritative_lifecycle_state(lifecycle) != LifecycleAction::StartDeath {
            return;
        }

        let death_pos = self.ship.pos;
        let killer_label = (death.killer_id != death.killed_id)
            .then(|| {
                self.network_world
                    .players
                    .get(&death.killer_id)
                    .map(|player| player.name.clone())
            })
            .flatten();
        self.drop_carried_flag_at(death_pos);
        self.push_notification(
            &player_destroyed_notification(
                self.local_player_name(),
                death.bounty as u32,
                killer_label.as_deref(),
            ),
            Color::from_rgba(255, 120, 90, 255),
        );
        let delay = self.browser_server_tick().map_or_else(
            || respawn_delay_seconds(self.tuning.enter_delay_seconds),
            |current_tick| {
                authoritative_respawn_delay_seconds(Some(death.respawn_at_tick), current_tick)
            },
        );
        self.start_player_respawn_with_delay(death_pos, delay);
    }

    fn apply_authoritative_lifecycle_state(
        &mut self,
        lifecycle: SnapshotLifecycleState,
    ) -> LifecycleAction {
        let authority_before = self.client_authority.view();
        let mut sequencer_scope = authority_before.scope;
        sequencer_scope.life_seq = Some(lifecycle.life_seq);
        sequencer_scope.alive = Some(lifecycle.alive);
        let respawn_remaining_seconds = if lifecycle.alive {
            0.0
        } else {
            self.browser_server_tick().map_or_else(
                || respawn_delay_seconds(self.tuning.enter_delay_seconds),
                |current_tick| {
                    authoritative_respawn_delay_seconds(lifecycle.respawn_at_tick, current_tick)
                },
            )
        };
        let sequencer_disposition = self.client_authority.observe_private_lifecycle(
            ClientAuthoritySource::Event,
            sequencer_scope,
            lifecycle.respawn_at_tick,
            respawn_remaining_seconds,
        );
        // A confirmed spectator is an observer, not a dead ship awaiting respawn.
        if !lifecycle.alive
            && (self.client_authority.view().phase == ClientAuthorityPhase::Spectating
                || (self.client_authority.view().lifecycle_intent
                    == Some(PendingLifecycleRequest::Ship(SPECTATOR_SHIP))
                    && self.client_authority.view().scope.life_seq == Some(lifecycle.life_seq)))
        {
            return LifecycleAction::Ignore;
        }
        let action = authoritative_lifecycle_action(
            authority_before.scope.life_seq,
            authority_before.scope.alive,
            lifecycle,
        );
        if action != LifecycleAction::Ignore
            && matches!(
                sequencer_disposition,
                ClientAuthorityDisposition::Applied
                    | ClientAuthorityDisposition::Replacement
                    | ClientAuthorityDisposition::Observed
                    | ClientAuthorityDisposition::Duplicate
            )
        {
            action
        } else {
            LifecycleAction::Ignore
        }
    }

    fn publish_network_damage_trace(
        &self,
        report: &ServerDamageReport,
        entry: &ServerDamageReportEntry,
        energy_before: u16,
    ) {
        let for_self = self.client_authority.view().scope.player_id == Some(report.player_id);
        let self_damage_seq = for_self.then_some(self.self_damage_event_seq);
        let self_damage_server_seq = for_self.then_some(entry.damage_seq).flatten();
        let self_damage_server_tick = for_self.then_some(report.timestamp);
        let self_damage_victim_life_seq = for_self.then_some(entry.victim_life_seq).flatten();
        let self_damage_shooter_life_seq = for_self.then_some(entry.shooter_life_seq).flatten();
        let self_damage_attacker_id = for_self.then_some(entry.attacker_id);
        let self_damage_energy_before = for_self.then_some(energy_before as f32);
        let self_damage_energy_after = for_self.then_some(entry.energy as f32);
        let self_damage_amount = for_self.then_some(entry.damage);
        let self_damage_weapon = for_self.then_some(entry.weapon_data);
        let remote_energy = self
            .network_world
            .players
            .get(&report.player_id)
            .and_then(|player| player.display_energy());
        let visible_energy = self.visible_hud_energy();
        publish_damage_event(&format!(
            "{{\"eventSeq\":{},\"selfPlayerId\":{},\"victimId\":{},\"attackerId\":{},\"damageSeq\":{},\"projectileId\":{},\"serverTick\":{},\"victimLifeSeq\":{},\"shooterLifeSeq\":{},\"explicitLethal\":{},\"sourceSnapshotId\":{},\"sourceSnapshotEpoch\":{},\"weapon\":{},\"damage\":{},\"energyBefore\":{},\"energyAfter\":{},\"forSelf\":{},\"playerEnergy\":{},\"playerPredictedEnergy\":{},\"visibleEnergyDigits\":{},\"playerMaxEnergy\":{},\"serverPlayerEnergy\":{},\"playerAlive\":{},\"serverPlayerAlive\":{},\"remoteEnergy\":{},\"selfDamageSeq\":{},\"selfDamageServerSeq\":{},\"selfDamageServerTick\":{},\"selfDamageVictimLifeSeq\":{},\"selfDamageShooterLifeSeq\":{},\"selfDamageAttackerId\":{},\"selfDamageEnergyBefore\":{},\"selfDamageEnergyAfter\":{},\"selfDamage\":{},\"selfDamageWeapon\":{}}}",
            self.network_damage_event_seq,
            json_opt_u16(self.client_authority.view().scope.player_id),
            report.player_id,
            entry.attacker_id,
            json_opt_u32(entry.damage_seq),
            json_opt_u32(entry.projectile_id),
            report.timestamp,
            json_opt_u32(entry.victim_life_seq),
            json_opt_u32(entry.shooter_life_seq),
            json_opt_bool(entry.explicit_lethal),
            json_opt_u16(entry.source_snapshot_id),
            json_opt_u32(entry.source_snapshot_epoch),
            entry.weapon_data,
            entry.damage,
            energy_before,
            entry.energy,
            for_self,
            json_opt_f32(Some(visible_energy)),
            json_opt_f32(Some(self.ship.energy)),
            visible_energy.max(0.0) as u32,
            json_opt_f32(Some(self.ship.max_energy)),
            json_opt_f32(self.last_self_server_energy),
            !self.spectator_mode && self.player_respawn_timer <= 0.0,
            json_opt_bool(self.client_authority.view().scope.alive),
            json_opt_f32(remote_energy),
            json_opt_u32(self_damage_seq),
            json_opt_u32(self_damage_server_seq),
            json_opt_u32(self_damage_server_tick),
            json_opt_u32(self_damage_victim_life_seq),
            json_opt_u32(self_damage_shooter_life_seq),
            json_opt_u16(self_damage_attacker_id),
            json_opt_f32(self_damage_energy_before),
            json_opt_f32(self_damage_energy_after),
            json_opt_u16(self_damage_amount),
            json_opt_u16(self_damage_weapon),
        ));
    }

    fn apply_authoritative_self_snapshot_energy(&mut self, energy: f32) {
        let authority = self.client_authority.view();
        if !self_runtime_snapshot_is_admissible(authority.scope.alive, authority.transition_pending)
        {
            return;
        }
        let was_respawning = self.player_respawn_timer > 0.0;
        self.last_self_server_energy = Some(energy);
        if was_respawning && energy > 0.0 {
            self.player_respawn_timer = 0.0;
            self.respawn_input_latch.arm();
            self.self_correction_count = self.self_correction_count.wrapping_add(1);
            self.last_self_correction_reason = SELF_CORRECTION_RESPAWN_AUTHORITY;
        }
        // [REVIEW M10] Reconcile the *predicted* energy with tolerance
        // instead of clamping it to the (lagged) server snapshot every
        // frame. The old unconditional `min` stacked with the display-level
        // server bound in `visible_hud_energy`, dragging the energy used for
        // fire/warp gating down to a ~1-RTT-stale value during recharge.
        // Prediction now leads recharge; the server still wins on real
        // divergence (drops beyond recharge-lag tolerance adopt downward
        // immediately), and the display still cannot outrun the server-fed
        // value. An authoritative negative-Energy transient may exceed the
        // new capacity until ordinary spend/recharge converges it. Respawn
        // adopts the authoritative spawn energy exactly.
        self.ship.energy = if self.network_prize_baseline_pending || was_respawning && energy > 0.0
        {
            energy
        } else {
            reconciled_self_energy(self.ship.energy, energy)
        };
    }

    fn apply_authoritative_self_snapshot(&mut self, update: &PlayerPositionUpdate) {
        if self.player_respawn_timer > 0.0 {
            self.ship.pos = vec2(update.x as f32, update.y as f32);
            self.ship.vel = vec2(
                crate::continuum_position::protocol_velocity_pixels_per_second(update.velocity_x),
                crate::continuum_position::protocol_velocity_pixels_per_second(update.velocity_y),
            );
            self.ship.orientation =
                crate::continuum_position::protocol_direction_to_orientation(update.direction);
        }
        if let Some(energy) = update.energy {
            self.apply_authoritative_self_snapshot_energy(energy as f32);
        }
    }

    fn handle_original_special_chat_sound(&mut self, sound: u8) -> bool {
        match original_special_chat_sound(sound) {
            Some(OriginalSpecialChatSound::MusicLoop) => {
                if !self.victory_music_looping {
                    if let Some(music) = self.assets.victory_music_long.as_ref() {
                        play_sound(
                            music,
                            PlaySoundParams {
                                looped: true,
                                volume: self.music_volume(0.32),
                            },
                        );
                        self.victory_music_looping = true;
                    }
                }
                true
            }
            Some(OriginalSpecialChatSound::MusicStop) => {
                if let Some(music) = self.assets.victory_music_long.as_ref() {
                    stop_sound(music);
                }
                self.victory_music_looping = false;
                true
            }
            Some(OriginalSpecialChatSound::MusicOnce) => {
                self.play_victory_music_once();
                true
            }
            Some(OriginalSpecialChatSound::Goal) => {
                if let Some(sound) = self.assets.goal_sound.as_ref() {
                    play_sound(
                        sound,
                        PlaySoundParams {
                            looped: false,
                            volume: self.chat_volume(0.42),
                        },
                    );
                }
                true
            }
            None => false,
        }
    }

    fn play_victory_music_once(&mut self) {
        if self.victory_music_looping {
            return;
        }
        if let Some(music) = self.assets.victory_music.as_ref() {
            play_sound(
                music,
                PlaySoundParams {
                    looped: false,
                    volume: self.music_volume(0.36),
                },
            );
        }
    }

    fn chat_sound(&self, sound: u8) -> Option<&Sound> {
        self.lvz_chat_sounds
            .get(&sound)
            .or_else(|| self.assets.chat_sound(sound))
    }

    pub(in crate::game) fn weapon_render_partial_seconds(&self) -> f32 {
        #[cfg(target_arch = "wasm32")]
        {
            self.browser_network.as_ref().map_or(0.0, |browser| {
                browser.server_clock.weapon_render_partial_seconds()
            })
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            0.0
        }
    }

    pub(in crate::game) fn browser_server_tick(&self) -> Option<u32> {
        #[cfg(target_arch = "wasm32")]
        {
            browser_server_tick_across_owner_poll(
                self.browser_network
                    .as_ref()
                    .and_then(|browser| browser.server_clock.current_server_tick()),
                self.last_observed_browser_server_tick,
            )
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(test)]
            {
                self.last_observed_browser_server_tick
            }
            #[cfg(not(test))]
            {
                None
            }
        }
    }

    /// Return only a tick carried by an authenticated server packet. The
    /// browser's continuously extrapolated protocol clock is intentionally
    /// excluded: using it as a receipt witness would keep freshness gates open
    /// forever after transport delivery stops.
    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn browser_authenticated_server_tick(&self) -> Option<u32> {
        self.browser_network
            .as_ref()
            .and_then(|browser| browser.server_clock.latest_authenticated_server_tick())
    }

    pub(in crate::game) fn combat_view_receipt_for_input(
        &mut self,
        input_sequence: u16,
    ) -> crate::combat_timeline::CombatViewReceiptV1 {
        let observer_life_seq = self.client_authority.view().scope.life_seq;
        let server_tick = self.browser_server_tick();
        self.observer_combat_timeline
            .receipt(observer_life_seq, input_sequence, server_tick)
    }

    /// Capture the receipt before local weapon prediction. Deferred inbound
    /// snapshots are applied later in this logical step, so regenerating the
    /// receipt during packet assembly could otherwise bind one input to two
    /// different presentation views.
    pub(in crate::game) fn prepare_combat_view_receipt_for_input(
        &mut self,
        input_sequence: u16,
    ) -> crate::combat_timeline::CombatViewReceiptV1 {
        let authority_segment = self.client_authority.view().segment;
        if let Some(prepared) = self.prepared_combat_view_receipt {
            if prepared.authority_segment != authority_segment {
                // This physical edge was captured against the previously
                // drawn authority scope. Inbound lifecycle work may replace
                // that scope before the logical step consumes the edge, but
                // must never rebind it to the successor life. Preserve a
                // fail-closed reservation for the action's structural input
                // sequence; earlier movement-only catch-up steps can still
                // publish truthful receipts for the current scope.
                let unavailable =
                    crate::combat_timeline::CombatViewReceiptV1::unavailable(input_sequence);
                self.prepared_combat_view_receipt = Some(PreparedCombatViewReceipt {
                    authority_segment,
                    receipt: unavailable,
                });
                return unavailable;
            }
            if prepared.authority_segment == authority_segment {
                if prepared.receipt.input_sequence == input_sequence {
                    return prepared.receipt;
                }
                let retarget_distance =
                    input_sequence.wrapping_sub(prepared.receipt.input_sequence);
                if prepared.receipt.input_sequence == self.next_input_seq
                    && retarget_distance != 0
                    && retarget_distance
                        <= u16::from(
                            crate::fixed_step::MAX_GAMEPLAY_CATCH_UP_STEPS.saturating_sub(1),
                        )
                {
                    // A one-shot can first arrive in a zero-step frame, when
                    // its eventual logical sequence is still the current
                    // tail, then be assigned to the final step of the next
                    // catch-up batch. Preserve the immutable drawn-view data
                    // and retarget only its structural sequence binding.
                    let mut retargeted = prepared;
                    retargeted.receipt.input_sequence = input_sequence;
                    self.prepared_combat_view_receipt = Some(retargeted);
                    return retargeted.receipt;
                }
                if input_sequence_precedes(input_sequence, prepared.receipt.input_sequence) {
                    // A render-boundary edge may be reserved for the final
                    // step of a bounded catch-up batch. Earlier movement-only
                    // steps still need truthful receipts, but must not replace
                    // the immutable future action receipt.
                    return self.combat_view_receipt_for_input(input_sequence);
                }
            }
            self.prepared_combat_view_receipt = None;
        }
        let receipt = self.combat_view_receipt_for_input(input_sequence);
        self.prepared_combat_view_receipt = Some(PreparedCombatViewReceipt {
            authority_segment,
            receipt,
        });
        receipt
    }

    pub(in crate::game) fn take_combat_view_receipt_for_input(
        &mut self,
        input_sequence: u16,
    ) -> crate::combat_timeline::CombatViewReceiptV1 {
        let authority_segment = self.client_authority.view().segment;
        if let Some(prepared) = self.prepared_combat_view_receipt {
            if prepared.authority_segment != authority_segment {
                self.prepared_combat_view_receipt = None;
                return crate::combat_timeline::CombatViewReceiptV1::unavailable(input_sequence);
            }
            if prepared.authority_segment == authority_segment {
                if prepared.receipt.input_sequence == input_sequence {
                    self.prepared_combat_view_receipt = None;
                    return prepared.receipt;
                }
                if input_sequence_precedes(input_sequence, prepared.receipt.input_sequence) {
                    return self.combat_view_receipt_for_input(input_sequence);
                }
            }
            self.prepared_combat_view_receipt = None;
        }
        self.combat_view_receipt_for_input(input_sequence)
    }

    #[cfg(target_arch = "wasm32")]
    fn queue_voice_playback(&mut self, voice: &PlayerVoice) {
        if !voice.has_wav_header() {
            return;
        }

        let wav_data = voice.wav_data.clone();
        self.voice_loads.push(start_coroutine(async move {
            load_sound_from_bytes(&wav_data).await.ok()
        }));
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn update_voice_audio(&mut self, dt: f32) {
        let mut dynamic_index = 0;
        while dynamic_index < self.dynamic_lvz_sound_loads.len() {
            match self.dynamic_lvz_sound_loads[dynamic_index].load.retrieve() {
                Some(Some(sound)) => {
                    let loaded = self.dynamic_lvz_sound_loads.swap_remove(dynamic_index);
                    if loaded.generation == self.dynamic_lvz_chat_sound_generation
                        && self
                            .dynamic_lvz_chat_sound_sources
                            .contains_key(&loaded.sound_id)
                    {
                        self.lvz_chat_sounds.insert(loaded.sound_id, sound);
                        self.dynamic_lvz_chat_sound_loaded_ids
                            .insert(loaded.sound_id);
                    }
                }
                Some(None) => {
                    self.dynamic_lvz_sound_loads.swap_remove(dynamic_index);
                }
                None => dynamic_index += 1,
            }
        }
        let mut index = 0;
        while index < self.voice_loads.len() {
            match self.voice_loads[index].retrieve() {
                Some(Some(sound)) => {
                    play_sound(
                        &sound,
                        PlaySoundParams {
                            looped: false,
                            volume: self.voice_volume(0.55),
                        },
                    );
                    self.active_voice_sounds.push(VoicePlayback {
                        sound,
                        ttl: NETWORK_VOICE_SOUND_TTL,
                    });
                    self.voice_loads.swap_remove(index);
                }
                Some(None) => {
                    self.voice_loads.swap_remove(index);
                }
                None => {
                    index += 1;
                }
            }
        }

        for playback in &mut self.active_voice_sounds {
            let _keep_alive = &playback.sound;
            playback.ttl -= dt;
        }
        self.active_voice_sounds
            .retain(|playback| playback.ttl > 0.0);
    }
}

fn max_energy_after_settings_update(
    current_max_energy: f32,
    tuning: ShipTuning,
    first_network_settings: bool,
) -> f32 {
    let initial_energy = tuning.energy_initial.min(tuning.energy_max);
    if first_network_settings {
        initial_energy
    } else {
        current_max_energy.max(initial_energy)
    }
}

/// The bootstrap clock can predate a later map security challenge, especially
/// while a headed/throttled browser is still loading assets. Door state is
/// anchored by that challenge, so an older cached tick must never rewind it.
/// A genuinely newer observed snapshot/time-sync tick still catches the seed
/// up to receipt time. Continuum tick comparison keeps the choice wrap-safe.
fn security_door_sync_tick(observed_tick: Option<u32>, challenge_tick: u32) -> u32 {
    observed_tick
        .filter(|&observed| crate::continuum::continuum_tick_diff(observed, challenge_tick) >= 0)
        .unwrap_or(challenge_tick)
}

/// Everything the arena must present before the client will simulate or send
/// gameplay in it. The zone term covers both halves of the file transfer: the
/// map (`ZoneMapPublished`) and every authored LVZ archive
/// (`ZoneFilesPublished`).
///
/// The files term is not decoration. A retained Chrome/Chaos capture of the
/// ordinary client URL showed one 158 ms frame gap, deterministically, about
/// 1.1 s after the first accepted input, attributed by CPU profile to
/// `LvzRenderCache::merge_archive` -> `rebuild_resolved_images` (113.7 ms of a
/// 158 ms frame) while the arena's 28,922 map objects streamed in. Merging
/// happens on the network/render path, not inside the logical step, so holding
/// only the step would have left the hitch in place with a ship that could not
/// move. Requiring files here closes the step, the baseline submission and the
/// outbound gameplay action together, which is what keeps the authored arena
/// whole: the player waits ~1.2 s longer at boot rather than flying through a
/// terrain stall. Progressive (per-frame budgeted) merging was the rejected
/// alternative because it would make the arena resolve on screen in pieces.
pub(in crate::game) fn arena_gameplay_ready(
    joined_arena: bool,
    security_synchronized: bool,
    door_clock_fresh: bool,
    zone_map_ready: bool,
    zone_files_ready: bool,
) -> bool {
    joined_arena && security_synchronized && door_clock_fresh && zone_map_ready && zone_files_ready
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OriginalSpecialChatSound {
    MusicLoop,
    MusicStop,
    MusicOnce,
    Goal,
}

fn original_special_chat_sound(sound: u8) -> Option<OriginalSpecialChatSound> {
    match sound {
        100 => Some(OriginalSpecialChatSound::MusicLoop),
        101 => Some(OriginalSpecialChatSound::MusicStop),
        102 => Some(OriginalSpecialChatSound::MusicOnce),
        104 => Some(OriginalSpecialChatSound::Goal),
        _ => None,
    }
}

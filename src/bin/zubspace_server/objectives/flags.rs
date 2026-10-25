use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::super::reliable::broadcast_reliable_to_joined;
use super::super::security::log_security_reject;
use super::super::state::ServerState;
use super::super::wire::current_tick;
use super::super::zone::ZoneFlag;
use super::packets::{flag_claim_packet, flag_position_packet};
use super::profile::ServerFlagProfile;
use super::{FLAG_REQUEST, FlagState};
use super::{legacy_flag_claim_packets, legacy_flag_drop_packets};

pub(crate) fn relay_flag_request(state: &Arc<Mutex<ServerState>>, player_id: u16, request: &[u8]) {
    if request.len() != 3 || request[0] != FLAG_REQUEST {
        log_security_reject(player_id, "flag", "malformed_claim");
        return;
    }
    let flag_id = u16::from_le_bytes(request[1..3].try_into().expect("length checked"));
    if flag_id == u16::MAX {
        log_security_reject(player_id, "flag", "invalid_claim_id");
        return;
    }
    // The claim is evaluated against the server's current registry and the
    // claimant's authoritative hull, not against anything the packet asserts.
    let tick = current_tick();
    let packets = {
        let mut locked = state.lock().expect("server state should not be poisoned");
        match legacy_flag_claim_packets(&mut locked, player_id, flag_id, tick) {
            Ok(packets) => packets,
            Err(reason) => {
                log_security_reject(player_id, "flag", reason);
                return;
            }
        }
    };
    tracing::info!(
        player_id,
        flag_id,
        packets = packets.len(),
        "objective flag claim granted"
    );
    for packet in packets {
        broadcast_reliable_to_joined(state, packet);
    }
}

pub(crate) fn relay_drop_flags(state: &Arc<Mutex<ServerState>>, player_id: u16, request: &[u8]) {
    if request != [super::DROP_FLAGS_REQUEST] {
        log_security_reject(player_id, "flag", "malformed_drop");
        return;
    }
    // The request can only ask the server to release what this player carries.
    // Where each flag lands — by the carrier, neutral or owned, at the authored
    // centre — is the zone profile's requested-drop policy.
    let tick = current_tick();
    let packets = {
        let mut locked = state.lock().expect("server state should not be poisoned");
        match legacy_flag_drop_packets(&mut locked, player_id, tick) {
            Ok(packets) => packets,
            Err(reason) => {
                log_security_reject(player_id, "flag", reason);
                return;
            }
        }
    };
    for packet in packets {
        broadcast_reliable_to_joined(state, packet);
    }
}

#[cfg(test)]
pub(crate) fn current_flag_packets(state: &Arc<Mutex<ServerState>>) -> Vec<Vec<u8>> {
    let state = state.lock().expect("server state should not be poisoned");
    current_flag_packets_for_state(&state)
}

pub(crate) fn current_flag_packets_for_state(state: &ServerState) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    for flag in state.objectives.replay().flags {
        // A `FlagPosition` event replaces the client's whole flag entry with no
        // version check, so every emission here can erase a carrier the
        // authoritative path already installed. Log the registry state each pair
        // was built from: when a client shows an on-map unowned flag while the
        // registry says `Carried`, this line names the sender.
        tracing::debug!(
            flag = flag.identity.id,
            generation = flag.identity.generation,
            version = flag.version.get(),
            state = ?flag.state,
            "legacy flag baseline packet emitted",
        );
        match flag.state {
            FlagState::Waiting { .. } => {}
            FlagState::OnMap { x, y, owner } => packets.push(flag_position_packet(ZoneFlag {
                id: flag.identity.id,
                x,
                y,
                owner: owner.unwrap_or(u16::MAX),
            })),
            FlagState::Carried {
                carrier,
                x,
                y,
                owner,
            } => {
                // A fresh client creates the flag identity from 0x12 before
                // applying 0x13. Retain and replay the last canonical map
                // position before the carried claim so late joiners converge.
                packets.push(flag_position_packet(ZoneFlag {
                    id: flag.identity.id,
                    x,
                    y,
                    owner: owner.unwrap_or(u16::MAX),
                }));
                packets.push(flag_claim_packet(flag.identity.id, carrier.player_id()));
            }
        }
    }
    packets
}

/// The only producer of the near-snapshot carry countdown. `flag_timer` means carry *remaining* in
/// continuum ticks - the quantity the offline path counts down and drops a flag at zero
/// (`src/game/flags.rs`) - and until now nothing reached it: the bytes sit in the client's own
/// `LARGE_POSITION` extension, but the server stores a canonical 23-byte position form that deliberately
/// drops client-provided time and prize state, so every near entity reported 0. The objective registry is
/// the only party that knows a carry deadline, and an actor's client cannot know one, so this map is the
/// whole truth: an actor in it reports its remaining ticks and everyone else reports 0.
///
/// A zone that authors `FlagCarryTimeoutTicks = 0` says a carry never expires
/// (`step_objective_deadlines` continues past a zero timeout), so there is nothing truthful to encode
/// for it and the map stays empty - which is every shipped zone today.
pub(crate) fn authoritative_near_flag_timers(state: &ServerState) -> BTreeMap<u16, u16> {
    let mut timers = BTreeMap::new();
    let ServerFlagProfile::Enabled(profile) = &state.objectives.profile().flags else {
        return timers;
    };
    if profile.carry_timeout_ticks == 0 {
        return timers;
    }
    let tick = current_tick();
    for flag in state.objectives.replay().flags {
        let FlagState::Carried { carrier, .. } = flag.state else {
            continue;
        };
        let Some(started) = state.objectives.flag_carried_since(flag.identity.id) else {
            continue;
        };
        let elapsed = crate::continuum::continuum_tick_diff(tick, started).max(0) as u32;
        timers.insert(
            carrier.session.player_id,
            u16::try_from(profile.carry_timeout_ticks.saturating_sub(elapsed)).unwrap_or(u16::MAX),
        );
    }
    timers
}

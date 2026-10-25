import { serverLogMessage, serverLogRecord } from "./server_log_fields.mjs";
import { logLines } from "./log_chunks.mjs";

export function parseServerDamageEvents(logs) {
  return parseZubspaceEvents(logs)
    .filter(({ event }) => event.type === "server_damage_applied")
    .map(({ event, logIndex }) => ({
      damageSeq: finiteNumber(event.damage_seq, "damage_seq", logIndex),
      projectileId:
        event.projectile_id == null
          ? null
          : finiteNumber(event.projectile_id, "projectile_id", logIndex),
      serverTick: finiteNumber(event.server_tick, "server_tick", logIndex),
      victimId: finiteNumber(event.victim_id, "victim_id", logIndex),
      shooterId: finiteNumber(event.shooter_id, "shooter_id", logIndex),
      victimLifeSeq: finiteNumber(event.victim_life_seq, "victim_life_seq", logIndex),
      shooterLifeSeq: finiteNumber(event.shooter_life_seq, "shooter_life_seq", logIndex),
      weapon: finiteNumber(event.weapon, "weapon", logIndex),
      damage: finiteNumber(event.damage, "damage", logIndex),
      unprotectedDamage: finiteNumber(
        event.unprotected_damage,
        "unprotected_damage",
        logIndex,
      ),
      shieldUntilTick: finiteNumber(event.shield_until_tick, "shield_until_tick", logIndex),
      shieldRemainingTicks: finiteNumber(
        event.shield_remaining_ticks,
        "shield_remaining_ticks",
        logIndex,
      ),
      shieldLifetimeTicks: finiteNumber(
        event.shield_lifetime_ticks,
        "shield_lifetime_ticks",
        logIndex,
      ),
      empUntilTick: finiteNumber(event.emp_until_tick, "emp_until_tick", logIndex),
      energyBefore: finiteNumber(event.energy_before, "energy_before", logIndex),
      energyAfter: finiteNumber(event.energy_after, "energy_after", logIndex),
      lethal: event.lethal === true,
      kind: event.kind || null,
      logIndex,
    }));
}

export function snapshotIdForwardDistance(candidate, previous) {
  if (
    !Number.isInteger(candidate) ||
    candidate <= 0 ||
    candidate > 0xffff ||
    !Number.isInteger(previous) ||
    previous <= 0 ||
    previous > 0xffff
  ) {
    return null;
  }
  const distance = (candidate - previous) & 0xffff;
  return distance < 0x8000 ? distance : null;
}

export function snapshotAckWithinHistory(latest, acked, historyLength = 32) {
  if (!Number.isSafeInteger(historyLength) || historyLength < 0 || historyLength >= 0x8000) {
    throw new Error(`snapshot history length must be an integer in [0, 32767], got ${historyLength}`);
  }
  const lag = snapshotIdForwardDistance(latest, acked);
  return lag !== null && lag <= historyLength;
}

export function parseServerDeathEvents(logs) {
  // Production INFO logs carry the same committed death fields without the
  // optional legacy DEBUG message. Accept both, never count one death twice.
  const candidates = parseZubspaceEvents(logs);
  let offset = 0;
  for (const line of logLines(logs)) {
    const logIndex = offset;
    offset += line.length + 1;
    if (!line.includes('"server player death"')) continue;
    const { fields, message } = serverLogRecord(line);
    if (message !== "server player death" || fields?.damage_seq == null) continue;
    candidates.push({ event: { ...fields, type: "server_player_death" }, logIndex });
  }
  const parsed = candidates
    .filter(({ event }) => event.type === "server_player_death")
    .map(({ event, logIndex }) => ({
      damageSeq: finiteNumber(event.damage_seq, "damage_seq", logIndex),
      projectileId: finiteNumber(event.projectile_id, "projectile_id", logIndex),
      serverTick: finiteNumber(event.server_tick, "server_tick", logIndex),
      killerId: finiteNumber(event.killer_id, "killer_id", logIndex),
      victimId: finiteNumber(event.victim_id, "victim_id", logIndex),
      victimLifeSeq: finiteNumber(event.victim_life_seq, "victim_life_seq", logIndex),
      victimNextLifeSeq: finiteNumber(event.victim_next_life_seq, "victim_next_life_seq", logIndex),
      killerLifeSeq: finiteNumber(event.killer_life_seq, "killer_life_seq", logIndex),
      weapon: finiteNumber(event.weapon, "weapon", logIndex),
      energyBefore: finiteNumber(event.energy_before, "energy_before", logIndex),
      energyAfter: finiteNumber(event.energy_after, "energy_after", logIndex),
      killedBounty: finiteNumber(event.killed_bounty, "killed_bounty", logIndex),
      flagTransfer: finiteNumber(event.flag_transfer, "flag_transfer", logIndex),
      greenId: finiteNumber(event.green_id, "green_id", logIndex),
      greenCollectibleId: finiteNumber(
        event.green_collectible_id,
        "green_collectible_id",
        logIndex,
      ),
      respawnDelayMs: finiteNumber(event.respawn_delay_ms, "respawn_delay_ms", logIndex),
      logIndex,
    }));
  const unique = new Map();
  for (const death of parsed.sort((a, b) => a.logIndex - b.logIndex)) {
    const { logIndex: _, ...fields } = death;
    const key = JSON.stringify([death.damageSeq, death.projectileId, death.serverTick,
      death.killerId, death.victimId, death.victimLifeSeq, death.killerLifeSeq]);
    const previous = unique.get(key);
    if (previous) {
      const { logIndex: _previousOffset, ...previousFields } = previous;
      if (JSON.stringify(previousFields) !== JSON.stringify(fields)) {
        throw new Error(`conflicting committed death evidence at log offset ${death.logIndex}`);
      }
    } else unique.set(key, death);
  }
  return [...unique.values()];
}

export function parseServerRespawnEvents(logs) {
  return parseZubspaceEvents(logs)
    .filter(({ event }) => event.type === "server_player_respawned")
    .map(({ event, logIndex }) => ({
      serverTick: finiteNumber(event.server_tick, "server_tick", logIndex),
      playerId: finiteNumber(event.player_id, "player_id", logIndex),
      lifeSeq: finiteNumber(event.life_seq, "life_seq", logIndex),
      ship: finiteNumber(event.ship, "ship", logIndex),
      frequency: finiteNumber(event.frequency, "frequency", logIndex),
      x: finiteNumber(event.x, "x", logIndex),
      y: finiteNumber(event.y, "y", logIndex),
      energy: finiteNumber(event.energy, "energy", logIndex),
      maxEnergy: finiteNumber(event.max_energy, "max_energy", logIndex),
      inputSequence: finiteNumber(event.input_sequence, "input_sequence", logIndex),
      inputBits: finiteNumber(event.input_bits, "input_bits", logIndex),
      inputDirection: finiteNumber(event.input_direction, "input_direction", logIndex),
      inputHistoryLen: finiteNumber(event.input_history_len, "input_history_len", logIndex),
      logIndex,
    }));
}

export function parseServerProjectileEvents(logs) {
  return parseZubspaceEvents(logs)
    .filter(({ event }) =>
      event.type === "server_projectile_spawned" || event.type === "server_projectile_hit"
    )
    .map(({ event, logIndex }) => {
      if (event.type === "server_projectile_spawned") {
        return {
          type: event.type,
          projectileId: finiteNumber(event.projectile_id, "projectile_id", logIndex),
          shotGroupId: finiteNumber(event.shot_group_id, "shot_group_id", logIndex),
          ownerId: finiteNumber(event.owner_id, "owner_id", logIndex),
          ownerLifeSeq: finiteNumber(event.owner_life_seq, "owner_life_seq", logIndex),
          serverTick: finiteNumber(event.server_tick, "server_tick", logIndex),
          weapon: finiteNumber(event.weapon, "weapon", logIndex),
          x: finiteNumber(event.x, "x", logIndex),
          y: finiteNumber(event.y, "y", logIndex),
          velocityX: finiteNumber(event.velocity_x, "velocity_x", logIndex),
          velocityY: finiteNumber(event.velocity_y, "velocity_y", logIndex),
          expiryTick: finiteNumber(event.expiry_tick, "expiry_tick", logIndex),
          bouncesRemaining: finiteNumber(event.bounces_remaining, "bounces_remaining", logIndex),
          logIndex,
        };
      }
      return {
        type: event.type,
        projectileId: finiteNumber(event.projectile_id, "projectile_id", logIndex),
        ownerId: finiteNumber(event.owner_id, "owner_id", logIndex),
        victimId: finiteNumber(event.victim_id, "victim_id", logIndex),
        ownerLifeSeq: finiteNumber(event.owner_life_seq, "owner_life_seq", logIndex),
        victimLifeSeq: finiteNumber(event.victim_life_seq, "victim_life_seq", logIndex),
        serverTick: finiteNumber(event.server_tick, "server_tick", logIndex),
        weapon: finiteNumber(event.weapon, "weapon", logIndex),
        x: finiteNumber(event.x, "x", logIndex),
        y: finiteNumber(event.y, "y", logIndex),
        logIndex,
      };
    });
}

export function parseZubspaceEvents(logs) {
  const events = [];
  let offset = 0;
  for (const line of logLines(logs)) {
    const lineStart = offset;
    offset += line.length + 1;
    // Most debug lines are transport traffic. Only decode candidate event
    // envelopes, preserving the text parser's cheap streaming filter.
    if (!line.includes("zubspace_event ")) continue;
    const message = serverLogMessage(line);
    const eventIndex = message.indexOf("zubspace_event ");
    if (eventIndex < 0) continue;
    let event = null;
    try {
      event = JSON.parse(message.slice(eventIndex + "zubspace_event ".length));
    } catch (error) {
      throw new Error(`malformed zubspace_event JSON at log offset ${lineStart}: ${error.message}`);
    }
    if (!event || typeof event !== "object" || typeof event.type !== "string") {
      throw new Error(`malformed zubspace_event payload at log offset ${lineStart}`);
    }
    events.push({ event, logIndex: lineStart });
  }
  return events;
}

function finiteNumber(value, field, logIndex) {
  const number = Number(value);
  if (!Number.isFinite(number)) {
    throw new Error(`zubspace_event ${field} is not finite at log offset ${logIndex}: ${value}`);
  }
  return number;
}

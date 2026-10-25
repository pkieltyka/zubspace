import {
  brakeClientVelocity,
  damageTraceMatches,
  driveClientToWorldTarget,
  waitUntil,
} from "./combat_replay.mjs";
import {
  parseServerDamageEvents,
  parseServerDeathEvents,
  parseZubspaceEvents,
} from "../../tools/server_events.mjs";
import { captureBrowserJsonSnapshot } from "./browser_json_snapshot.mjs";
import { logLines } from "../../tools/log_chunks.mjs";
import { serverLogRecord } from "../../tools/server_log_fields.mjs";
import { approachLiveBotEncounter } from "./bot_encounter_staging.mjs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { continuousDoorReceipt, continuousDoorDisplayJump, reenteredDoorReceipt, offscreenMotionFinding } from "./bot_motion_boundaries.mjs";
import { outsideAoiRespawnEvidence } from "./bot_respawn_aoi.mjs";

const CONTINUUM_TICK_MODULUS = 0x8000_0000;
const BOT_RESPAWN_PRESENTATION_SLACK_TICKS = 150;
const MAX_OVERTAKEN_RESPAWN_LIVES = 4;
// One atomic server transaction can occupy separate unordered RTC event
// messages. Permit its exact presentations across at most adjacent browser
// render frames while still rejecting a delayed/stale-life visual tail.
const ATOMIC_DAMAGE_PRESENTATION_MAX_SPAN_MS = 50;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const serverDamageReaders = new WeakMap();

function serverDamageReader(server) {
  let reader = serverDamageReaders.get(server);
  if (reader != null) return reader;
  reader = {
    chunkIndex: 0,
    totalLength: 0,
    pending: "",
    pendingStart: 0,
    events: [],
  };
  serverDamageReaders.set(server, reader);
  return reader;
}

function refreshServerDamageReader(server) {
  const reader = serverDamageReader(server);
  while (reader.chunkIndex < server.logs.length) {
    const chunk = String(server.logs[reader.chunkIndex++] || "");
    const combinedStart = reader.pending.length > 0
      ? reader.pendingStart
      : reader.totalLength;
    const combined = reader.pending + chunk;
    reader.totalLength += chunk.length;
    const newline = combined.lastIndexOf("\n");
    if (newline < 0) {
      if (reader.pending.length === 0) reader.pendingStart = combinedStart;
      reader.pending = combined;
      continue;
    }
    const complete = combined.slice(0, newline + 1);
    reader.events.push(...parseServerDamageEvents(complete).map((event) => ({
      ...event,
      logIndex: event.logIndex + combinedStart,
    })));
    reader.pending = combined.slice(newline + 1);
    reader.pendingStart = combinedStart + newline + 1;
  }
  return reader;
}

export function serverDamageBoundary(server) {
  return refreshServerDamageReader(server).totalLength;
}

export function serverDamageEventsSince(server, boundary = 0) {
  return refreshServerDamageReader(server).events.filter((event) =>
    event.logIndex >= boundary
  );
}

export function exactLifePrimaryBombTerminal(events, playerId, target) {
  const terminal = events.find(event => event.lethal === true && event.energyAfter === 0 &&
    event.victimId === target.victimId && event.victimLifeSeq === target.victimLifeSeq);
  if (!terminal) return { kind: "pending", terminal: null };
  return { kind: terminal.shooterId === playerId && [3,4].includes(terminal.weapon & 31)
    ? "primary-bomb" : "retired", terminal };
}

export async function renewRetiredBombTarget({ actor, server, playerId, target,
  damageBoundary, label, timeoutMs = 10_000, brake = brakeClientVelocity }) {
  const prior = exactLifePrimaryBombTerminal(serverDamageEventsSince(server, 0), playerId, target);
  if (prior.kind === "pending" || (prior.kind === "primary-bomb" &&
      prior.terminal.logIndex >= damageBoundary)) return null;
  // A fragment can own this life while the real-key pilot is retreating.
  // Polling alone lets that pilot coast out of the successor's AOI. Stop via
  // the normal keyboard controller before waiting; never accept an unseen
  // life or turn the fragment into the required primary-bomb terminal.
  const deadline = Date.now() + timeoutMs;
  // A fast Chaos ship needs over two seconds of thrust alone, plus turning
  // and WebDriver samples. Keep the total successor deadline unchanged.
  const coastBrake = await brake(actor, Math.min(8_000, timeoutMs));
  if (coastBrake?.braked !== true) {
    throw new Error(`${label} could not brake the coasting pilot before successor staging: ${JSON.stringify(coastBrake)}`);
  }
  const remainingMs = deadline - Date.now();
  if (remainingMs <= 0) {
    throw new Error(`${label} exhausted its successor deadline before it binds a presented live successor: ${JSON.stringify(coastBrake)}`);
  }
  let replacement = null;
  await waitUntil(`${label} binds a presented live successor before the next bomb attempt`, async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    const live = presentedLiveBotSuccessor(metrics, target.victimId, target.victimLifeSeq);
    if (!live || !Number.isInteger(metrics.selfWins)) return false;
    const next = { victimId: live.id, victimLifeSeq: live.lifeSeq };
    if (exactLifePrimaryBombTerminal(serverDamageEventsSince(server, 0), playerId, next).kind !== "pending") return false;
    replacement = { target: next, damageBoundary: serverDamageBoundary(server),
      presentationBefore: metrics, winsBefore: metrics.selfWins,
      previousTarget: { ...target }, retirement: prior.terminal, coastBrake };
    return true;
  }, remainingMs);
  return replacement;
}

export function assertHumanProjectilePresentationGeometry(server, events, label) {
  const collisionWitnesses = parseZubspaceEvents(server.logs)
    .map(({ event }) => event)
    .filter((event) => event.type === "server_collision_witness");
  const evidence = events.map((damage) => {
    const matches = collisionWitnesses.filter((witness) =>
      witness.projectile_id === damage.projectileId &&
      witness.owner_id === damage.shooterId &&
      witness.owner_life_seq === damage.shooterLifeSeq &&
      witness.target_id === damage.victimId &&
      witness.target_life_seq === damage.victimLifeSeq &&
      witness.adjusted_damage === damage.damage
    );
    const witness = matches[0];
    const geometry = witness?.presentation_geometry;
    const resolved = typeof geometry?.status === "string" &&
      geometry.status.startsWith("resolved_");
    if (
      matches.length !== 1 ||
      witness.history_source !== "observer_presentation" ||
      !resolved ||
      !Number.isInteger(geometry.accepted_timeline_generation) ||
      geometry.accepted_timeline_generation <= 0 ||
      geometry.effective_timeline_generation !== geometry.accepted_timeline_generation ||
      witness.requested_target_tick !== geometry.render_tick ||
      witness.selected_target_tick !== geometry.render_tick ||
      witness.collision_anchor_x !== geometry.target_x ||
      witness.collision_anchor_y !== geometry.target_y ||
      !Number.isFinite(geometry.authority_error) ||
      geometry.authority_error > 12
    ) {
      throw new Error(`${label} did not retain one verified observer-presentation geometry: ${JSON.stringify({
        damage,
        matches,
      })}`);
    }
    return {
      damageSeq: damage.damageSeq,
      projectileId: damage.projectileId,
      victimId: damage.victimId,
      victimLifeSeq: damage.victimLifeSeq,
      historySource: witness.history_source,
      contactReason: witness.contact_reason,
      renderTick: geometry.render_tick,
      selectedTargetTick: witness.selected_target_tick,
      timelineGeneration: geometry.accepted_timeline_generation,
      authorityError: geometry.authority_error,
    };
  });
  if (evidence.length === 0) {
    throw new Error(`${label} had no human-projectile bot damage to prove`);
  }
  return evidence;
}

function exactServerDamageEventMatches(left, right) {
  return left.damageSeq === right.damageSeq &&
    left.projectileId === right.projectileId &&
    left.serverTick === right.serverTick &&
    left.victimId === right.victimId &&
    left.shooterId === right.shooterId &&
    left.victimLifeSeq === right.victimLifeSeq &&
    left.shooterLifeSeq === right.shooterLifeSeq &&
    left.weapon === right.weapon &&
    left.damage === right.damage &&
    left.energyBefore === right.energyBefore &&
    left.energyAfter === right.energyAfter &&
    left.lethal === right.lethal;
}

export function snapshotFreeDamageCommitEvidence(logs, cycleHits) {
  if (!Array.isArray(cycleHits) || cycleHits.length < 2) return null;
  const ordered = [...cycleHits].sort((left, right) => left.logIndex - right.logIndex);
  if (
    ordered.some((event) => !Number.isSafeInteger(event.logIndex) || event.logIndex < 0) ||
    ordered.some((event, index) => index > 0 && event.logIndex <= ordered[index - 1].logIndex)
  ) return null;

  const text = Array.isArray(logs) ? logs.join("") : String(logs || "");
  const firstLogIndex = ordered[0].logIndex;
  const lastLineEnd = text.indexOf("\n", ordered.at(-1).logIndex);
  if (firstLogIndex >= text.length || lastLineEnd < 0) return null;
  const precedingSnapshotLogIndex = text.lastIndexOf(
    "server bot snapshot trace",
    firstLogIndex,
  );
  // Snapshot tracing is process-scoped and immutable in these combat runs.
  // A prior emitted-snapshot witness makes absence inside the commit span
  // meaningful; without it this check must fail closed rather than infer that
  // snapshot tracing was enabled.
  if (precedingSnapshotLogIndex < 0) return null;
  const span = text.slice(firstLogIndex, lastLineEnd + 1);
  const parsedByLogIndex = new Map(parseServerDamageEvents(span).map((event) => [
    event.logIndex + firstLogIndex,
    event,
  ]));
  if (ordered.some((event) => {
    const parsed = parsedByLogIndex.get(event.logIndex);
    return parsed == null || !exactServerDamageEventMatches(parsed, event);
  })) return null;

  // Every debug combat replay traces each sent snapshot. Damage events with
  // no snapshot trace between their exact structured log records were
  // committed in one server-tick application batch: emit_snapshots_for_tick
  // runs only after that complete FIFO batch. Unlike collision tick distance,
  // this proves whether an intermediate remote-energy frame could exist.
  const snapshotBoundaryCount = (span.match(/server bot snapshot trace/g) || []).length;
  if (snapshotBoundaryCount !== 0) return null;
  return {
    firstLogIndex,
    lastLogIndex: ordered.at(-1).logIndex,
    precedingSnapshotLogIndex,
    spanBytes: span.length,
    damageSeqs: ordered.map((event) => event.damageSeq),
    projectileIds: ordered.map((event) => event.projectileId),
    snapshotBoundaryCount,
  };
}

function traceField(line, name) {
  const match = line.match(new RegExp(`(?:^|\\s)${name}=([^\\s]+)`));
  return match?.[1] ?? null;
}

export function stationaryBotMotionEvidence(logs, {
  botId,
  requiredLifeSeqs,
  requiredPassiveFireLifeSeqs = [],
  minimumSamplesPerLife = 2,
  maximumSpeed = 0.01,
  maximumDisplacement = 0.01,
}) {
  if (!Number.isInteger(botId) || botId < 0) {
    throw new Error(`stationary bot evidence requires a nonnegative bot id, got ${botId}`);
  }
  if (!Array.isArray(requiredLifeSeqs) || requiredLifeSeqs.length === 0 ||
      requiredLifeSeqs.some((lifeSeq) => !Number.isInteger(lifeSeq) || lifeSeq <= 0)) {
    throw new Error("stationary bot evidence requires at least one nonzero integer life");
  }
  if (!Array.isArray(requiredPassiveFireLifeSeqs) ||
      requiredPassiveFireLifeSeqs.some((lifeSeq) =>
        !Number.isInteger(lifeSeq) || lifeSeq <= 0 || !requiredLifeSeqs.includes(lifeSeq)
      )) {
    throw new Error("passive-fire evidence must name stationary required lives");
  }
  const text = Array.isArray(logs) ? logs.join("") : String(logs || "");
  const lines = text.split(/\r?\n/);
  const activations = lines.filter((line) =>
    line.includes("server bot stationary E2E geometry fixture active") &&
    Number(traceField(line, "bot_id")) === botId
  ).map((line) => ({
    serverTick: Number(traceField(line, "server_tick")),
    botId: Number(traceField(line, "bot_id")),
    lifeSeq: Number(traceField(line, "life_seq")),
    x: Number(traceField(line, "x")),
    y: Number(traceField(line, "y")),
  }));
  const samples = lines.filter((line) =>
    line.includes("server bot movement trace") &&
    traceField(line, "e2e_stationary_motion") === "true" &&
    Number(traceField(line, "bot_id")) === botId
  ).map((line) => ({
    serverTick: Number(traceField(line, "server_tick")),
    botId: Number(traceField(line, "bot_id")),
    lifeSeq: Number(traceField(line, "life_seq")),
    x: Number(traceField(line, "x")),
    y: Number(traceField(line, "y")),
    velocityX: Number(traceField(line, "velocity_x")),
    velocityY: Number(traceField(line, "velocity_y")),
    speed: Number(traceField(line, "speed")),
    inSafeZone: traceField(line, "bot_in_safe_zone") === "true",
    passiveFire: traceField(line, "e2e_passive_fire") === "true",
  })).filter((sample) => Object.entries(sample).every(([name, value]) =>
    name === "inSafeZone" || name === "passiveFire" || Number.isFinite(value)
  ));

  const lives = [...new Set(requiredLifeSeqs)].map((lifeSeq) => {
    const lifeSamples = samples.filter((sample) => sample.lifeSeq === lifeSeq);
    const activation = activations.find((event) => event.lifeSeq === lifeSeq) || null;
    if (activation == null || lifeSamples.length < minimumSamplesPerLife) {
      throw new Error(`stationary bot life ${botId}/${lifeSeq} lacks activation or samples: ${JSON.stringify({
        activation,
        sampleCount: lifeSamples.length,
        minimumSamplesPerLife,
      })}`);
    }
    const origin = lifeSamples[0];
    const observedMaximumSpeed = Math.max(...lifeSamples.map((sample) => sample.speed));
    const observedMaximumDisplacement = Math.max(...lifeSamples.map((sample) =>
      Math.hypot(sample.x - origin.x, sample.y - origin.y)
    ));
    const passiveFireRequired = requiredPassiveFireLifeSeqs.includes(lifeSeq);
    if (
      lifeSamples.some((sample) => sample.inSafeZone) ||
      observedMaximumSpeed > maximumSpeed ||
      observedMaximumDisplacement > maximumDisplacement ||
      (passiveFireRequired && lifeSamples.some((sample) => !sample.passiveFire))
    ) {
      throw new Error(`stationary bot life ${botId}/${lifeSeq} violated exact motion/passive evidence: ${JSON.stringify({
        observedMaximumSpeed,
        observedMaximumDisplacement,
        maximumSpeed,
        maximumDisplacement,
        safeSamples: lifeSamples.filter((sample) => sample.inSafeZone).length,
        passiveFireRequired,
        activeFireSamples: lifeSamples.filter((sample) => !sample.passiveFire).length,
      })}`);
    }
    return {
      botId,
      lifeSeq,
      activation,
      sampleCount: lifeSamples.length,
      firstServerTick: lifeSamples[0].serverTick,
      lastServerTick: lifeSamples.at(-1).serverTick,
      x: origin.x,
      y: origin.y,
      observedMaximumSpeed,
      observedMaximumDisplacement,
      passiveFireRequired,
      passiveFireSamples: lifeSamples.filter((sample) => sample.passiveFire).length,
    };
  });
  return { botId, lives };
}

export function stationaryFixtureLifeSeqs(candidateLifeSeqs, fromLifeSeq, staticLifeSeqs = null) {
  if (
    !Array.isArray(candidateLifeSeqs) ||
    !Number.isInteger(fromLifeSeq) ||
    fromLifeSeq <= 0
  ) {
    throw new Error("stationary fixture life selection requires an activation life");
  }
  if (staticLifeSeqs !== null && (!Array.isArray(staticLifeSeqs) ||
    staticLifeSeqs.some((lifeSeq) => !Number.isInteger(lifeSeq) || lifeSeq <= 0))) {
    throw new Error("finite stationary fixture requires explicit positive lives");
  }
  return [...new Set(candidateLifeSeqs)].filter((lifeSeq) =>
    Number.isInteger(lifeSeq) &&
    lifeSeq > 0 &&
    (staticLifeSeqs === null
      ? u32IsEqualOrStrictlyNewer(lifeSeq, fromLifeSeq)
      : staticLifeSeqs.includes(lifeSeq))
  );
}

// The combat audit must prove every configured static life, including an
// intervening life retired by bullets. A later moving successor is not a
// static fixture merely because its life number is greater than the first.
export function requiredStationaryFixtureLifeSeqs(candidateLifeSeqs, fromLifeSeq, staticLifeSeqs = null) {
  return stationaryFixtureLifeSeqs(
    [...(staticLifeSeqs || []), ...candidateLifeSeqs], fromLifeSeq, staticLifeSeqs,
  );
}

export function movingHullPathIsClear({
  x,
  y,
  velocityX,
  velocityY,
  seconds,
  hullIsOpen,
  maximumStepPixels = 4,
}) {
  if (
    !Number.isFinite(x) || !Number.isFinite(y) ||
    !Number.isFinite(velocityX) || !Number.isFinite(velocityY) ||
    !Number.isFinite(seconds) || seconds < 0 ||
    typeof hullIsOpen !== "function" ||
    !Number.isFinite(maximumStepPixels) || maximumStepPixels <= 0
  ) return false;

  const travel = Math.hypot(velocityX, velocityY) * seconds;
  const samples = Math.max(1, Math.ceil(travel / maximumStepPixels));
  for (let index = 0; index <= samples; index += 1) {
    const elapsed = seconds * index / samples;
    if (!hullIsOpen(
      x + velocityX * elapsed,
      y + velocityY * elapsed,
    )) return false;
  }
  return true;
}

export function protectedAimExitPathIsClear({
  origin,
  lead,
  hullClearsSolid,
  positionIsOnSafeTile,
  exitDistance = 80,
  maximumStepPixels = 4,
}) {
  const length = Math.hypot(lead?.x ?? 0, lead?.y ?? 0);
  if (
    !Number.isFinite(origin?.x) || !Number.isFinite(origin?.y) ||
    !Number.isFinite(length) || length <= 0.01 ||
    typeof hullClearsSolid !== "function" ||
    typeof positionIsOnSafeTile !== "function" ||
    !Number.isFinite(exitDistance) || exitDistance <= 0 ||
    !Number.isFinite(maximumStepPixels) || maximumStepPixels <= 0
  ) return false;

  const unitX = lead.x / length;
  const unitY = lead.y / length;
  const samples = Math.ceil(exitDistance / maximumStepPixels);
  for (let index = 0; index <= samples; index += 1) {
    const distance = Math.min(exitDistance, index * maximumStepPixels);
    if (!hullClearsSolid(
      origin.x + unitX * distance,
      origin.y + unitY * distance,
    )) return false;
  }
  return !positionIsOnSafeTile(
    origin.x + unitX * exitDistance,
    origin.y + unitY * exitDistance,
  );
}

export function chaosBombStationaryLaneDisposition({
  arena,
  weaponMode,
  targetInFireRange,
  lineIsClear,
  distance,
  pilotSpeed,
  holdingRange,
  stoppedSpeed = 3.5,
  targetOwnsPursuit = true,
}) {
  const shouldHold = arena === "chaos-zone" &&
    weaponMode === "bomb" &&
    targetInFireRange !== true &&
    lineIsClear === true &&
    Number.isFinite(distance) &&
    Number.isFinite(holdingRange) &&
    distance <= holdingRange &&
    targetOwnsPursuit === true &&
    // Chaos' immediate proximity lane admits 68..80px separately. Within the
    // existing firing range, preserve motion samples while the bot closes.
    // Outside it, waiting can leave the pilot indefinitely outside any legal
    // shot; ordinary keyboard pursuit must remain possible there.
    distance > 80;
  if (!shouldHold) return { hold: false, action: "pursue" };
  if (!Number.isFinite(pilotSpeed) || pilotSpeed > stoppedSpeed) {
    return { hold: true, action: "brake" };
  }
  // Preserve the authoritative target-motion samples while already stopped.
  // Resetting them on every poll makes targetMotionReady permanently false
  // and therefore makes an otherwise-valid bomb lane impossible to admit.
  return { hold: true, action: "wait" };
}

export function stationaryChaosBombHoldingPoint({
  fixtureActive,
  arena,
  weaponMode,
  lineIsClear,
  humanDamageObserved,
  targetMotionReady,
  targetSpeed,
  playerX,
  playerY,
  targetX,
  targetY,
  desiredDistance = 76,
  hullIsOpen,
  firingLineIsClear,
}) {
  if (
    fixtureActive !== true ||
    arena !== "chaos-zone" ||
    weaponMode !== "bomb" ||
    lineIsClear !== true ||
    humanDamageObserved !== true ||
    targetMotionReady !== true ||
    !Number.isFinite(targetSpeed) ||
    targetSpeed > 3.5 ||
    ![playerX, playerY, targetX, targetY, desiredDistance].every(Number.isFinite) ||
    desiredDistance <= 0
  ) {
    return null;
  }
  const dx = targetX - playerX;
  const dy = targetY - playerY;
  const distance = Math.hypot(dx, dy);
  if (distance <= 0.01 || (distance >= desiredDistance - 4 && distance <= 80)) {
    return null;
  }
  if (typeof hullIsOpen !== "function" || typeof firingLineIsClear !== "function") {
    throw new Error("stationary bomb holding-point setup requires authored hull and firing geometry");
  }
  const radial = {
    x: targetX - dx / distance * desiredDistance,
    y: targetY - dy / distance * desiredDistance,
  };
  const angle = Math.atan2(radial.y - targetY, radial.x - targetX);
  // Keep the same76px annulus and3px driver tolerance. A clear centre ray
  // does not establish ship clearance when retreating beside a wall. Search
  // the nearest clear arc instead of repeatedly driving an impossible point.
  for (let step = 0; step < 40; step++) {
    const offset = step === 0 ? 0 : Math.ceil(step / 2) * (step % 2 ? 1 : -1) * Math.PI / 20;
    const point = step === 0 ? radial : {
      x: targetX + Math.cos(angle + offset) * desiredDistance,
      y: targetY + Math.sin(angle + offset) * desiredDistance,
    };
    if (![-3, 0, 3].every(x => [-3, 0, 3].every(y => hullIsOpen(point.x + x, point.y + y) === true)) ||
        firingLineIsClear(point.x, point.y, targetX, targetY) !== true ||
        !movingHullPathIsClear({ x: playerX, y: playerY,
          velocityX: point.x - playerX, velocityY: point.y - playerY, seconds: 1, hullIsOpen })) continue;
    return point;
  }
  return null;
}

// The authored swept-contact geometry, mirroring `CONTINUUM_WEAPON_CONTACT_HALF_EXTENT`
// and the ship hull radius in `src/continuum_weapon.rs`.
export const CONTINUUM_EXPLOSIVE_CONTACT_HALF_EXTENT = 4;
export const CONTINUUM_SHIP_HULL_RADIUS = 14;

export function explosiveContactOverlapsHull({
  weaponX,
  weaponY,
  shipX,
  shipY,
  shipRadius = CONTINUUM_SHIP_HULL_RADIUS,
  weaponHalfExtent = CONTINUUM_EXPLOSIVE_CONTACT_HALF_EXTENT,
}) {
  if (
    !Number.isFinite(weaponX) || !Number.isFinite(weaponY) ||
    !Number.isFinite(shipX) || !Number.isFinite(shipY) ||
    !Number.isFinite(shipRadius) || shipRadius < 0 ||
    !Number.isFinite(weaponHalfExtent) || weaponHalfExtent < 0
  ) return false;
  const truncatedWeaponX = Math.trunc(weaponX);
  const truncatedWeaponY = Math.trunc(weaponY);
  const truncatedShipX = Math.trunc(shipX);
  const truncatedShipY = Math.trunc(shipY);
  // The reference game passes the ship box first; swapping boxes reverses exact edges.
  return truncatedShipX + shipRadius >= truncatedWeaponX - weaponHalfExtent &&
    truncatedShipX - shipRadius < truncatedWeaponX + weaponHalfExtent &&
    truncatedShipY + shipRadius >= truncatedWeaponY - weaponHalfExtent &&
    truncatedShipY - shipRadius < truncatedWeaponY + weaponHalfExtent;
}

/// Decide whether a sensorless explosive may be fired. Continuum truncates the
/// ship's orientation into one of 40 protocol direction cells, so a visually
/// exact bearing can select the adjacent cell and put the whole swept path
/// outside the authored contact envelope (`explosiveContactOverlapsHull`). A
/// closest approach that cannot fit that envelope is a guaranteed miss, so the
/// pilot must reposition instead of spending the bomb. `quantized` is the best
/// cell the ship can emit - it decides whether the lane is worth holding;
/// `current` is the solve for the cell the ship is emitting right now
/// (`fixedDirection`), and when it is supplied the key press additionally
/// requires it, so a half-bin of turn cadence cannot silently change the shot.
/// Chaos is excluded: its authored bomb owns radial contact with a proximity
/// sensor, so the launched line does not decide contact there.
export function bombWireAimDisposition({ weaponMode, arena, quantized, current }) {
  if (weaponMode !== "bomb" || arena === "chaos-zone") {
    return {
      usesWireAim: false, fire: true, missDistance: null, currentMissDistance: null,
    };
  }
  const fits = (solve) => solve?.feasible === true &&
    Number.isFinite(solve?.missDistance) &&
    solve.missDistance <= CONTINUUM_EXPLOSIVE_CONTACT_HALF_EXTENT + CONTINUUM_SHIP_HULL_RADIUS;
  return {
    usesWireAim: true,
    fire: fits(quantized) && (current === undefined || current === null || fits(current)),
    missDistance: Number.isFinite(quantized?.missDistance) ? quantized.missDistance : null,
    currentMissDistance: Number.isFinite(current?.missDistance) ? current.missDistance : null,
  };
}

// A keyboard pilot can alternate between two continuous headings without ever
// sampling a narrow cell-centre tolerance. For wire-aimed bombs, firing the
// exact solved protocol cell is the requirement; continuous centring adds no
// accuracy once both the emitted cell and its contact prediction are proved.
export function botFiringHeadingReady({ delta, tolerance, wireAim, quantized, current }) {
  if (!Number.isFinite(delta) || !Number.isFinite(tolerance) || tolerance <= 0) return false;
  if (wireAim?.usesWireAim === true) {
    return wireAim.fire === true && Number.isInteger(quantized?.direction) &&
      Number.isInteger(current?.direction) && current.direction === quantized.direction;
  }
  return Math.abs(delta) <= tolerance;
}

export function presentedBotRepelEvidence({ applied, target }) {
  const awayX = applied.target_x - applied.origin_x;
  const awayY = applied.target_y - applied.origin_y;
  const awayLength = Math.hypot(awayX, awayY);
  const displacementX = target.positionX - applied.target_x;
  const displacementY = target.positionY - applied.target_y;
  const outwardDisplacement = awayLength > 0
    ? (displacementX * awayX + displacementY * awayY) / awayLength
    : 0;
  const presentedSpeed = Math.hypot(target.velocityX, target.velocityY);
  const presentedAlignment = awayLength > 0 && presentedSpeed > 0
    ? (awayX * target.velocityX + awayY * target.velocityY) /
      (awayLength * presentedSpeed)
    : -1;
  // Open-space delivery is visible as sustained displacement. If a Repel
  // points a bot into nearby authored geometry, the server collision response
  // can reverse it before 12 pixels; in that case require the browser to have
  // presented the near-exact 500 px/s outward impulse itself. This is a more
  // direct delivery assertion than treating a legitimate wall bounce as loss.
  const delivered = outwardDisplacement >= 12 || (
    outwardDisplacement > 0 &&
    presentedSpeed >= 450 &&
    presentedAlignment >= 0.98
  );
  return {
    delivered,
    outwardDisplacement,
    presentedSpeed,
    presentedAlignment,
  };
}

export function repelPresentationCorridorIsOpen({
  sourceX,
  sourceY,
  targetX,
  targetY,
  hullIsOpen,
  requiredOpenPixels = 48,
}) {
  const distance = Math.hypot(targetX - sourceX, targetY - sourceY);
  if (!(distance > 1) || !Number.isFinite(requiredOpenPixels) || requiredOpenPixels <= 0) {
    return false;
  }
  return movingHullPathIsClear({
    x: targetX,
    y: targetY,
    velocityX: (targetX - sourceX) / distance * 500,
    velocityY: (targetY - sourceY) / distance * 500,
    seconds: requiredOpenPixels / 500,
    hullIsOpen,
  });
}

export async function proveAuthoritativeBotRepel({
  actor,
  server,
  playerId,
  botIds,
  positionIsOnAuthoredSafeTile,
  authoredHullIsOpen,
  label,
}) {
  const repelEvents = () => parseZubspaceEvents(server.logs)
    .map(({ event }) => event)
    .filter((event) => event.type === "server_repel_bot_applied");

  const approachStartedAt = Date.now();
  const approaches = [];

  for (let attempt = 0; attempt < 3; attempt += 1) {
    // The passive feel observation can include a real death. Random respawn
    // need not put the pilot back inside Repel range. Approach through genuine
    // hull-clear flight before the original12sec range/corridor proof; do not
    // make that proof depend on the bot volunteering a timely long journey.
    const remainingApproachMs = 90_000 - (Date.now() - approachStartedAt);
    if (remainingApproachMs <= 0) throw new Error(`${label} exceeded the bounded Repel approach budget`);
    approaches.push(await approachLiveBotEncounter({ actor, server, botIds, label,
      positionIsOnAuthoredSafeTile,
      timeoutMs: remainingApproachMs,
    }));
    let staged = null;
    await waitUntil(`${label} has a live hostile bot inside Repel range`, async () => {
      const state = await actor.metricsState();
      const metrics = state.uiMetrics || {};
      const local = metrics.localGameplayState || {};
      if (
        metrics.playerAlive !== true ||
        metrics.serverPlayerAlive !== true ||
        metrics.inSafeZone === true ||
        !(local.repels > 0) ||
        (local.repelCooldown ?? 0) > 0
      ) return false;
      const target = (metrics.remotePlayers || [])
        .filter((candidate) =>
          botIds.includes(candidate.id) &&
          Number.isInteger(candidate.lifeSeq) &&
          Number.isFinite(candidate.positionX) &&
          Number.isFinite(candidate.positionY) &&
          candidate.energy > 0 &&
          !positionIsOnAuthoredSafeTile(candidate.positionX, candidate.positionY)
        )
        .map((candidate) => ({
          ...candidate,
          distance: Math.hypot(
            candidate.positionX - metrics.playerX,
            candidate.positionY - metrics.playerY,
          ),
        }))
        .filter((candidate) => {
          if (!(candidate.distance > 1 && candidate.distance <= 400)) return false;
          // This browser assertion proves that the authoritative Repel is
          // visibly delivered. Do not stage it into geometry that reverses
          // the impulse on the same server tick before a snapshot can exist.
          // Wall-axis/corner collision remains pinned by the deterministic
          // authority suite; here require 48 authored pixels of open hull in
          // the exact source-to-target direction before pressing real Q.
          return repelPresentationCorridorIsOpen({
            sourceX: metrics.playerX,
            sourceY: metrics.playerY,
            targetX: candidate.positionX,
            targetY: candidate.positionY,
            hullIsOpen: authoredHullIsOpen,
          });
        })
        .sort((left, right) => left.distance - right.distance)[0];
      if (target == null) return false;
      staged = {
        sourceLifeSeq: metrics.playerLifeSeq,
        inventoryBefore: local.repels,
        semanticBefore: metrics.semanticInputCounts?.repel ?? 0,
        target,
      };
      return true;
    }, 12_000);

    const eventBoundary = repelEvents().length;
    // A fixed-duration tap can fit entirely between rendered frames when the
    // debug WASM browser is busy publishing a large zone or processing the
    // deterministic delay profile. Keep the genuine DevTools/WebDriver key
    // down until the client proves that it sampled the press-time semantic
    // edge, then release it. This still exercises the normal browser input
    // path and prevents a missed browser frame from masquerading as a lost
    // server Repel.
    await actor.keyDown("q");
    try {
      await waitUntil(`${label} samples the real Q semantic edge`, async () => {
        const metrics = (await actor.metricsState()).uiMetrics || {};
        return (metrics.semanticInputCounts?.repel ?? 0) ===
          staged.semanticBefore + 1;
      }, 1_000);
    } finally {
      await actor.keyUp("q");
    }
    let applied = null;
    try {
      await waitUntil(`${label} real Q repels the exact hostile bot life`, async () => {
        applied = repelEvents().slice(eventBoundary).find((event) =>
          event.source_player_id === playerId &&
          event.source_life_seq === staged.sourceLifeSeq &&
          event.target_player_id === staged.target.id &&
          event.target_life_seq === staged.target.lifeSeq
        ) || null;
        return applied != null;
      }, 3_000);
    } catch {
      // A bot can die or leave range between the browser's snapshot and the
      // server tick that admits Q. Preserve genuine input semantics and retry
      // only while authored inventory remains.
      continue;
    }

    const speed = Math.hypot(applied.velocity_x, applied.velocity_y);
    const awayX = applied.target_x - applied.origin_x;
    const awayY = applied.target_y - applied.origin_y;
    const awayLength = Math.hypot(awayX, awayY);
    const alignment = awayLength > 0 && speed > 0
      ? (awayX * applied.velocity_x + awayY * applied.velocity_y) / (awayLength * speed)
      : -1;
    if (Math.abs(speed - 500) > 0.01 || alignment < 0.999) {
      throw new Error(`${label} server Repel vector diverged from authored reference-game velocity: ${JSON.stringify({ applied, speed, alignment })}`);
    }

    let presented = null;
    await waitUntil(`${label} browser receives the repelled bot displacement`, async () => {
      const state = await actor.metricsState();
      const metrics = state.uiMetrics || {};
      const local = metrics.localGameplayState || {};
      const target = (metrics.remotePlayers || []).find((candidate) =>
        candidate.id === applied.target_player_id &&
        candidate.lifeSeq === applied.target_life_seq &&
        Number.isFinite(candidate.positionX) &&
        Number.isFinite(candidate.positionY)
      );
      if (target == null) return false;
      const presentation = presentedBotRepelEvidence({ applied, target });
      const inventorySpent = local.repels === staged.inventoryBefore - 1;
      const semanticAdvanced = (metrics.semanticInputCounts?.repel ?? 0) ===
        staged.semanticBefore + 1;
      if (!presentation.delivered || !inventorySpent || !semanticAdvanced) return false;
      presented = {
        targetId: target.id,
        targetLifeSeq: target.lifeSeq,
        positionX: target.positionX,
        positionY: target.positionY,
        ...presentation,
        inventoryAfter: local.repels,
        semanticAfter: metrics.semanticInputCounts?.repel ?? 0,
      };
      return true;
    }, 4_000);

    return {
      control: "q",
      attempt: attempt + 1,
      approaches,
      staged,
      applied,
      speed,
      alignment,
      presented,
    };
  }
  throw new Error(`${label} did not apply a browser-driven hostile bot Repel in three authored attempts`);
}

function signedOrientationDelta(current, desired) {
  return ((desired - current + 1.5) % 1) - 0.5;
}

export function counterfireRouteNeedsBrake(route) {
  // A steering slice expiring is not a failed route. The next slice observes
  // fresh geometry and controls velocity again; stopping after every slice
  // discards that progress. Arrival, stalled progress, and invalid metrics
  // still require a stop before any further navigation decision.
  return route?.reached !== false || route?.reason !== "route timeout";
}

export function routedMapCounterfireWaypoint({
  metrics,
  target,
  authoredHullIsTraversable,
  authoredHullIsOpen,
  authoredLineIsClear,
  authoredTargetAimIsClear = null,
}) {
  // Movement clearance includes safe floor; a firing destination must also
  // satisfy combat eligibility. Never use the latter to test route segments.
  if (typeof authoredHullIsTraversable !== "function") {
    throw new TypeError("counterfire routing requires an explicit traversal predicate");
  }
  const pilotCanReach = (x, y) =>
    authoredLineIsClear(metrics.playerX, metrics.playerY, x, y) &&
    movingHullPathIsClear({
      x: metrics.playerX,
      y: metrics.playerY,
      velocityX: x - metrics.playerX,
      velocityY: y - metrics.playerY,
      seconds: 1,
      hullIsOpen: authoredHullIsTraversable,
    });
  const goalTiles = new Set();
  const directCandidates = [];
  for (const radius of [120, 150, 180]) {
    for (let step = 0; step < 32; step += 1) {
      const angle = step / 32 * Math.PI * 2;
      const x = target.positionX + Math.cos(angle) * radius;
      const y = target.positionY + Math.sin(angle) * radius;
      if (
        !authoredHullIsOpen(x, y) ||
        !authoredLineIsClear(x, y, target.positionX, target.positionY) ||
        (authoredTargetAimIsClear != null &&
          !authoredTargetAimIsClear(target.positionX, target.positionY, x, y))
      ) {
        continue;
      }
      goalTiles.add(Math.floor(y / 16) * 1_024 + Math.floor(x / 16));
      if (pilotCanReach(x, y)) {
        directCandidates.push({
          x,
          y,
          distance: Math.hypot(x - metrics.playerX, y - metrics.playerY),
          routed: false,
          routeTiles: 0,
        });
      }
    }
  }
  directCandidates.sort((left, right) => left.distance - right.distance);
  if (directCandidates.length > 0) return directCandidates[0];
  if (goalTiles.size === 0) return null;

  const startX = Math.floor(metrics.playerX / 16);
  const startY = Math.floor(metrics.playerY / 16);
  const targetTileX = Math.floor(target.positionX / 16);
  const targetTileY = Math.floor(target.positionY / 16);
  const margin = 64;
  const localBounds = {
    minX: Math.max(0, Math.min(startX, targetTileX) - margin),
    maxX: Math.min(1_023, Math.max(startX, targetTileX) + margin),
    minY: Math.max(0, Math.min(startY, targetTileY) - margin),
    maxY: Math.min(1_023, Math.max(startY, targetTileY) + margin),
  };
  const goals = [...goalTiles].map(key => ({ x: key % 1_024, y: Math.floor(key / 1_024) }));
  const goalBounds = { minX: Math.min(...goals.map(p => p.x)), maxX: Math.max(...goals.map(p => p.x)),
    minY: Math.min(...goals.map(p => p.y)), maxY: Math.max(...goals.map(p => p.y)) };
  const search = ({ minX, maxX, minY, maxY }, guided = false) => {
    const startKey = startY * 1_024 + startX;
    const queue = [];
    const predecessor = new Map([[startKey, null]]);
    const costs = new Map([[startKey, 0]]);
    // The local search retains FIFO order. The bounded map-wide fallback uses
    // A* with Manhattan distance to the goals' bounding rectangle, a consistent
    // lower bound even when no firing tile occupies the nearest rectangle cell.
    const push = (key, cost) => {
      const x = key % 1_024, y = Math.floor(key / 1_024);
      const rank = cost + Math.max(goalBounds.minX - x, 0, x - goalBounds.maxX) +
        Math.max(goalBounds.minY - y, 0, y - goalBounds.maxY);
      const item = { key, cost, rank };
      queue.push(item);
      if (!guided) return;
      let index = queue.length - 1;
      while (index > 0) {
        const parent = (index - 1) >>> 1;
        if (queue[parent].rank <= item.rank) break;
        queue[index] = queue[parent]; index = parent;
      }
      queue[index] = item;
    };
    const pop = () => {
      if (!guided) return queue[queueIndex++];
      const first = queue[0], last = queue.pop();
      if (queue.length > 0) {
        let index = 0;
        while (index * 2 + 1 < queue.length) {
          let child = index * 2 + 1;
          if (child + 1 < queue.length && queue[child + 1].rank < queue[child].rank) child++;
          if (last.rank <= queue[child].rank) break;
          queue[index] = queue[child]; index = child;
        }
        queue[index] = last;
      }
      return first;
    };
    let queueIndex = 0;
    push(startKey, 0);
    const directions = [[1, 0], [-1, 0], [0, 1], [0, -1]];
    while ((guided ? queue.length > 0 : queueIndex < queue.length) && predecessor.size <= 65_536) {
      const { key, cost } = pop();
      if (costs.get(key) !== cost) continue;
      if (goalTiles.has(key)) {
        const reversed = [];
        for (let cursor = key; cursor != null; cursor = predecessor.get(cursor)) {
          reversed.push(cursor);
        }
        return reversed.reverse();
      }
      const tileX = key % 1_024;
      const tileY = Math.floor(key / 1_024);
      for (const [dx, dy] of directions) {
        const nextX = tileX + dx;
        const nextY = tileY + dy;
        if (nextX < minX || nextX > maxX || nextY < minY || nextY > maxY) continue;
        const nextKey = nextY * 1_024 + nextX;
        if ((costs.get(nextKey) ?? Infinity) <= cost + 1) continue;
        if (!predecessor.has(nextKey) && predecessor.size >= 65_536) continue;
        const centerX = nextX * 16 + 8;
        const centerY = nextY * 16 + 8;
        if (!authoredHullIsTraversable(centerX, centerY)) continue;
        predecessor.set(nextKey, key);
        costs.set(nextKey, cost + 1);
        push(nextKey, cost + 1);
      }
    }
    return null;
  };
  // A legal random spawn can sit in a pocket whose only exit bends outside
  // the local target box (the retained Chaos7432/9304 bomb pilot did exactly
  // this). Keep the cheap search first, then allow a map-wide detour with the
  // same finite65,536-node ceiling. Neither path can cut a hull-blocked wall;
  // exhaustion still reports no route and never establishes a combat pass.
  const path = search(localBounds) ?? search({ minX: 0, maxX: 1_023, minY: 0, maxY: 1_023 }, true);
  if (path == null) return null;
  // The keyboard driver brakes at its waypoint. A fixed four-tile lookahead
  // repeatedly stops it in a long clear corridor before it can reach the
  // firing lane around a wall. Follow the furthest directly traversable path
  // point instead, checking the whole hull so visibility cannot cut a corner.
  for (let index = path.length - 1; index > 0; index -= 1) {
    const waypointKey = path[index];
    const x = (waypointKey % 1_024) * 16 + 8;
    const y = Math.floor(waypointKey / 1_024) * 16 + 8;
    if (!pilotCanReach(x, y)) continue;
    return { x, y, routed: true, routeTiles: path.length };
  }
  return null;
}

export function authoredFiringRingWaypoint({
  metrics,
  victim,
  radius = 120,
  bins = 16,
  minimumHopPx = 40,
  authoredHullIsTraversable,
  authoredHullIsOpen,
  authoredLineIsClear,
}) {
  // Closes the last firing gap on a ring fixed at the caller's own assertion
  // radius. The counterfire planner searches 120/150/180 px rings around its
  // target and returns the nearest *reachable* candidate, so it can legitimately
  // hand back a point 180 px away from a ship that then has to be inside 160 px
  // of the victim: the two retained lifecycle lanes show both shapes of that
  // residual - `no hull-clear lifecycle aim route` thrown at 197.17 px of
  // separation, and `keyboard lifecycle aim route livelock` at 196.76 px after a
  // 19.03 px micro-hop the driver refuses to drive.
  //
  // This ring keeps the ship on the bearing it already holds: candidates are
  // tried from the current attacker-to-victim bearing outward, so the hop never
  // asks a settled ship to turn around. Movement clearance uses the traversable
  // predicate (safe floor is legal to drive over) while the destination must be
  // combat eligible, and the candidate must hold a clear authored line to the
  // victim - a firing position that cannot see its target is not a firing
  // position.
  if (typeof authoredHullIsTraversable !== "function" || typeof authoredHullIsOpen !== "function") {
    throw new TypeError("firing ring requires explicit traversal and eligibility predicates");
  }
  const baseBearing = Math.atan2(
    metrics.playerY - victim.positionY,
    metrics.playerX - victim.positionX,
  );
  for (let offset = 0; offset < bins; offset += 1) {
    for (const sign of offset === 0 ? [1] : [1, -1]) {
      const angle = baseBearing + sign * (offset / bins) * Math.PI * 2;
      const x = victim.positionX + Math.cos(angle) * radius;
      const y = victim.positionY + Math.sin(angle) * radius;
      if (!authoredHullIsOpen(x, y)) continue;
      if (!authoredLineIsClear(x, y, victim.positionX, victim.positionY)) continue;
      if (!authoredLineIsClear(metrics.playerX, metrics.playerY, x, y)) continue;
      const distance = Math.hypot(x - metrics.playerX, y - metrics.playerY);
      if (distance < minimumHopPx) continue;
      if (!movingHullPathIsClear({
        x: metrics.playerX,
        y: metrics.playerY,
        velocityX: x - metrics.playerX,
        velocityY: y - metrics.playerY,
        seconds: 1,
        hullIsOpen: authoredHullIsTraversable,
      })) continue;
      return { x, y, distance, bearingBins: sign * offset, routed: false };
    }
  }
  return null;
}

/// The world driver stops within roughly 29 px of a point (the retained
/// `no route progress` at 28.98 px), so a routed waypoint that is already inside
/// the caller's arrival circle is not a hop: the recorded Chrome/Chaos livelock
/// drove 36 px corridor hops with `pulses: 0` and crawled 0.05 px per stage for
/// 16 stages. Accepting the driver's `no route progress` at close range keeps the
/// stage meaningful - the ship moved as far as this hop allows - while the caller
/// measures real progress against the distance to the target.
export function routedHopAccepted(route, acceptancePx = 32) {
  if (route?.reached === true) return true;
  const best = Number(route?.bestDistance);
  return route?.reason === "no route progress"
    && Number.isFinite(best)
    && best <= acceptancePx;
}

/// True when a staging stage actually moved the ship. The guard used to measure
/// the attacker-to-victim distance, but a legal route is allowed to raise that
/// distance: the retained Firefox/Chaos mild lane (mild-verify-11.sh.log:32460)
/// was killed at a best of 284.32 px while it worked the planner's remaining path
/// down 35 -> 15 -> 14 tiles around the wall south of a victim that sat 137 px
/// away, covering 15-23 px per stage. Movement is the quantity a stall actually
/// stops, and the recorded corridor crawl is 0.05 px per stage, so the two cases
/// are nowhere near the same threshold.
export function stageAdvanced({ from, after, minimumAdvancePx = 8 }) {
  const advancePx = Math.hypot(
    (after?.playerX ?? 0) - (from?.playerX ?? 0),
    (after?.playerY ?? 0) - (from?.playerY ?? 0),
  );
  return Number.isFinite(advancePx) && advancePx >= minimumAdvancePx;
}

// A route controller that caps its cruise speed has a reach of cap x deadline,
// so a fixed budget silently fails once the start is farther away than that
// reach. The retained Firefox/Chaos lane flew a straight line at its measured
// 41.2 px/s cap, inputBits 0 and no controller fault, closing 1040 px -> 273 px
// monotonically before expiring 273 px short: 42 px/s x 20 s is only 840 px.
// The retained Chrome/Extreme lane had the same shape at the other end: the
// victim settled 1,094 px from the attacker's spawn, one direct waypoint sat
// 914 px away, and the fixed 30 s Extreme budget timed out 290 px short after
// seven pulses because 914 px needs 21.8 s of cruise alone. Keep the cap
// (arrivals stay gentle) and size the deadline from the measured separation.
export function lifecycleRouteBudgetMs(separation, maximumDesiredSpeed) {
  const cruiseMs = (separation / Math.max(maximumDesiredSpeed, 1)) * 1_000;
  return Math.min(90_000, Math.max(20_000, Math.round(cruiseMs * 1.5) + 8_000));
}

export async function stageExtremeSafeEdgeCombat({
  actor,
  botIdBase,
  authoredTiles,
  positionIsOnAuthoredSafeTile,
  authoredTileIsSolid,
  authoredHullIsOpen = null,
  authoredLineIsClear,
  preferredTargetId = null,
  preferredEdge = null,
  crossingFireKey = null,
  crossingFireHoldMs = 120,
  crossingFireReleaseSettleMs = 40,
  crossingProjectileSpeed = null,
  crossingProjectileMaxTime = 0.75,
  crossingApproachDistance = 340,
  crossingMinimumApproachDistance = 0,
  crossingFinalApproachDistance = crossingApproachDistance,
  crossingEdgeSearchTimeout = 10_000,
  crossingMinimumApproachAlignment = 0.9,
  crossingProtectedAimTimeout = 3_000,
  crossingExposedAimTimeout = null,
  crossingMaximumDistance = 340,
  crossingExposedMaximumDistance = null,
  crossingMaximumRecedingSpeed = Number.POSITIVE_INFINITY,
  crossingOrientationTolerance = 0.018,
  crossingMaximumPredictedMissDistance = 6,
  crossingThrustModifierKey = null,
  crossingExposedSettleDistance = 0,
  crossingRequiredExposedRunwayDistance = 0,
  crossingProtectedStagingDistance = 32,
  crossingPreserveProtectedAim = false,
  crossingRevalidateProtectedAim = false,
  crossingReleaseThrustBeforeProtectedFire = false,
  crossingMinimumExitAimAlignment = 0.97,
  crossingPreFireDelaySeconds = 0,
  crossingExpectedExitSpeed = 0,
  crossingExpectedExitSpeedAlongAim = null,
  diagnostics = null,
}) {
  const reject = (phase, details = {}) => {
    diagnostics?.push({ at: Date.now(), phase, ...details });
    return false;
  };
  const authoritativePosition = (player) => ({
    x: player?.positionX,
    y: player?.positionY,
  });
  const fireSemanticMetric = crossingFireKey === "z" ? "bomb" : "primary";
  const initialFireSemanticCount = crossingFireKey == null
    ? null
    : Number((await actor.metricsState()).uiMetrics?.semanticInputCounts?.[fireSemanticMetric]);
  const exposedHoldingPoint = (outside, direction) => ({
    x: outside.x + direction.dx * crossingExposedSettleDistance,
    y: outside.y + direction.dy * crossingExposedSettleDistance,
  });
  const exposedHoldingLaneIsClear = (outside, direction) => {
    const requiredDistance = Math.max(
      crossingExposedSettleDistance,
      crossingRequiredExposedRunwayDistance,
    );
    if (!(requiredDistance > 0)) return true;
    for (let distance = 0; distance <= requiredDistance; distance += 4) {
      const x = outside.x + direction.dx * distance;
      const y = outside.y + direction.dy * distance;
      if (
        positionIsOnAuthoredSafeTile(x, y) ||
        (authoredHullIsOpen != null && !authoredHullIsOpen(x, y))
      ) return false;
    }
    return true;
  };
  const authoredHullClearsSolid = (x, y, radius = 14) => {
    const minTileX = Math.floor((x - radius) / 16);
    const maxTileX = Math.floor((x + radius) / 16);
    const minTileY = Math.floor((y - radius) / 16);
    const maxTileY = Math.floor((y + radius) / 16);
    if (
      minTileX < 0 || minTileY < 0 ||
      maxTileX >= 1_024 || maxTileY >= 1_024
    ) return false;
    for (let tileY = minTileY; tileY <= maxTileY; tileY += 1) {
      for (let tileX = minTileX; tileX <= maxTileX; tileX += 1) {
        if (authoredTileIsSolid(authoredTiles[tileY * 1_024 + tileX])) {
          return false;
        }
      }
    }
    return true;
  };
  const protectedExitPathIsClearFrom = (origin, lead) =>
    protectedAimExitPathIsClear({
      origin,
      lead,
      hullClearsSolid: authoredHullClearsSolid,
      positionIsOnSafeTile: positionIsOnAuthoredSafeTile,
    });
  const initialMetrics = (await actor.metricsState()).uiMetrics || {};
  if (
    initialMetrics.playerAlive !== true ||
    initialMetrics.serverPlayerAlive !== true ||
    initialMetrics.inSafeZone !== true
  ) {
    return reject("initial_lifecycle", {
      playerAlive: initialMetrics.playerAlive,
      serverPlayerAlive: initialMetrics.serverPlayerAlive,
      inSafeZone: initialMetrics.inSafeZone,
    });
  }
  const directions = [
    { name: "west", dx: -1, dy: 0 },
    { name: "east", dx: 1, dy: 0 },
    { name: "north", dx: 0, dy: -1 },
    { name: "south", dx: 0, dy: 1 },
  ];
  const candidateBotsForMetrics = (metrics, requiredLifeSeq = null) =>
    (metrics.remotePlayers || [])
      .filter((player) =>
        player.id >= botIdBase &&
        (preferredTargetId == null || player.id === preferredTargetId) &&
        (requiredLifeSeq == null || player.lifeSeq === requiredLifeSeq) &&
        Number.isFinite(authoritativePosition(player).x) &&
        Number.isFinite(authoritativePosition(player).y) &&
        !positionIsOnAuthoredSafeTile(player.positionX, player.positionY)
      )
      .sort((left, right) =>
        Math.hypot(left.positionX - metrics.playerX, left.positionY - metrics.playerY) -
        Math.hypot(right.positionX - metrics.playerX, right.positionY - metrics.playerY)
      );
  const initialCandidateBots = candidateBotsForMetrics(initialMetrics);
  if (initialCandidateBots.length === 0) return reject("no_candidate_bot");
  const requiredTargetLifeSeq = preferredTargetId == null
    ? null
    : initialCandidateBots[0].lifeSeq;
  const selectEdge = (metrics) => {
    const candidateBots = candidateBotsForMetrics(metrics, requiredTargetLifeSeq);
    if (preferredEdge != null) {
      const bot = candidateBots[0];
      if (bot == null) return null;
      const authoritative = authoritativePosition(bot);
      const botDistance = Math.hypot(
        authoritative.x - preferredEdge.outside.x,
        authoritative.y - preferredEdge.outside.y,
      );
      if (botDistance < crossingMinimumApproachDistance) return null;
      return {
        bot,
        botDistance,
        direction: preferredEdge.direction,
        inside: preferredEdge.inside,
        outside: preferredEdge.outside,
      };
    }
    const candidates = [];
    const pilotTileX = Math.floor(metrics.playerX / 16);
    const pilotTileY = Math.floor(metrics.playerY / 16);
    for (const bot of candidateBots) {
      const authoritative = authoritativePosition(bot);
      for (let tileY = Math.max(0, pilotTileY - 40); tileY <= Math.min(1_023, pilotTileY + 40); tileY += 1) {
        for (let tileX = Math.max(0, pilotTileX - 40); tileX <= Math.min(1_023, pilotTileX + 40); tileX += 1) {
          if (authoredTiles[tileY * 1_024 + tileX] !== 171) continue;
          for (const direction of directions) {
            const firstProtectedTileX = tileX - direction.dx;
            const firstProtectedTileY = tileY - direction.dy;
            const secondProtectedTileX = tileX - direction.dx * 2;
            const secondProtectedTileY = tileY - direction.dy * 2;
            if (
              firstProtectedTileX < 0 || firstProtectedTileX >= 1_024 ||
              firstProtectedTileY < 0 || firstProtectedTileY >= 1_024 ||
              secondProtectedTileX < 0 || secondProtectedTileX >= 1_024 ||
              secondProtectedTileY < 0 || secondProtectedTileY >= 1_024 ||
              authoredTiles[firstProtectedTileY * 1_024 + firstProtectedTileX] !== 171 ||
              authoredTiles[secondProtectedTileY * 1_024 + secondProtectedTileX] !== 171
            ) continue;
            const outsideTileX = tileX + direction.dx;
            const outsideTileY = tileY + direction.dy;
            if (
              outsideTileX < 0 || outsideTileX >= 1_024 ||
              outsideTileY < 0 || outsideTileY >= 1_024
            ) continue;
            const outsideTile = authoredTiles[outsideTileY * 1_024 + outsideTileX];
            if (outsideTile === 171 || authoredTileIsSolid(outsideTile)) continue;
            const inside = { x: tileX * 16 + 8, y: tileY * 16 + 8 };
            const outside = { x: outsideTileX * 16 + 8, y: outsideTileY * 16 + 8 };
            const protectedPoint = {
              x: inside.x - direction.dx * crossingProtectedStagingDistance,
              y: inside.y - direction.dy * crossingProtectedStagingDistance,
            };
            if (!positionIsOnAuthoredSafeTile(
              protectedPoint.x,
              protectedPoint.y,
            )) continue;
            if (authoredHullIsOpen != null && !authoredHullIsOpen(outside.x, outside.y)) {
              continue;
            }
            if (!exposedHoldingLaneIsClear(outside, direction)) continue;
            if (!authoredLineIsClear(
              outside.x,
              outside.y,
              authoritative.x,
              authoritative.y,
            )) continue;
            const botOffsetX = authoritative.x - outside.x;
            const botOffsetY = authoritative.y - outside.y;
            const botDistance = Math.hypot(botOffsetX, botOffsetY);
            if (botDistance < crossingMinimumApproachDistance) continue;
            const outwardAlignment = botDistance <= 0.01
              ? 1
              : (botOffsetX * direction.dx + botOffsetY * direction.dy) / botDistance;
            if (outwardAlignment < 0.9) continue;
            if (
              crossingPreserveProtectedAim && !crossingRevalidateProtectedAim
            ) {
              const targetVelocityX = Number(bot.velocityX);
              const targetVelocityY = Number(bot.velocityY);
              if (
                crossingProjectileSpeed == null ||
                !Number.isFinite(targetVelocityX) ||
                !Number.isFinite(targetVelocityY)
              ) continue;
              const protectedRelativeX = botOffsetX +
                targetVelocityX * crossingPreFireDelaySeconds;
              const protectedRelativeY = botOffsetY +
                targetVelocityY * crossingPreFireDelaySeconds;
              const protectedAim = quantizedProjectileIntercept({
                relativeX: protectedRelativeX,
                relativeY: protectedRelativeY,
                targetVelocityX,
                targetVelocityY,
                playerVelocityX: direction.dx * crossingExpectedExitSpeed,
                playerVelocityY: direction.dy * crossingExpectedExitSpeed,
                projectileSpeed: crossingProjectileSpeed,
                maxTime: crossingProjectileMaxTime,
                playerSpeedAlongDirection: crossingExpectedExitSpeedAlongAim,
              });
              if (
                protectedAim?.feasible !== true ||
                protectedAim.missDistance > crossingMaximumPredictedMissDistance
              ) continue;
              const protectedAimVector = {
                x: protectedAim.projectileVelocityX * protectedAim.time,
                y: protectedAim.projectileVelocityY * protectedAim.time,
              };
              const protectedAimLength = Math.hypot(
                protectedAimVector.x,
                protectedAimVector.y,
              );
              const protectedExitAlignment =
                (protectedAimVector.x * direction.dx +
                  protectedAimVector.y * direction.dy) /
                Math.max(0.001, protectedAimLength);
              if (
                crossingMinimumExitAimAlignment != null &&
                protectedExitAlignment < crossingMinimumExitAimAlignment
              ) continue;
              if (!protectedExitPathIsClearFrom(
                protectedPoint,
                protectedAimVector,
              )) continue;
            }
            candidates.push({ bot, botDistance, direction, inside, outside });
          }
        }
      }
    }
    return candidates.sort((left, right) => left.botDistance - right.botDistance)[0] || null;
  };
  let edge = selectEdge(initialMetrics);
  let edgeSearchSamples = 1;
  const edgeSearchDeadline = Date.now() + crossingEdgeSearchTimeout;
  while (edge == null && Date.now() < edgeSearchDeadline) {
    await sleep(50);
    const metrics = (await actor.metricsState()).uiMetrics || {};
    edgeSearchSamples += 1;
    if (
      metrics.playerAlive !== true || metrics.serverPlayerAlive !== true ||
      metrics.inSafeZone !== true
    ) {
      return reject("edge_search_lifecycle", {
        edgeSearchSamples,
        playerAlive: metrics.playerAlive,
        serverPlayerAlive: metrics.serverPlayerAlive,
        inSafeZone: metrics.inSafeZone,
      });
    }
    edge = selectEdge(metrics);
  }
  if (!edge) {
    return reject("no_safe_edge", {
      edgeSearchSamples,
      requiredTargetLifeSeq,
      timeoutMs: crossingEdgeSearchTimeout,
    });
  }
  const nearestBot = edge.bot;
  if (
    !positionIsOnAuthoredSafeTile(edge.inside.x, edge.inside.y) ||
    positionIsOnAuthoredSafeTile(edge.outside.x, edge.outside.y)
  ) {
    throw new Error(`Extreme encounter selected a non-authored safe edge: ${JSON.stringify(edge)}`);
  }

  // Stage two authored-safe tiles behind the boundary and explicitly settle
  // residual velocity there. The weapon edge is still emitted only after the
  // pilot has crossed completely into nonsafe space.
  const protectedStagingPoint = {
    x: edge.inside.x - edge.direction.dx * crossingProtectedStagingDistance,
    y: edge.inside.y - edge.direction.dy * crossingProtectedStagingDistance,
  };
  if (!positionIsOnAuthoredSafeTile(
    protectedStagingPoint.x,
    protectedStagingPoint.y,
  )) {
    throw new Error(`Extreme encounter staging point left authored safety: ${JSON.stringify({
      edge,
      protectedStagingPoint,
    })}`);
  }
  const staged = await driveClientToWorldTarget(
    actor,
    protectedStagingPoint,
    20_000,
    true,
    7.75,
    null,
    100,
  );
  if (!staged.reached) return reject("protected_staging_not_reached", { staged });
  const braked = await brakeClientVelocity(actor, 3_000);
  const brakedMetrics = (await actor.metricsState()).uiMetrics || {};
  if (
    braked.braked !== true ||
    brakedMetrics.inSafeZone !== true ||
    brakedMetrics.playerAlive !== true ||
    brakedMetrics.serverPlayerAlive !== true
  ) {
    return reject("protected_staging_not_braked", {
      braked,
      inSafeZone: brakedMetrics.inSafeZone,
      playerAlive: brakedMetrics.playerAlive,
      serverPlayerAlive: brakedMetrics.serverPlayerAlive,
    });
  }
  const protectedExitPathIsClear = (lead) =>
    protectedExitPathIsClearFrom(protectedStagingPoint, lead);
  let targetVelocity = null;
  let lastAim = null;
  const observeTargetMotion = (player) => {
    if (
      !Number.isFinite(player?.velocityX) ||
      !Number.isFinite(player?.velocityY)
    ) return targetVelocity;
    targetVelocity = { x: player.velocityX, y: player.velocityY };
    return targetVelocity;
  };
  const orientToEdgeDirection = async (timeoutMs) => {
    const desired = ((Math.atan2(
      edge.direction.dx,
      -edge.direction.dy,
    ) / (Math.PI * 2)) + 1) % 1;
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      if (
        metrics.playerAlive !== true || metrics.serverPlayerAlive !== true ||
        metrics.inSafeZone !== true || !Number.isFinite(metrics.playerOrientation)
      ) return false;
      const current = ((metrics.playerOrientation % 1) + 1) % 1;
      const turn = signedOrientationDelta(current, desired);
      if (Math.abs(turn) <= crossingOrientationTolerance) return true;
      await actor.hold(
        turn > 0 ? "ArrowRight" : "ArrowLeft",
        Math.min(48, Math.max(24, Math.abs(turn) * 1_400)),
      );
    }
    return false;
  };
  if (
    crossingPreserveProtectedAim &&
    !await orientToEdgeDirection(1_500)
  ) {
    return reject("protected_edge_orientation_failed");
  }
  // A ship below the shared 3 px/s braking threshold is settled for ordinary
  // routing, but can still coast about 45 px during this 15-second encounter
  // window. That is wider than the protected runway behind some Extreme safe
  // edges. Keep the pilot on the authored staging point with ordinary keyboard
  // feedback while waiting; otherwise the test can silently become exposed
  // before it acquires the protected shot it claims to prove.
  const approachDeadline = Date.now() + 15_000;
  // The point is two complete authored-safe tiles behind the boundary. A
  // 12 px centering envelope still leaves the 14 px ship hull protected, and
  // prevents Firefox's subpixel coast from turning an otherwise-valid wait
  // into an endless 4 px route/brake oscillation.
  const protectedCenterTolerance = 12;
  const protectedSpeedTolerance = 12;
  const targetApproachesEdge = (
    metrics,
    maximumDistance = crossingApproachDistance,
  ) =>
    (metrics.remotePlayers || []).some((player) => {
      const authoritative = authoritativePosition(player);
      if (
        player.id !== nearestBot.id || player.lifeSeq !== nearestBot.lifeSeq ||
        !Number.isFinite(authoritative.x) || !Number.isFinite(authoritative.y) ||
        positionIsOnAuthoredSafeTile(player.positionX, player.positionY)
      ) return false;
      observeTargetMotion(player);
      const offsetX = authoritative.x - edge.outside.x;
      const offsetY = authoritative.y - edge.outside.y;
      const distance = Math.hypot(offsetX, offsetY);
      const alignment = distance <= 0.01
        ? 1
        : (offsetX * edge.direction.dx + offsetY * edge.direction.dy) / distance;
      return distance >= crossingMinimumApproachDistance &&
        distance <= maximumDistance &&
        alignment >= crossingMinimumApproachAlignment && authoredLineIsClear(
          edge.outside.x,
          edge.outside.y,
          authoritative.x,
          authoritative.y,
        );
    });
  let approached = false;
  let lastApproachDisposition = null;
  while (Date.now() < approachDeadline && !approached) {
    let metrics = (await actor.metricsState()).uiMetrics || {};
    if (metrics.playerAlive !== true || metrics.serverPlayerAlive !== true) {
      lastApproachDisposition = {
        reason: "pilot_lifecycle",
        playerAlive: metrics.playerAlive,
        serverPlayerAlive: metrics.serverPlayerAlive,
        inSafeZone: metrics.inSafeZone,
      };
      break;
    }
    const stagingDrift = Math.hypot(
      metrics.playerX - protectedStagingPoint.x,
      metrics.playerY - protectedStagingPoint.y,
    );
    if (metrics.inSafeZone !== true || stagingDrift > protectedCenterTolerance) {
      const recentered = await driveClientToWorldTarget(
        actor,
        protectedStagingPoint,
        Math.min(4_000, Math.max(500, approachDeadline - Date.now())),
        false,
        8,
        null,
        60,
        0.25,
        3_000,
      );
      const rebraked = recentered.reached
        ? await brakeClientVelocity(
            actor,
            Math.min(2_000, Math.max(250, approachDeadline - Date.now())),
          )
        : null;
      metrics = (await actor.metricsState()).uiMetrics || {};
      const remainingDrift = Math.hypot(
        metrics.playerX - protectedStagingPoint.x,
        metrics.playerY - protectedStagingPoint.y,
      );
      const remainingSpeed = Math.hypot(
        metrics.playerVelocityX ?? 0,
        metrics.playerVelocityY ?? 0,
      );
      if (
        !recentered.reached ||
        (rebraked?.braked !== true && remainingSpeed > protectedSpeedTolerance) ||
        metrics.inSafeZone !== true ||
        remainingDrift > protectedCenterTolerance
      ) {
        lastApproachDisposition = {
          reason: "protected_staging_drift",
          stagingDrift,
          remainingDrift,
          remainingSpeed,
          recentered,
          rebraked,
          inSafeZone: metrics.inSafeZone,
        };
        if (Date.now() >= approachDeadline) break;
        await sleep(25);
        continue;
      }
    }
    approached = targetApproachesEdge(metrics);
    if (!approached) await sleep(25);
  }
  if (!approached) {
    return reject("bot_never_approached_edge", { lastApproachDisposition });
  }

  // The bot can take most of the approach window to reach this edge. Settle
  // once more immediately before aiming so residual subpixel coast cannot
  // carry the protected pilot across the boundary while Firefox turns. This
  // is ordinary keyboard movement and is independently rechecked against the
  // authored safe tile before any weapon key is accepted.
  // Centering and braking are coupled: the opposite-thrust pulse that removes
  // Firefox's residual velocity can carry the ship back across a nearby safe
  // boundary. Close that feedback loop against observed authoritative state
  // instead of assuming one route followed by one brake is sufficient. Every
  // correction below remains an ordinary WebDriver/DevTools keyboard action.
  const finalProtectedSettleDeadline = Date.now() + 9_000;
  const finalProtectedSettles = [];
  let finalProtectedCenter = null;
  let finalProtectedBrake = null;
  let finalProtectedMetrics = {};
  let finalProtectedDrift = Number.POSITIVE_INFINITY;
  let finalProtectedSpeed = Number.POSITIVE_INFINITY;
  let finalProtectedSettled = false;
  while (Date.now() < finalProtectedSettleDeadline && !finalProtectedSettled) {
    const remainingBeforeCenter = finalProtectedSettleDeadline - Date.now();
    finalProtectedCenter = await driveClientToWorldTarget(
      actor,
      protectedStagingPoint,
      Math.min(4_000, Math.max(500, remainingBeforeCenter)),
      false,
      8,
      null,
      60,
      0.25,
      3_000,
    );
    const remainingBeforeBrake = finalProtectedSettleDeadline - Date.now();
    finalProtectedBrake = finalProtectedCenter.reached && remainingBeforeBrake > 0
      ? await brakeClientVelocity(
          actor,
          Math.min(3_000, Math.max(250, remainingBeforeBrake)),
        )
      : null;
    finalProtectedMetrics = (await actor.metricsState()).uiMetrics || {};
    finalProtectedDrift = Math.hypot(
      finalProtectedMetrics.playerX - protectedStagingPoint.x,
      finalProtectedMetrics.playerY - protectedStagingPoint.y,
    );
    finalProtectedSpeed = Math.hypot(
      finalProtectedMetrics.playerVelocityX ?? 0,
      finalProtectedMetrics.playerVelocityY ?? 0,
    );
    finalProtectedSettled = finalProtectedCenter.reached === true &&
      (finalProtectedBrake?.braked === true ||
        finalProtectedSpeed <= protectedSpeedTolerance) &&
      finalProtectedMetrics.playerAlive === true &&
      finalProtectedMetrics.serverPlayerAlive === true &&
      finalProtectedMetrics.inSafeZone === true &&
      finalProtectedDrift <= protectedCenterTolerance;
    finalProtectedSettles.push({
      center: finalProtectedCenter,
      brake: finalProtectedBrake,
      drift: finalProtectedDrift,
      speed: finalProtectedSpeed,
      inSafeZone: finalProtectedMetrics.inSafeZone,
      playerAlive: finalProtectedMetrics.playerAlive,
      serverPlayerAlive: finalProtectedMetrics.serverPlayerAlive,
    });
  }
  if (!finalProtectedSettled) {
    return reject("final_protected_staging_not_settled", {
      finalProtectedCenter,
      finalProtectedBrake,
      finalProtectedDrift,
      finalProtectedSpeed,
      inSafeZone: finalProtectedMetrics.inSafeZone,
      finalProtectedSettles,
    });
  }

  // With exposed revalidation enabled, a protected intercept is deliberately
  // discarded after the real safe->unsafe transition. Waiting for that exact
  // intercept here was therefore both redundant and harmful: the bot could
  // orbit away during the protected turn even though the actual exposed shot
  // would be recomputed. Bind the crossing instead to a fresh exact-life edge
  // approach while the pilot remains settled in authored safety.
  if (crossingPreserveProtectedAim && crossingRevalidateProtectedAim) {
    const finalApproachDeadline = Date.now() + 15_000;
    let finalApproachDisposition = null;
    let finalApproachReady = false;
    while (Date.now() < finalApproachDeadline && !finalApproachReady) {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      const stagingDrift = Math.hypot(
        metrics.playerX - protectedStagingPoint.x,
        metrics.playerY - protectedStagingPoint.y,
      );
      if (
        metrics.playerAlive !== true || metrics.serverPlayerAlive !== true ||
        metrics.inSafeZone !== true || stagingDrift > protectedCenterTolerance
      ) {
        finalApproachDisposition = {
          reason: "protected_staging_lost",
          playerAlive: metrics.playerAlive,
          serverPlayerAlive: metrics.serverPlayerAlive,
          inSafeZone: metrics.inSafeZone,
          stagingDrift,
        };
        break;
      }
      finalApproachReady = targetApproachesEdge(
        metrics,
        crossingFinalApproachDistance,
      );
      if (!finalApproachReady) {
        finalApproachDisposition = { reason: "target_not_at_edge", stagingDrift };
        await sleep(25);
      }
    }
    if (!finalApproachReady) {
      return reject("final_target_approach_failed", { finalApproachDisposition });
    }
  }

  const orientToTarget = async (
    timeoutMs,
    projectileSpeed = null,
    aimOrigin = null,
    {
      maximumDistance = crossingMaximumDistance,
      maximumRecedingSpeed = crossingMaximumRecedingSpeed,
      requireProtectedPilot = false,
      requireExposedPilot = false,
      minimumExitAimAlignment = null,
      preFireDelaySeconds = 0,
      expectedFireVelocity = null,
      expectedFireSpeedAlongAim = null,
    } = {},
  ) => {
    const deadline = Date.now() + timeoutMs;
    let lastDisposition = null;
    while (Date.now() < deadline) {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      if (
        metrics.playerAlive !== true || metrics.serverPlayerAlive !== true ||
        (requireProtectedPilot && metrics.inSafeZone !== true) ||
        (requireExposedPilot && metrics.inSafeZone !== false)
      ) {
        lastDisposition = {
          reason: "pilot_lifecycle",
          playerAlive: metrics.playerAlive,
          serverPlayerAlive: metrics.serverPlayerAlive,
          inSafeZone: metrics.inSafeZone,
        };
        reject("aim_rejected", lastDisposition);
        return false;
      }
      const currentBot = (metrics.remotePlayers || []).find((player) =>
        player.id === nearestBot.id && player.lifeSeq === nearestBot.lifeSeq &&
        Number.isFinite(player.positionX) && Number.isFinite(player.positionY)
      );
      if (!currentBot || !Number.isFinite(metrics.playerOrientation)) {
        lastDisposition = { reason: "target_or_orientation_missing" };
        reject("aim_rejected", lastDisposition);
        return false;
      }
      const authoritative = authoritativePosition(currentBot);
      const measuredVelocity = observeTargetMotion(currentBot);
      if (projectileSpeed != null && measuredVelocity == null) {
        lastDisposition = { reason: "authoritative_velocity_missing" };
        await sleep(25);
        continue;
      }
      const originX = Number.isFinite(aimOrigin?.x)
        ? aimOrigin.x
        : metrics.playerX;
      const originY = Number.isFinite(aimOrigin?.y)
        ? aimOrigin.y
        : metrics.playerY;
      // A protected safe-edge shot is oriented before the real-key crossing.
      // Lead the target through that measured crossing interval as well as
      // through projectile flight; otherwise a 320 px/s Extreme bot can move
      // roughly a complete ship width before the weapon is legally exposed.
      const relativeX = authoritative.x - originX +
        (measuredVelocity?.x ?? 0) * preFireDelaySeconds;
      const relativeY = authoritative.y - originY +
        (measuredVelocity?.y ?? 0) * preFireDelaySeconds;
      if (
        projectileSpeed != null &&
        Math.hypot(relativeX, relativeY) > maximumDistance
      ) {
        lastDisposition = {
          reason: "distance",
          distance: Math.hypot(relativeX, relativeY),
          maximumDistance,
        };
        await sleep(25);
        continue;
      }
      const solverPlayerVelocityX = expectedFireVelocity?.x ??
        (metrics.playerVelocityX ?? 0);
      const solverPlayerVelocityY = expectedFireVelocity?.y ??
        (metrics.playerVelocityY ?? 0);
      const lead = projectileSpeed == null
        ? { x: relativeX, y: relativeY, feasible: true, time: 0 }
        : projectileInterceptVector(
            relativeX,
            relativeY,
            measuredVelocity.x - solverPlayerVelocityX,
            measuredVelocity.y - solverPlayerVelocityY,
            projectileSpeed,
          crossingProjectileMaxTime,
          );
      const current = ((metrics.playerOrientation % 1) + 1) % 1;
      const currentDirection = Number.isInteger(metrics.playerProtocolDirection)
        ? metrics.playerProtocolDirection
        : Math.floor(Math.fround(current) * 40) % 40;
      const quantizedAim = projectileSpeed == null
        ? null
        : quantizedProjectileIntercept({
            relativeX,
            relativeY,
            targetVelocityX: measuredVelocity.x,
            targetVelocityY: measuredVelocity.y,
            playerVelocityX: solverPlayerVelocityX,
            playerVelocityY: solverPlayerVelocityY,
            projectileSpeed,
            maxTime: crossingProjectileMaxTime,
          playerSpeedAlongDirection: expectedFireSpeedAlongAim,
        });
      const currentDirectionAim = projectileSpeed == null
        ? null
        : quantizedProjectileIntercept({
            relativeX,
            relativeY,
            targetVelocityX: measuredVelocity.x,
            targetVelocityY: measuredVelocity.y,
            playerVelocityX: solverPlayerVelocityX,
            playerVelocityY: solverPlayerVelocityY,
            projectileSpeed,
            maxTime: crossingProjectileMaxTime,
            playerSpeedAlongDirection: expectedFireSpeedAlongAim,
            fixedDirection: currentDirection,
          });
      const aimRadialVelocity = (aim) => {
        if (aim == null) return null;
        return projectileRadialVelocity(
          relativeX,
          relativeY,
          (measuredVelocity?.x ?? 0) - aim.playerVelocityX,
          (measuredVelocity?.y ?? 0) - aim.playerVelocityY,
        );
      };
      const bestRadialVelocity = aimRadialVelocity(quantizedAim);
      const currentRadialVelocity = aimRadialVelocity(currentDirectionAim);
      const currentDirectionCanHit = currentDirectionAim?.feasible === true &&
        currentDirectionAim.missDistance <= crossingMaximumPredictedMissDistance &&
        currentRadialVelocity != null &&
        currentRadialVelocity <= maximumRecedingSpeed;
      const selectedAim = currentDirectionCanHit
        ? currentDirectionAim
        : quantizedAim;
      const modeledPlayerVelocityX = selectedAim?.playerVelocityX ??
        solverPlayerVelocityX;
      const modeledPlayerVelocityY = selectedAim?.playerVelocityY ??
        solverPlayerVelocityY;
      const relativeVelocityX = (measuredVelocity?.x ?? 0) -
        modeledPlayerVelocityX;
      const relativeVelocityY = (measuredVelocity?.y ?? 0) -
        modeledPlayerVelocityY;
      const radialVelocity = projectileSpeed == null
        ? null
        : aimRadialVelocity(selectedAim);
      if (
        projectileSpeed != null &&
        ((expectedFireSpeedAlongAim == null && lead?.feasible !== true) ||
          quantizedAim?.feasible !== true ||
          quantizedAim.missDistance > crossingMaximumPredictedMissDistance ||
          bestRadialVelocity == null ||
          bestRadialVelocity > maximumRecedingSpeed)
      ) {
        lastDisposition = {
          reason: "intercept_or_radial_velocity",
          lead,
          quantizedAim,
          currentDirectionAim,
          crossingMaximumPredictedMissDistance,
          radialVelocity: bestRadialVelocity,
          maximumRecedingSpeed,
          measuredVelocity,
        };
        await sleep(25);
        continue;
      }
      // A server snapshot can reach the browser immediately before an
      // authoritative wall bounce. A mathematically valid lead against that
      // predecessor velocity then becomes wrong before the real key edge is
      // sampled. Require the target's complete measured pre-fire + flight
      // corridor to remain source-valid before accepting the intercept. This
      // does not predict AI steering; it excludes only a lead whose current
      // canonical velocity is already guaranteed to collide with authored
      // geometry during the modeled shot.
      const targetTravelSeconds = projectileSpeed == null
        ? 0
        : preFireDelaySeconds + (selectedAim?.time ?? lead?.time ?? 0);
      if (
        projectileSpeed != null &&
        authoredHullIsOpen != null &&
        !movingHullPathIsClear({
          x: authoritative.x,
          y: authoritative.y,
          velocityX: measuredVelocity.x,
          velocityY: measuredVelocity.y,
          seconds: targetTravelSeconds,
          hullIsOpen: authoredHullIsOpen,
        })
      ) {
        lastDisposition = {
          reason: "target_motion_path_blocked",
          targetTravelSeconds,
          target: authoritative,
          measuredVelocity,
          selectedAim,
        };
        await sleep(25);
        continue;
      }
      const aimVector = selectedAim == null
        ? lead
        : {
            x: selectedAim.projectileVelocityX * selectedAim.time,
            y: selectedAim.projectileVelocityY * selectedAim.time,
          };
      const exitAimAlignment = minimumExitAimAlignment == null
        ? null
        : (aimVector.x * edge.direction.dx + aimVector.y * edge.direction.dy) /
          Math.max(0.001, Math.hypot(aimVector.x, aimVector.y));
      if (
        minimumExitAimAlignment != null &&
        exitAimAlignment < minimumExitAimAlignment
      ) {
        lastDisposition = {
          reason: "exit_aim_alignment",
          exitAimAlignment,
          minimumExitAimAlignment,
          lead: aimVector,
        };
        await sleep(25);
        continue;
      }
      if (
        minimumExitAimAlignment != null &&
        !protectedExitPathIsClear(aimVector)
      ) {
        lastDisposition = {
          reason: "exit_hull_path_blocked",
          exitAimAlignment,
          lead: aimVector,
          protectedStagingPoint,
          edge: {
            direction: edge.direction,
            inside: edge.inside,
            outside: edge.outside,
          },
        };
        await sleep(25);
        continue;
      }
      if (
        projectileSpeed != null &&
        !authoredLineIsClear(
          originX,
          originY,
          originX + aimVector.x,
          originY + aimVector.y,
        )
      ) {
        lastDisposition = { reason: "authored_line_blocked", lead: aimVector };
        await sleep(25);
        continue;
      }
      const desired = quantizedAim == null
        ? ((Math.atan2(
            lead.x,
            -lead.y,
          ) / (Math.PI * 2)) + 1) % 1
        : quantizedAim.orientation;
      const turn = signedOrientationDelta(current, desired);
      lastAim = {
        botId: currentBot.id,
        botLifeSeq: currentBot.lifeSeq,
        targetServerTick: currentBot.lastServerPositionTick,
        relativeX,
        relativeY,
        targetVelocityX: measuredVelocity?.x ?? 0,
        targetVelocityY: measuredVelocity?.y ?? 0,
        relativeVelocityX,
        relativeVelocityY,
        radialVelocity,
        playerVelocityX: metrics.playerVelocityX ?? 0,
        playerVelocityY: metrics.playerVelocityY ?? 0,
        projectileSpeed,
        crossingProjectileMaxTime,
        crossingMaximumDistance: maximumDistance,
        preFireDelaySeconds,
        expectedFireVelocityX: modeledPlayerVelocityX,
        expectedFireVelocityY: modeledPlayerVelocityY,
        interceptX: lead.x,
        interceptY: lead.y,
        interceptSeconds: lead.time,
        interceptFeasible: lead.feasible,
        protocolDirection: selectedAim?.direction ?? null,
        predictedMissDistance: selectedAim?.missDistance ?? null,
        quantizedInterceptSeconds: selectedAim?.time ?? null,
        exitAimAlignment,
        orientationError: turn,
      };
      // `playerOrientation` is intentionally rounded for human-readable UI
      // diagnostics. At a 1/40 boundary that rounded value can name the next
      // bin while the exact f32 transmitted by the client still belongs to
      // the predecessor bin. Fire only from the exact protocol direction
      // exported by the same client conversion that builds the input record.
      if (
        quantizedAim != null
          ? currentDirectionCanHit
          : Math.abs(turn) <= crossingOrientationTolerance
      ) return true;
      lastDisposition = {
        reason: "orientation",
        orientationError: turn,
        tolerance: crossingOrientationTolerance,
        lastAim,
      };
      const closeProjectileTrack = projectileSpeed != null &&
        Math.hypot(relativeX, relativeY) <= maximumDistance;
      await actor.hold(
        turn > 0 ? "ArrowRight" : "ArrowLeft",
        Math.min(
          closeProjectileTrack ? 48 : 180,
          Math.max(24, Math.abs(turn) * 1_400),
        ),
      );
    }
    reject("aim_timeout", { timeoutMs, lastDisposition, lastAim });
    return false;
  };
  // Only a shot that is submitted on the first exposed sample owns a
  // protected predecessor aim. An exposed shot is deliberately aimed after
  // the crossing from its actual canonical position and inherited velocity;
  // pre-aiming it here and then turning back to the edge direction would
  // measure a target state that the emitted weapon can never use.
  if (
    crossingPreserveProtectedAim && !crossingRevalidateProtectedAim &&
    !await orientToTarget(
      crossingProtectedAimTimeout,
      crossingProjectileSpeed,
      crossingFireKey == null ? null : edge.outside,
      {
        requireProtectedPilot: true,
        minimumExitAimAlignment: crossingMinimumExitAimAlignment,
        preFireDelaySeconds: crossingPreFireDelaySeconds,
        expectedFireVelocity: {
          x: edge.direction.dx * crossingExpectedExitSpeed,
          y: edge.direction.dy * crossingExpectedExitSpeed,
        },
        expectedFireSpeedAlongAim: crossingExpectedExitSpeedAlongAim,
      },
    )
  ) return reject("protected_aim_failed", { lastAim });
  if (
    (!crossingPreserveProtectedAim || crossingRevalidateProtectedAim) &&
    !await orientToEdgeDirection(1_500)
  ) {
    return reject("protected_edge_orientation_failed", { lastAim });
  }

  let crossed = false;
  let protectedShotSubmitted = false;
  let protectedShotAim = null;
  if (crossingThrustModifierKey != null) {
    await actor.keyDown(crossingThrustModifierKey);
  }
  await actor.keyDown("ArrowUp");
  try {
    const deadline = Date.now() + 2_500;
    while (Date.now() < deadline) {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      if (metrics.playerAlive !== true || metrics.serverPlayerAlive !== true) break;
      const outwardProgress =
        (metrics.playerX - edge.inside.x) * edge.direction.dx +
        (metrics.playerY - edge.inside.y) * edge.direction.dy;
      if (metrics.inSafeZone === false && outwardProgress >= 12) {
        crossed = true;
        if (
          crossingFireKey != null && crossingPreserveProtectedAim &&
          !crossingRevalidateProtectedAim
        ) {
          // The protected intercept is selected against edge.outside and its
          // complete authored hull corridor. Submit it on the first observed
          // authoritative-safe transition only when the caller explicitly
          // accepts that predecessor aim. Direct-contact bombs revalidate on
          // the exposed canonical sample below because a deterministic orbit
          // is curved over the real keyboard crossing interval.
          // A slow headed browser can otherwise keep accelerating throughout
          // the fire-key sampling interval after the solver has bound the
          // crossing velocity. Release genuine thrust first when requested;
          // the shot retains its inherited crossing velocity without adding
          // an unmodeled second acceleration phase.
          protectedShotAim = lastAim == null ? null : { ...lastAim };
          if (crossingReleaseThrustBeforeProtectedFire) {
            await actor.keyUp("ArrowUp").catch(() => {});
            if (crossingThrustModifierKey != null) {
              await actor.keyUp(crossingThrustModifierKey).catch(() => {});
            }
          }
          await actor.keyDown(crossingFireKey);
          await sleep(crossingFireHoldMs);
          await actor.keyUp(crossingFireKey).catch(() => {});
          await sleep(crossingFireReleaseSettleMs);
          protectedShotSubmitted = true;
        }
        break;
      }
      await sleep(25);
    }
  } finally {
    await actor.keyUp("ArrowUp").catch(() => {});
    if (crossingThrustModifierKey != null) {
      await actor.keyUp(crossingThrustModifierKey).catch(() => {});
    }
  }
  if (!crossed) {
    const finalMetrics = (await actor.metricsState()).uiMetrics || {};
    return reject("safe_edge_not_crossed", {
      lastAim,
      edge: {
        direction: edge.direction,
        inside: edge.inside,
        outside: edge.outside,
        protectedStagingPoint,
      },
      finalPlayer: {
        x: finalMetrics.playerX,
        y: finalMetrics.playerY,
        velocityX: finalMetrics.playerVelocityX,
        velocityY: finalMetrics.playerVelocityY,
        orientation: finalMetrics.playerOrientation,
        inSafeZone: finalMetrics.inSafeZone,
        playerAlive: finalMetrics.playerAlive,
        serverPlayerAlive: finalMetrics.serverPlayerAlive,
      },
    });
  }
  if (crossingExposedSettleDistance > 0) {
    const holdingPoint = exposedHoldingPoint(edge.outside, edge.direction);
    const settled = await driveClientToWorldTarget(
      actor,
      holdingPoint,
      5_000,
      true,
      8,
      null,
      60,
      0.25,
      3_000,
    );
    const reachedMetrics = (await actor.metricsState()).uiMetrics || {};
    const reachedDrift = Math.hypot(
      reachedMetrics.playerX - holdingPoint.x,
      reachedMetrics.playerY - holdingPoint.y,
    );
    // The routing helper's `reached` result also owns its stringent terminal
    // speed threshold. A headed frame can land within the exact holding
    // envelope with a few px/s still remaining and report a route timeout.
    // Brake that real-key terminal explicitly, then bind acceptance to the
    // final nonsafe position instead of conflating proximity with rest.
    const holdingBrake =
      reachedMetrics.playerAlive === true &&
      reachedMetrics.serverPlayerAlive === true &&
      reachedMetrics.inSafeZone === false &&
      reachedDrift <= 12
        ? await brakeClientVelocity(actor, 2_000)
        : null;
    const settledMetrics = (await actor.metricsState()).uiMetrics || {};
    const holdingDrift = Math.hypot(
      settledMetrics.playerX - holdingPoint.x,
      settledMetrics.playerY - holdingPoint.y,
    );
    if (
      holdingBrake?.braked !== true ||
      settledMetrics.playerAlive !== true ||
      settledMetrics.serverPlayerAlive !== true ||
      settledMetrics.inSafeZone !== false ||
      holdingDrift > 12
    ) {
      return reject("exposed_staging_not_settled", {
        settled,
        reachedDrift,
        holdingBrake,
        holdingPoint,
        holdingDrift,
        playerAlive: settledMetrics.playerAlive,
        serverPlayerAlive: settledMetrics.serverPlayerAlive,
        inSafeZone: settledMetrics.inSafeZone,
      });
    }
  }
  if (crossingFireKey != null && !protectedShotSubmitted) {
    // The protected aim chooses a safe exit lane, but crossing it takes real
    // simulation time. On a slow headed browser the accepted weapon edge can
    // trail that sample by hundreds of milliseconds, so firing the protected
    // intercept verbatim targets a predecessor bot position. Stop thrust on
    // the observed safe->unsafe transition and reacquire once from the actual
    // exposed player position. This remains genuine keyboard input and is
    // deliberately bounded below one ordinary Extreme lethal volley.
    const exposedMaximumDistance = crossingExposedMaximumDistance ?? Math.max(
      crossingMaximumDistance,
      crossingApproachDistance + 160,
    );
    if (!await orientToTarget(
      crossingExposedAimTimeout ??
        (crossingThrustModifierKey == null ? 650 : 1_500),
      crossingProjectileSpeed,
      null,
      {
        maximumDistance: exposedMaximumDistance,
        maximumRecedingSpeed: crossingMaximumRecedingSpeed,
        requireExposedPilot: true,
      },
    )) return reject("exposed_aim_failed", { lastAim });
    await actor.keyDown(crossingFireKey);
    await sleep(crossingFireHoldMs);
    await actor.keyUp(crossingFireKey).catch(() => {});
    await sleep(crossingFireReleaseSettleMs);
  } else if (!protectedShotSubmitted && !await orientToTarget(
    crossingProjectileSpeed == null ? 1_500 : 1_250,
    crossingProjectileSpeed,
    null,
    { requireExposedPilot: true },
  )) {
    return false;
  }

  // Staging owns only real-key geometry and the optional real-key edge. The
  // caller still proves acceptance through server-attributed damage/death;
  // no gameplay or packet state is injected here.
  const metrics = (await actor.metricsState()).uiMetrics || {};
  if (
    crossingFireKey != null && Number.isFinite(initialFireSemanticCount) &&
    !(
      Number(metrics.semanticInputCounts?.[fireSemanticMetric]) >
      initialFireSemanticCount
    )
  ) {
    return reject("fire_edge_not_sampled", {
      crossingFireKey,
      fireSemanticMetric,
      crossingFireHoldMs,
      crossingFireReleaseSettleMs,
      initialFireSemanticCount,
      finalFireSemanticCount:
        metrics.semanticInputCounts?.[fireSemanticMetric] ?? null,
    });
  }
  const accepted = metrics.playerAlive === true &&
    metrics.serverPlayerAlive === true &&
    metrics.inSafeZone === false;
  return accepted
    ? {
        botId: nearestBot.id,
        botLifeSeq: nearestBot.lifeSeq,
        edge,
        aim: lastAim,
        protectedShotAim,
      }
    : reject("post_fire_lifecycle", {
        playerAlive: metrics.playerAlive,
        serverPlayerAlive: metrics.serverPlayerAlive,
        inSafeZone: metrics.inSafeZone,
        lastAim,
      });
}

function continuumTickDiff(current, anchor) {
  let distance = (current - anchor) % CONTINUUM_TICK_MODULUS;
  if (distance < 0) distance += CONTINUUM_TICK_MODULUS;
  if (distance >= CONTINUUM_TICK_MODULUS / 2) {
    distance -= CONTINUUM_TICK_MODULUS;
  }
  return distance;
}

export function ownedProjectileTailSettled({
  projectileEvents,
  ownerId,
  latestServerTick,
  weaponTypes,
}) {
  if (!Number.isInteger(ownerId) || !Number.isInteger(latestServerTick)) return false;
  const relevant = (projectileEvents || []).filter((event) =>
    event.ownerId === ownerId && weaponTypes.has(event.weapon & 0x1f)
  );
  const spawns = relevant.filter((event) => event.type === "server_projectile_spawned");
  if (spawns.length === 0) return false;
  const resolved = new Set(
    relevant
      .filter((event) => event.type === "server_projectile_hit")
      .map((event) => event.projectileId),
  );
  return spawns.every((spawn) =>
    resolved.has(spawn.projectileId) ||
    continuumTickDiff(latestServerTick, spawn.expiryTick) >= 0
  );
}

export function continuumPositionVelocity(previous, current) {
  if (
    !Number.isInteger(previous?.serverTick) ||
    !Number.isInteger(current?.serverTick) ||
    !Number.isFinite(previous?.x) ||
    !Number.isFinite(previous?.y) ||
    !Number.isFinite(current?.x) ||
    !Number.isFinite(current?.y)
  ) return null;
  const elapsedTicks =
    (current.serverTick - previous.serverTick + 0x1_0000) % 0x1_0000;
  if (elapsedTicks === 0 || elapsedTicks >= 0x8000) return null;
  const elapsedSeconds = elapsedTicks / 100;
  return {
    x: (current.x - previous.x) / elapsedSeconds,
    y: (current.y - previous.y) / elapsedSeconds,
    elapsedTicks,
  };
}

export function projectileInterceptVector(
  relativeX,
  relativeY,
  relativeVelocityX,
  relativeVelocityY,
  speed,
  maxTime = 0.75,
) {
  if (
    !Number.isFinite(relativeX) ||
    !Number.isFinite(relativeY) ||
    !Number.isFinite(relativeVelocityX) ||
    !Number.isFinite(relativeVelocityY) ||
    !Number.isFinite(speed) ||
    speed <= 0 ||
    !Number.isFinite(maxTime) ||
    maxTime < 0
  ) return null;
  const a = relativeVelocityX * relativeVelocityX +
    relativeVelocityY * relativeVelocityY - speed * speed;
  const b = 2 * (relativeX * relativeVelocityX + relativeY * relativeVelocityY);
  const c = relativeX * relativeX + relativeY * relativeY;
  let time = Math.sqrt(c) / speed;
  let feasible = false;
  if (Math.abs(a) > 1e-6) {
    const discriminant = b * b - 4 * a * c;
    if (discriminant >= 0) {
      const root = Math.sqrt(discriminant);
      const candidates = [(-b - root) / (2 * a), (-b + root) / (2 * a)]
        .filter((candidate) => candidate > 0 && Number.isFinite(candidate));
      if (candidates.length > 0) {
        time = Math.min(...candidates);
        feasible = true;
      }
    }
  } else if (Math.abs(b) > 1e-6) {
    const candidate = -c / b;
    if (candidate > 0 && Number.isFinite(candidate)) {
      time = candidate;
      feasible = true;
    }
  } else if (c <= 1e-6) {
    time = 0;
    feasible = true;
  }
  feasible = feasible && time <= maxTime;
  time = Math.min(maxTime, Math.max(0, time));
  return {
    x: relativeX + relativeVelocityX * time,
    y: relativeY + relativeVelocityY * time,
    feasible,
    time,
  };
}

export function quantizedProjectileIntercept({
  relativeX,
  relativeY,
  targetVelocityX,
  targetVelocityY,
  playerVelocityX,
  playerVelocityY,
  projectileSpeed,
  maxTime,
  playerSpeedAlongDirection = null,
  fixedDirection = null,
}) {
  if (![
    relativeX,
    relativeY,
    targetVelocityX,
    targetVelocityY,
    playerVelocityX,
    playerVelocityY,
    projectileSpeed,
    maxTime,
  ].every(Number.isFinite) || projectileSpeed <= 0 || maxTime <= 0 ||
    (playerSpeedAlongDirection != null &&
      (!Number.isFinite(playerSpeedAlongDirection) || playerSpeedAlongDirection < 0)) ||
    (fixedDirection != null &&
      (!Number.isInteger(fixedDirection) || fixedDirection < 0 || fixedDirection >= 40))
  ) return null;

  let best = null;
  const firstDirection = fixedDirection ?? 0;
  const directionLimit = fixedDirection == null ? 40 : fixedDirection + 1;
  for (let direction = firstDirection; direction < directionLimit; direction += 1) {
    const angle = direction / 40 * Math.PI * 2;
    const headingX = Math.sin(angle);
    const headingY = -Math.cos(angle);
    const modeledPlayerVelocityX = playerSpeedAlongDirection == null
      ? playerVelocityX
      : headingX * playerSpeedAlongDirection;
    const modeledPlayerVelocityY = playerSpeedAlongDirection == null
      ? playerVelocityY
      : headingY * playerSpeedAlongDirection;
    const projectileVelocityX = modeledPlayerVelocityX + headingX * projectileSpeed;
    const projectileVelocityY = modeledPlayerVelocityY + headingY * projectileSpeed;
    const relativeVelocityX = targetVelocityX - projectileVelocityX;
    const relativeVelocityY = targetVelocityY - projectileVelocityY;
    const relativeSpeedSquared = relativeVelocityX * relativeVelocityX +
      relativeVelocityY * relativeVelocityY;
    if (relativeSpeedSquared <= 1e-9) continue;
    const time = Math.min(maxTime, Math.max(
      0,
      -(relativeX * relativeVelocityX + relativeY * relativeVelocityY) /
        relativeSpeedSquared,
    ));
    if (time <= 0) continue;
    const missX = relativeX + relativeVelocityX * time;
    const missY = relativeY + relativeVelocityY * time;
    const missDistance = Math.hypot(missX, missY);
    if (best == null || missDistance < best.missDistance) {
      best = {
        direction,
        // Aim at the middle of the chosen truncating protocol cell so browser
        // cadence cannot accidentally spill into an adjacent direction.
        orientation: (direction + 0.5) / 40,
        projectileVelocityX,
        projectileVelocityY,
        playerVelocityX: modeledPlayerVelocityX,
        playerVelocityY: modeledPlayerVelocityY,
        time,
        missX,
        missY,
        missDistance,
        feasible: time <= maxTime,
      };
    }
  }
  return best;
}

export function projectileRadialVelocity(
  relativeX,
  relativeY,
  relativeVelocityX,
  relativeVelocityY,
) {
  if (
    !Number.isFinite(relativeX) ||
    !Number.isFinite(relativeY) ||
    !Number.isFinite(relativeVelocityX) ||
    !Number.isFinite(relativeVelocityY)
  ) return null;
  const distance = Math.hypot(relativeX, relativeY);
  if (distance <= 1e-6) return 0;
  return (
    relativeX * relativeVelocityX + relativeY * relativeVelocityY
  ) / distance;
}

export function worldPointOnCenteredViewport(
  frame,
  worldX,
  worldY,
  margin = 24,
) {
  if (
    !Number.isFinite(frame?.playerX) ||
    !Number.isFinite(frame?.playerY) ||
    !Number.isFinite(frame?.screenWidth) ||
    !Number.isFinite(frame?.screenHeight) ||
    !Number.isFinite(worldX) ||
    !Number.isFinite(worldY) ||
    !Number.isFinite(margin)
  ) return false;
  const screenX = worldX - frame.playerX + frame.screenWidth / 2;
  const screenY = worldY - frame.playerY + frame.screenHeight / 2;
  return screenX >= -margin && screenX <= frame.screenWidth + margin &&
    screenY >= -margin && screenY <= frame.screenHeight + margin;
}

export function metricsTraceWithViewport(state) {
  const screenWidth = state?.uiMetrics?.screenWidth;
  const screenHeight = state?.uiMetrics?.screenHeight;
  return (state?.metricsTrace || []).map((frame) => ({
    ...frame,
    screenWidth: Number.isFinite(frame?.screenWidth)
      ? frame.screenWidth
      : screenWidth,
    screenHeight: Number.isFinite(frame?.screenHeight)
      ? frame.screenHeight
      : screenHeight,
  }));
}

// Full browser state is intentionally large: packet, metrics, sprite, and
// lifecycle rings can total tens of megabytes in a debug WASM session. Reading
// that state through CDP/WebDriver while combat is live pauses the browser long
// enough to distort the very delivery/presentation timing being measured.
// Keep the complete rings in-browser and transfer only exact bot/projectile
// rows needed by the presentation assertions. Failure/success artifact capture
// may still read the full state after gameplay has disconnected.
export function compactBotPresentationState(
  rawState,
  botIds,
  startedAt = 0,
  includePacketAudit = true,
) {
  const selected = new Set(
    (botIds || []).filter((id) => Number.isInteger(id)),
  );
  const rawUiMetrics = rawState?.uiMetrics || null;
  const actorTrace = Array.isArray(rawUiMetrics?.authoritativeSnapshotActorTrace)
    ? rawUiMetrics.authoritativeSnapshotActorTrace.slice(-1)
    : [];
  const rawActorStream = rawUiMetrics?.authoritativeSnapshotActorStream;
  const uiMetrics = rawUiMetrics
    ? {
        ...rawUiMetrics,
        authoritativeSnapshotActorTrace: actorTrace,
        authoritativeSnapshotActorStream:
          rawActorStream && typeof rawActorStream === "object"
            ? { ...rawActorStream, frames: actorTrace }
            : rawActorStream,
      }
    : null;
  const projectMotionFrame = (frame) => ({
      at: frame.at,
      monotonicMs: frame.monotonicMs,
      renderOrdinal: frame.renderOrdinal,
      authoritativeSnapshotId: frame.authoritativeSnapshotId,
      latestSnapshotId: frame.latestSnapshotId,
      latestSnapshotServerTick: frame.latestSnapshotServerTick,
      latestServerTick: frame.latestServerTick,
      combatViewTimeline: frame.combatViewTimeline || null,
      playerX: frame.playerX,
      playerY: frame.playerY,
      screenWidth: frame.screenWidth,
      screenHeight: frame.screenHeight,
      authoritativeSnapshotActors: (frame.authoritativeSnapshotActors || [])
        .filter((actor) => selected.has(actor.id)),
      remoteActorPresentation: (frame.remoteActorPresentation || [])
        .filter((actor) => selected.has(actor.id)),
      remotePlayers: (frame.remotePlayers || [])
        .filter((actor) => selected.has(actor.id)),
      remoteProjectileSources: (frame.remoteProjectileSources || [])
        .filter((projectile) => selected.has(projectile.playerId)),
      presentedRemoteProjectileSourcesOverflow: frame.presentedRemoteProjectileSourcesOverflow ?? null,
      presentedRemoteProjectileSources: Array.isArray(frame.presentedRemoteProjectileSources)
        ? frame.presentedRemoteProjectileSources.filter((projectile) => selected.has(projectile.playerId))
        : null,
    });
  const metricsTrace = (rawState?.metricsTrace || [])
    .filter((frame) => Number.isFinite(frame?.at) && frame.at >= startedAt)
    .map(projectMotionFrame);
  const motionBoundaryTrace = (rawState?.motionBoundaryTrace || [])
    .filter((frame) => Number.isFinite(frame?.at) && frame.at >= startedAt)
    .map(projectMotionFrame);
  const remoteSpriteTrace = (rawState?.remoteSpriteTrace || [])
    .filter((frame) => Number.isFinite(frame?.at) && frame.at >= startedAt)
    .map((frame) => ({
      at: frame.at,
      monotonicMs: frame.monotonicMs,
      renderOrdinal: frame.renderOrdinal,
      spriteFrameOrdinal: frame.spriteFrameOrdinal,
      screenWidth: frame.screenWidth,
      screenHeight: frame.screenHeight,
      snapshotId: frame.snapshotId,
      presentationRenderTick: frame.presentationRenderTick,
      entries: (frame.entries || []).filter((entry) => selected.has(entry.id)),
      projectiles: {
        ...(frame.projectiles || {}),
        entries: (frame.projectiles?.entries || [])
          .filter((entry) => selected.has(entry.sourcePlayerId)),
      },
    }));
  const packetLog = includePacketAudit ? rawState?.packetLog || [] : [];
  const legacyPositionRelays = includePacketAudit
    ? packetLog.filter((entry) =>
        entry?.direction === "received" && entry?.bytes?.[0] === 0x3a
      )
    : [];
  const truncatedPackets = packetLog.filter((entry) =>
    entry?.bytesTruncated === true
  ).length;
  const maximumRetainedBytes = packetLog.reduce(
    (maximum, entry) => Math.max(maximum, entry?.bytes?.length || 0),
    0,
  );
  return {
    uiMetrics,
    metricsTrace,
    motionBoundaryTrace,
    remoteSpriteTrace,
    motionBoundarySprites: rawState?.motionBoundarySprites || [],
    packetAudit: {
      inspectedPackets: packetLog.length,
      truncatedPackets,
      maximumRetainedBytes,
      receivedLegacyPositionRelays: legacyPositionRelays.length,
      samples: legacyPositionRelays.slice(0, 3),
    },
    audioUnlock: rawState?.audioUnlock || null,
    botAudioProbe: rawState?.botAudioProbe || null,
    firefoxAudioProbe: rawState?.firefoxAudioProbe || null,
    networkStats: rawState?.networkStats || {},
  };
}

export async function captureBotPresentationState(
  actor,
  botIds,
  startedAt = 0,
  includePacketAudit = false,
  { measure = process.env.ZUBSPACE_E2E_PRESENTATION_READ_TIMING === "1" } = {},
) {
  if (typeof actor?.evaluate !== "function") {
    throw new Error("bot presentation capture requires a browser evaluator");
  }
  const expression = `(() => {
    const capture = window.__zubspaceBotMotionCapture;
    if (capture && (capture.trace !== window.__zubspaceMetricsTrace || capture.overflow !== 0 || capture.observed < 1)) {
      throw new Error("bot motion boundary capture lost coverage");
    }
    return (${compactBotPresentationState.toString()})({
    uiMetrics: window.__zubspaceUiMetrics || null,
    metricsTrace: window.__zubspaceMetricsTrace || [],
    motionBoundaryTrace: capture?.frames || [],
    motionBoundarySprites: capture?.sprites || [],
    remoteSpriteTrace: window.__zubspaceRemoteSpriteTrace || [],
    packetLog: ${includePacketAudit ? "window.__zubspacePacketLog || []" : "[]"},
    audioUnlock: window.__zubspaceAudioUnlockState || null,
    botAudioProbe: window.__zubspaceBotAudioProbe || null,
    firefoxAudioProbe: window.__zubFirefoxAudioProbe || null,
    networkStats: window.__zubspaceNetworkStats || {},
  }, ${JSON.stringify(botIds)}, ${JSON.stringify(startedAt)}, ${JSON.stringify(includePacketAudit)}); })()`;
  const captured = await captureBrowserJsonSnapshot(actor, expression);
  if (!measure) return captured.value;
  // Total read time includes frames that can render between bounded RPCs;
  // it is not itself a frame-stall measurement. Never reset the stall observer.
  const observedAt = await actor.evaluate("performance.now()");
  const timing = { kind: "diagnostic-presentation-read", ...captured.timing,
    browserObservedAfterReadAt: observedAt,
    readWindowMs: observedAt - captured.timing.browserStartedAt, startedAt, includePacketAudit };
  console.log(JSON.stringify(timing));
  return { ...captured.value, diagnosticReadTiming: timing };
}

export function timedMovementWindows(
  samples,
  {
    windowMs = 4_000,
    minimumSpanMs = 3_500,
    maximumSampleGapMs = 250,
    stride = 60,
  } = {},
) {
  const windows = [];
  for (let start = 0; start < samples.length; start += stride) {
    const window = samples.filter((sample) =>
      sample.at >= samples[start].at && sample.at <= samples[start].at + windowMs
    );
    if (window.length < 2 || window.at(-1).at - window[0].at < minimumSpanMs) continue;
    if (window.some((sample, index) =>
      index > 0 && sample.at - window[index - 1].at > maximumSampleGapMs
    )) continue;
    const xValues = window.map((sample) => sample.x);
    const yValues = window.map((sample) => sample.y);
    windows.push({
      startedAt: window[0].at,
      endedAt: window.at(-1).at,
      displacement: Math.hypot(
        Math.max(...xValues) - Math.min(...xValues),
        Math.max(...yValues) - Math.min(...yValues),
      ),
    });
  }
  return windows;
}

// A stopped source must remain stopped at its observer-time position, even
// after newer authority has resumed moving. Prove only the degenerate, exact
// stationary segment here; do not approximate the production interpolator.
// Both endpoints must have been observed already and be consecutive snapshots
// (or an exact source tick). Missing data never grants a hold allowance.
function stationaryPresentationWitness(history, previous, current) {
  const timeline = current.combatViewTimeline;
  const prior = previous.combatViewTimeline;
  const tick = timeline?.presentationRenderTick;
  const advance = (tick - prior?.presentationRenderTick) >>> 0;
  if (!Number.isInteger(tick) || tick < 0 || tick > 0xffff_ffff ||
      !Number.isInteger(prior?.presentationRenderTick) ||
      timeline.generation !== prior.generation ||
      !Number.isInteger(timeline.generation) || timeline.generation <= 0 ||
      timeline.holdOrStall !== false || timeline.scheduledRenderTick !== tick ||
      advance === 0 || advance >= 0x8000_0000) return null;
  let lower = null;
  let upper = null;
  for (const sample of history) {
    const offset = (sample.positionTick - tick) | 0;
    if (offset <= 0 && (lower == null || offset > lower.offset)) lower = { sample, offset };
    if (offset >= 0 && (upper == null || offset < upper.offset)) upper = { sample, offset };
  }
  if (lower == null || upper == null) return null;
  const a = lower.sample;
  const b = upper.sample;
  if (a.positionTick !== b.positionTick &&
      ((b.positionSnapshotId - a.positionSnapshotId) & 0xffff) !== 1) return null;
  if ([a, b].some((sample) => sample.positionX !== current.displayX ||
      sample.positionY !== current.displayY)) return null;
  return {
    at: current.at, lifeSeq: current.lifeSeq, renderTick: tick,
    lowerTick: a.positionTick, upperTick: b.positionTick,
    lowerSnapshotId: a.positionSnapshotId, upperSnapshotId: b.positionSnapshotId,
    x: current.displayX, y: current.displayY,
  };
}

// Speed and displacement belong to the source interval being drawn, not the
// newest authority sample. A Repel/wall bounce can reverse or stop authority
// while the observer is still drawing its older fast segment (and vice versa).
// Establish a complete, already-observed source interval before using it. This
// is a kinematic bound, not a copy of the production Hermite interpolator.
function presentationMotionWitness(history, previous, current) {
  const prior = previous.combatViewTimeline;
  const timeline = current.combatViewTimeline;
  const tick = timeline?.presentationRenderTick;
  const priorTick = prior?.presentationRenderTick;
  const validTick = (value) => Number.isInteger(value) && value >= 0 && value <= 0xffff_ffff;
  if (!validTick(tick) || !validTick(priorTick) ||
      !Number.isInteger(timeline.generation) || timeline.generation <= 0 ||
      timeline.generation !== prior.generation ||
      timeline.holdOrStall !== false || prior.holdOrStall !== false ||
      timeline.scheduledRenderTick !== tick || prior.scheduledRenderTick !== priorTick ||
      ((tick - priorTick) >>> 0) === 0 || ((tick - priorTick) >>> 0) >= 0x8000_0000) return null;
  let lowerIndex = -1;
  let upperIndex = -1;
  for (let index = 0; index < history.length; index += 1) {
    if (((history[index].positionTick - priorTick) | 0) <= 0) lowerIndex = index;
    if (upperIndex < 0 && ((history[index].positionTick - tick) | 0) >= 0) upperIndex = index;
  }
  if (lowerIndex < 0 || upperIndex <= lowerIndex) return null;
  const sources = history.slice(lowerIndex, upperIndex + 1);
  let maximumSpeed = 0;
  for (let index = 0; index < sources.length; index += 1) {
    const source = sources[index];
    if (source.lifeSeq !== current.lifeSeq ||
        ![source.velocityX, source.velocityY].every(Number.isFinite)) return null;
    maximumSpeed = Math.max(maximumSpeed, Math.hypot(source.velocityX, source.velocityY));
    if (index > 0) {
      const before = sources[index - 1];
      if (((source.positionSnapshotId - before.positionSnapshotId) & 0xffff) !== 1) return null;
      const ticks = (source.positionTick - before.positionTick) >>> 0;
      if (ticks === 0 || ticks >= 0x8000_0000) return null;
      // Snapshot ticks are quantized to 10ms while movement steps run at 60Hz;
      // observed segment travel may exceed velocity * one quantized tick.
      maximumSpeed = Math.max(maximumSpeed, Math.hypot(
        source.positionX - before.positionX, source.positionY - before.positionY,
      ) / (ticks * 0.01));
    }
  }
  return {
    at: current.at, lifeSeq: current.lifeSeq, generation: timeline.generation,
    previousRenderTick: priorTick, renderTick: tick, maximumSpeed,
    sources: sources.map((source) => ({
      snapshotId: source.positionSnapshotId, tick: source.positionTick,
      x: source.positionX, y: source.positionY,
      velocityX: source.velocityX, velocityY: source.velocityY,
    })),
  };
}

// Measure presentation continuity only across frames that the browser really
// drew. Slow actors may legitimately repeat a sub-pixel position, while fast
// Extreme actors may move more than a fixed number of pixels per frame. Bind
// the jump ceiling to measured velocity/authoritative travel, and report a
// hold only when authority travels materially while the exact-life sprite is
// frozen across several ordinary render frames.
export function remoteActorMotionSmoothnessEvidence(
  samples,
  {
    maximumFrameGapMs = 50,
    minimumMaterialHoldMs = 80,
    minimumMaterialHoldTravel = 8,
    minimumJumpPixels = 12,
    minimumMaterialRenderGapMs = 80,
    rejectRenderFrameGaps = true,
  } = {},
) {
  const finite = (samples || []).filter((sample) =>
    Number.isFinite(sample?.at) &&
    Number.isInteger(sample?.lifeSeq) &&
    [sample.positionX, sample.positionY, sample.displayX, sample.displayY]
      .every(Number.isFinite)
  );
  const materialHolds = [];
  const excessiveJumps = [];
  const presentationSnaps = [];
  const renderFrameGaps = [];
  const sourceHistory = [];
  let stationaryTimelineFrames = 0;
  const stationaryTimelineWitnesses = [];
  let presentationMotionFrames = 0;
  const presentationMotionWitnesses = [];
  const rememberSource = (sample) => {
    if (!Number.isInteger(sample.positionTick) || sample.positionTick < 0 ||
        sample.positionTick > 0xffff_ffff || !Number.isInteger(sample.positionSnapshotId) ||
        sample.positionSnapshotId < 0 || sample.positionSnapshotId > 0xffff) {
      sourceHistory.length = 0;
      return;
    }
    const last = sourceHistory.at(-1);
    if (last != null) {
      const tickDelta = (sample.positionTick - last.positionTick) >>> 0;
      const snapshotDelta = (sample.positionSnapshotId - last.positionSnapshotId) & 0xffff;
      if (tickDelta === 0 && snapshotDelta === 0 &&
          sample.positionX === last.positionX && sample.positionY === last.positionY) return;
      if (tickDelta === 0 || tickDelta >= 0x8000_0000 ||
          snapshotDelta === 0 || snapshotDelta >= 0x8000) sourceHistory.length = 0;
    }
    sourceHistory.push(sample);
    if (sourceHistory.length > 128) sourceHistory.shift();
  };
  if (finite.length > 0) rememberSource(finite[0]);
  let hold = null;
  let maximumDisplayStep = 0;
  let maximumAuthorityStep = 0;

  const retainHoldFrame = (sample) => {
    if (hold.lastFrameAt === sample.at) return;
    hold.lastFrameAt = sample.at;
    hold.frameCount += 1;
    if (hold.frames.length < 64) hold.frames.push(sample);
  };
  const finishHold = (current) => {
    if (hold == null) return;
    retainHoldFrame(current);
    const durationMs = current.at - hold.startedAt;
    const authorityTravel = Math.hypot(
      current.positionX - hold.positionX,
      current.positionY - hold.positionY,
    );
    if (
      durationMs >= minimumMaterialHoldMs &&
      authorityTravel >= minimumMaterialHoldTravel
    ) {
      materialHolds.push({
        startedAt: hold.startedAt,
        endedAt: current.at,
        durationMs,
        authorityTravel,
        displayX: hold.displayX,
        displayY: hold.displayY,
        frames: hold.frames,
        frameCount: hold.frameCount,
        omittedFrames: hold.frameCount - hold.frames.length,
      });
    }
    hold = null;
  };

  for (let index = 1; index < finite.length; index += 1) {
    const previous = finite[index - 1];
    const current = finite[index];
    const dtMs = current.at - previous.at;
    const previousTimeline = previous.combatViewTimeline;
    const currentTimeline = current.combatViewTimeline;
    const sameLife = current.lifeSeq === previous.lifeSeq;
    if (
      sameLife && dtMs >= minimumMaterialRenderGapMs &&
      Math.hypot(
        current.positionX - previous.positionX,
        current.positionY - previous.positionY,
      ) >= 1
    ) {
      renderFrameGaps.push({
        startedAt: previous.at,
        endedAt: current.at,
        durationMs: dtMs,
        lifeSeq: current.lifeSeq,
        displayStep: Math.hypot(
          current.displayX - previous.displayX,
          current.displayY - previous.displayY,
        ),
        authorityStep: Math.hypot(
          current.positionX - previous.positionX,
          current.positionY - previous.positionY,
        ),
        previous,
        current,
      });
    }
    const snapDelta = sameLife && Number.isInteger(previous.snapCount) &&
      Number.isInteger(current.snapCount)
      ? (current.snapCount - previous.snapCount) >>> 0
      : 0;
    if (
      snapDelta > 0 && snapDelta < 0x8000_0000 &&
      dtMs > 0
    ) {
      presentationSnaps.push({
        at: current.at,
        dtMs,
        lifeSeq: current.lifeSeq,
        snapDelta,
        snapReason: current.lastSnapReason ?? null,
        displayStep: Math.hypot(
          current.displayX - previous.displayX,
          current.displayY - previous.displayY,
        ),
        authorityStep: Math.hypot(
          current.positionX - previous.positionX,
          current.positionY - previous.positionY,
        ),
        previous,
        current,
      });
    }
    const sameSegment = current.lifeSeq === previous.lifeSeq &&
      current.snapCount === previous.snapCount &&
      current.initialCount === previous.initialCount;
    if (!sameSegment || dtMs <= 0 ||
        currentTimeline?.generation !== previousTimeline?.generation) sourceHistory.length = 0;
    rememberSource(current);
    if (!sameSegment || dtMs <= 0) {
      finishHold(previous);
      continue;
    }

    const authorityStep = Math.hypot(
      current.positionX - previous.positionX,
      current.positionY - previous.positionY,
    );
    const displayStep = Math.hypot(
      current.displayX - previous.displayX,
      current.displayY - previous.displayY,
    );
    maximumAuthorityStep = Math.max(maximumAuthorityStep, authorityStep);
    maximumDisplayStep = Math.max(maximumDisplayStep, displayStep);

    // A missing render interval cannot prove a continuously held sprite, but
    // its snap counter and displacement remain evidence. In particular, do
    // not leave 50–80 ms frames between the jump and render-stall assertions.
    const stationary = displayStep <= 0.05
      ? stationaryPresentationWitness(sourceHistory, previous, current)
      : null;
    if (dtMs > maximumFrameGapMs) {
      finishHold(previous);
    } else if (stationary != null) {
      finishHold(previous);
      if (authorityStep > 0.05) {
        stationaryTimelineFrames += 1;
        if (stationaryTimelineWitnesses.length < 16) stationaryTimelineWitnesses.push(stationary);
      }
    } else if (displayStep <= 0.05 && authorityStep > 0.05) {
      hold ||= {
        startedAt: previous.at,
        positionX: previous.positionX,
        positionY: previous.positionY,
        displayX: previous.displayX,
        displayY: previous.displayY,
        frames: [previous],
        frameCount: 1,
        lastFrameAt: previous.at,
      };
      retainHoldFrame(current);
    } else if (displayStep > 0.05) {
      finishHold(current);
    }

    const presentationTickDelta =
      Number.isInteger(previousTimeline?.presentationRenderTick) &&
      Number.isInteger(currentTimeline?.presentationRenderTick)
        ? (currentTimeline.presentationRenderTick -
          previousTimeline.presentationRenderTick) >>> 0
        : null;
    // CDP/WebDriver sampling is not synchronized with requestAnimationFrame:
    // two observations only 19 ms apart can include several genuinely drawn
    // recovery frames. Use the exact presentation-cursor advance when it is
    // available, rather than misclassifying valid bounded catch-up as a jump.
    const presentationAdvanceMs = presentationTickDelta != null && presentationTickDelta <= 32
      ? presentationTickDelta * 10
      : 0;
    const effectiveAdvanceMs = Math.max(dtMs, presentationAdvanceMs);
    const previousSpeed = Math.hypot(previous.velocityX || 0, previous.velocityY || 0);
    const currentSpeed = Math.hypot(current.velocityX || 0, current.velocityY || 0);
    const latestAuthorityExpectedStep = Math.max(
      authorityStep,
      ((previousSpeed + currentSpeed) * 0.5 * effectiveAdvanceMs) / 1_000,
    );
    const sourceMotion = presentationMotionWitness(sourceHistory, previous, current);
    const expectedStep = sourceMotion == null
      ? latestAuthorityExpectedStep
      : sourceMotion.maximumSpeed * effectiveAdvanceMs / 1_000;
    const jumpLimit = Math.max(minimumJumpPixels, expectedStep * 2 + 4);
    if (sourceMotion != null) {
      presentationMotionFrames += 1;
      const latestAuthorityJumpLimit = Math.max(minimumJumpPixels, latestAuthorityExpectedStep * 2 + 4);
      if ((displayStep > jumpLimit) !== (displayStep > latestAuthorityJumpLimit) &&
          presentationMotionWitnesses.length < 16) {
        presentationMotionWitnesses.push({
          ...sourceMotion, displayStep, expectedStep, jumpLimit,
          latestAuthorityExpectedStep, latestAuthorityJumpLimit,
        });
      }
    }
    if (displayStep > jumpLimit) {
      excessiveJumps.push({
        at: current.at,
        dtMs,
        displayStep,
        authorityStep,
        presentationTickDelta,
        effectiveAdvanceMs,
        expectedStep,
        jumpLimit,
        sourceMotion,
        previous,
        current,
      });
    }
  }
  if (finite.length > 0) finishHold(finite.at(-1));

  return {
    ok: finite.length >= 2 && materialHolds.length === 0 &&
      excessiveJumps.length === 0 && presentationSnaps.length === 0 &&
      (!rejectRenderFrameGaps || renderFrameGaps.length === 0),
    sampleCount: finite.length,
    maximumDisplayStep,
    maximumAuthorityStep,
    materialHolds,
    excessiveJumps,
    presentationSnaps,
    renderFrameGaps,
    stationaryTimelineFrames,
    stationaryTimelineWitnesses,
    presentationMotionFrames,
    presentationMotionWitnesses,
  };
}

// Uninterrupted feel windows keep the strict cadence and hold checks. Only
// the existing exact committed-door witnesses can explain receipt/display
// discontinuities; retain both raw findings and the matching server facts.
export function remoteActorMotionWithDoorEvidence(samples, playerId, doorRelocations = []) {
  const raw = remoteActorMotionSmoothnessEvidence(samples);
  const authorizedDoorRelocations = [];
  const presentationSnaps = raw.presentationSnaps.filter(finding => {
    const proof = continuousDoorReceipt(finding, playerId, doorRelocations);
    if (!proof) return true;
    authorizedDoorRelocations.push(proof); return false;
  });
  const excessiveJumps = raw.excessiveJumps.filter(finding => {
    const proof = continuousDoorDisplayJump(finding, playerId, doorRelocations);
    if (!proof) return true;
    authorizedDoorRelocations.push(proof); return false;
  });
  return { ...raw, ok: raw.sampleCount >= 2 && raw.materialHolds.length === 0 &&
    raw.renderFrameGaps.length === 0 && presentationSnaps.length === 0 && excessiveJumps.length === 0,
    observedPresentationSnaps: raw.presentationSnaps, observedExcessiveJumps: raw.excessiveJumps,
    presentationSnaps, excessiveJumps, authorizedDoorRelocations };
}

// Join the browser's bounded presentation rings captured at several gameplay
// milestones. Gaps between retained rings remain gaps; they cannot prove a
// visible discontinuity. Every continuously sampled same-life interval does
// retain the exact snap counter/reason and observer schedule boundary, so an
// otherwise rare Repel, wall, combat, or respawn-adjacent jump cannot hide in
// the much shorter startup-only feel window.
export function observedActorSourceTick(positionTick, timeline) {
  // Remote actor packets carry a u16 position tick; the applied snapshot and
  // shared presentation cursor carry u32 ticks. Expand only against that
  // already-observed snapshot, never the wall clock or a future render frame.
  // A source at least half a short-tick range old (including an apparent
  // future source) cannot establish an unambiguous historical bracket.
  const anchor = timeline?.gameAppliedServerTick;
  if (!Number.isInteger(positionTick) || positionTick < 0 || positionTick > 0xffff ||
      !Number.isInteger(anchor) || anchor < 0 || anchor > 0xffff_ffff) return undefined;
  const age = (anchor - positionTick) & 0xffff;
  return age < 0x8000 ? (anchor - age) >>> 0 : undefined;
}

export function botDoorRelocationEvidence(logs) {
  const events = [];
  for (const line of logLines(logs)) {
    if (!line.includes("door_overlap_bot_relocated") && !line.includes("bot_warp_prize_relocated")) continue;
    const { fields } = serverLogRecord(line);
    let event;
    if (fields) {
      if (!["door_overlap_bot_relocated", "bot_warp_prize_relocated"].includes(fields.event)) continue;
      event = { playerId: fields.player_id, lifeSeq: fields.life_seq,
        serverTick: fields.server_tick, destinationX: fields.destination_x,
        destinationY: fields.destination_y };
    } else {
      // Match the committed tracing record, never a quoted message containing
      // similar field text. Duplicate records cannot prove one unique warp.
      const match = line.match(/^(?:\S+\s+)?INFO\s+(?:server bot Warp prize relocation )?event="(?:door_overlap_bot_relocated|bot_warp_prize_relocated)" player_id=(\d+) life_seq=(\d+) server_tick=(\d+) destination_x=(\S+) destination_y=(\S+)(?: collectible_id=\d+ prize_id=-?\d+)?\s*$/);
      if (!match) continue;
      event = { playerId: Number(match[1]), lifeSeq: Number(match[2]),
        serverTick: Number(match[3]), destinationX: Number(match[4]),
        destinationY: Number(match[5]) };
    }
    if (validDoorRelocation(event)) events.push(event);
  }
  return events;
}

function validDoorRelocation(event) {
  return Number.isInteger(event?.playerId) && event.playerId >= 0 && event.playerId <= 0xffff &&
    Number.isInteger(event.lifeSeq) && event.lifeSeq > 0 && event.lifeSeq <= 0xffff_ffff &&
    Number.isInteger(event.serverTick) && event.serverTick >= 0 && event.serverTick <= 0xffff_ffff &&
    Number.isFinite(event.destinationX) && Number.isFinite(event.destinationY);
}

function milestoneDoorRelocation(snap, botId, events) {
  const { previous, current } = snap;
  // The warp frames were outside the retained rings. Require one Flash
  // increment per unique committed relocation over a missing interval; continuous windows keep their
  // original strict checks. The full raw snap and gap remain in the result.
  if (snap.dtMs < 80 || !Number.isInteger(snap.snapDelta) ||
      snap.snapDelta < 1 || snap.snapDelta > 64 || snap.snapReason !== 3 ||
      !Number.isInteger(previous.initialCount) || previous.initialCount < 0 ||
      previous.initialCount !== current.initialCount ||
      !Number.isInteger(previous.combatViewTimeline?.generation) ||
      previous.combatViewTimeline.generation !== current.combatViewTimeline?.generation ||
      ![previous.positionTick, current.positionTick].every(tick =>
        Number.isInteger(tick) && tick >= 0 && tick <= 0xffff_ffff) ||
      ![previous.positionSnapshotId, current.positionSnapshotId].every(id =>
        Number.isInteger(id) && id > 0 && id <= 0xffff)) return null;
  const span = (current.positionTick - previous.positionTick) >>> 0;
  const snapshotSpan = (current.positionSnapshotId - previous.positionSnapshotId) & 0xffff;
  if (span === 0 || span >= 0x8000_0000 || snapshotSpan === 0 || snapshotSpan >= 0x8000) return null;
  const matching = events.filter(event => {
    if (!validDoorRelocation(event) || event.playerId !== botId || event.lifeSeq !== snap.lifeSeq) return false;
    const offset = (event.serverTick - previous.positionTick) >>> 0;
    return offset > 0 && offset <= span;
  });
  return matching.length === snap.snapDelta &&
    new Set(matching.map(event => event.serverTick)).size === matching.length
    ? { snap, serverRelocation: matching[0], serverRelocations: matching } : null;
}

export function remoteActorMotionAcrossStatesEvidence(states, botIds, { doorRelocations = [] } = {}) {
  const spriteFrames = (states || []).flatMap(state => [...(state?.remoteSpriteTrace || []), ...(state?.motionBoundarySprites || [])]);
  const selectedBotIds = [...new Set(
    (botIds || []).filter((botId) => Number.isInteger(botId)),
  )].sort((left, right) => left - right);
  let frames = (states || [])
    .flatMap((state) => [...(state?.metricsTrace || []), ...(state?.motionBoundaryTrace || [])])
    .filter((frame) => Number.isFinite(frame?.at));
  const monotonic = frames.some(frame => frame.monotonicMs !== undefined ||
    frame.renderOrdinal !== undefined);
  if (monotonic) {
    // The browser already publishes both fields. Wall time can step backwards
    // during a run; sorting by it interleaves different moments and fabricates
    // jumps. Keep the wall timestamp for correlation, but measure elapsed time
    // and order with the clock and identity of the actual rendered frame.
    const unique = new Map();
    for (const frame of frames) {
      if (!Number.isFinite(frame.monotonicMs) || frame.monotonicMs < 0 ||
          !Number.isSafeInteger(frame.renderOrdinal) || frame.renderOrdinal < 0) {
        throw new Error("incomplete monotonic presentation frame identity");
      }
      const previous = unique.get(frame.renderOrdinal);
      if (previous && JSON.stringify(previous) !== JSON.stringify(frame)) {
        throw new Error("conflicting presentation frame identity");
      }
      unique.set(frame.renderOrdinal, frame);
    }
    frames = [...unique.values()].sort((left, right) => left.renderOrdinal - right.renderOrdinal);
    for (let index = 1; index < frames.length; index++) {
      if (frames[index].monotonicMs < frames[index - 1].monotonicMs) {
        throw new Error("presentation monotonic clock moved backwards");
      }
    }
    frames = frames.map(frame => ({ ...frame, wallAt: frame.at, at: frame.monotonicMs }));
  } else {
    // Retained historical fixtures predate the monotonic fields. They remain
    // readable, without inventing clocks for old captures.
    frames.sort((left, right) => left.at - right.at);
  }
  const bots = selectedBotIds.map((botId) => {
    const samplesByLife = new Map();
    const seenFrames = new Set();
    for (const frame of frames) {
      const player = (frame.remotePlayers || []).find((entry) => entry.id === botId);
      const presentation = (frame.remoteActorPresentation || [])
        .find((entry) => entry.id === botId);
      if (!player || !Number.isInteger(player.lifeSeq)) continue;
      const frameKey = `${monotonic ? frame.renderOrdinal : frame.at}:${player.lifeSeq}`;
      if (seenFrames.has(frameKey)) continue;
      seenFrames.add(frameKey);
      const samples = samplesByLife.get(player.lifeSeq) || [];
      samples.push({
        at: frame.at,
        ...(monotonic ? { wallAt: frame.wallAt, renderOrdinal: frame.renderOrdinal } : {}),
        lifeSeq: player.lifeSeq,
        snapCount: presentation?.snapCount,
        initialCount: presentation?.initialCount,
        lastSnapReason: presentation?.lastSnapReason,
        positionX: player.positionX,
        positionY: player.positionY,
        positionTick: observedActorSourceTick(player.lastServerPositionTick, frame.combatViewTimeline),
        positionSnapshotId: presentation?.lastSeenSnapshotId,
        displayX: player.displayX,
        displayY: player.displayY,
        velocityX: player.velocityX,
        velocityY: player.velocityY,
        combatViewTimeline: frame.combatViewTimeline || null,
      });
      samplesByLife.set(player.lifeSeq, samples);
    }
    const lives = [...samplesByLife.entries()]
      .map(([lifeSeq, samples]) => {
        const raw = remoteActorMotionSmoothnessEvidence(samples, {
          rejectRenderFrameGaps: false,
        });
        const authorizedDoorRelocations = [];
        const unexpected = raw.presentationSnaps.filter(snap => {
          const authorized = milestoneDoorRelocation(snap, botId, doorRelocations) ||
            continuousDoorReceipt(snap, botId, doorRelocations) ||
            reenteredDoorReceipt(snap, botId, doorRelocations, frames);
          if (!authorized) return true;
          authorizedDoorRelocations.push(authorized);
          return false;
        });
        const authorizedOffscreenFindings = [];
        const unexplainedHolds = raw.materialHolds.filter(finding => {
          const authorized = offscreenMotionFinding(finding, samples, botId, spriteFrames);
          if (!authorized) return true;
          authorizedOffscreenFindings.push(authorized);
          return false;
        });
        const unexplainedJumps = raw.excessiveJumps.filter(finding => {
          const door = continuousDoorDisplayJump(finding, botId, doorRelocations);
          if (door) { authorizedDoorRelocations.push(door); return false; }
          const authorized = offscreenMotionFinding(finding, samples, botId, spriteFrames);
          if (!authorized) return true;
          authorizedOffscreenFindings.push(authorized);
          return false;
        });
        return {
          lifeSeq,
          sampleCount: samples.length,
          // These milestone rings are captured around WebDriver/CDP reads that
          // can pause the instrumented browser. Retain their frame gaps as
          // evidence, but reserve the hard frame-cadence assertion for the
          // uninterrupted passive feel window in each real-browser scenario.
          smoothness: {
            ...raw,
            ok: raw.sampleCount >= 2 && unexplainedHolds.length === 0 &&
              unexplainedJumps.length === 0 && unexpected.length === 0,
            observedMaterialHolds: raw.materialHolds,
            observedExcessiveJumps: raw.excessiveJumps,
            materialHolds: unexplainedHolds,
            excessiveJumps: unexplainedJumps,
            authorizedOffscreenFindings,
            observedPresentationSnaps: raw.presentationSnaps,
            presentationSnaps: unexpected,
            authorizedDoorRelocations,
          },
        };
      })
      .filter((life) => life.sampleCount >= 2);
    return {
      botId,
      sampleCount: lives.reduce((total, life) => total + life.sampleCount, 0),
      lives,
      failedLives: lives.filter((life) => !life.smoothness.ok),
    };
  });
  return {
    ok: bots.length > 0 && bots.every((bot) =>
      bot.sampleCount >= 2 && bot.failedLives.length === 0
    ),
    capturedStateCount: states?.length || 0,
    frameCount: frames.length,
    bots,
  };
}

export function sustainedExactLifeAttackEvidence(
  damageEvents,
  {
    botId,
    lifeSeqs,
    minimumHits = 2,
  },
) {
  if (!Number.isInteger(botId) || botId < 0) return null;
  if (!Array.isArray(lifeSeqs) || lifeSeqs.length === 0) return null;
  if (!Number.isInteger(minimumHits) || minimumHits < 2) return null;

  const presentedLives = new Set(lifeSeqs.filter((lifeSeq) =>
    Number.isInteger(lifeSeq) && lifeSeq > 0
  ));
  const hitsByLife = new Map();
  for (const event of damageEvents || []) {
    if (
      event?.shooterId !== botId ||
      !presentedLives.has(event.shooterLifeSeq) ||
      !Number.isInteger(event.damageSeq) ||
      !Number.isInteger(event.serverTick)
    ) continue;
    const hits = hitsByLife.get(event.shooterLifeSeq) || [];
    if (!hits.some((hit) => hit.damageSeq === event.damageSeq)) hits.push(event);
    hitsByLife.set(event.shooterLifeSeq, hits);
  }

  for (const [lifeSeq, unorderedHits] of hitsByLife) {
    const hits = [...unorderedHits].sort((left, right) =>
      left.serverTick - right.serverTick || left.damageSeq - right.damageSeq
    );
    const lethalHits = hits.filter((hit) => hit.lethal === true);
    if (hits.length < minimumHits || lethalHits.length === 0) continue;
    return {
      botId,
      lifeSeq,
      hitCount: hits.length,
      lethalHitCount: lethalHits.length,
      firstServerTick: hits[0].serverTick,
      lastServerTick: hits.at(-1).serverTick,
      damageSeqs: hits.map((hit) => hit.damageSeq),
      lethalDamageSeqs: lethalHits.map((hit) => hit.damageSeq),
    };
  }
  return null;
}

export function timedTraceContinuity(
  samples,
  {
    minimumSpanMs,
    maximumSampleGapMs = 250,
  } = {},
) {
  if (!Number.isFinite(minimumSpanMs) || minimumSpanMs <= 0 || samples.length < 2) {
    return {
      ok: false,
      sampleCount: samples.length,
      spanMs: 0,
      maximumGapMs: null,
    };
  }
  let maximumGapMs = 0;
  for (let index = 1; index < samples.length; index += 1) {
    maximumGapMs = Math.max(maximumGapMs, samples[index].at - samples[index - 1].at);
  }
  const spanMs = samples.at(-1).at - samples[0].at;
  return {
    ok: spanMs >= minimumSpanMs && maximumGapMs <= maximumSampleGapMs,
    sampleCount: samples.length,
    spanMs,
    maximumGapMs,
  };
}

export function longestTimedTraceSpan(
  samples,
  { maximumSampleGapMs = 250 } = {},
) {
  if (samples.length < 2) return 0;
  let longestSpanMs = 0;
  let segmentStartedAt = samples[0].at;
  for (let index = 1; index < samples.length; index += 1) {
    if (samples[index].at - samples[index - 1].at > maximumSampleGapMs) {
      segmentStartedAt = samples[index].at;
      continue;
    }
    longestSpanMs = Math.max(longestSpanMs, samples[index].at - segmentStartedAt);
  }
  return longestSpanMs;
}

function u32IsEqualOrStrictlyNewer(candidate, predecessor) {
  if (!Number.isInteger(candidate) || !Number.isInteger(predecessor)) return false;
  const distance = (candidate - predecessor + 0x1_0000_0000) % 0x1_0000_0000;
  return distance < 0x8000_0000;
}

export function presentedLiveBotSuccessor(
  metrics,
  playerId,
  predecessorLifeSeq,
) {
  return (metrics?.remotePlayers || []).find((remote) =>
    remote.id === playerId &&
    Number.isInteger(remote.lifeSeq) &&
    remote.lifeSeq !== predecessorLifeSeq &&
    u32IsEqualOrStrictlyNewer(remote.lifeSeq, predecessorLifeSeq) &&
    remote.energy > 0 &&
    actorForLife(metrics, playerId, remote.lifeSeq) != null
  ) || null;
}

// Optional encounter setup for a declared spawn fixture. After a long bomb
// flight the pilot can be several thousand pixels from the bot's respawn.
// Navigate with ordinary keys until the browser actually admits the new life;
// this never supplies a server position or substitutes for the timing audit.
export async function prepareBotSuccessorObservation({ actor, playerId, predecessorLifeSeq,
  searchPoint, timeoutMs = 45_000, navigate = navigateAuthoredTarget,
  brake = brakeClientVelocity }) {
  if (searchPoint == null) return null;
  if (![searchPoint.x, searchPoint.y].every(Number.isFinite) ||
      searchPoint.x < 0 || searchPoint.y < 0 || searchPoint.x >= 16384 || searchPoint.y >= 16384 ||
      !Number.isFinite(timeoutMs) || timeoutMs <= 0 || timeoutMs > 45_000)
    throw new Error('invalid bounded bot successor observation setup');
  const read = async () => (await actor.metricsState()).uiMetrics || {};
  const ready = metrics => presentedLiveBotSuccessor(metrics, playerId, predecessorLifeSeq) != null;
  if (ready(await read())) return { needed: false };
  const deadline = Date.now() + timeoutMs;
  const remaining = () => {
    const left = deadline - Date.now();
    if (left <= 0) throw new Error('bot successor observation setup exceeded its budget');
    return left;
  };
  const routes = [], originRecoveries = [];
  while (!ready(await read())) {
    const before = await read();
    if (before.playerAlive !== true || before.serverPlayerAlive !== true) {
      await waitUntil('live pilot before bot successor observation setup', async () => {
        const m=await read();return m.playerAlive === true && m.serverPlayerAlive === true;
      }, Math.min(15_000,remaining()));
      continue;
    }
    try {
      routes.push(await navigate(actor, searchPoint, null, {
        timeoutMs: remaining(), arrivalRadiusPx: 64, maximumDesiredSpeed: 250,
        complete: async () => ready(await read()),
      }));
      // Reaching the search point alone proves nothing. Keep the original
      // presentation wait responsible for a still-missing actor.
      break;
    } catch (error) {
      if (error.message === 'authored approach origin lacks conservative hull clearance' &&
          originRecoveries.length === 0 && typeof actor.prepareBotObservationOrigin === 'function') {
        originRecoveries.push(await actor.prepareBotObservationOrigin(deadline));
        remaining();
        continue;
      }
      const after=await read();
      if (error.message !== 'authored navigation crossed a lifecycle boundary' ||
          (after.playerLifeSeq === before.playerLifeSeq && after.playerAlive === true && after.serverPlayerAlive === true)) throw error;
      remaining();
    }
  }
  const beforeBrake=await read();
  const braking=beforeBrake.playerAlive === true && beforeBrake.serverPlayerAlive === true
    ? await brake(actor,Math.min(3_000,remaining())) : null;
  return { needed:true,keysOnly:true,searchPoint,routes,originRecoveries,braking };
}

export async function waitForPresentedLiveBotSuccessor({
  actor,
  playerId,
  predecessorLifeSeq,
  label,
  timeoutMs = 10_000,
}) {
  await prepareBotSuccessorObservation({ actor, playerId, predecessorLifeSeq,
    searchPoint: actor.botRespawnSearchPoint });
  let successor = null;
  await waitUntil(`${label} browser presents a live successor bot life`, async () => {
    const state = await actor.metricsState();
    successor = presentedLiveBotSuccessor(
      state.uiMetrics || {},
      playerId,
      predecessorLifeSeq,
    );
    return successor != null;
  }, timeoutMs);
  return successor;
}

function matchingDamageTrace(state, event) {
  return (state.damageTrace || []).find((entry) => damageTraceMatches(entry, event)) || null;
}

function matchingDamagePresentationTrace(state, event) {
  return (state.damagePresentationTrace || []).find((entry) => {
    const damage = entry?.damage;
    return damage?.damageSeq === event.damageSeq &&
      damage?.projectileId === event.projectileId &&
      damage?.serverTick === event.serverTick &&
      damage?.victimId === event.victimId &&
      damage?.victimLifeSeq === event.victimLifeSeq &&
      damage?.attackerId === event.shooterId &&
      damage?.shooterLifeSeq === event.shooterLifeSeq &&
      (damage?.weapon & 0x1f) === (event.weapon & 0x1f) &&
      damage?.amount === event.damage &&
      Math.abs(damage?.energyBefore - event.energyBefore) <= 1 &&
      Math.abs(damage?.energyAfter - event.energyAfter) <= 1 &&
      damage?.explicitLethal === event.lethal;
  }) || null;
}

export function exactLifeDamagePresentationEvidence(
  state,
  event,
  { requireEnergy = !event?.lethal } = {},
) {
  const trace = matchingDamageTrace(state, event);
  const witness = matchingDamagePresentationTrace(state, event);
  const currentSprite = witness?.sprite;
  const currentSpriteDrawn = currentSprite?.drawn === true &&
    currentSprite?.suppression === "none";
  const presentedSprite = currentSpriteDrawn
    ? currentSprite
    : witness?.drawnSprite;
  const presentedAt = currentSpriteDrawn
    ? witness?.at
    : witness?.drawnSpriteObservedAt;
  const presentationAgeMs = witness?.at - presentedAt;
  const presentationTickAge = continuumTickDiff(
    event?.serverTick,
    presentedSprite?.lastServerPositionTick,
  );
  if (
    trace == null ||
    witness == null ||
    presentedSprite?.drawn !== true ||
    presentedSprite?.suppression !== "none" ||
    presentedSprite?.id !== event.victimId ||
    presentedSprite?.lifeSeq !== event.victimLifeSeq ||
    // Nonlethal damage must be visible in the event's current frame. A
    // lethal terminal may use only a recent exact-life draw because removing
    // the dead actor is itself the correct terminal presentation.
    (!currentSpriteDrawn && event?.lethal !== true) ||
    (!currentSpriteDrawn && (
      !Number.isFinite(presentationAgeMs) ||
      presentationAgeMs < 0 ||
      presentationAgeMs > 1_000 ||
      presentationTickAge < 0 ||
      presentationTickAge > 100
    ))
  ) {
    return null;
  }
  if (!requireEnergy) {
    return { trace, witness, energySnapshot: null, presentedSprite, presentedAt };
  }

  const energySnapshot = witness.sameLifeEnergySnapshot;
  const elapsedTicks = continuumTickDiff(
    energySnapshot?.serverTick,
    event.serverTick,
  );
  if (
    energySnapshot?.playerId !== event.victimId ||
    energySnapshot?.lifeSeq !== event.victimLifeSeq ||
    !Number.isInteger(energySnapshot?.snapshotId) ||
    !Number.isFinite(energySnapshot?.energy) ||
    !(energySnapshot.energy < event.energyBefore) ||
    elapsedTicks < 0 ||
    elapsedTicks > 1_000
  ) {
    return null;
  }
  return { trace, witness, energySnapshot, presentedSprite, presentedAt };
}

export function presentedBotCombatCycleEvidence(
  state,
  cycleHits,
  lethalHit,
  { requireNonlethal = true, terminalCommitEvidence = null } = {},
) {
  const entries = cycleHits.map((event) => {
    const presentation = exactLifeDamagePresentationEvidence(state, event, {
      requireEnergy: false,
    });
    const energyPresentation = event.lethal === true ||
        !(event.energyAfter < event.energyBefore)
      ? null
      : exactLifeDamagePresentationEvidence(state, event, {
          requireEnergy: true,
        });
    return {
      event,
      evidence: energyPresentation ?? presentation,
    };
  });
  if (entries.some(({ evidence }) => evidence == null)) return null;

  const nonlethal = entries.filter(({ event }) => !event.lethal);
  // WATCH_DAMAGE presents every exact hit immediately, but replaceable
  // snapshots may legitimately coalesce multiple same-life hits that land in
  // one browser delivery interval. Ordinarily require one exact visible
  // snapshot decrement. A frozen server transaction may instead commit a
  // complete contiguous damage chain only after the last same-life snapshot,
  // leaving no intermediate state that a browser could render. Admit that
  // terminal form only when every exact event was presented in one bounded
  // adjacent-frame interval, the energy chain has no gaps, and the bounded
  // tick sequence ends at the selected explicit lethal zero. The event lane
  // is unordered, so an atomic same-tick transaction cannot require identical
  // browser arrival order or one identical render frame.
  const hasVisibleNonlethalEnergy = nonlethal.some(({ event, evidence }) =>
    event.energyAfter < event.energyBefore && evidence.energySnapshot != null
  );
  const terminalPresentationTimes = entries
    .map(({ evidence }) => evidence.witness?.at)
    .filter((at) => Number.isFinite(at));
  const terminalPresentationSpanMs = terminalPresentationTimes.length === entries.length
    ? Math.max(...terminalPresentationTimes) - Math.min(...terminalPresentationTimes)
    : Number.POSITIVE_INFINITY;
  const terminalCoalesced = requireNonlethal && !hasVisibleNonlethalEnergy &&
    terminalCommitEvidence?.snapshotBoundaryCount === 0 &&
    terminalCommitEvidence.damageSeqs?.length === entries.length &&
    entries.every(({ event }, index) =>
      terminalCommitEvidence.damageSeqs[index] === event.damageSeq &&
      terminalCommitEvidence.projectileIds[index] === event.projectileId
    ) &&
    entries.length >= 2 &&
    entries.at(-1)?.event.damageSeq === lethalHit.damageSeq &&
    entries.at(-1)?.event.lethal === true &&
    entries.at(-1)?.event.energyAfter === 0 &&
    Number.isInteger(entries.at(-1)?.evidence.witness?.snapshotId) &&
    Number.isFinite(entries.at(-1)?.evidence.witness?.at) &&
    terminalPresentationSpanMs <= ATOMIC_DAMAGE_PRESENTATION_MAX_SPAN_MS &&
    entries.every(({ event, evidence }, index) => {
      const previous = index === 0 ? null : entries[index - 1].event;
      return event.energyAfter < event.energyBefore &&
        evidence.energySnapshot == null &&
        (previous == null || (
          event.energyBefore === previous.energyAfter &&
          continuumTickDiff(event.serverTick, previous.serverTick) >= 0
        ));
    }) &&
    continuumTickDiff(
      entries.at(-1).event.serverTick,
      entries[0].event.serverTick,
    ) >= 0;
  if (requireNonlethal && !hasVisibleNonlethalEnergy && !terminalCoalesced) return null;

  const lethal = entries.find(({ event }) =>
    event.damageSeq === lethalHit.damageSeq
  );
  if (
    !lethal ||
    lethal.evidence.trace.explicitLethal !== true ||
    lethal.evidence.trace.energyAfter !== 0 ||
    lethal.event.energyAfter !== 0
  ) {
    return null;
  }
  return entries.map((entry) => ({
    ...entry,
    terminalCoalesced,
    terminalPresentationSpanMs: terminalCoalesced ? terminalPresentationSpanMs : null,
  }));
}

function actorForLife(metrics, playerId, lifeSeq) {
  return (metrics.authoritativeSnapshotActors || []).find((actor) =>
    actor.id === playerId && actor.lifeSeq === lifeSeq
  ) || null;
}

function remoteForLife(metrics, playerId, lifeSeq) {
  return (metrics.remotePlayers || []).find((player) =>
    player.id === playerId && player.lifeSeq === lifeSeq
  ) || null;
}

function retainedRespawnPresentation(state, playerId, lifeSeq, deathTick) {
  const compactCandidates = (state.remoteLifeTrace || []).flatMap((entry) => {
    if (
      entry.playerId !== playerId || entry.lifeSeq !== lifeSeq ||
      !Number.isInteger(entry.latestSnapshotServerTick)
    ) {
      return [];
    }
    return [{
      observedAt: entry.at,
      latestSnapshotServerTick: entry.latestSnapshotServerTick,
      elapsedTicks: continuumTickDiff(entry.latestSnapshotServerTick, deathTick),
      playerId: entry.playerId,
      lifeSeq: entry.lifeSeq,
      energy: entry.energy,
      serverEnergy: entry.serverEnergy,
    }];
  });
  const frameCandidates = (state.metricsTrace || []).flatMap((metrics) => {
    const actor = actorForLife(metrics, playerId, lifeSeq);
    const remote = remoteForLife(metrics, playerId, lifeSeq);
    const observedTick = metrics.latestSnapshotServerTick;
    if (!actor || !remote || !(remote.energy > 0) || !Number.isInteger(observedTick)) {
      return [];
    }
    return [{
      observedAt: metrics.at,
      latestSnapshotServerTick: observedTick,
      elapsedTicks: continuumTickDiff(observedTick, deathTick),
      playerId: remote.id,
      lifeSeq: remote.lifeSeq,
      energy: remote.energy,
      serverEnergy: remote.serverEnergy,
    }];
  });
  const candidates = [...compactCandidates, ...frameCandidates];
  candidates.sort((left, right) => left.elapsedTicks - right.elapsedTicks);
  return candidates[0] || null;
}

export function resolvePresentedRespawnChain(deaths, initialDeath, presentedLifeSeq) {
  let terminalDeath = initialDeath;
  let expectedLifeSeq = initialDeath.victimNextLifeSeq;
  const overtakenDeaths = [];
  const visitedLives = new Set();

  for (let depth = 0; depth <= MAX_OVERTAKEN_RESPAWN_LIVES; depth += 1) {
    if (presentedLifeSeq === expectedLifeSeq) {
      return {
        terminalDeath,
        presentedLifeSeq,
        overtakenDeaths,
      };
    }
    if (visitedLives.has(expectedLifeSeq)) {
      throw new Error(`bot respawn death chain contains a life cycle: ${JSON.stringify({
        initialDeath,
        presentedLifeSeq,
        expectedLifeSeq,
        overtakenDeaths,
      })}`);
    }
    visitedLives.add(expectedLifeSeq);

    const candidates = deaths.filter((event) =>
      event.victimId === initialDeath.victimId &&
      event.victimLifeSeq === expectedLifeSeq &&
      event.logIndex > terminalDeath.logIndex
    );
    if (candidates.length === 0) return null;
    if (candidates.length !== 1) {
      throw new Error(`bot respawn death chain is ambiguous: ${JSON.stringify({
        initialDeath,
        presentedLifeSeq,
        expectedLifeSeq,
        candidates,
      })}`);
    }
    const nextDeath = candidates[0];
    if (
      nextDeath.energyAfter !== 0 ||
      nextDeath.respawnDelayMs <= 0 ||
      nextDeath.victimNextLifeSeq === expectedLifeSeq ||
      !u32IsEqualOrStrictlyNewer(nextDeath.victimNextLifeSeq, expectedLifeSeq)
    ) {
      throw new Error(`bot respawn death chain contains an invalid transition: ${JSON.stringify({
        initialDeath,
        presentedLifeSeq,
        expectedLifeSeq,
        nextDeath,
      })}`);
    }
    overtakenDeaths.push(nextDeath);
    terminalDeath = nextDeath;
    expectedLifeSeq = nextDeath.victimNextLifeSeq;
  }

  throw new Error(`bot respawn death chain exceeded its bounded evidence window: ${JSON.stringify({
    initialDeath,
    presentedLifeSeq,
    overtakenDeaths,
    maxOvertakenLives: MAX_OVERTAKEN_RESPAWN_LIVES,
  })}`);
}

export function lethalHitWithNonlethalPrecursor(hits, preferredVictimId = null) {
  // The attack pilot can survive several unrelated bot lives before one
  // complete cycle ends its loop. Bind follow-up timing to the newest
  // qualifying lethal, not an older cycle that may already have respawned.
  for (let index = hits.length - 1; index >= 0; index -= 1) {
    const lethal = hits[index];
    if (
      lethal.lethal === true &&
      (preferredVictimId == null || lethal.victimId === preferredVictimId) &&
      hits.some((earlier) =>
        earlier.lethal !== true &&
        earlier.victimId === lethal.victimId &&
        earlier.victimLifeSeq === lethal.victimLifeSeq &&
        earlier.damageSeq !== lethal.damageSeq &&
        continuumTickDiff(lethal.serverTick, earlier.serverTick) >= 0 &&
        continuumTickDiff(lethal.serverTick, earlier.serverTick) <= 1_000 &&
        earlier.energyAfter < earlier.energyBefore
      )
    ) {
      return lethal;
    }
  }
  return null;
}

export function exactRemoteDeathExplosionWitness(metrics, death, baselineCount = 0) {
  const entries = metrics?.remoteShipExplosionWitnesses;
  if (!Array.isArray(entries) || entries.length > 64)
    throw new Error("missing or unbounded exact remote-death explosion history");
  const u32 = value => Number.isInteger(value) && value >= 0 && value <= 0xffffffff;
  if (![baselineCount, metrics.remoteShipExplosionCount].every(u32)) return null;
  const matches = entries.filter(entry => {
    if (!entry || ![entry.ordinal, entry.nextLifeSeq, entry.respawnAtTick, entry.greenCollectibleId].every(u32) ||
        !Number.isFinite(entry.x) || !Number.isFinite(entry.y) ||
        typeof entry.authoritySegment !== "string" || !/^[0-9]{1,20}$/.test(entry.authoritySegment) ||
        BigInt(entry.authoritySegment) > 0xffffffffffffffffn) return false;
    const newOrdinal = (entry.ordinal - baselineCount) >>> 0;
    const publishedOrdinal = (metrics.remoteShipExplosionCount - entry.ordinal) >>> 0;
    return entry.playerId === death.victimId && entry.nextLifeSeq === death.victimNextLifeSeq &&
      entry.greenCollectibleId === death.greenCollectibleId &&
      newOrdinal > 0 && newOrdinal < 0x80000000 && publishedOrdinal < 0x80000000;
  });
  // Duplicated or cross-generation deaths are not rescued by choosing the
  // nearest one. Exactly one admitted new effect must identify this death.
  return matches.length === 1 ? { ...matches[0] } : null;
}

export function killNotificationNamesPlayer(notification, playerName) {
  return typeof notification === "string" && typeof playerName === "string" && playerName.length > 0 &&
    notification.endsWith(` killed by: ${playerName}`);
}

export async function provePresentedBotCombatCycle({
  actor,
  server,
  playerId,
  playerName,
  hits,
  lethalHit,
  winsBefore,
  label,
  requireNonlethal = true,
  presentationBefore,
}) {
  if (typeof playerName !== "string" || playerName.length === 0) {
    throw new Error(`${label} requires the expected player handle for kill presentation`);
  }
  const cycleHits = hits.filter((event) =>
    event.shooterId === playerId &&
    event.victimId === lethalHit.victimId &&
    event.victimLifeSeq === lethalHit.victimLifeSeq &&
    continuumTickDiff(lethalHit.serverTick, event.serverTick) >= 0 &&
    continuumTickDiff(lethalHit.serverTick, event.serverTick) <= 1_000
  );
  if (cycleHits.length < 1 || !cycleHits.some((event) => event.damageSeq === lethalHit.damageSeq)) {
    throw new Error(`${label} server damage cycle did not contain the lethal event: ${JSON.stringify({
      lethalHit,
      cycleHits,
    })}`);
  }
  if (requireNonlethal && !cycleHits.some((event) => !event.lethal)) {
    throw new Error(`${label} had no nonlethal energy decrement before death: ${JSON.stringify(cycleHits)}`);
  }

  // A close bomb can leave the browser ship coasting at authored top speed
  // after the exact lethal transaction. The bot respawns near the arena
  // spawn, so an otherwise-idle observer can leave its legitimate AOI before
  // the configured respawn delay expires and then (correctly) receive an
  // out-of-scope actor tombstone instead of a drawable successor. Stabilize
  // the observer immediately with ordinary keyboard counter-thrust. This
  // does not manufacture the successor or extend its presentation deadline;
  // it keeps the browser in scope so the assertions below can require the
  // real snapshot admission and ship-sprite draw.
  const observerBeforeBrake = (await actor.metricsState()).uiMetrics || {};
  const respawnObserverBrake =
    observerBeforeBrake.playerAlive === true &&
    observerBeforeBrake.serverPlayerAlive === true
      ? await brakeClientVelocity(actor, 3_000)
      : {
          braked: false,
          reason: "observer lifecycle is not live after lethal transaction",
          metrics: observerBeforeBrake,
        };

  let damagePresentation = null;
  const terminalCommitEvidence = snapshotFreeDamageCommitEvidence(
    server.logs,
    cycleHits,
  );
  await waitUntil(`${label} browser presents exact server bot damage`, async () => {
    const state = await actor.metricsState();
    const entries = presentedBotCombatCycleEvidence(
      state,
      cycleHits,
      lethalHit,
      { requireNonlethal, terminalCommitEvidence },
    );
    if (entries == null) return false;
    damagePresentation = entries.map(({
      event,
      evidence,
      terminalCoalesced,
      terminalPresentationSpanMs,
    }) => ({
      damageSeq: event.damageSeq,
      projectileId: event.projectileId,
      serverTick: event.serverTick,
      victimId: event.victimId,
      victimLifeSeq: event.victimLifeSeq,
      weapon: event.weapon,
      damage: event.damage,
      energyBefore: event.energyBefore,
      energyAfter: event.energyAfter,
      lethal: event.lethal,
      browserRemoteEnergy: evidence.energySnapshot?.energy ?? null,
      browserRemoteEnergySnapshotId:
        evidence.energySnapshot?.snapshotId ?? null,
      browserRemoteEnergyServerTick:
        evidence.energySnapshot?.serverTick ?? null,
      browserExplicitLethal: evidence.trace.explicitLethal,
      browserObservedAt: evidence.witness.at,
      browserSpriteDrawn: evidence.presentedSprite.drawn,
      browserSpriteObservedAt: evidence.presentedAt,
      browserSpriteAuthoritativeX:
        evidence.witness.sprite?.authoritativeX ??
        evidence.presentedSprite.authoritativeX ?? null,
      browserSpriteAuthoritativeY:
        evidence.witness.sprite?.authoritativeY ??
        evidence.presentedSprite.authoritativeY ?? null,
      browserTerminalCoalesced: terminalCoalesced,
      browserTerminalPresentationSpanMs: terminalPresentationSpanMs,
      serverTerminalCommitEvidence: terminalCoalesced
        ? terminalCommitEvidence
        : null,
    }));
    return true;
  }, 10_000);

  const death = parseServerDeathEvents(server.logs).find((event) =>
    event.damageSeq === lethalHit.damageSeq &&
    event.victimId === lethalHit.victimId &&
    event.victimLifeSeq === lethalHit.victimLifeSeq &&
    event.killerId === playerId
  );
  if (!death || death.energyAfter !== 0 || death.respawnDelayMs <= 0) {
    throw new Error(`${label} has no exact structured server death/respawn contract: ${JSON.stringify({
      lethalHit,
      death: death || null,
    })}`);
  }

  let deathPresentation = null;
  // A five-second conjunction of six independent client-side witnesses fails today with only the
  // label, which is enough to see *that* presentation lagged and not enough to say which. Keep the
  // last evaluated conjunction so the timeout names the unmet witness instead of guessing.
  let lastPresentationConjunction = null;
  const describeConjunction = (state) => {
    const metrics = state.uiMetrics || {};
    const entries = damagePresentation.find((entry) => entry.damageSeq === lethalHit.damageSeq);
    const explosion = exactRemoteDeathExplosionWitness(metrics, death,
      presentationBefore?.remoteShipExplosionCount ?? 0);
    const explosionDistance = Math.hypot(
      explosion?.x - entries?.browserSpriteAuthoritativeX,
      explosion?.y - entries?.browserSpriteAuthoritativeY,
    );
    return {
      observedAt: Date.now(),
      deadActorStillPresented: !!actorForLife(metrics, lethalHit.victimId, lethalHit.victimLifeSeq),
      deadRemoteStillPresented: !!remoteForLife(metrics, lethalHit.victimId, lethalHit.victimLifeSeq),
      selfWins: metrics.selfWins ?? null,
      selfWinsRequired: winsBefore + 1,
      killNotificationCount: metrics.combatKillNotificationCount ?? null,
      killNotificationCountRequired: (presentationBefore?.combatKillNotificationCount ?? 0) + 1,
      killNotificationText: metrics.lastCombatKillNotification ?? null,
      expectedKillerName: playerName,
      killNotificationNamesExpectedPlayer: killNotificationNamesPlayer(metrics.lastCombatKillNotification, playerName),
      remoteExplosionCount: metrics.remoteShipExplosionCount ?? null,
      remoteExplosionCountRequired: (presentationBefore?.remoteShipExplosionCount ?? 0) + 1,
      explosionDistance: Number.isFinite(explosionDistance)
        ? Number(explosionDistance.toFixed(1)) : null,
      explosionDistanceLimit: 32,
      hasLethalPresentationEntry: !!entries,
      exactExplosionWitness: explosion,
    };
  };
  try {
    await waitUntil(`${label} browser omits the dead bot life and presents the score`, async () => {
      const state = await actor.metricsState();
      const metrics = state.uiMetrics || {};
      const oldActor = actorForLife(metrics, lethalHit.victimId, lethalHit.victimLifeSeq);
      const oldRemote = remoteForLife(metrics, lethalHit.victimId, lethalHit.victimLifeSeq);
      const notificationAdvanced =
        (metrics.combatKillNotificationCount ?? 0) >=
          (presentationBefore?.combatKillNotificationCount ?? 0) + 1 &&
        killNotificationNamesPlayer(metrics.lastCombatKillNotification, playerName);
      const explosionAdvanced =
        (metrics.remoteShipExplosionCount ?? 0) >=
          (presentationBefore?.remoteShipExplosionCount ?? 0) + 1;
      const lethalPresentation = damagePresentation.find((entry) =>
        entry.damageSeq === lethalHit.damageSeq
      );
      const explosion = exactRemoteDeathExplosionWitness(metrics, death,
        presentationBefore?.remoteShipExplosionCount ?? 0);
      const explosionDistance = Math.hypot(
        explosion?.x - lethalPresentation?.browserSpriteAuthoritativeX,
        explosion?.y - lethalPresentation?.browserSpriteAuthoritativeY,
      );
      lastPresentationConjunction = describeConjunction(state);
      if (
        oldActor || oldRemote || !(metrics.selfWins >= winsBefore + 1) ||
        !notificationAdvanced || !explosionAdvanced ||
        !Number.isFinite(explosionDistance) || explosionDistance > 32
      ) return false;
      deathPresentation = {
        observedAt: Date.now(),
        latestSnapshotServerTick: metrics.latestSnapshotServerTick,
        deadActorOmitted: true,
        deadRemoteOmitted: true,
        selfWins: metrics.selfWins,
        combatKillNotificationCount: metrics.combatKillNotificationCount,
        lastCombatKillNotification: metrics.lastCombatKillNotification,
        remoteShipExplosionCount: metrics.remoteShipExplosionCount,
        lastRemoteShipExplosionX: metrics.lastRemoteShipExplosionX,
        lastRemoteShipExplosionY: metrics.lastRemoteShipExplosionY,
        exactExplosionWitness: explosion,
        exactLifeExplosionDistance: explosionDistance,
      };
      return true;
    }, 5_000);
  } catch (error) {
    throw new Error(`${error.message}: last conjunction ` +
      `${JSON.stringify(lastPresentationConjunction)}`, { cause: error });
  }

  const respawnObserverReacquisition = await prepareBotSuccessorObservation({ actor,
    playerId: lethalHit.victimId, predecessorLifeSeq: lethalHit.victimLifeSeq,
    searchPoint: actor.botRespawnSearchPoint });
  let respawnPresentation = null;
  await waitUntil(`${label} browser presents the configured newer bot life`, async () => {
    const state = await actor.metricsState();
    const metrics = state.uiMetrics || {};
    const liveCandidates = (metrics.remotePlayers || []).filter((remote) =>
      remote.id === lethalHit.victimId &&
      remote.energy > 0 &&
      actorForLife(metrics, lethalHit.victimId, remote.lifeSeq) != null
    );
    let chain = null;
    let remote = null;
    const currentDeaths = parseServerDeathEvents(server.logs);
    for (const candidate of liveCandidates) {
      const candidateChain = resolvePresentedRespawnChain(
        currentDeaths,
        death,
        candidate.lifeSeq,
      );
      if (candidateChain != null) {
        chain = candidateChain;
        remote = candidate;
        break;
      }
    }
    if (chain == null || remote == null) return false;
    const observedTick = metrics.latestSnapshotServerTick;
    if (!Number.isInteger(observedTick)) return false;
    const elapsedTicks = continuumTickDiff(observedTick, chain.terminalDeath.serverTick);
    const configuredTicks = Math.ceil(chain.terminalDeath.respawnDelayMs / 10);
    let observed = {
      observedAt: Date.now(),
      latestSnapshotServerTick: observedTick,
      elapsedTicks,
      playerId: remote.id,
      lifeSeq: remote.lifeSeq,
      energy: remote.energy,
      serverEnergy: remote.serverEnergy,
      evidenceSource: "current_metrics",
    };
    if (elapsedTicks > configuredTicks + BOT_RESPAWN_PRESENTATION_SLACK_TICKS) {
      const retainedState = typeof actor.presentationState === "function"
        ? await actor.presentationState()
        : typeof actor.pageState === "function"
          ? await actor.pageState()
          : null;
      const retained = retainedState != null
        ? retainedRespawnPresentation(
            retainedState,
            lethalHit.victimId,
            chain.presentedLifeSeq,
            chain.terminalDeath.serverTick,
          )
        : null;
      if (retained != null) {
        observed = { ...retained, evidenceSource: "retained_metrics_trace" };
      }
    }
    const aoiEligibility = observed.elapsedTicks > configuredTicks + BOT_RESPAWN_PRESENTATION_SLACK_TICKS
      ? outsideAoiRespawnEvidence(server.logs, chain.terminalDeath, observed, playerId,
          BOT_RESPAWN_PRESENTATION_SLACK_TICKS)
      : null;
    if (
      observed.elapsedTicks < configuredTicks ||
      (observed.elapsedTicks > configuredTicks + BOT_RESPAWN_PRESENTATION_SLACK_TICKS && !aoiEligibility)
    ) {
      throw new Error(`${label} bot respawn presentation violated its configured deadline: ${JSON.stringify({
        initialDeath: death,
        terminalDeath: chain.terminalDeath,
        overtakenDeaths: chain.overtakenDeaths,
        observed,
        configuredTicks,
      })}`);
    }
    respawnPresentation = {
      ...observed,
      configuredTicks,
      presentationSlackTicks: observed.elapsedTicks - configuredTicks,
      aoiEligibility,
      initialExpectedLifeSeq: death.victimNextLifeSeq,
      overtakenDeaths: chain.overtakenDeaths,
      terminalDeath: chain.terminalDeath,
    };
    return true;
  }, Math.max(10_000, death.respawnDelayMs + 5_000));

  // `remotePlayers` and snapshot membership prove state admission, but not
  // that the renderer ever enqueued the exact respawn-life ship texture. Bind
  // the combat cycle to the actual sprite trace so a bot cannot die, respawn,
  // and resume authoritative fire as an invisible actor.
  const respawnSpriteState = typeof actor.presentationState === "function"
    ? await actor.presentationState()
    : await actor.pageState();
  const respawnSpriteCandidates = (respawnSpriteState.remoteSpriteTrace || [])
    .flatMap((frame) => (frame.entries || [])
      .filter((entry) =>
        entry.id === respawnPresentation.playerId &&
        entry.lifeSeq === respawnPresentation.lifeSeq
      )
      .map((entry) => ({ at: frame.at, entry })))
    .sort((left, right) => right.at - left.at);
  const respawnSprite = respawnSpriteCandidates.find(({ entry }) =>
    entry.authoritativeMember === true &&
    entry.drawn === true &&
    entry.suppression === "none" &&
    entry.alpha > 0 &&
    entry.positionPresent === true &&
    entry.renderPositionPresent === true
  );
  if (respawnSprite == null) {
    throw new Error(`${label} admitted a respawned bot life without an actual ship sprite draw: ${JSON.stringify({
      respawnPresentation,
      candidates: respawnSpriteCandidates.slice(0, 12),
    })}`);
  }
  respawnPresentation.sprite = {
    observedAt: respawnSprite.at,
    playerId: respawnSprite.entry.id,
    lifeSeq: respawnSprite.entry.lifeSeq,
    screenX: respawnSprite.entry.screenX,
    screenY: respawnSprite.entry.screenY,
    shipFrame: respawnSprite.entry.shipFrame,
    alpha: respawnSprite.entry.alpha,
  };

  return {
    label,
    victimId: lethalHit.victimId,
    victimLifeSeq: lethalHit.victimLifeSeq,
    nextLifeSeq: respawnPresentation.lifeSeq,
    winsBefore,
    winsAfter: deathPresentation.selfWins,
    damagePresentation,
    death,
    deathPresentation,
    respawnPresentation,
    respawnObserverBrake,
    respawnObserverReacquisition,
  };
}

export function postRespawnDamageEvent({
  hits,
  playerId,
  victimId,
  victimLifeSeq,
}) {
  const eligible = hits.filter((candidate) =>
    candidate.shooterId === playerId &&
    candidate.victimId === victimId &&
    // Bot-v-bot combat may kill the just-respawned life before the browser
    // pilot can reach it. Accept that exact life or any wrap-safe newer life,
    // while retaining the exact shooter/victim/damage identity below.
    u32IsEqualOrStrictlyNewer(candidate.victimLifeSeq, victimLifeSeq) &&
    candidate.energyAfter < candidate.energyBefore
  );
  // Prefer the newest exact lethal presentation when an impaired delivery
  // coalesces the whole same-life burst after the last available snapshot.
  // A lethal damage event plus the exact drawn predecessor is independently
  // presentable; selecting the first nonlethal hit would demand an
  // intermediate energy frame that never existed. Ordinary nonlethal-only
  // paths still retain the exact snapshot decrement requirement below.
  return [...eligible].reverse().find((candidate) => candidate.lethal === true) ??
    eligible[0];
}

export async function provePostRespawnBotDamage({
  actor,
  hits,
  playerId,
  victimId,
  victimLifeSeq,
  label,
}) {
  const event = postRespawnDamageEvent({ hits, playerId, victimId, victimLifeSeq });
  if (!event) {
    throw new Error(`${label} produced no damage against the respawned bot life: ${JSON.stringify({
      victimId,
      victimLifeSeq,
      hits,
    })}`);
  }
  let evidence = null;
  await waitUntil(`${label} browser presents post-respawn bot damage`, async () => {
    const state = await actor.metricsState();
    evidence = exactLifeDamagePresentationEvidence(state, event);
    return evidence != null;
  }, 10_000);
  return {
    event,
    browser: {
      observedAt: evidence.witness.at,
      remoteEnergy: evidence.energySnapshot?.energy ?? evidence.trace.energyAfter,
      remoteEnergySnapshotId: evidence.energySnapshot?.snapshotId ?? null,
      remoteEnergyServerTick: evidence.energySnapshot?.serverTick ?? null,
      explicitLethal: evidence.trace.explicitLethal,
      spriteDrawn: evidence.witness.sprite.drawn,
    },
  };
}

// A mobile hostile bot that parks on an authored safe tile cannot be bombed, and a pilot parked
// deep in the central reserve cannot reach it either: the safe->unsafe staging solve rejects at
// `crossingMaximumDistance`. Both lanes need the same bait-and-retreat to make such an encounter
// stageable, so it lives here and both lanes drive it with their own tile predicates.
const EXTREME_SAFETY_LURE_WAYPOINTS = [
  { x: 8_720, y: 8_200 },
  { x: 8_720, y: 7_840 },
];

export function nearestAuthoredSafetyEdge(safety, metrics) {
  const {
    authoredTiles,
    positionIsOnAuthoredSafeTile,
    authoredTileIsSolid,
    authoredHullIsOpen,
    authoredLineIsClear,
  } = safety;
  if (!Number.isFinite(metrics.playerX) || !Number.isFinite(metrics.playerY)) return null;
  const pilotTileX = Math.floor(metrics.playerX / 16);
  const pilotTileY = Math.floor(metrics.playerY / 16);
  const directions = [
    { dx: -1, dy: 0 },
    { dx: 1, dy: 0 },
    { dx: 0, dy: -1 },
    { dx: 0, dy: 1 },
  ];
  const entries = [];
  for (let tileY = Math.max(0, pilotTileY - 40); tileY <= Math.min(1_023, pilotTileY + 40); tileY += 1) {
    for (let tileX = Math.max(0, pilotTileX - 40); tileX <= Math.min(1_023, pilotTileX + 40); tileX += 1) {
      if (!positionIsOnAuthoredSafeTile(tileX * 16 + 8, tileY * 16 + 8)) continue;
      for (const direction of directions) {
        const outsideTileX = tileX + direction.dx;
        const outsideTileY = tileY + direction.dy;
        if (
          outsideTileX < 0 || outsideTileX >= 1_024 ||
          outsideTileY < 0 || outsideTileY >= 1_024
        ) continue;
        const outsideTile = authoredTiles[outsideTileY * 1_024 + outsideTileX];
        if (
          positionIsOnAuthoredSafeTile(outsideTileX * 16 + 8, outsideTileY * 16 + 8) ||
          authoredTileIsSolid(outsideTile)
        ) continue;
        const inside = { x: tileX * 16 + 8, y: tileY * 16 + 8 };
        const outside = { x: outsideTileX * 16 + 8, y: outsideTileY * 16 + 8 };
        if (!authoredHullIsOpen(outside.x, outside.y)) continue;
        if (!authoredLineIsClear(metrics.playerX, metrics.playerY, outside.x, outside.y)) continue;
        entries.push({ inside, outside });
      }
    }
  }
  return entries.sort((left, right) =>
    Math.hypot(metrics.playerX - left.outside.x, metrics.playerY - left.outside.y) -
    Math.hypot(metrics.playerX - right.outside.x, metrics.playerY - right.outside.y)
  )[0] ?? null;
}

export async function enterNearestAuthoredSafety(safety, actor, expectedLifeSeq, drive = driveClientToWorldTarget) {
  const before = (await actor.metricsState()).uiMetrics || {};
  const entry = nearestAuthoredSafetyEdge(safety, before);
  if (entry == null) return false;
  const approached = await drive(actor, entry.outside, 12_000, false, 10, null, 80);
  if (!approached.reached) return false;
  const entered = await drive(actor, entry.inside, 6_000, false, 10, null, 60);
  const after = (await actor.metricsState()).uiMetrics || {};
  return entered.reached && after.playerAlive === true &&
    after.serverPlayerAlive === true && after.playerLifeSeq === expectedLifeSeq &&
    after.inSafeZone === true;
}

export async function leaveAuthoredSafety(actor, drive = driveClientToWorldTarget,
  waypoints = EXTREME_SAFETY_LURE_WAYPOINTS) {
  for (const waypoint of waypoints) {
    const reached = await drive(actor, waypoint, 20_000, false, 12, null, 100);
    if (!reached.reached) return false;
  }
  return (await actor.metricsState()).uiMetrics?.inSafeZone === false;
}

/// Baits a specific bot life off its authored safe tile and brings the pilot back into the nearest
/// safety entry so the safe->unsafe staging solve starts within bomb range. Returns false when the
/// target never followed, which the caller must report instead of staging a hopeless geometry.
export async function lureTargetOutOfSafety({
  safety,
  actor,
  preferredTargetId,
  preferredTargetLifeSeq,
  label,
  attempts = 4,
  followWindowMs = 10_000,
  drive = driveClientToWorldTarget,
  leaveSafety,
  enterSafety = enterNearestAuthoredSafety,
}) {
  const findTarget = async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return {
      metrics,
      target: (metrics.remotePlayers || []).find((player) =>
        player.id === preferredTargetId && player.lifeSeq === preferredTargetLifeSeq &&
        Number.isFinite(player.positionX) && Number.isFinite(player.positionY)
      ) ?? null,
    };
  };
  const targetIsExposed = (target) => target != null &&
    !safety.positionIsOnAuthoredSafeTile(target.positionX, target.positionY);
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    await waitUntil(`${label} lure owns a live pilot`, async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return metrics.playerAlive === true && metrics.serverPlayerAlive === true;
    }, 15_000);
    let { metrics, target } = await findTarget();
    if (targetIsExposed(target)) return true;
    if (metrics.inSafeZone === true && !await leaveSafety(actor, drive)) continue;

    const pilotLifeSeq = (await actor.metricsState()).uiMetrics?.playerLifeSeq;
    const deadline = Date.now() + followWindowMs;
    let targetExited = false;
    while (Date.now() < deadline) {
      ({ metrics, target } = await findTarget());
      if (
        metrics.playerAlive !== true || metrics.serverPlayerAlive !== true ||
        metrics.playerLifeSeq !== pilotLifeSeq
      ) break;
      if (targetIsExposed(target)) {
        targetExited = true;
        break;
      }
      await sleep(50);
    }
    if (!targetExited) continue;
    if (!await enterSafety(safety, actor, pilotLifeSeq, drive)) continue;
    ({ target } = await findTarget());
    if (targetIsExposed(target)) return true;
  }
  return false;
}

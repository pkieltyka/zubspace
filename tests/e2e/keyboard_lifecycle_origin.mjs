import { movingHullPathIsClear } from "./bot_combat_evidence.mjs";
import { parseZubspaceEvents } from "../../tools/server_events.mjs";
import { brakeClientVelocity, brakeVelocityDisposition, waitUntil } from "./combat_replay.mjs";

export function initialAuthorityReady(state) {
  const metrics = state?.uiMetrics || {};
  const local = metrics.localGameplayState || {};
  return metrics.playerAlive === true && metrics.serverPlayerAlive === true &&
    Number.isInteger(metrics.selfPlayerId) && Number.isInteger(metrics.playerLifeSeq) &&
    local.zoneFilesReady === true && local.zoneMapReady === true &&
    local.pendingShipResetPosition === false &&
    (local.lifecycleBaselineReceiptsMatched ?? 0) >= 1 &&
    (local.lifecycleBaselineReceiptsUnmatched ?? 0) === 0 &&
    metrics.combatViewTimeline?.weaponReady === true;
}

// Setup may follow a chord that leaves the pilot beside an authored door.
// Preserve the20px planner margin: use the ordinary authoritative random Warp
// when needed, then restore the original full-energy/same-life precondition.
export async function prepareKeyboardLifecycleOrigin({ actor, server, playerId, initialLifeSeq,
  geometry, deadline = Date.now() + 180_000 }) {
  if (typeof geometry?.authoredHullIsTraversable !== "function") throw new Error("keyboard origin setup requires authored geometry");
  const scopeOf = m => JSON.stringify([m.localGameplayState?.authorityScopeServerSessionId,
    m.localGameplayState?.authorityScopeArenaStreamId, m.selfPlayerId, m.playerLifeSeq]);
  const first = (await actor.metricsState()).uiMetrics;
  const scope = scopeOf(first), warps = [];
  const read = async () => {
    const state = await actor.metricsState(), m = state.uiMetrics;
    if (m.selfPlayerId !== playerId || m.playerLifeSeq !== initialLifeSeq || scopeOf(m) !== scope)
      throw new Error("keyboard origin setup crossed its exact life/scope");
    return state;
  };
  const budget = maximum => {
    const remaining = deadline - Date.now();
    if (remaining <= 0) throw new Error("keyboard origin setup exceeded its encounter budget");
    return Math.min(maximum, remaining);
  };
  const clear = m => geometry.authoredHullIsTraversable(m.playerX, m.playerY, 20) === true;
  // A valid brake can retain up to3.5px/s. Keep the same20px hull margin
  // through the next caller's4s facing +8s braking preflight as well; a
  // point-in-time clearance beside a wall otherwise expires before planning.
  const handoffCoastSeconds = 12;
  const clearThroughHandoff = m => movingHullPathIsClear({ x: m.playerX, y: m.playerY,
    velocityX: m.playerVelocityX, velocityY: m.playerVelocityY, seconds: handoffCoastSeconds,
    hullIsOpen: (x, y) => geometry.authoredHullIsTraversable(x, y, 20) === true });
  if (typeof actor.releaseKeys === "function") await actor.releaseKeys();
  else {
    // Chrome exposes individual genuine CDP releases; Firefox additionally
    // exposes WebDriver release-actions. These are the prior matrix's held keys.
    for (const key of ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Control", "Shift"])
      await actor.keyUp(key);
  }
  let state = await read(), stop = null;
  if (clear(state.uiMetrics)) {
    stop = await brakeClientVelocity(actor, budget(8_000));
    if (!stop.braked) throw new Error("keyboard origin setup could not stop before certification");
    state = await read();
  }
  for (let attempt = 0; !clearThroughHandoff(state.uiMetrics) && attempt < 2; attempt++) {
    await waitUntil("full authoritative energy before ordinary origin-staging Warp", async () => {
      state = await read(); const m = state.uiMetrics;
      return initialAuthorityReady(state) && m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy;
    }, budget(30_000));
    const offset = server.logs.length;
    await actor.keyDown("Shift");
    try { await actor.press("0", 55); } finally { await actor.keyUp("Shift"); }
    let terminal = null;
    await waitUntil("ordinary random Warp applies for the same-life route origin", async () => {
      terminal = parseZubspaceEvents(server.logs.slice(offset)).map(({ event }) => event)
        .find(e => e.type === "server_portal_warp_terminal" && e.player_id === playerId) || null;
      if (terminal && (terminal.outcome !== 1 || terminal.used_portal || terminal.life_seq !== initialLifeSeq))
        throw new Error(`keyboard origin Warp rejected or changed scope: ${JSON.stringify(terminal)}`);
      state = await read(); const m = state.uiMetrics;
      return terminal && !m.localGameplayState.pendingPortalWarp && initialAuthorityReady(state) &&
        Math.hypot(m.playerX - terminal.position_x, m.playerY - terminal.position_y) <= 16;
    }, budget(8_000));
    warps.push(terminal);
    await waitUntil("full energy and rest restored after ordinary origin-staging Warp", async () => {
      state = await read(); const m = state.uiMetrics;
      return initialAuthorityReady(state) && brakeVelocityDisposition(m).braked &&
        m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy;
    }, budget(30_000));
  }
  if (!clearThroughHandoff(state.uiMetrics) || !brakeVelocityDisposition(state.uiMetrics).braked)
    throw new Error("keyboard lifecycle origin still lacks conservative stopped clearance");
  return { policy: "ordinary-key Warp only for an invalid conservative route origin or handoff coast", scope, handoffCoastSeconds,
    initial: { x: first.playerX, y: first.playerY, velocityX: first.playerVelocityX, velocityY: first.playerVelocityY },
    stop: stop && { braked: stop.braked, speed: stop.speed }, warps,
    final: { x: state.uiMetrics.playerX, y: state.uiMetrics.playerY,
      lifeSeq: state.uiMetrics.playerLifeSeq, energy: state.uiMetrics.playerEnergy,
      serverEnergy: state.uiMetrics.serverPlayerEnergy, maxEnergy: state.uiMetrics.playerMaxEnergy } };
}


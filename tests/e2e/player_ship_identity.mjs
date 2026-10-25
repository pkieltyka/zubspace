// Read-only witnesses: maximum entry energy is shared by different ships.
export function hasPlayerShip(metrics, ship) {
  return Number.isInteger(ship) && ship >= 0 && ship < 8 &&
    metrics?.playerShip === ship && metrics?.selfRosterShip === ship;
}

export function playerShipReady(metrics) {
  const local = metrics?.localGameplayState;
  return metrics?.playerAlive === true && metrics?.serverPlayerAlive === true &&
    Number.isInteger(metrics?.playerLifeSeq) && metrics.playerLifeSeq > 0 &&
    local?.zoneMapReady === true && local?.zoneFilesReady === true &&
    local?.doorClockFresh === true && local?.authorityReadinessSecurity === true &&
    local?.authorityPhase === "Live" && local?.pendingShipResetPosition === false &&
    local?.pendingShipResetDeliveryId == null && local?.authorityTransitionKind == null;
}

export async function selectPlayerShip({ read, press, waitUntil, ship, fromShip, label }) {
  if (!Number.isInteger(ship) || ship < 0 || ship >= 8) throw new Error("invalid playable ship");
  let before;
  await waitUntil(`${label}: full-health predecessor ready`, async () => {
    before = await read();
    return playerShipReady(before) && before.playerMaxEnergy > 0 &&
      before.playerEnergy >= before.playerMaxEnergy &&
      (fromShip == null || hasPlayerShip(before, fromShip));
  }, 60_000);
  const alreadySelected = hasPlayerShip(before, ship);
  if (!alreadySelected) {
    // One real edge, held across frames. Never repeat it during a baseline.
    await press("Escape", 200);
    await press(String(ship + 1), 200);
  }
  let after;
  await waitUntil(`${label}: authoritative ship and exact life applied`, async () => {
    after = await read();
    const expectedLife = alreadySelected ? before.playerLifeSeq : ((before.playerLifeSeq + 1) >>> 0);
    return hasPlayerShip(after, ship) && playerShipReady(after) && after.playerLifeSeq === expectedLife;
  }, 30_000);
  return {
    fromLifeSeq: before.playerLifeSeq, lifeSeq: after.playerLifeSeq,
    fromShip: before.playerShip, fromRosterShip: before.selfRosterShip,
    ship: after.playerShip, rosterShip: after.selfRosterShip,
    maxEnergy: after.playerMaxEnergy, selectionRequested: !alreadySelected,
  };
}

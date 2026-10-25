import assert from "node:assert/strict";

// Compare actual ship draws at the launch's gameplay tick, not wall-clock
// arrival of the independent event channel or a later moving projectile.
export function botFacingEvidence(states, botIds, {minimumShotServerTick = null} = {}) {
  if (minimumShotServerTick !== null && (!Number.isInteger(minimumShotServerTick) ||
      minimumShotServerTick < 0 || minimumShotServerTick > 0xffffffff))
    throw new Error('invalid minimum firing tick for facing collection');
  const shots = new Map();
  const sources = new Map();
  const eventSources = new Map();
  const sourceKey = (session, life, group, projectile, child) =>
    `${session}:${life}:${group}:${projectile}:${child}`;
  const draws = new Map();
  const drawFrames = new Map();
  const firingKinds = new Set(["bullet", "burst", "bomb", "mine", "thor"]);
  for (const state of states) {
    for (const frame of state.metricsTrace || []) {
      for (const shot of frame.remoteProjectileSources || []) {
        // Explosion fragments inherit the parent's firing orientation/group,
        // but begin their own lifetime at explosion commit, after the ship may
        // have turned. They are not a new muzzle launch and must neither count
        // toward the floor nor overwrite that group's genuine launch tick.
        if (!firingKinds.has(shot.kind) || !botIds.includes(shot.playerId) || !Number.isInteger(shot.serverTick) ||
          !Number.isInteger(shot.lifeSeq) || !Number.isFinite(shot.fireOrientation) ||
          !shot.sessionId || !shot.shotGroupId) continue;
        if (minimumShotServerTick !== null &&
            ((shot.serverTick - minimumShotServerTick) >>> 0) >= 0x80000000) continue;
        shots.set(`${shot.sessionId}:${shot.lifeSeq}:${shot.shotGroupId}`, shot);
        if (shot.authenticated === true && Number.isInteger(shot.authorityProjectileId) &&
            Number.isInteger(shot.sourceProjectileIndex) &&
            firingKinds.has(shot.kind) &&
            shot.fireOrientation >= 0 && shot.fireOrientation < 1) {
          sources.set(sourceKey(shot.sessionId, shot.lifeSeq, shot.shotGroupId,
            shot.authorityProjectileId, shot.sourceProjectileIndex), shot);
        }
        // The authenticated event lane can present a fresh birth before the
        // independent game carrier assigns its native projectile ID. Bind
        // that path to the exact event/group/child, never a wildcard ID.
        if (shot.authenticated === true && shot.authorityProjectileId == null &&
            Number.isInteger(shot.eventId) && shot.eventId >= 0 && shot.eventId <= 0xffffffff &&
            Number.isInteger(shot.sourceProjectileIndex) && shot.sourceProjectileIndex >= 0 &&
            shot.fireOrientation >= 0 && shot.fireOrientation < 1) {
          eventSources.set(sourceKey(shot.sessionId, shot.lifeSeq, shot.shotGroupId,
            shot.eventId, shot.sourceProjectileIndex), shot);
        }
      }
    }
    for (const frame of state.remoteSpriteTrace || []) {
      if (!Number.isInteger(frame.presentationRenderTick)) continue;
      for (const draw of frame.entries || []) {
        if (!botIds.includes(draw.id) || draw.drawn !== true ||
          draw.suppression !== "none" || !draw.authoritativeMember ||
          !Number.isInteger(draw.shipFrame) || !draw.sourceSessionId) continue;
        const key = `${draw.sourceSessionId}:${draw.lifeSeq}:${frame.presentationRenderTick}`;
        if (!draws.has(key)) {
          draws.set(key, draw);
          drawFrames.set(key, frame);
        }
      }
    }
  }
  // Fresh weapon births are a second authoritative heading input. The
  // production prepare_remote_weapon_views path presents that exact firing
  // heading before drawing the actor. A quantized position update alone omits
  // this input. Bind it to the actual same-frame projectile and exact source;
  // a later flight/recovery draw cannot authorize rewinding the ship's facing.
  const birthHeadings = new Map();
  for (const state of states) {
    for (const frame of state.remoteSpriteTrace || []) {
      const tick = frame.presentationRenderTick;
      if (!Number.isInteger(tick)) continue;
      for (const projectile of frame.projectiles?.entries || []) {
        if (projectile.drawn !== true || projectile.sourceServerTick !== tick ||
            projectile.poseTick !== tick) continue;
        const source = sources.get(sourceKey(projectile.sourceSessionId,
          projectile.sourceLifeSeq, projectile.shotGroupId,
          projectile.authorityProjectileId, projectile.sourceProjectileIndex)) ||
          (projectile.authorityProjectileId == null && Number.isInteger(projectile.eventId)
            ? eventSources.get(sourceKey(projectile.sourceSessionId, projectile.sourceLifeSeq,
                projectile.shotGroupId, projectile.eventId, projectile.sourceProjectileIndex))
            : null);
        if (!source || source.playerId !== projectile.sourcePlayerId ||
            source.serverTick !== tick) continue;
        const actor = `${source.sessionId}:${source.lifeSeq}`;
        const drawKey = `${actor}:${tick}`;
        const draw = draws.get(drawKey);
        if (drawFrames.get(drawKey) !== frame || !draw || draw.id !== source.playerId ||
            draw.directedPresentation !== true) continue;
        const list = birthHeadings.get(actor) || [];
        list.push({ tick, heading: source.fireOrientation, group: source.shotGroupId });
        birthHeadings.set(actor, list);
      }
    }
  }
  // Per-actor render history, oldest first: the previously sampled drawn heading is the reference
  // point the next draw's delta is measured from. It is not a bound - each update re-seats the
  // renderer's interpolation start at whatever heading it was displaying then, so the draw can leave
  // the span since this sample while still sitting on an arc the life was told into.
  const history = new Map();
  for (const [key, draw] of draws) {
    const [sessionId, lifeSeq, tick] = key.split(":");
    const actor = `${sessionId}:${lifeSeq}`;
    const list = history.get(actor) || [];
    list.push({ tick: Number(tick), draw });
    history.set(actor, list);
  }
  for (const list of history.values()) list.sort((a, b) => a.tick - b.tick);
  const previousEntry = (actor, tick) => {
    const list = history.get(actor) || [];
    let found = null;
    for (const entry of list) {
      if (entry.tick >= tick) break;
      found = entry;
    }
    return found;
  };
  // The heading the client was told only answers for a launch when the position update behind it
  // is that launch's own tick. A frame can already carry a newer update (snapshots run ahead of
  // the presentation cursor), so search the actor's sampled frames for the one whose update tick
  // is the launch tick; `lastServerPositionTick` is a u16 continuation of the launch's u32 tick.
  const toldUpdate = (actor, launchTick) => {
    const wanted = launchTick % 65_536;
    for (const entry of history.get(actor) || []) {
      if (entry.draw.lastServerPositionTick === wanted) return entry;
    }
    return null;
  };
  // The reference game uses the same truncating f32 bins for sprite selection and
  // weapon launch. Reconstruct f32 before multiplication at seam values.
  const frameFor = (turns) => {
    const remainder = Math.fround(turns) % 1;
    const wrapped = remainder < 0 ? Math.fround(remainder + 1) : remainder;
    return Math.trunc(Math.fround(wrapped * 40)) % 40;
  };
  const ringBins = (a, b) => Math.min(Math.abs(a - b), 40 - Math.abs(a - b));
  const matched = [];
  for (const shot of shots.values()) {
    const draw = draws.get(`${shot.sessionId}:${shot.lifeSeq}:${shot.serverTick}`);
    if (!draw || draw.id !== shot.playerId) continue;
    if (!Number.isFinite(draw.displayOrientation) || !Number.isFinite(draw.authoritativeOrientation)) {
      throw new Error(`bot facing needs the drawn and authoritative headings of the launch tick: ${JSON.stringify({
        playerId: shot.playerId,
        lifeSeq: shot.lifeSeq,
        shotGroupId: shot.shotGroupId,
        serverTick: shot.serverTick,
      })}`);
    }
    // Weapon simulation uses protocol_direction's truncating f32 bins, as
    // does sprite selection; a smoothed display heading can still trail it.
    // In particular, a launch at 0.98809 turns fires in bin 39, not bin 0.
    const remainder = Math.fround(shot.fireOrientation) % 1;
    const turns = remainder < 0 ? Math.fround(remainder + 1) : remainder;
    const expectedFrame = Math.trunc(Math.fround(turns * 40)) % 40;
    const actor = `${shot.sessionId}:${shot.lifeSeq}`;
    // The heading the client's render path was measured at for its previous draw: the origin the
    // delta below is signed from. The bound that authorises the draw is the life's told-heading hull
    // computed under the span comment, not this sample.
    const before = previousEntry(actor, shot.serverTick);
    const arcStart = before && Number.isFinite(before.draw.displayOrientation)
      ? before.draw.displayOrientation
      : null;
    // Signed heading relative to the previous draw, on the shorter arc, in turns of a circle.
    const signed = (value) => ((((value - arcStart + 0.5) % 1) + 1) % 1) - 0.5;
    // Everything the renderer was told, up to and including the frame that carries this launch's own
    // position update: both heading paths clamp at their target (`advance_orientation_bounded`;
    // `interpolate_orientation` with `t` clamped to [0,1]), so the drawn heading can never leave the
    // span of those headings. It can legitimately pass the *current* target when the one it was
    // already chasing was further along and the server then reversed it — which is what a
    // single-endpoint arc condemned in the sustained Chrome/Extreme cell: 0.0235 turns off the arc,
    // about two thirds of a sprite bin, with the authority and renderer-honesty claims both clean.
    //
    // The window's upper bound is the tick of the frame that carries this launch's own position
    // update, not the launch tick. The trace is keyed by the presentation cursor, and a cursor at
    // tick R renders by interpolating into the bracketing sample *ahead* of it, so the heading
    // behind a launch is legitimately evidenced by a frame whose cursor tick is later than the
    // launch. Cutting the window at the launch tick hid exactly that heading and condemned the
    // Firefox/Extreme cell: span 0.000256 turns, drawn delta 0.01275 turns, `authorityErrorBins: 0`
    // and `presentationBins: 0` — the frame carrying the launch's update was sampled two cursor
    // ticks later and reported the heading the draw was rotating into.
    const told = toldUpdate(actor, shot.serverTick);
    const toldUpperTick = told == null || told.tick < shot.serverTick
      ? shot.serverTick
      : told.tick;
    //
    // The window's *lower* end is the start of the actor's life, not its previous draw. Each position
    // update re-seats the renderer's interpolation start at the heading it was displaying when the
    // update arrived (`begin_orientation_interpolation` stores `current_display_orientation` as
    // `previous_orientation`) and runs clamped-`t` interpolation to the newest heading it was told
    // (`display_orientation`, `t.clamp(0.0, 1.0)`). So the drawn heading can never leave the hull of
    // the headings this life was told - but it can leave the span since its previous *sampled* draw,
    // because the arc that produced it started from a display still chasing a target told before that
    // draw, and updates that arrived between samples are invisible here. Measured on the captures the
    // red `test-e2e-immediate-bot-combat` retained: the previous-draw window condemned 3/1192 Extreme
    // and 6/1052 Chaos draws (worst 0.012 turns, half a sprite bin, `authorityErrorBins: 0`,
    // `rendererFrame` honest), the life hull condemns 0 of 2244. The hull is the client's contract;
    // the window was a guess about it, and it flaked the frozen matrix.
    const toldHeadings = (history.get(actor) || [])
      .filter((entry) => entry.tick <= toldUpperTick)
      .map((entry) => entry.draw.authoritativeOrientation)
      .filter((value) => Number.isFinite(value))
      .map(signed);
    const births = (birthHeadings.get(actor) || []).filter((birth) => birth.tick <= shot.serverTick);
    toldHeadings.push(...births.map((birth) => signed(birth.heading)));
    const firingAnchor = births.some((birth) => birth.tick === shot.serverTick &&
      birth.group === shot.shotGroupId);
    const firingHeadingError = firingAnchor
      ? Math.abs(((((draw.displayOrientation - shot.fireOrientation + .5) % 1) + 1) % 1) - .5)
      : null;
    const headingDelta = arcStart === null ? 0 : signed(draw.displayOrientation);
    const headingLo = Math.min(0, ...toldHeadings);
    const headingHi = Math.max(0, ...toldHeadings);
    matched.push({
      playerId: shot.playerId, lifeSeq: shot.lifeSeq,
      shotGroupId: shot.shotGroupId, serverTick: shot.serverTick,
      expectedFrame, drawnFrame: draw.shipFrame,
      errorBins: ringBins(draw.shipFrame, expectedFrame),
      // Authority: the launch must match the heading the server told this client for the launch's
      // own tick. The presentation frame can already carry a newer update, so the told heading is
      // read from the frame whose update is the launch tick; when no sampled frame holds that
      // update the client was never told this launch's heading and the claim is not decidable.
      authorityTold: told !== null,
      authorityErrorBins: told === null || !Number.isFinite(told.draw.authoritativeOrientation)
        ? 0
        : ringBins(frameFor(told.draw.authoritativeOrientation), expectedFrame),
      // Renderer honesty: the sprite must be the bin of the heading the renderer was given.
      rendererFrame: frameFor(draw.displayOrientation),
      presentationBins: ringBins(draw.shipFrame, frameFor(draw.authoritativeOrientation)),
      // The span of headings the renderer was told, and where this draw sits inside it.
      headingLo, headingHi, headingDelta,
      arcStartHeading: arcStart,
      firingAnchor, firingHeadingError,
      smoothed: arcStart !== null,
    });
  }
  // 1. Authority: the launch must match the heading the client was told for that launch's tick,
  //    within the one sprite bin of f32/bin quantisation. Perpendicular or rear-facing fire must
  //    never be excused.
  const toldSamples = matched.filter((sample) => sample.authorityTold);
  const authorityFailures = toldSamples.filter((sample) => sample.authorityErrorBins > 1);
  assert.equal(
    authorityFailures.length,
    0,
    `bot fire does not match its authoritative heading: ${JSON.stringify(authorityFailures.slice(0, 8))}`,
  );
  // 2. Renderer honesty: no sprite bin other than the one the drawn heading maps to.
  const rendererFailures = matched.filter((sample) => sample.drawnFrame !== sample.rendererFrame);
  assert.equal(
    rendererFailures.length,
    0,
    `bot sprite bin does not match the heading it was given: ${JSON.stringify(rendererFailures.slice(0, 8))}`,
  );
  // 3. Render honesty: each position update re-seats the renderer's interpolation start at the
  //    heading it was displaying then (`begin_orientation_interpolation`) and both heading paths clamp
  //    at their target, so a drawn heading can only ever lie inside the hull of the headings this
  //    life has been told through positions or an observed authenticated weapon birth.
  //    Anything outside that hull is a heading the client never received.
  const arcFailures = matched.filter((sample) => sample.smoothed &&
    (sample.headingDelta < sample.headingLo - 1e-3 || sample.headingDelta > sample.headingHi + 1e-3));
  assert.equal(
    arcFailures.length,
    0,
    `bot heading is outside the span of headings the renderer was told for this life: ${JSON.stringify(arcFailures.slice(0, 8))}`,
  );
  // Metrics round launch headings to five decimal places. Require the exact
  // firing heading within that recording precision, including circular seams.
  const birthFailures = matched.filter((sample) => sample.firingAnchor && sample.firingHeadingError > 1e-5);
  assert.equal(birthFailures.length, 0,
    `bot birth draw does not match its exact firing heading: ${JSON.stringify(birthFailures.slice(0, 8))}`);
  // 4. Smoothing bound: Extreme authors `MaximumRotation=360` deg/s, i.e. 40 sprite bins per
  //    second, so even a 200 ms smoothing window explains eight bins of lag. Beyond that the
  //    client is not smoothing an authoritative heading, it is holding something else.
  const lagFailures = matched.filter((sample) => sample.presentationBins > 8);
  assert.equal(
    lagFailures.length,
    0,
    `bot sprite is further behind its authoritative heading than any authored smoothing: ${JSON.stringify(lagFailures.slice(0, 8))}`,
  );
  assert.ok(matched.length >= 3, `bot facing requires three exact-tick launches; matched ${matched.length}/${shots.size}`);
  assert.ok(
    toldSamples.length >= 3,
    `bot facing requires three launches whose tick the client was told; told ${toldSamples.length}/${matched.length}`,
  );
  assert.ok(matched.some((sample) => sample.expectedFrame >= 10), "bot facing must cover headings outside the old compressed arc");
  return {
    shots: shots.size, matched: matched.length, unmatched: shots.size - matched.length,
    toldLaunches: toldSamples.length,
    maximumErrorBins: Math.max(...matched.map((sample) => sample.errorBins)),
    maximumAuthorityErrorBins: Math.max(...toldSamples.map((sample) => sample.authorityErrorBins)),
    maximumPresentationBins: Math.max(...matched.map((sample) => sample.presentationBins)),
    samples: matched.slice(0, 16),
  };
}

/// The coverage floor above needs launches from more than one quadrant of the
/// 40-bin sprite ring, and an authoritative bot aims at the pilot. Baiting it
/// out of safety repeatedly therefore re-aims it along nearly the same bearing
/// - every launch lands in the same handful of bins and the floor is met only
/// by luck, which is how the retained Firefox/Extreme cell failed
/// `bot facing must cover headings outside the old compressed arc`. On the
/// diversity half of each pair of collection cycles the pilot instead takes a
/// genuine keyboard station a quarter turn around the hostile at the range it
/// is already engaged at, which turns the hostile's facing by ten sprite bins
/// before the next bait.
export function facingBearingDiversityStep({
  cycle,
  playerX,
  playerY,
  targetX,
  targetY,
  minimumRadiusPx = 96,
  maximumCoordinate = 16_384,
}) {
  if (!Number.isInteger(cycle) || cycle % 2 === 0) return null;
  if (![playerX, playerY, targetX, targetY].every(Number.isFinite)) return null;
  const offsetX = playerX - targetX;
  const offsetY = playerY - targetY;
  if (!(Math.hypot(offsetX, offsetY) >= minimumRadiusPx)) return null;
  const direction = (cycle + 1) % 4 === 0 ? -1 : 1;
  const x = targetX - offsetY * direction;
  const y = targetY + offsetX * direction;
  if (x < 0 || y < 0 || x > maximumCoordinate || y > maximumCoordinate) return null;
  return { x, y, quarterTurn: direction };
}

// The witness above needs three exact-tick launches, but a bot's firing is
// opportunistic: a single post-combat population can easily hold five launches
// whose ticks never coincide with an authoritative draw (retained
// `bot facing requires three exact-tick launches; matched 2/5`). The floor and
// the exact-tick matching are the point of the assertion, so grow the observed
// population instead: sample additional presentation states, re-running the
// unmodified assertion over the union, and rethrow its final verdict once the
// budget expires. The caller's own state array is never mutated, so the other
// consumers of that population keep seeing exactly what they saw before.
export async function collectBotFacingEvidence({
  states,
  sample,
  botIds,
  timeoutMs = 30_000,
  intervalMs = 750,
  provoke = null,
  minimumShotServerTick = null,
  sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
}) {
  const base = states || [];
  const collected = [...base];
  const startedAt = Date.now();
  let provokeFailure = null;
  let verdict = null;
  for (;;) {
    try {
      return {
        ...botFacingEvidence(collected, botIds, {minimumShotServerTick}),
        ...(minimumShotServerTick === null ? {} : {minimumShotServerTick}),
        sampledStates: collected.length - base.length,
        waitedMs: Date.now() - startedAt,
        ...(provokeFailure == null ? {} : { provokeFailure }),
      };
    } catch (error) {
      verdict = error;
    }
    if (Date.now() - startedAt >= timeoutMs) throw verdict;
    // The window is only useful while the bot is actually launching: after the combat phase the
    // hostile usually holds at range and fires nothing, so sampling alone grows a population of
    // frames with no launch in them. Provoking is best effort — a failed bait must never replace
    // the witness verdict, it is reported alongside it.
    if (provoke != null) {
      try {
        // The cycle index lets the caller alternate between baiting a launch and
        // moving the geometry, which is what makes heading coverage deterministic.
        await provoke(collected.length - base.length);
        provokeFailure = null;
      } catch (error) {
        provokeFailure = error?.message ?? String(error);
      }
    }
    await sleep(intervalMs);
    collected.push(await sample());
  }
}

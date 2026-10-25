// Scenario precondition only. Callers supply genuine keyboard combat and the
// existing exact-life browser readiness waiter; this helper grants no authority.
import { serverLogField } from '../../tools/server_log_fields.mjs';
import { playerShipReady } from './player_ship_identity.mjs';

export function counterfireAssignmentWitness(text, {humanId, sinceTick, botIds}) {
  const latest = new Map();
  const field = (line, name) => serverLogField(line, name) ??
    line.match(new RegExp(`(?:^|\\s)${name}=([^\\s]+)`))?.[1];
  for (const line of text.split('\n')) {
    if (!line.includes('server bot movement trace')) continue;
    const row = Object.fromEntries(['server_tick', 'bot_id', 'life_seq', 'assigned_human_player_id',
      'human_assignment_rotation'].map(name => [name, Number(field(line, name))]));
    if (!Object.values(row).every(Number.isInteger) || !botIds.includes(row.bot_id) ||
        row.server_tick < 0 || row.server_tick > 0xffffffff || row.life_seq <= 0 ||
        ((row.server_tick - sinceTick) >>> 0) >= 0x80000000) continue;
    const prior = latest.get(row.bot_id);
    if (!prior || ((row.server_tick - prior.server_tick) >>> 0) < 0x80000000)
      latest.set(row.bot_id, row);
  }
  if (latest.size !== botIds.length) return null;
  const assigned = [...latest.values()].filter(row => row.assigned_human_player_id === humanId);
  if (assigned.length > 1) throw new Error('counterfire observed multiple hunters for one human');
  if (!assigned.length) return null;
  return {assignedBotId: assigned[0].bot_id, traces: [...latest.values()]};
}

// Preserve the controlled bot's own passive→hostile life proof. Native hunter
// rotation can move its human slot to another bot after a human death. Rebind
// through ordinary full-health ship selection, never by changing bot policy.
export async function alignCounterfireAssignment({read, changeShip, waitUntil, server,
  botIds, desiredBotId, setupBudgetMs = 90_000}) {
  if (!Array.isArray(botIds) || !botIds.length || new Set(botIds).size !== botIds.length ||
      !botIds.includes(desiredBotId) || !Array.isArray(server?.logs) ||
      !Number.isFinite(setupBudgetMs) || setupBudgetMs <= 0 || setupBudgetMs > 90_000)
    throw new Error('invalid bounded counterfire assignment setup');
  const deadline = Date.now() + setupBudgetMs, changes = [], observations = [];
  const remaining = () => {
    const value = deadline - Date.now();
    if (value <= 0) throw new Error('counterfire assignment exceeded setup budget');
    return value;
  };
  const scope = m => JSON.stringify([m.localGameplayState?.authorityScopeServerSessionId,
    m.localGameplayState?.authorityScopeArenaStreamId, m.selfPlayerId]);
  let originalScope = null;
  for (let attempt = 0; attempt < botIds.length; attempt++) {
    let before;
    await waitUntil('live pilot before counterfire assignment', async () => {
      before = await read(); return playerShipReady(before);
    }, Math.min(15_000, remaining()));
    originalScope ??= scope(before);
    if (scope(before) !== originalScope) throw new Error('counterfire assignment changed player scope');
    const logStart = server.logs.length;
    let witness;
    await waitUntil('fresh authoritative hunter assignment', async () => {
      const current = await read();
      if (!playerShipReady(current) || scope(current) !== originalScope || current.playerLifeSeq !== before.playerLifeSeq)
        throw new Error('counterfire assignment observation crossed a lifecycle boundary');
      witness = counterfireAssignmentWitness(server.logs.slice(logStart).join(''), {
        humanId: before.selfPlayerId, sinceTick: before.latestSnapshotServerTick, botIds,
      });
      return witness != null;
    }, Math.min(5_000, remaining()));
    observations.push({humanLifeSeq: before.playerLifeSeq, ...witness});
    if (witness.assignedBotId === desiredBotId) {
      remaining();
      return {keysOnly: true, desiredBotId, humanLifeSeq: before.playerLifeSeq, changes, observations};
    }
    if (attempt + 1 === botIds.length) break;
    const selection = await changeShip(before, before.playerShip === 0 ? 6 : 0,
      (label, predicate, timeout) => waitUntil(label, predicate, Math.min(timeout, remaining())));
    const after = await read();
    if (!playerShipReady(after) || scope(after) !== originalScope ||
        after.playerLifeSeq !== ((before.playerLifeSeq + 1) >>> 0))
      throw new Error('counterfire assignment requires an exact genuine replacement life');
    changes.push(selection);
    remaining();
  }
  throw new Error(`controlled counterfire source never acquired its human: ${JSON.stringify(observations)}`);
}

export async function prepareHostileCounterfireLife({target, passiveLifeSeqs, retire, successor}) {
  const retirements = [];
  const id = target?.id;
  const valid = value => value?.id === id && Number.isInteger(value.lifeSeq) &&
    value.lifeSeq > 0 && value.lifeSeq <= 0xffffffff;
  if (!Number.isInteger(id) || !valid(target)) throw new Error('counterfire target identity missing');
  while (passiveLifeSeqs.includes(target.lifeSeq)) {
    if (retirements.length >= passiveLifeSeqs.length) throw new Error('counterfire passive-life cycle');
    const hit = await retire(target);
    if (hit?.victimId !== id || hit.victimLifeSeq !== target.lifeSeq || hit.lethal !== true) {
      throw new Error('counterfire preparation requires exact-life authoritative lethal damage');
    }
    const next = await successor(target);
    const advance = (next?.lifeSeq - target.lifeSeq) >>> 0;
    if (!valid(next) || advance === 0 || advance >= 0x80000000) {
      throw new Error('counterfire preparation requires a presented live successor');
    }
    retirements.push(hit);
    target = next;
  }
  return {target, retirements};
}

// The counterfire proof intentionally controls one exact bot. After it kills
// the pilot, native fairness can assign a different bot to the new human life.
// Finish passive-fixture combat for every potential source before starting the
// unchanged facing collection window, rather than baiting a bot forbidden to fire.
export async function prepareBotFacingSources({botIds, passiveLifeSeqs, readTarget,
  retire, successor, setupBudgetMs = null}) {
  setupBudgetMs ??= Array.isArray(botIds) ? botIds.length * 120_000 : 0;
  if (!Array.isArray(botIds) || !botIds.length || new Set(botIds).size !== botIds.length ||
      !Number.isFinite(setupBudgetMs) || setupBudgetMs <= 0 ||
      setupBudgetMs > botIds.length * 120_000)
    throw new Error('invalid bounded facing source setup');
  const deadline = Date.now() + setupBudgetMs;
  const remaining = (limit) => {
    const value = Math.min(limit, deadline - Date.now());
    if (value <= 0) throw new Error('facing source setup exceeded budget');
    return value;
  };
  const sources = [];
  for (const id of botIds) {
    const target = await readTarget(id, remaining(10_000));
    if (target?.id !== id) throw new Error('facing source setup changed bot identity');
    sources.push(await prepareHostileCounterfireLife({target, passiveLifeSeqs,
      retire: target => retire(target, remaining(90_000)),
      successor: target => successor(target, remaining(10_000))}));
    remaining(Infinity);
  }
  return {keysOnly: true, sources};
}

// Use recent, full-width native assignment traces, not array order or a copy
// of the fairness formula. Match the currently presented exact bot life too.
export function facingHunterTarget(metrics, botIds, text) {
  if (!playerShipReady(metrics) || !Number.isInteger(metrics.latestSnapshotServerTick)) return null;
  const witness = counterfireAssignmentWitness(text, {humanId: metrics.selfPlayerId,
    sinceTick: (metrics.latestSnapshotServerTick - 100) >>> 0, botIds});
  if (!witness) return null;
  const trace = witness.traces.find(row => row.bot_id === witness.assignedBotId);
  return (metrics.remotePlayers || []).find(player => player.id === witness.assignedBotId &&
    player.lifeSeq === trace.life_seq && Number.isFinite(player.positionX) &&
    Number.isFinite(player.positionY)) || null;
}

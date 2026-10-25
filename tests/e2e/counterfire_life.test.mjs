import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import vm from 'node:vm';
import {prepareHostileCounterfireLife, counterfireAssignmentWitness, alignCounterfireAssignment,
  prepareBotFacingSources, facingHunterTarget} from './counterfire_life.mjs';

test('passive counterfire life requires exact lethal and presented hostile successor', async()=>{
  const calls=[];
  const result=await prepareHostileCounterfireLife({target:{id:768,lifeSeq:3},passiveLifeSeqs:[1,3],
    retire:async target=>{calls.push(['retire',target.lifeSeq]);return {victimId:768,victimLifeSeq:3,lethal:true};},
    successor:async target=>{calls.push(['successor',target.lifeSeq]);return {id:768,lifeSeq:4};}});
  assert.equal(result.target.lifeSeq,4);
  assert.equal(result.retirements.length,1);
  assert.deepEqual(calls,[['retire',3],['successor',3]]);
});

test('every controlled moving collision source must retire before hostile counterfire', async () => {
  const retirements = [];
  const lives = new Map([[768, 1], [769, 2]]);
  const result = await prepareBotFacingSources({botIds: [768, 769], passiveLifeSeqs: [1, 2],
    readTarget: async id => ({id, lifeSeq: lives.get(id)}),
    retire: async target => {
      retirements.push([target.id, target.lifeSeq]);
      return {victimId: target.id, victimLifeSeq: target.lifeSeq, lethal: true};
    },
    successor: async target => {
      lives.set(target.id, target.lifeSeq + 1);
      return {id: target.id, lifeSeq: lives.get(target.id)};
    }});
  assert.deepEqual(retirements, [[768, 1], [768, 2], [769, 2]]);
  assert.deepEqual(result.sources.map(source => source.target.lifeSeq), [3, 3]);
});

test('the two-bot collision fixture cannot silently become stationary, impaired or wider isolation', () => {
  const source = fs.readFileSync(new URL('./bot_authority.mjs', import.meta.url), 'utf8');
  const start = source.indexOf('const supportedPassiveCollisionFixture =');
  const end = source.indexOf('\nif (!Number.isInteger(hiddenPrizeSeed)', start);
  assert.ok(start >= 0 && end > start);
  const base = {arena: 'chaos-zone', expectedBotCount: 2, networkProfile: '',
    passiveBotLifeSeqs: [1, 2], passiveBotLifeSeq: 2,
    stationaryBotAfterHumanDamage: false, stationaryBotFromLifeSeq: 0, stationaryBotStaticLifeSeqs: []};
  const check = change => {
    const options = {...base, ...change};
    options.passiveBotLifeSeq = options.passiveBotLifeSeqs.at(-1) || 0;
    return vm.runInNewContext(source.slice(start, end), options);
  };
  assert.doesNotThrow(() => check({}));
  for (const change of [{expectedBotCount: 1}, {expectedBotCount: 3}, {networkProfile: 'mild'},
    {stationaryBotAfterHumanDamage: true}, {stationaryBotFromLifeSeq: 1},
    {stationaryBotStaticLifeSeqs: [2]}, {passiveBotLifeSeqs: [1, 2, 3]},
    {passiveBotLifeSeqs: [1]}, {arena: 'extreme'}])
    assert.throws(() => check(change), /passive combat isolation/);
  assert.doesNotThrow(() => check({expectedBotCount: 1, networkProfile: 'mild', passiveBotLifeSeqs: [2]}));
  assert.doesNotThrow(() => check({arena: 'extreme', passiveBotLifeSeqs: [1, 3]}));
  assert.doesNotThrow(() => check({passiveBotLifeSeqs: []}));
});
test('Firefox moving collision fixture has the same narrow isolation guard and explicit recipe', () => {
  const source = fs.readFileSync(new URL('./bot_authority_firefox.mjs', import.meta.url), 'utf8');
  const start = source.indexOf('const supportedPassiveCollisionFixture =');
  const end = source.indexOf('\nif (!["", "mild", "slow"].includes(networkProfile))', start);
  assert.ok(start >= 0 && end > start);
  const base = {arena:'chaos-zone',expectedBotCount:2,networkProfile:'',
    passiveBotLifeSeqs:[1,2],passiveBotLifeSeq:2,stationaryBotAfterHumanDamage:false,
    stationaryBotFromLifeSeq:0,stationaryBotStaticLifeSeqs:[]};
  const check = change => {
    const options={...base,...change};options.passiveBotLifeSeq=options.passiveBotLifeSeqs.at(-1)||0;
    return vm.runInNewContext(source.slice(start,end),options);
  };
  assert.doesNotThrow(()=>check({}));
  for(const change of [{expectedBotCount:1},{expectedBotCount:3},{networkProfile:'mild'},
    {stationaryBotAfterHumanDamage:true},{stationaryBotFromLifeSeq:1},
    {stationaryBotStaticLifeSeqs:[2]},{passiveBotLifeSeqs:[1,2,3]},
    {passiveBotLifeSeqs:[1]},{arena:'extreme'}])
    assert.throws(()=>check(change),/passive combat isolation/);
  assert.doesNotThrow(()=>check({passiveBotLifeSeqs:[]}));
  const makefile=fs.readFileSync('Makefile','utf8');
  const recipe=makefile.slice(makefile.indexOf('test-e2e-bots-firefox:'),makefile.indexOf('test-e2e-active-projectile-recovery:'));
  assert.match(recipe,/ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,2/);
  assert.doesNotMatch(recipe,/STATIONARY_BOT|NETWORK_PROFILE=/);
  assert.match(source,/incomingSourcePreparation/);
});
test('hostile life needs no extra combat and passive preparation rejects unrelated or stale evidence',async()=>{
  const base={target:{id:768,lifeSeq:3},passiveLifeSeqs:[3],
    retire:async()=>({victimId:768,victimLifeSeq:3,lethal:true}),
    successor:async()=>({id:768,lifeSeq:4})};
  const hostile=await prepareHostileCounterfireLife({...base,passiveLifeSeqs:[],
    retire:async()=>assert.fail('hostile source must not be retired')});
  assert.deepEqual(hostile.retirements,[]);
  for(const hit of [{victimId:769,victimLifeSeq:3,lethal:true},
    {victimId:768,victimLifeSeq:2,lethal:true},{victimId:768,victimLifeSeq:3,lethal:false}]) {
    await assert.rejects(prepareHostileCounterfireLife({...base,retire:async()=>hit}),/exact-life/);
  }
  for(const target of [{id:769,lifeSeq:4},{id:768,lifeSeq:3},{id:768,lifeSeq:2}]) {
    await assert.rejects(prepareHostileCounterfireLife({...base,successor:async()=>target}),/live successor/);
  }
});

const assignmentLines = (hunter = 769, tick = 70001) => [768, 769].map(id =>
  `INFO server bot movement trace server_tick=${tick} bot_id=${id} life_seq=${id === 768 ? 4 : 1} assigned_human_player_id=${id === hunter ? 0 : 65535} human_assignment_rotation=${hunter === 769 ? 1 : 0}`
).join('\n') + '\n';

test('assignment witnesses retain native hunter rotation across full u16 tick wrap', () => {
  const options = {humanId: 0, sinceTick: 70000, botIds: [768, 769]};
  const result = counterfireAssignmentWitness(assignmentLines(), options);
  assert.equal(result.assignedBotId, 769, 'old controlled source768 is not the currently assigned hunter');
  assert.equal(counterfireAssignmentWitness(assignmentLines(768, 4465), options), null,
    'short position ticks cannot impersonate fresh full-width server traces');
  assert.equal(counterfireAssignmentWitness(assignmentLines().split('\n')[0], options), null,
    'a partial bot roster cannot establish the assignment');
  assert.throws(() => counterfireAssignmentWitness(assignmentLines().replace('assigned_human_player_id=65535',
    'assigned_human_player_id=0'), options), /multiple hunters/);
  const structured = [768, 769].map(id => JSON.stringify({fields: {message: 'server bot movement trace',
    server_tick: 70001, bot_id: id, life_seq: 4, assigned_human_player_id: id === 768 ? 0 : 65535,
    human_assignment_rotation: 0}})).join('\n');
  assert.equal(counterfireAssignmentWitness(structured, options).assignedBotId, 768);
});

async function alignmentFixture(mode = 'rotate') {
  const metrics = {selfPlayerId: 0, playerLifeSeq: 4, playerShip: 0, selfRosterShip: 0,
    playerAlive: true, serverPlayerAlive: true, latestSnapshotServerTick: 70000,
    localGameplayState: {zoneMapReady: true, zoneFilesReady: true, doorClockFresh: true,
      authorityReadinessSecurity: true, authorityPhase: 'Live', pendingShipResetPosition: false,
      pendingShipResetDeliveryId: null, authorityTransitionKind: null,
      authorityScopeServerSessionId: '1', authorityScopeArenaStreamId: '1'}};
  const server = {logs: []}, changes = [];
  const read = async () => structuredClone(metrics);
  const waitUntil = async (label, predicate, timeout) => {
    assert(timeout > 0 && timeout <= 90_000);
    const hunter = mode === 'aligned' || (mode !== 'never' && changes.length) ? 768 : 769;
    server.logs.push(assignmentLines(hunter, metrics.latestSnapshotServerTick + 1));
    assert.equal(await predicate(), true, label);
  };
  try {
    const result = await alignCounterfireAssignment({read, waitUntil, server, botIds: [768, 769], desiredBotId: 768,
      changeShip: async (before, ship) => {
        changes.push({beforeLife: before.playerLifeSeq, ship});
        metrics.playerLifeSeq += mode === 'skip-life' ? 2 : 1;
        metrics.playerShip = ship; metrics.selfRosterShip = ship; metrics.latestSnapshotServerTick += 2;
        if (mode === 'change-scope') metrics.localGameplayState.authorityScopeServerSessionId = '2';
        return changes.at(-1);
      }});
    return {result, changes};
  } catch (error) { return {error, changes}; }
}

test('counterfire setup rebinds the same controlled source through one genuine replacement life', async () => {
  const {result, error, changes} = await alignmentFixture();
  assert.ifError(error);
  assert.equal(result.keysOnly, true); assert.equal(result.desiredBotId, 768);
  assert.equal(result.humanLifeSeq, 5);
  assert.deepEqual(changes, [{beforeLife: 4, ship: 6}]);
  assert.deepEqual(result.observations.map(row => row.assignedBotId), [769, 768]);
  assert(result.observations.every(row => row.traces.find(trace => trace.bot_id === 768).life_seq === 4),
    'human setup must not substitute a different bot life for the controlled successor');
});

test('an already assigned source needs no ship change; failed rebinding cannot relax identity', async () => {
  const {result, error, changes} = await alignmentFixture('aligned');
  assert.ifError(error); assert.equal(result.humanLifeSeq, 4); assert.deepEqual(changes, []);
  for (const mode of ['never', 'skip-life', 'change-scope']) {
    const failed = await alignmentFixture(mode);
    assert(failed.error); assert.equal(failed.changes.length, 1);
    assert.match(failed.error.message, /never acquired|exact genuine replacement life/);
  }
});

test('facing preparation retires a newly assigned passive source without replacing the controlled source', async () => {
  const lives = new Map([[768, 4], [769, 1]]), retired = [];
  const result = await prepareBotFacingSources({botIds: [768, 769], passiveLifeSeqs: [1, 3],
    readTarget: async (id, timeout) => {
      assert(timeout > 0 && timeout <= 10_000); return {id, lifeSeq: lives.get(id)};
    },
    retire: async (target, timeout) => {
      assert(timeout > 0 && timeout <= 90_000); retired.push(target);
      return {victimId: target.id, victimLifeSeq: target.lifeSeq, lethal: true};
    },
    successor: async (target, timeout) => {
      assert(timeout > 0 && timeout <= 10_000);
      lives.set(target.id, target.lifeSeq + 1); return {id: target.id, lifeSeq: lives.get(target.id)};
    }});
  assert.equal(result.keysOnly, true);
  assert.deepEqual(retired, [{id: 769, lifeSeq: 1}]);
  assert.deepEqual(result.sources.map(row => row.target), [{id: 768, lifeSeq: 4}, {id: 769, lifeSeq: 2}]);
});

test('facing preparation fails closed on wrong bot, missing lethal, stale successor and exhausted setup', async (t) => {
  const base = {botIds: [768], passiveLifeSeqs: [1, 3],
    readTarget: async id => ({id, lifeSeq: 1}),
    retire: async target => ({victimId: target.id, victimLifeSeq: target.lifeSeq, lethal: true}),
    successor: async target => ({id: target.id, lifeSeq: 2})};
  for (const [change, expected] of [
    [{readTarget: async () => ({id: 769, lifeSeq: 1})}, /changed bot identity/],
    [{retire: async () => null}, /exact-life authoritative lethal/],
    [{successor: async target => target}, /presented live successor/],
    [{botIds: [768, 768]}, /invalid bounded/],
  ]) await assert.rejects(prepareBotFacingSources({...base, ...change}), expected);
  let tick = 0;
  t.mock.method(Date, 'now', () => tick++);
  await assert.rejects(prepareBotFacingSources({...base, setupBudgetMs: 1}), /exceeded budget/);
});

test('facing bait follows the fresh exact-life native hunter rather than roster order', () => {
  const metrics = {selfPlayerId: 0, playerLifeSeq: 4, playerShip: 0, selfRosterShip: 0,
    playerAlive: true, serverPlayerAlive: true, latestSnapshotServerTick: 70000,
    localGameplayState: {zoneMapReady: true, zoneFilesReady: true, doorClockFresh: true,
      authorityReadinessSecurity: true, authorityPhase: 'Live', pendingShipResetPosition: false,
      pendingShipResetDeliveryId: null, authorityTransitionKind: null},
    remotePlayers: [{id: 768, lifeSeq: 4, positionX: 8000, positionY: 8200},
      {id: 769, lifeSeq: 1, positionX: 8100, positionY: 8200}]};
  assert.equal(facingHunterTarget(metrics, [768, 769], assignmentLines()).id, 769);
  assert.equal(facingHunterTarget(metrics, [768, 769], assignmentLines(768)).id, 768);
  assert.equal(facingHunterTarget(metrics, [768, 769], assignmentLines(769, 68000)), null);
  assert.equal(facingHunterTarget({...metrics, playerAlive: false}, [768, 769], assignmentLines()), null);
  assert.equal(facingHunterTarget({...metrics, remotePlayers: [metrics.remotePlayers[0],
    {...metrics.remotePlayers[1], lifeSeq: 2}]}, [768, 769], assignmentLines()), null);
  assert.equal(facingHunterTarget(metrics, [768, 769], assignmentLines().split('\n')[0]), null);
});

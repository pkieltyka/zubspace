import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import path from 'node:path';
import { createSpawnRegionFixture } from './fixture_spawn_region.mjs';
import { loadRendezvousMap, planAuthoredApproach } from './player_rendezvous.mjs';
import { authoredMapGeometry, authoredSafetyDistance, bestBoostHeading,
  runwayAlongHeading } from '../../tests/e2e/authored_map_geometry.mjs';
import { CHAOS_WORMHOLE_FIELD_AVOID_PX } from '../../tests/e2e/chaos_safe_route.mjs';
import { brakeVelocityDisposition } from '../../tests/e2e/combat_replay.mjs';

// Native coordinator regressions over recorded setup poses and shipped map
// geometry. These virtual actors do not establish browser input/physics proof.
const combat = fs.readFileSync(new URL('../../tests/e2e/combat_replay.mjs', import.meta.url), 'utf8');
const keyboard = fs.readFileSync(new URL('../../tests/e2e/keyboard_chord_matrix.mjs', import.meta.url), 'utf8');
function between(source, first, next) {
  const start = source.indexOf(first), end = source.indexOf(next, start + first.length);
  assert.ok(start >= 0 && end > start);
  return source.slice(start, end);
}

async function replayDoor({ invalidBrake = false } = {}) {
  let approaches = 0, stalls = 0, brakes = 0;
  const pose = { playerX:7581.75, playerY:8360.93,
    localGameplayState:{ positionX:7581.75, positionY:8360.93,
      doorClockFresh:true, doorSeed:0, doorSeedNext:0, doorSeedAfterNext:0 } };
  const actor = { metricsState:async()=>({uiMetrics:structuredClone(pose)}),
    evaluate:async()=>{ stalls++; throw new Error('invalid staging must never arm a fault'); } };
  const context = vm.createContext({
    prepareStaleDoorRouteOrigin:async()=>({}),
    navigateAuthoredTarget:async()=>{ approaches++; return {reached:true}; },
    driveClientToWorldTarget:async()=>({reached:true}),
    brakeClientVelocity:async()=>({braked:!(invalidBrake && ++brakes === 2),speed:2.2403571143904717}),
    orientClientForVelocity:async()=>({oriented:true}), sleep:async()=>{},
  });
  vm.runInContext(between(combat, 'function overlapsDoorWithReferenceGameHull(',
    '\nasync function runStaleDoorReadinessGate('), context);
  vm.runInContext(between(combat, 'async function runStaleDoorReadinessGate(',
    '\nasync function '), context);
  let error;
  try { await context.runStaleDoorReadinessGate({actor,peer:{},server:{},playerId:1}); }
  catch (caught) { error = caught; }
  return {error,approaches,stalls};
}

test('captured quarter-pixel door overlap uses only three bounded pre-fault restages', async()=>{
  const r=await replayDoor();
  assert.match(r.error?.message ?? '', /setup restarted three bounded times before the fault seam/);
  assert.equal(r.approaches,3);
  assert.equal(r.stalls,0);
});

test('invalid braking remains a terminal staging failure with no fault injection', async()=>{
  const r=await replayDoor({invalidBrake:true});
  assert.match(r.error?.message ?? '', /actor could not stage safely/);
  assert.equal(r.approaches,1);
  assert.equal(r.stalls,0);
});

const geometry=authoredMapGeometry(fs.readFileSync(new URL('../../assets/zones/chaos-zone/bdhadar.lvl',import.meta.url)),
  {wormholeAvoidRadiusPx:CHAOS_WORMHOLE_FIELD_AVOID_PX});
async function replayRunway({ loseRunwayAfterTurn = false } = {}) {
  const metrics={playerX:7528,playerY:9480,playerVelocityX:0,playerVelocityY:0,
    playerOrientation:0,playerAlive:true,serverPlayerAlive:true,selfPlayerId:1,playerLifeSeq:2};
  let brakes=0,completionWasPassed=false;
  const visited=[], movingChecks=[];
  const actor={metricsState:async()=>({uiMetrics:structuredClone(metrics)}),releaseKeys:async()=>{}};
  const context=vm.createContext({ authoredRouteGeometry:geometry,
    distanceToAuthoredSafety:authoredSafetyDistance([]), brakeVelocityDisposition,
    bestBoostHeading,runwayAlongHeading,console:{log:()=>{}},compactMetrics:x=>x,
    prepareKeyboardLifecycleOrigin:async()=>({}),
    brakeClientVelocity:async()=>{
      brakes++; metrics.playerVelocityX=metrics.playerVelocityY=0;
      if(loseRunwayAfterTurn && brakes===2){metrics.playerX=7528;metrics.playerY=9480;}
      return {braked:true};
    },
    orientClientForProtocolDirection:async(_actor,dx,dy)=>{
      metrics.playerOrientation=(Math.atan2(dx,-dy)/(Math.PI*2)+1)%1;
      return {oriented:true};
    },
    waitUntil:async(_label,predicate)=>{assert.equal(await predicate(),true);},
    navigateAuthoredTarget:async(_actor,_destination,_server,options)=>{
      completionWasPassed=typeof options.complete==='function';
      assert.equal(completionWasPassed,true,'runway setup must stop at its real precondition, before the redundant centre detour');
      for(const point of [{x:7400,y:9480},{x:7400,y:9400}]){
        metrics.playerX=point.x;metrics.playerY=point.y;visited.push(point);
        metrics.playerVelocityX=100;
        movingChecks.push(await options.complete());
        metrics.playerVelocityX=0;
        if(await options.complete())return {completed:true,reached:false};
      }
      throw new Error('the retained second waypoint has the full authored runway');
    },
  });
  vm.runInContext(between(keyboard,'async function prepareChaosChordAttempt(',
    '\nasync function settleLifecycleClient('),context);
  const result=await context.prepareChaosChordAttempt(actor,{afterburner:true,server:{}});
  return {result,visited,movingChecks,completionWasPassed};
}

test('captured Chaos route stops at its earlier full runway, only at rest, and recertifies after turning',async()=>{
  const r=await replayRunway();
  assert.deepEqual(r.visited,[{x:7400,y:9480},{x:7400,y:9400}]);
  assert.deepEqual(r.movingChecks,[false,false]);
  assert.equal(r.result.runway.runwayPx,512);
  assert.equal(r.result.braked,true);
  assert.equal(r.result.runwayRoute.completed,true);
  assert.equal(r.result.runwayRoute.reached,false);
});

test('early runway completion cannot bypass the actual post-turn clearance check',async()=>{
  await assert.rejects(replayRunway({loseRunwayAfterTurn:true}),/actual settled heading lacks its512px runway/);
});


test('door-only declared spawn matches its canonical recovery centre and full-hull approach', t => {
  const setup = between(combat, '  const staleDoorSpawnFixture =', '\n  const declaredSpawnFixture =');
  for (const enabled of [false, true]) {
    const context = vm.createContext({ staleDoorGate: enabled, arena: 'chaos-zone', createSpawnRegionFixture });
    const fixture = vm.runInContext(setup + '\nstaleDoorSpawnFixture;', context);
    if (!enabled) {
      assert.equal(fixture, null, 'ordinary gameplay must keep its production spawn policy');
      continue;
    }
    t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
    assert.equal(fixture.spawnRegion.x * 16 + 8, 8200);
    assert.equal(fixture.spawnRegion.y * 16 + 8, 8200);
    assert.equal(fixture.spawnRegion.radius, 0);
    const { geometry } = loadRendezvousMap('chaos-zone', [fixture.root]);
    assert.equal(geometry.authoredHullIsTraversable(8200, 8200, 20), true);
    const route = planAuthoredApproach(geometry, { x: 8200, y: 8200 },
      { x: 7620, y: 8360 }, { arrivalRadiusPx: 12 });
    assert.ok(route.routes[0].drivePoints.length > 0);
    const original = fs.readFileSync(new URL('../../assets/zones/chaos-zone/arena.conf', import.meta.url), 'utf8');
    const changed = fs.readFileSync(fixture.root + '/chaos-zone/arena.conf', 'utf8');
    const maskSpawn = settings => settings.replace(/^(Team[0-3]-(X|Y|Radius))=-?\d+/gm, '$1=<spawn>');
    assert.equal(maskSpawn(changed), maskSpawn(original), 'every non-spawn arena setting stays identical');
    assert.deepEqual(fs.readFileSync(fixture.root + '/chaos-zone/bdhadar.lvl'),
      fs.readFileSync(new URL('../../assets/zones/chaos-zone/bdhadar.lvl', import.meta.url)));
  }
});


test('door route preparation delegates the retained open-door pose with its exact life and real geometry', async () => {
  const pose = { selfPlayerId: 1, playerLifeSeq: 7, playerX: 7581.62, playerY: 8358.84 };
  const actor = { evaluate: async () => 'chaos-zone', metricsState: async () => ({ uiMetrics: pose }) };
  const server = {}, root = path.resolve(new URL('../..', import.meta.url).pathname);
  let delegated = null;
  const context = vm.createContext({ path, repoRoot: root, process: { env: {} }, loadRendezvousMap,
    prepareKeyboardLifecycleOrigin: async options => {
      delegated = options;
      assert.equal(options.actor, actor);
      assert.equal(options.server, server);
      assert.equal(options.playerId, 1);
      assert.equal(options.initialLifeSeq, 7);
      assert.equal(options.geometry.authoredHullIsTraversable(pose.playerX, pose.playerY, 20), false);
      assert.ok(options.deadline > Date.now() && options.deadline <= Date.now() + 90_000);
      return { warps: [{ outcome: 1, life_seq: 7 }] };
    } });
  vm.runInContext(between(combat, 'async function prepareStaleDoorRouteOrigin(',
    '\nfunction overlapsDoorWithReferenceGameHull('), context);
  const result = await context.prepareStaleDoorRouteOrigin({ actor, server, playerId: 1 });
  assert.ok(delegated);
  assert.equal(result.warps[0].life_seq, 7);
});

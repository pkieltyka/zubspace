import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import { movingBombAttackPlan, movingBombControlStep, movingBombOwnerClearance, movingBombNeedsVelocityControl } from "./moving_bomb_pilot.mjs";

const geometry = { relativeX: 0, relativeY: -352,
  targetVelocityX: 0, targetVelocityY: 190, playerVelocityX: 0, playerVelocityY: 190,
  projectileSpeed: 200, maximumFlightSeconds: 2, explosionRadius: 160,
  proximityRadius: 72, currentDirection: 0 };

test("four retained native self-splashes are inside the authored blast at actual contact time", () => {
  const fixture = JSON.parse(fs.readFileSync("tests/fixtures/moving_bomb_self_splash_8137398.json"));
  assert.equal(fixture.contacts.length, 4);
  assert.deepEqual(fixture.contacts.map(row => row.adjusted_damage), [266, 490, 198, 538]);
  for (const row of fixture.contacts) {
    assert.equal(row.owner_id, row.target_id);
    assert.equal(row.explosion_radius, 160);
    const time = (row.collision_tick - row.projectile_created_tick) / 100;
    const clearance = movingBombOwnerClearance(time, 200, row.explosion_radius);
    assert.equal(clearance.clear, false);
    // Retained owner pose and the inherited-velocity calculation agree to
    // about one tick/pixel truncation; this is an observed contact oracle,
    // not a claim to reconstruct unretained launch-time target motion.
    assert.ok(Math.abs(clearance.separation - row.collision_center_distance) < 3);
    assert.ok(row.collision_center_distance < row.explosion_radius);
  }
});

test("matching an approaching bot's motion gives a safe inherited-velocity retreating shot", () => {
  const plan = movingBombAttackPlan(geometry);
  assert.equal(plan.candidate.direction, 0);
  assert.equal(plan.current.safeToAttempt, true);
  assert.equal(plan.current.time, 1.76);
  assert.equal(plan.current.predictedOwnerSeparation, 352);
  assert.equal(plan.velocityError, 0);
  // The bomb has only -10px/s world velocity; the190px/s pursuing bot
  // reaches it while its owner keeps retreating190px/s away from the blast.
  assert.equal(geometry.playerVelocityY - geometry.projectileSpeed, -10);
});

test("the same distance can be unsafe at a different relative speed", () => {
  const plan = movingBombAttackPlan({ ...geometry, playerVelocityY: -190 });
  assert.equal(plan.current.safeToAttempt, false);
  assert.ok(plan.current.time < 0.7);
  // A different firing direction can have a later valid intercept. Do not
  // falsely certify the unsafe current direction from that other solution.
  assert.equal(plan.candidate.direction, 20);
  assert.ok(plan.candidate.time > 1.9);
});

test("a mathematically valid intercept cannot substitute the current-target line for its own blocked path", () => {
  const paths = [];
  const plan = movingBombAttackPlan({ ...geometry, playerVelocityY: -190,
    projectilePathIsClear: (x, y, seconds) => { paths.push({ x, y, seconds }); return y < 0; } });
  assert.ok(paths.some(path => path.y > 0), "the rear intercept really travels in the other map direction");
  assert.equal(plan.candidate, null);
  assert.equal(plan.current.safeToAttempt, false);
});

test("short close-range, expired, receding and wrong-facing shots do not qualify", () => {
  for (const changed of [
    { relativeY: -140 }, { relativeY: -800 },
    { playerVelocityY: 0, targetVelocityY: -300 }, { currentDirection: 20 },
  ]) {
    const plan = movingBombAttackPlan({ ...geometry, ...changed });
    assert.equal(plan.current.safeToAttempt, false);
  }
  for (const changed of [{ projectileSpeed: NaN }, { explosionRadius: 0 },
    { currentDirection: -1 }, { currentDirection: 40 }, { proximityRadius: undefined }]) {
    assert.throws(() => movingBombAttackPlan({ ...geometry, ...changed }), /geometry/);
  }
});

test("a blocked retreat selects a checked lateral path or declares all paths blocked", () => {
  const seen = [];
  const plan = movingBombAttackPlan({ ...geometry,
    velocityPathIsClear: (x, y) => { seen.push([x, y]); return Math.abs(y) < 1; } });
  assert.equal(plan.pathBlocked, false);
  assert.ok(Math.abs(plan.desiredVelocityX) > 100);
  assert.equal(plan.desiredVelocityY, 0);
  assert.equal(seen.length, 2);
  assert.equal(movingBombAttackPlan({ ...geometry, velocityPathIsClear: () => false }).pathBlocked, true);
});

test("aligned reverse thrust moves away without turning the bow away from its target", () => {
  const plan = movingBombAttackPlan({ ...geometry, relativeY: -140, playerVelocityY: 0 });
  const step = movingBombControlStep(plan, { playerVelocityX: 0, playerVelocityY: 0, playerOrientation: 0 });
  assert.deepEqual(step.keys, ["s"]);
  assert.equal(step.action, "thrust");
  assert.ok(step.holdMs > 0 && step.holdMs <= 55);
});

test("a safe firing solution gets aim priority over standoff velocity changes", () => {
  const plan = movingBombAttackPlan({ ...geometry, playerVelocityY: 100 });
  assert.ok(plan.velocityError > 24);
  assert.equal(plan.current.safeToAttempt, true);
  assert.equal(plan.current.direction, plan.candidate.direction);
  assert.equal(movingBombNeedsVelocityControl(plan, true), false);
  // Range, source/target safety and clear geometry remain caller-owned gates.
  assert.equal(movingBombNeedsVelocityControl(plan, false), true);
  assert.equal(movingBombNeedsVelocityControl({ ...plan, candidate: null }, true), true);
  assert.equal(movingBombNeedsVelocityControl({ ...plan,
    current: { ...plan.current, direction: 20 } }, true), false,
    "turn toward the safe solution instead of discarding it for another velocity target");
  assert.equal(movingBombNeedsVelocityControl({ ...plan,
    current: { ...plan.current, safeToAttempt: false } }, true), false,
    "the aim branch, rather than this navigation decision, still owns firing admission");
  assert.equal(movingBombNeedsVelocityControl({ ...plan,
    candidate: { ...plan.candidate, safeToAttempt: false } }, true), true,
    "an unsafe prospective shot still needs velocity control");
});

const arenaConfig = fs.readFileSync("assets/zones/chaos-zone/arena.conf", "utf8");
const sections = {};
let section;
for (const raw of arenaConfig.split(/\r?\n/)) {
  const line = raw.split(";", 1)[0].trim();
  if (/^\[.*\]$/.test(line)) section = line.slice(1, -1);
  const pair = line.match(/^([^=]+)=(\d+)$/);
  if (pair) (sections[section] ||= {})[pair[1]] = Number(pair[2]);
}
const warbird = { ...sections.AllShips, ...sections.Warbird };
const actuatorProfiles = [
  { name: "authored maximum Warbird", acceleration: warbird.MaximumThrust * 10,
    speedCap: warbird.MaximumSpeed / 10, rotationPerSecond: warbird.MaximumRotation / 400 },
  { name: "earlier high-acceleration probe", acceleration: 367, speedCap: 290, rotationPerSecond: 0.625 },
];
for (const actuator of actuatorProfiles) for (const initialDistance of [140, 400, 600]) {
  test(`actual planner/control loop reaches a blast-safe moving standoff from${initialDistance}px (${actuator.name})`, () => {
    // Independent inertial actuators with60Hz key quantization, including
    // the actual authored maximum Warbird movement envelope. Native check, not combat
    // acceptance: no server bot AI, damage, map or browser is simulated.
    const actor = { playerX: 0, playerY: 0, playerVelocityX: 0, playerVelocityY: 0, playerOrientation: 0 };
    const target = { x: 0, y: -initialDistance };
    let safeMovingFrames = 0, elapsed = 0;
    for (let pulse = 0; pulse < 800 && elapsed < 20; pulse++) {
      const plan = movingBombAttackPlan({ ...geometry,
        relativeX: target.x - actor.playerX, relativeY: target.y - actor.playerY,
        playerVelocityX: actor.playerVelocityX, playerVelocityY: actor.playerVelocityY,
        currentDirection: Math.floor(actor.playerOrientation * 40) % 40 });
      const step = movingBombControlStep(plan, actor);
      if (plan.velocityError <= 24 && plan.current.safeToAttempt &&
          Math.abs(actor.playerVelocityY) > 150 && Math.abs(plan.distance - plan.desiredRange) < 40) safeMovingFrames++;
      const frames = Math.max(1, Math.round(step.holdMs * 60 / 1000));
      for (let frame = 0; frame < frames; frame++) {
        const dt = 1 / 60;
        if (step.keys.includes("a")) actor.playerOrientation -= actuator.rotationPerSecond * dt;
        if (step.keys.includes("d")) actor.playerOrientation += actuator.rotationPerSecond * dt;
        actor.playerOrientation = (actor.playerOrientation + 1) % 1;
        const thrust = step.keys.includes("w") ? 1 : step.keys.includes("s") ? -1 : 0;
        actor.playerVelocityX += Math.sin(actor.playerOrientation * 2 * Math.PI) * thrust * actuator.acceleration * dt;
        actor.playerVelocityY -= Math.cos(actor.playerOrientation * 2 * Math.PI) * thrust * actuator.acceleration * dt;
        const speed = Math.hypot(actor.playerVelocityX, actor.playerVelocityY);
        if (speed > actuator.speedCap) { actor.playerVelocityX *= actuator.speedCap / speed; actor.playerVelocityY *= actuator.speedCap / speed; }
        actor.playerX += actor.playerVelocityX * dt; actor.playerY += actor.playerVelocityY * dt;
        target.y += 190 * dt;
        elapsed += dt;
      }
    }
    assert.ok(safeMovingFrames >= 30, `need sustained safe moving geometry, got${safeMovingFrames}`);
  });
}


test("a turning retreat makes velocity progress before perfect heading alignment", () => {
  // A within-bin pose for the retained ~135px/s retreat correction
  // reproduces its small-turn deadzone; this is an inertial control probe,
  // not a claim to reconstruct unretained server motion.
  const plan={desiredVelocityX:-289.21992219460816,desiredVelocityY:-79.69841030876847};
  const state={playerVelocityX:-153.69,playerVelocityY:-81.27,playerOrientation:.2875};
  const before=Math.hypot(plan.desiredVelocityX-state.playerVelocityX,plan.desiredVelocityY-state.playerVelocityY);
  const step=movingBombControlStep(plan,state);
  // Independent inertial actuators using the authored Warbird envelope.
  const frames=Math.max(1,Math.round(step.holdMs*60/1000));
  for(let frame=0;frame<frames;frame++){
    state.playerOrientation=(state.playerOrientation+(step.keys.includes('d')?1:step.keys.includes('a')?-1:0)*actuatorProfiles[0].rotationPerSecond/60+1)%1;
    const sign=step.keys.includes('w')?1:step.keys.includes('s')?-1:0;
    state.playerVelocityX+=Math.sin(state.playerOrientation*2*Math.PI)*sign*actuatorProfiles[0].acceleration/60;
    state.playerVelocityY-=Math.cos(state.playerOrientation*2*Math.PI)*sign*actuatorProfiles[0].acceleration/60;
  }
  const after=Math.hypot(plan.desiredVelocityX-state.playerVelocityX,plan.desiredVelocityY-state.playerVelocityY);
  assert.ok(after<before-3,`turning must reduce velocity error,${before}->${after}`);
  const wide=movingBombControlStep({desiredVelocityX:100,desiredVelocityY:0},
    {playerVelocityX:0,playerVelocityY:0,playerOrientation:0});
  assert.ok(!wide.keys.some(key=>key==='w'||key==='s'),'a perpendicular correction must turn before accelerating');
});


test("a safe current wire heading fires without chasing a slightly smaller miss in another cell", () => {
 const g={...geometry,relativeX:5,relativeY:-220,targetVelocityY:150,playerVelocityY:150,currentDirection:1};
 const plan=movingBombAttackPlan(g);
 assert.equal(plan.current.safeToAttempt,true);
 assert.ok(plan.current.missDistance>20&&plan.current.missDistance<=32);
 assert.equal(plan.candidate.direction,1,'an already valid shot must not wait for a new heading');
 const blocked=movingBombAttackPlan({...g,projectilePathIsClear:x=>x<=0});
 assert.equal(blocked.current.safeToAttempt,false);
 assert.equal(blocked.candidate.direction,0,'blocked current geometry still requires another clear heading');
 const wide=movingBombAttackPlan({...g,currentDirection:2});
 assert.equal(wide.current.safeToAttempt,false);
 assert.notEqual(wide.candidate.direction,2,'the existing miss limit cannot be bypassed');
});

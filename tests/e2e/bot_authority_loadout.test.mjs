import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import { botAuthorityLoadout, assertBotAuthorityLoadout, stationaryWarbirdBombLaneMinimum,
  botBombSafetyProfile, botBombSafetyFloor } from "./bot_authority_loadout.mjs";

test('maximum-entry moving bomb setup honors actual Extreme safety after a Lancaster respawn', () => {
  const config = fs.readFileSync(new URL('../../assets/zones/extreme/arena.conf', import.meta.url), 'utf8');
  const maximum = botBombSafetyProfile(botAuthorityLoadout({}, 'extreme'), config);
  assert.equal(botBombSafetyFloor(maximum, 3), 52);
  assert.ok(botBombSafetyFloor(maximum, 3) > 44.4, 'retained no-projectile key edges were inside the48px exclusion');
  assert.equal(botBombSafetyFloor(maximum, 2), 36, 'use the current bomb level, not a Warbird-only constant');
  assert.equal(botBombSafetyFloor(maximum, 0), 0);
  assert.equal(botBombSafetyFloor(botBombSafetyProfile(botAuthorityLoadout({ZUBSPACE_E2E_SPAWN_LOADOUT:'reference'}, 'extreme'), config), 3), 0);
  assert.equal(botBombSafetyFloor(botBombSafetyProfile(botAuthorityLoadout({}, 'extreme'), config.replace('BombSafety=1', 'BombSafety=0')), 3), 0);
});

test('Chaos retains its original L2 floor while live higher levels require their own radius', () => {
  const config = fs.readFileSync(new URL('../../assets/zones/chaos-zone/arena.conf', import.meta.url), 'utf8');
  const profile = botBombSafetyProfile(botAuthorityLoadout({}, 'chaos-zone'), config);
  assert.equal(botBombSafetyFloor(profile, 2), 68);
  assert.equal(botBombSafetyFloor(profile, 3), 84);
  for (const level of [undefined, NaN, -1, 5]) assert.throws(() => botBombSafetyFloor(profile, level), /live bomb level/);
  for (const changed of [config.replace('BombSafety=1', ''), config.replace('BombSafety=1', 'BombSafety=no'),
    config.replace('BombSafety=1', 'BombSafety=1\nBombSafety=1')]) {
    assert.throws(() => botBombSafetyProfile(botAuthorityLoadout({}, 'chaos-zone'), changed), /authored/);
  }
});

test("stationary maximum-entry staging rejects the retained BombSafety overshoot lane", () => {
  const arenaConfig = fs.readFileSync(new URL("../../assets/zones/extreme/arena.conf", import.meta.url), "utf8");
  const options = { arenaConfig, expectedExitSpeed: 200, rotationPerSecond: 300 / 400 };
  const maximum = stationaryWarbirdBombLaneMinimum({ ...options,
    fixture: botAuthorityLoadout({}, "extreme") });
  const direct = stationaryWarbirdBombLaneMinimum({ ...options,
    fixture: botAuthorityLoadout({ ZUBSPACE_E2E_SPAWN_LOADOUT: "reference" }, "extreme") });
  // Retained exact life 3: the old 66px minimum admitted this lane. X was
  // pressed at 45.14px (inside both the old 54px and corrected 48px exclusions), then the accepted
  // shot traveled away from the bot after the pilot crossed to its far side.
  const unsafeLaneDistance = Math.hypot(7882 - 7944, 8161 - 8136);
  assert.ok(direct < unsafeLaneDistance);
  assert.ok(maximum > unsafeLaneDistance);
  // This wider candidate clears the distance guard; map/route eligibility is
  // a separate browser decision. Retain BombSafety and exact lethal proof.
  assert.ok(maximum < Math.hypot(7882 - 7992, 8161 - 8136));
  assert.equal(maximum, 96);
  assert.equal(direct, 66);
  assert.equal(stationaryWarbirdBombLaneMinimum({ ...options,
    expectedExitSpeed: 170, fixture: botAuthorityLoadout({}, "extreme") }), 93);
});

test("stationary bomb staging follows authored safety policy and rejects missing settings", () => {
  const arenaConfig = fs.readFileSync(new URL("../../assets/zones/extreme/arena.conf", import.meta.url), "utf8");
  const options = { arenaConfig, expectedExitSpeed: 200, rotationPerSecond: 0.75,
    fixture: botAuthorityLoadout({}, "extreme") };
  assert.equal(stationaryWarbirdBombLaneMinimum({ ...options,
    arenaConfig: arenaConfig.replace("BombSafety=1", "BombSafety=0") }), 66);
  assert.throws(() => stationaryWarbirdBombLaneMinimum({ ...options,
    arenaConfig: arenaConfig.replace("ProximityDistance=1", "") }), /ProximityDistance/);
});

test("maximum entry requires exact proximity bombs; reference Extreme retains exact direct bombs", () => {
  const retainedLethal = { weapon: 17092, victimId: 768, victimLifeSeq: 1, lethal: true };
  for (const arena of ["chaos-zone", "extreme"]) {
    const fixture = botAuthorityLoadout({}, arena);
    assert.equal(fixture.maxedSpawnLoadout, true);
    assert.equal(fixture.bombWeaponType, retainedLethal.weapon & 0x1f);
    assert.notEqual(fixture.bombWeaponType, 3);
    assert.notEqual(fixture.bombWeaponType, 66 & 0x1f);
  }
  const reference = botAuthorityLoadout({ ZUBSPACE_E2E_SPAWN_LOADOUT: "reference" }, "extreme");
  assert.equal(reference.maxedSpawnLoadout, false);
  assert.equal(reference.bombWeaponType, 3);
  assert.notEqual(reference.bombWeaponType, retainedLethal.weapon & 0x1f);
  assert.equal(botAuthorityLoadout({ ZUBSPACE_E2E_SPAWN_LOADOUT: "reference" }, "chaos-zone").bombWeaponType, 4);
});

test("unknown profiles and a reference-loadout aged lane fail closed", () => {
  assert.throws(() => botAuthorityLoadout({}, "other"), /unknown/);
  assert.throws(() => botAuthorityLoadout({ ZUBSPACE_E2E_SPAWN_LOADOUT: "typo" }, "extreme"), /unknown/);
  assert.throws(() => botAuthorityLoadout({ ZUBSPACE_E2E_SPAWN_LOADOUT: "reference",
    ZUBSPACE_E2E_JOIN_MIN_UPTIME_SECONDS: "350" }, "extreme"), /requires production maximum/);
});

test("server status must confirm the declared loadout and initial arena", async (t) => {
  const fixture = botAuthorityLoadout({}, "extreme");
  t.mock.method(globalThis, "fetch", async url => {
    const path = new URL(url).pathname;
    const payload = path === "/v1/status" ? {ok:true,service:"zubspace-server",total_players:0} :
      path === "/v1/zones" ? {total_players:0,zones:[{id:"chaos-zone",status:"open",default:true,total_players:0,free_for_all:true,maxed_spawn_loadout:true}]} :
      path === "/v1/players" ? {total_players:0,players:[]} :
      {ok:true,zone:"chaos-zone",status:"open",total:0,snapshot_acked_clients:0};
    return Response.json(payload);
  });
  assert.equal((await assertBotAuthorityLoadout(9002, fixture, "chaos-zone")).observedMaxedSpawnLoadout, true);
  await assert.rejects(assertBotAuthorityLoadout(9002, fixture, "extreme"), /mismatch/);
  await assert.rejects(assertBotAuthorityLoadout(9002, { ...fixture, maxedSpawnLoadout: false }, "chaos-zone"), /mismatch/);
});

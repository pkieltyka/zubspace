import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { assertMenuFlightIsolation, captureMenuFlight } from './menu_flight_evidence.mjs';

function fixture() {
  const scope = { playerId: 0, lifeSeq: 3, session: '77', arena: '2',
    authorityPlayerId: 0, authorityLifeSeq: 3, alive: true, phase: 'Live' };
  const frame = { renderOrdinal: 1871, clientInputSeq: 16087, selfPlayerId: 0, playerLifeSeq: 3,
    playerX: 8615.34, playerY: 8812.85, playerVelocityX: -0.02, playerVelocityY: 2.09,
    playerOrientation: 0.3625, localInputBits: 0 };
  const frames = Array.from({ length: 106 }, (_, i) => ({ ...frame,
    renderOrdinal: frame.renderOrdinal + i + 1, clientInputSeq: frame.clientInputSeq + i + 1,
    playerX: frame.playerX + frame.playerVelocityX * (i + 1) / 60,
    playerY: frame.playerY + frame.playerVelocityY * (i + 1) / 60 }));
  return { before: { scope, layer: { list: true, menu: true }, frame },
    after: { scope: { ...scope }, layer: { list: true, menu: true }, frame: frames.at(-1), frames } };
}

for (const layer of ['list', 'menu']) test(`${layer} blocks inputs while preserving ordinary inertial flight`, () => {
  const { before, after } = fixture();
  assert.ok(Math.hypot(after.frame.playerX - before.frame.playerX, after.frame.playerY - before.frame.playerY) > 1,
    'the original fixed-position oracle rejects this legal coast');
  assert.equal(assertMenuFlightIsolation(before, after, layer).renderFrames, 106);
});

for (const [name, change] of [
  ['one sampled thrust', a => { a.frames[30].localInputBits = 1; }],
  ['a cancelled intermediate turn', a => { a.frames[30].playerOrientation += 0.01; }],
  ['an intermediate acceleration', a => { a.frames[30].playerVelocityY += 5; }],
  ['a displaced trajectory', a => { a.frames[30].playerX += 2; }],
  ['missing frames', a => { a.frames.splice(30, 1); }],
  ['repeated render ordinal', a => { a.frames[30].renderOrdinal--; }],
  ['wrong life', a => { a.frames[30].playerLifeSeq++; }],
  ['inactive layer', a => { a.layer.list = false; }],
  ['changed session', a => { a.scope.session = '78'; }],
]) test(`menu isolation refuses ${name}`, () => {
  const { before, after } = fixture(); change(after);
  assert.throws(() => assertMenuFlightIsolation(before, after, 'list'));
});

test('canonical input sequence wrap preserves the same inertial proof', () => {
  const { before, after } = fixture(); before.frame.clientInputSeq = 65530;
  after.frames.forEach((f, i) => { f.clientInputSeq = (65531 + i) & 0xffff; });
  assert.equal(assertMenuFlightIsolation(before, after, 'menu').inputSteps, 106);
});


test('the browser collector only reads the bounded original frame rows and current scope', async () => {
  const { before, after } = fixture();
  const window = { __zubspaceMetricsTrace: [before.frame, ...after.frames],
    __zubspaceUiMetrics: { selfPlayerId: 0, playerLifeSeq: 3, scoreboardVisible: true,
      localGameplayState: { menuVisible: false, authorityScopeServerSessionId: '77',
        authorityScopeArenaStreamId: '2', authorityScopePlayerId: 0, authorityScopeLifeSeq: 3,
        authorityScopeAlive: true, authorityPhase: 'Live' } } };
  const original = JSON.stringify(window);
  const client = { evaluate: async expression => JSON.parse(JSON.stringify(vm.runInNewContext(expression, {window}))) };
  const captured = await captureMenuFlight(client, before.frame.renderOrdinal);
  assert.equal(captured.frames.length, 106);
  assert.deepEqual(captured.frame, after.frame);
  assert.equal(JSON.stringify(window), original, 'collection cannot alter game state or the original trace');
  assert.equal(assertMenuFlightIsolation(before, captured, 'list').renderFrames, 106);
});

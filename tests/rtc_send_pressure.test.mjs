import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const html = fs.readFileSync('index.html', 'utf8');
function actualFunction(name) {
  const begin = html.indexOf(`          function ${name}(`);
  const end = html.indexOf('\n          }', begin);
  assert.ok(begin >= 0 && end > begin, `actual ${name} source required`);
  return html.slice(begin, end + '\n          }'.length);
}
const limit = html.match(/const RTC_GAME_BUFFERED_DROP_THRESHOLD = ([^;]+);/);
assert.ok(limit);
const drop = vm.runInNewContext(`(() => {
  const RTC_GAME_BUFFERED_DROP_THRESHOLD = ${limit[1]};
  ${actualFunction('clientPositionWeapon')}
  ${actualFunction('shouldDropRealtimeSend')}
  return shouldDropRealtimeSend;
})()`);
function position(weapon = 0) {
  const packet = new Uint8Array(635);
  packet[0] = 3; packet[20] = weapon & 255; packet[21] = weapon >> 8;
  return packet;
}
const channel = bufferedAmount => ({label: 'zubspace-game', bufferedAmount});
test('real browser send policy retains fresh history and stops a disposable queue before 2 KiB', () => {
  assert.equal(drop(position(), channel(0)), false);
  assert.equal(drop(position(), channel(1413)), false);
  assert.equal(drop(position(), channel(1414)), true);
  const repair = new Uint8Array(580); repair[0] = 0x62;
  assert.equal(drop(repair, channel(1468)), false);
  assert.equal(drop(repair, channel(1469)), true);
  assert.equal(drop(repair, channel(16384)), true,
    'standalone catch-up repairs cannot bypass the same queue bound');
});
test('pressure never suppresses weapon edges, quality echoes or reliable/event traffic', () => {
  for (const weapon of [1, 3, 4, 15, 0xffff]) {
    assert.equal(drop(position(weapon), channel(1e6)), false);
  }
  for (const type of [0x82, 0, 0x70, 0x7d]) {
    const packet = new Uint8Array(30); packet[0] = type;
    assert.equal(drop(packet, channel(1e6)), false);
  }
  for (const label of ['zubspace-events', 'zubspace-reliable']) {
    assert.equal(drop(position(), {label, bufferedAmount: 1e6}), false);
  }
});

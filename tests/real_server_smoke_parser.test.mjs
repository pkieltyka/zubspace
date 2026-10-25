import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";
import { spawnSync } from "node:child_process";

const source = fs.readFileSync("tools/browser_real_server_smoke.mjs", "utf8");

test("cold weapon evidence keeps captured born/terminal bytes and reports bounded omissions", () => {
  const start = source.indexOf("function retainedSmokeWeaponPackets(");
  const end = source.indexOf("\nfunction channelDeliveryAudit(", start);
  assert.ok(start >= 0 && end > start);
  const retain = vm.runInNewContext(`${source.slice(start, end)};retainedSmokeWeaponPackets`, { Buffer });
  const captured = JSON.parse(fs.readFileSync("tests/fixtures/spectator_instant_hit_sound.json"));
  const frames = { sent: [Buffer.from([0x24, 1, 2])], received: [] };
  for (const entry of captured.packets) {
    for (const channel of ["zubspace-game", "zubspace-events"]) {
      const packet = Buffer.from(entry.hex, "hex");
      packet.zubspaceDetail = { at: entry.at, channel };
      frames.received.push(packet);
    }
  }
  frames.received.push(Buffer.from([0x2a, 1, 2, 3]));
  const evidence = retain(frames);
  assert.equal(evidence.total, 4);
  assert.equal(evidence.complete, true);
  assert.equal(evidence.omitted, 0);
  assert.ok(evidence.packets.every((packet, index) =>
    Buffer.from(packet.bytes).equals(frames.received[index]) &&
    packet.detail.channel === frames.received[index].zubspaceDetail.channel));
  const bounded = retain(frames, 3);
  assert.equal(bounded.complete, false);
  assert.equal(bounded.omitted, 1);
  assert.equal(bounded.packets.length, 3);
  const prefix = Buffer.from(frames.received[0].subarray(0, 1));
  prefix.zubspaceDetail = { ...frames.received[0].zubspaceDetail,
    byteLength: frames.received[0].length, bytesTruncated: true, packetSequence: 123 };
  const truncated = retain({ sent: [], received: [prefix] });
  assert.equal(truncated.complete, false);
  assert.equal(truncated.truncatedPackets, 1);
  assert.equal(truncated.packets[0].detail.packetSequence, 123);
  assert.equal(truncated.packets[0].retainedByteLength, 1);
  assert.equal(truncated.packets[0].byteLength, frames.received[0].length);
});

for (const filename of ["tools/browser_real_server_smoke.mjs", "tests/e2e/bot_authority_firefox.mjs", "tests/e2e/bot_authority.mjs"]) {
test(`actual ${filename} failure handler flushes a retained-size diagnostic through stderr`, () => {
  // The8137398 dynamic-zone failure retains329,835 characters in failure.txt,
  // but the child pipe ends mid-string. Exercise the actual terminal handler
  // without opening a browser; the old forced exit truncates a large write.
  const payload = "Error: timed out waiting for duplicated game/event weapon carriers\n" + "x".repeat(330_000);
  const script = `import fs from 'node:fs';import vm from 'node:vm';
    const source=fs.readFileSync(process.argv[1],'utf8');
    const marker='main().catch(';
    const body=source.slice(source.lastIndexOf(marker)+marker.length,source.lastIndexOf(');'));
    const selected=process.argv[2]==='control'?body.replace('process.exit(1)','(process.exitCode = 1)'):body;
    const handler=vm.runInNewContext('('+selected+')',{console,process});
    const error=new Error();error.stack=fs.readFileSync(0,'utf8');
    handler(error);`;
  const control = spawnSync(process.execPath, ["--input-type=module", "-e", script, filename, "control"],
    { input: payload, encoding: "utf8", maxBuffer: 1_000_000 });
  const child = spawnSync(process.execPath, ["--input-type=module", "-e", script, filename],
    { input: payload, encoding: "utf8", maxBuffer: 1_000_000 });
  assert.ifError(child.error);
  assert.ifError(control.error);
  assert.equal(child.status, 1);
  assert.equal(control.status, 1);
  assert.ok(control.stderr.includes(payload), "the native console control retains the complete diagnostic");
  assert.equal(child.stderr.length, control.stderr.length, "failure diagnostics must not end inside the JSON/string");
  assert.ok(child.stderr === control.stderr, "the handler retains the native console's complete formatting");
});
}
const definition = source.slice(source.indexOf("function logicalReliablePayloads("),
  source.indexOf("function parseReceivedPositionUpdates("));
const parse = vm.runInNewContext(`${definition}\nlogicalReliablePayloads`, {
  Buffer,
  reliablePayload: packet => packet.subarray(6),
  reliableMessageId: packet => packet.readUInt32LE(2),
});
function packet(origin, id, payload) {
  const result = Buffer.alloc(6 + payload.length);
  result[1] = 3;
  result.writeUInt32LE(id, 2);
  Buffer.from(payload).copy(result, 6);
  result.zubspaceDetail = { documentOrigin: origin };
  return result;
}

test("reloaded pages may reuse reliable IDs while retransmissions in a page stay deduplicated", () => {
  const result = parse([packet(100, 0, [0x24, 1]), packet(100, 0, [0x24, 1]), packet(200, 0, [0x24, 2])]);
  assert.deepEqual(Array.from(result, buffer => [...buffer]), [[0x24, 1], [0x24, 2]]);
});

test("an incomplete reliable chunk cannot cross a page/session boundary", () => {
  const result = parse([packet(100, 0, [0, 8, 1]), packet(200, 0, [0, 8, 2]), packet(200, 1, [0, 9, 3])]);
  assert.deepEqual(Array.from(result, buffer => [...buffer]), [[2, 3]]);
});


test("snapshot reader consumes persistent motion generations before the following entity", () => {
  const start = source.indexOf("const SNAPSHOT_FIELD_LEN =");
  const end = source.indexOf("function parseBatchedPositionUpdates(", start);
  const readSnapshot = vm.runInNewContext(
    `${source.slice(start, end)}\nparseSnapshotPositionUpdates`, { Buffer });
  const header = Buffer.alloc(21);
  header[0] = 0x70;
  header[1] = 7;
  header.writeUInt16LE(1, 2);
  header.writeUInt32LE(100, 6);
  header[18] = 1;
  header[19] = 1;
  const near = Buffer.alloc(12);
  near.writeUInt16LE(768);
  near.writeUInt16LE(0x2003, 2);
  near.writeUInt16LE(100, 4);
  near.writeUInt16LE(200, 6);
  near.writeUInt32LE(2, 8);
  const mid = Buffer.alloc(8);
  mid.writeUInt16LE(9);
  mid.writeUInt16LE(3, 2);
  mid.writeUInt16LE(555, 4);
  mid.writeUInt16LE(666, 6);
  for (const flags of [7, 15]) {
    header[1] = flags;
    header[19] = 1;
    const encodedNear = flags === 15 ? Buffer.concat([near, Buffer.from([1, 0, 0, 0])]) : near;
    const packet = Buffer.concat([header, encodedNear, mid]);
    const updates = readSnapshot(packet, 123, "game");
    assert.deepEqual(Array.from(updates, ({ source, x, y }) => [source, x, y]),
      [[768, 100, 200], [9, 555, 666]]);
    header[19] = 0;
    assert.equal(readSnapshot(Buffer.concat([header, encodedNear.subarray(0, encodedNear.length - 2)]), 123, "game").length, 0);
  }
});

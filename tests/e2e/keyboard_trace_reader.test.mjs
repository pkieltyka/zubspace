import assert from "node:assert/strict";
import test from "node:test";
import { createKeyboardTraceReader, keyboardTraceRecords,
  keyboardTraceReaderStats } from "./keyboard_trace_reader.mjs";

test("keyboard records survive every chunk split with original field types", () => {
  const line = 'INFO human authority input trace player_id=2 x=-3.25 active=true label="a b" empty="" missing=null\r\n';
  const expected = [{ player_id: 2, x: -3.25, active: true, label: "a b", empty: "", missing: "null" }];
  for (let split = 0; split <= line.length; split++) {
    const reader = createKeyboardTraceReader("human authority input trace"), chunks = [line.slice(0, split)];
    reader.read(chunks);
    chunks.push(line.slice(split));
    assert.deepEqual(reader.read(chunks), expected);
    assert.equal(reader.stats.characters, line.length);
    assert.equal(reader.stats.chunks, 2);
  }
});

test("a provisional last row is replaced as bytes arrive, not committed twice", () => {
  const reader = createKeyboardTraceReader("input"), chunks = ["input x=1"];
  const earlier = reader.read(chunks);
  assert.deepEqual(earlier, [{ x: 1 }]);
  chunks.push("2 active=tr");
  assert.deepEqual(reader.read(chunks), [{ x: 12, active: "tr" }]);
  chunks.push("ue\ninput x=30\n");
  assert.deepEqual(reader.read(chunks), [{ x: 12, active: true }, { x: 30 }]);
  assert.deepEqual(earlier, [{ x: 1 }]);
});

test("feedback reads scan new chunks only and return independent snapshots", () => {
  const server = { logs: ["irrelevant\ninput x=1\nprize id=2\n"] };
  const first = keyboardTraceRecords(server, "input");
  for (let n = 0; n < 100; n++) keyboardTraceRecords(server, "input");
  assert.equal(keyboardTraceReaderStats(server, "input").chunks, 1);
  assert.deepEqual(keyboardTraceRecords(server, "prize"), [{ id: 2 }]);
  first[0].x = 999;
  first.push({ x: 1000 });
  server.logs.push("input x=3\n");
  assert.deepEqual(keyboardTraceRecords(server, "input"), [{ x: 1 }, { x: 3 }]);
  assert.equal(keyboardTraceReaderStats(server, "input").chunks, 2);
  assert.equal(keyboardTraceReaderStats(server, "prize").chunks, 1);
});

test("replacement logs and server instances cannot borrow a predecessor cache", () => {
  const logs = ["input x=1\n"], first = { logs }, second = { logs };
  assert.deepEqual(keyboardTraceRecords(first, "input"), [{ x: 1 }]);
  assert.deepEqual(keyboardTraceRecords(second, "input"), [{ x: 1 }]);
  first.logs = ["input x=7\n"];
  assert.deepEqual(keyboardTraceRecords(first, "input"), [{ x: 7 }]);
  assert.deepEqual(keyboardTraceRecords(second, "input"), [{ x: 1 }]);
});

test("bounds and log mutation fail explicitly and cannot later resume duplicate ingestion", () => {
  const reader = createKeyboardTraceReader("input", { maximumRecords: 2 });
  const chunks = ["input x=1\ninput x=2\ninput x=3\n"];
  assert.throws(() => reader.read(chunks), /record bound/);
  chunks.length = 0;
  assert.throws(() => reader.read(chunks), /record bound/);
  const partial = createKeyboardTraceReader("input", { maximumLineCharacters: 8 });
  assert.throws(() => partial.read(["input x=1"]), /partial log line bound/);
  const whole = createKeyboardTraceReader("input", { maximumLineCharacters: 8 });
  assert.throws(() => whole.read(["input x=1\n"]), /log line bound/);
  const mutated = createKeyboardTraceReader("input"), original = ["input x=1\n"];
  mutated.read(original); original[0] = "input x=2\n";
  assert.throws(() => mutated.read(original), /append-only cursor/);
  const truncated = createKeyboardTraceReader("input"), removed = ["input x=1\n"];
  truncated.read(removed); removed.length = 0;
  assert.throws(() => truncated.read(removed), /append-only cursor/);
});

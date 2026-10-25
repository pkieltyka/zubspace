import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { logLines, writeLogChunks, writeLogChunksAsync } from "../tools/log_chunks.mjs";
import { parseZubspaceEvents } from "../tools/server_events.mjs";

test("all split boundaries preserve lines, Unicode and event offsets", () => {
  const text = 'noise 😀\r\nINFO zubspace_event {"type":"one"}\nlast';
  const expected = [{event: {type: "one"}, logIndex: text.indexOf("INFO")}];
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-log-chunks-"));
  const file = path.join(dir, "log");
  try {
    for (let i = 0; i <= text.length; i++) {
      const chunks = [text.slice(0, i), "", text.slice(i)];
      chunks.join = () => { throw new Error("whole-log join forbidden"); };
      assert.deepEqual([...logLines(chunks)], text.split("\n"));
      assert.deepEqual(parseZubspaceEvents(chunks), expected);
      writeLogChunks(file, chunks);
      assert.equal(fs.readFileSync(file, "utf8"), text);
    }
  } finally {
    fs.unlinkSync(file);
    fs.rmdirSync(dir);
  }
});

test("malformed events retain their exact preceding character offset", () => {
  assert.throws(() => parseZubspaceEvents(["abc\n", "zubspace_", "event nope\n"]), /log offset 4/);
  assert.deepEqual([...logLines([])], [""]);
  assert.deepEqual(parseZubspaceEvents(["no events\n"]), []);
});

test("async evidence writes preserve all Unicode split boundaries and freeze their source", async t => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-async-log-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const text = 'noise 😀\r\nINFO zubspace_event {"type":"one"}\nlast';
  for (let i = 0; i <= text.length; i++) {
    const chunks = [text.slice(0, i), "", text.slice(i)];
    chunks.join = () => { throw new Error("whole-log join forbidden"); };
    const filename = path.join(dir, "log");
    const written = writeLogChunksAsync(filename, chunks);
    chunks.push("arrived after capture started");
    await written;
    assert.equal(fs.readFileSync(filename, "utf8"), text);
  }
});

test("async evidence writes retain surrogate pairs across bounded batches and large chunks", async t => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-async-log-batches-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const text = "x".repeat(256 * 1024 - 1) + "😀" + "y".repeat(1024 * 1024) + "\ud800";
  const expected = Buffer.from(text);
  for (const chunks of [[text], [text.slice(0, 256 * 1024), text.slice(256 * 1024)]]) {
    const filename = path.join(dir, "log");
    await writeLogChunksAsync(filename, chunks);
    assert.deepEqual(fs.readFileSync(filename), expected);
  }
  await writeLogChunksAsync(path.join(dir, "empty"), []);
  assert.equal(fs.statSync(path.join(dir, "empty")).size, 0);
  await assert.rejects(writeLogChunksAsync(path.join(dir, "missing", "file"), ["data"]), /ENOENT/);
});

test("aggregate log larger than V8's string limit parses without aggregation", () => {
  const chunk = "x".repeat(1024 * 1024 - 1) + "\n";
  const chunks = Array(600).fill(chunk);
  chunks.push('zubspace_event {"type":"tail"}\n');
  chunks.join = () => { throw new Error("whole-log join forbidden"); };
  assert.deepEqual(parseZubspaceEvents(chunks), [{event: {type: "tail"}, logIndex: 600 * 1024 * 1024}]);
});

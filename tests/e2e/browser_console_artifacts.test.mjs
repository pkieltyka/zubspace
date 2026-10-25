import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { saveBrowserE2eArtifacts } from "./combat_replay.mjs";
import { validateArtifactPathReferences } from "./artifact_paths.mjs";

test("published renderer failure evidence survives rejection of a success verdict", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-console-artifacts-"));
  try {
    const directory = path.join(root, "capture");
    const client = { events: [],
      pageState: async () => ({ qaTraceState: { enabled: false } }),
    };
    const laterPeer = { events: [],
      pageState: async () => ({ qaTraceState: { enabled: false } }),
      screenshot: async () => {
        client.events.push({ type: "warning", text: "capacity exceeded. Entire call rejected" });
        return null;
      },
    };
    const artifacts = await saveBrowserE2eArtifacts({ artifactDir: directory,
      server: { logs: [] }, clients: [{ label: "actor", client }, { label: "later-peer", client: laterPeer }], provenance: {} });
    const result = { ok: true, successArtifacts: artifacts };
    const resultPath = path.join(directory, "result.json");
    fs.writeFileSync(resultPath, JSON.stringify(result));
    assert.throws(() => validateArtifactPathReferences(result, resultPath), /renderer rejected a draw/);
    const retained = JSON.parse(fs.readFileSync(path.join(directory, "actor-console.json")));
    assert.equal(retained.rejectedDraws, 1);
    assert.equal(retained.events[0].type, "warning");
    assert.equal(artifacts.browserConsoleAudits[0].rejectedDraws, 1);
    assert.ok(artifacts.artifactFiles.some(entry => entry.path === "actor-console.json"));
    assert.equal(validateArtifactPathReferences({ ...result, ok: false }, resultPath).ok, true,
      "original failure evidence must remain inspectable");
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

test("legacy or unavailable console evidence stays explicit instead of becoming zero warnings", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-console-missing-"));
  try {
    const directory = path.join(root, "capture");
    const artifacts = await saveBrowserE2eArtifacts({ artifactDir: directory,
      server: { logs: [] }, clients: [{ label: "not-open", client: null }], provenance: {} });
    const result = { ok: true, successArtifacts: artifacts };
    const resultPath = path.join(directory, "result.json");
    const audit = validateArtifactPathReferences(result, resultPath).browserConsoleAudit;
    assert.deepEqual(audit, { available: false, captures: 1, unavailable: 1 });
    assert.equal(artifacts.browserConsoleAudits[0].rejectedDraws, null);
    const legacy = { ...result, successArtifacts: { ...artifacts } };
    delete legacy.successArtifacts.browserConsoleAudits;
    assert.equal(validateArtifactPathReferences(legacy, resultPath).browserConsoleAudit.available, false);
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

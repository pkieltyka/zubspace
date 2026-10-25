import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { assertCompleteQaTraceCaptures, captureBrowserArtifactState } from "./qa_trace_capture.mjs";
import { writeQaTraceArtifact } from "./qa_trace_artifacts.mjs";
import { assertRendererConsoleClean, browserConsoleEvidence } from "../../tools/browser_console_evidence.mjs";

export function assertSmokeQaCaptures({ label, captures, cacheRejoin = false, discovery = false }) {
  assertCompleteQaTraceCaptures(captures);
  const roles = ["alpha-final", "beta-final"];
  if (cacheRejoin) roles.push("cache-predecessor");
  if (discovery) roles.push("default-chaos");
  assert.equal(captures.length, roles.length, "missing or extra smoke session captures");
  assert.equal(new Set(captures.map((entry) => entry.bundleId)).size, roles.length,
    "distinct smoke sessions must not reuse a trace bundle");
  for (const role of roles) {
    assert.equal(captures.filter((entry) => entry.label.startsWith(`${label}-${role}-`) &&
      entry.status === "captured").length, 1, `missing enabled capture for ${role}`);
  }
}

// Opt-in recorder for a smoke runner embedded in an E2E scenario. Captures
// happen only at explicit cold boundaries, never as a side effect of polling.
export function createSmokeQaRecorder({ directory, label, provenance, serverLogs }) {
  if (!directory) return { capture: async () => null, audit: () => [] };
  assert.match(label, /^[a-z0-9_-]+$/, "invalid smoke QA label");
  const audits = [];
  return {
    audit: () => [...audits],
    async capture(role, client, { required = true } = {}) {
      assert.match(role, /^[a-z0-9_-]+$/, "invalid smoke QA role");
      const prefix = `${label}-${role}-${audits.length + 1}`;
      // Startup may fail before the joined-server trace exists. Keep browser
      // errors independently of the large state dump and stderr pipe limits.
      // This is a cold, local artifact only; it is not part of trace uploads.
      // Use the shared bounded recorder format. A missing startup client is
      // explicitly unavailable, rather than indistinguishable from an empty
      // complete console. Counters retain errors/panics beyond the text tail.
      const consoleSummary = browserConsoleEvidence(client);
      fs.writeFileSync(path.join(directory, `${prefix}-console.json`), `${JSON.stringify(consoleSummary, null, 2)}\n`);
      const state = await captureBrowserArtifactState(client);
      fs.writeFileSync(path.join(directory, `${prefix}-state.json`), `${JSON.stringify(state, null, 2)}\n`);
      // Keep exact logs and an index incrementally: a later process/scenario
      // failure must not lose a client already closed for cache rejoin.
      fs.writeFileSync(path.join(directory, `${label}-server.log`), serverLogs.join(""));
      const audit = writeQaTraceArtifact({ directory, label: prefix, state, serverLogs, provenance });
      audits.push(audit);
      fs.writeFileSync(path.join(directory, `${label}-qa-index.json`), `${JSON.stringify({
        provenance, qaTraceCaptureAudit: audits,
      }, null, 2)}\n`);
      if (typeof client?.screenshot === "function") {
        const encoded = await client.screenshot().catch(() => null);
        if (encoded) fs.writeFileSync(path.join(directory, `${prefix}.png`), Buffer.from(encoded, "base64"));
      }
      if (required) {
        assertCompleteQaTraceCaptures([audit]);
        assertRendererConsoleClean(consoleSummary);
        assert.equal(consoleSummary.errors, 0, `browser errors in ${prefix}`);
      }
      return audit;
    },
  };
}

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  assertDataChannelDeliveryAudit,
  assertSecurityRejectAudit,
  saveBrowserE2eArtifacts,
} from "./combat_replay.mjs";
import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import { artifactFileManifest, assertRetainedArtifactDirectoryAvailable, validateArtifactPathReferences } from "./artifact_paths.mjs";
import { validateQaTraceArtifacts } from "./qa_trace_artifacts.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { writeLogChunks } from "../../tools/log_chunks.mjs";

export function assertFocusedConnectionContinuity(states) {
  assert.ok(states.length > 0, "focused acceptance requires client connection evidence");
  return states.map(({ label, networkLog }) => {
    assert.ok(Array.isArray(networkLog), `${label} is missing its connection log`);
    const connections = networkLog.filter((entry) => entry.message === "rtc connection state");
    const invalid = connections.filter((entry) => !["connecting", "connected"].includes(entry.detail?.state));
    assert.equal(invalid.length, 0, `${label} lost its RTC connection: ${JSON.stringify(invalid)}`);
    assert.equal(connections.at(-1)?.detail?.state, "connected", `${label} is not connected`);
    const channels = new Set(networkLog.filter((entry) => entry.message === "rtc data channel open")
      .map((entry) => entry.detail?.label));
    for (const channel of ["zubspace-game", "zubspace-events", "zubspace-reliable"]) {
      assert.ok(channels.has(channel), `${label} is missing ${channel}`);
    }
    return { label, connections, channels: [...channels].sort() };
  });
}

// Only run at scenario boundaries. Gameplay drivers still own their assertions
// and genuine keyboard input; this helper cannot declare gameplay success.
export function createFocusedBrowserEvidence({ e2eScript, artifactDir, wasm, clientEntry, serverBinary }, operations = {}) {
  const collect = operations.collectArtifactProvenance ?? collectArtifactProvenance;
  const collectLive = operations.collectArtifactProvenanceAsync ?? collectArtifactProvenanceAsync;
  const delivery = operations.assertDataChannelDeliveryAudit ?? assertDataChannelDeliveryAudit;
  const security = operations.assertSecurityRejectAudit ?? assertSecurityRejectAudit;
  const identityOptions = { e2eScript, runStartedAt: new Date().toISOString(),
    ...(wasm ? { wasm } : {}), ...(clientEntry ? { clientEntry } : {}),
    ...(serverBinary ? { serverBinary } : {}) };

  const initialProvenance = collect(identityOptions);
  const directory = path.resolve(artifactDir || process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
    path.join(process.env.ZUBSPACE_E2E_ARTIFACT_ROOT || os.tmpdir(), `zubspace-${path.basename(e2eScript, ".mjs")}-${Date.now()}-${process.pid}`));
  assertRetainedArtifactDirectoryAvailable(
    artifactDir || process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR,
  );

  function publish(document, target) {
    const resultPath = path.join(target, "result.json");
    validateArtifactPathReferences(document, resultPath);
    validateQaTraceArtifacts(document, resultPath);
    fs.writeFileSync(resultPath, `${JSON.stringify(document, null, 2)}\n`, { flag: "wx" });
    return document;
  }

  return {
    async succeed(result, { serverPort, ...capture }) {
      assert.equal(result.ok, true, "the scenario must independently pass before evidence finalization");
      // The delivery audit requires an ingress drain, and that drain is disconnect-cleanup evidence
      // (`docs/e2e-diagnostics.md`), so it can only run once the gate's sessions are gone: every step
      // that still needs a live DataChannel — artifact capture reads the live pages, and the continuity
      // assertion reads their connection logs — runs first, and the audit closes the gate.
      assert.equal(
        typeof capture.disconnect,
        "function",
        "a focused gate must hand its own browser sessions to the delivery audit so it can disconnect " +
          "before asserting the drain",
      );
      const securityAudit = security(capture.server);
      const provenance = await collectLive(identityOptions);
      assertArtifactProvenanceMatches(initialProvenance, provenance);
      const captureLogStartChunk = capture.server.logs.length;
      const successArtifacts = await saveBrowserE2eArtifacts({ ...capture,
        artifactDir: directory, provenance });
      const connectionContinuity = assertFocusedConnectionContinuity(
        successArtifacts.qaTraceCaptureAudit.map(({ label }) => ({
          label,
          ...JSON.parse(fs.readFileSync(path.join(directory, `${label}-state.json`), "utf8")),
        })),
      );
      // Capture can take time; bind its end as well as the last gameplay step.
      assertArtifactProvenanceMatches(initialProvenance, await collectLive(identityOptions));
      const dataChannelDeliveryAudit = await delivery({
        server: capture.server,
        serverPort,
        browserLogs: Object.values(capture.extraLogs || {}),
        disconnect: capture.disconnect,
      });
      // Artifact export/QA upload itself can perturb the server. The original
      // server.log is frozen before cleanup; retain the overlapping capture and
      // cleanup tail too, instead of attaching later counters without raw logs.
      assertArtifactProvenanceMatches(initialProvenance, await collectLive(identityOptions));
      const finalSecurityAudit = security(capture.server);
      const captureLogEndChunk = capture.server.logs.length;
      const captureLogName = "server-capture-and-cleanup.log";
      writeLogChunks(path.join(directory, captureLogName),
        capture.server.logs.slice(captureLogStartChunk, captureLogEndChunk));
      successArtifacts.extraLogPaths.push(captureLogName);
      successArtifacts.artifactFiles = artifactFileManifest(directory);
      return publish({ ...result, initialProvenance, provenance,
        dataChannelDeliveryAudit, securityAudit: finalSecurityAudit,
        securityAuditBeforeCapture: securityAudit, connectionContinuity,
        captureLogScope: { startChunk: captureLogStartChunk, endChunk: captureLogEndChunk,
          phase: "artifact-capture-start-through-final-disconnect-audit", overlapsInitialServerLog: true },
        successArtifacts }, directory);
    },

    async fail(error, { serverPort: _serverPort, ...capture }) {
      const provenance = await collectLive(identityOptions);
      const message = String(error.stack || error.message || error);
      const failureDirectory = `${directory}-failure`;
      const failureArtifacts = await saveBrowserE2eArtifacts({ ...capture,
        artifactDir: failureDirectory, provenance,
        extraLogs: { ...capture.extraLogs, failure: message } });
      publish({ ok: false, error: message, initialProvenance, provenance,
        failureArtifacts }, failureDirectory);
      return failureArtifacts;
    },
  };
}

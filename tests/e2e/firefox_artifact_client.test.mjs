import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import {firefoxArtifactClient} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {captureBrowserArtifactState} from "./qa_trace_capture.mjs";
import { browserConsoleEvidence, createConsoleRecorder } from "../../tools/browser_console_evidence.mjs";

test("Firefox artifact adapters retain their own console across driver session replacement", () => {
  const old = createConsoleRecorder("old session");
  const driver = { browserConsole: old };
  const actor = firefoxArtifactClient(driver);
  driver.browserConsole = createConsoleRecorder("replacement session");
  old.record({ type: "warning", text: "Entire call rejected; original session" });
  assert.equal(browserConsoleEvidence(actor).rejectedDraws, 1);
  assert.equal(browserConsoleEvidence(actor).source, "old session");
  assert.equal(driver.browserConsole.snapshot().rejectedDraws, 0);
});

test("shared Firefox adapter retains a captured first stall through normal artifact collection", async () => {
  const driver = {sessionId: "test-session"};
  const bundle = {schema: "zubspace-qa-trace-v1", version: 1,
    bundleId: "a".repeat(32), limits: {maxBytes: 240 * 1024}};
  const incident = {phase: "captured", bundle};
  let reads = 0;
  const actor = firefoxArtifactClient(driver, {
    executeScript: async (actual, script) => {
      assert.equal(actual, driver);
      reads++;
      return vm.runInNewContext(`(function(){${script}})()`, {
        window: {__zubspaceFirstStallCapture: () => incident},
      });
    },
    readPageState: async actual => {
      assert.equal(actual, driver);
      return {qaTraceState:{enabled:true,ready:true,bundleId:bundle.bundleId,
        firstStall:{phase:"captured"}}};
    },
    freezeTrace: async actual => {assert.equal(actual, driver); return bundle;},
  });
  const result = await captureBrowserArtifactState(actor);
  assert.equal(result.qaTraceCapture.status, "captured");
  assert.equal(reads, 1);
  assert.deepEqual(result.firstStallCapture, incident);
});

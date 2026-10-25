// The QA trace coverage contract, pinned against a fixture shaped like a live artifact so each clause fails loudly
// if it is ever dropped. The point is that "the run passed" and "every required capture was taken" are different
// statements, and only the second one is coverage.
import assert from "node:assert/strict";
import test from "node:test";

import { assertQaTraceCoverage } from "./qa_trace_coverage.mjs";

const BUNDLE = "2fc9e5634a7b2cec3da3772f93810333";

function captured(bundleId = BUNDLE) {
  return { bundleId, bytes: null, error: null, truncated: true };
}

function liveShapedDocument() {
  return {
    ok: true,
    browser: "firefox",
    arena: "chaos-zone",
    clean: { bundleId: BUNDLE, redacted: true },
    forcedFailure: { message: "induced", bundleId: BUNDLE },
    disabled: { bundleId: null, bytes: 0 },
    automaticUploads: [
      { mode: "clean", resultPath: "/tmp/session-clean/result.json", saved: { bundle: captured() } },
      { mode: "dropped", resultPath: "/tmp/session-dropped/result.json", saved: { bundle: captured("0c8d") } },
    ],
    successArtifacts: { qaTraceCaptureAudit: [{ bundleId: BUNDLE, bytes: 208913 }] },
  };
}

test("a complete drill result satisfies the coverage contract", () => {
  const summary = assertQaTraceCoverage(liveShapedDocument());
  assert.deepEqual(summary.modes, ["clean", "dropped"]);
  assert.equal(summary.uploads, 2);
  assert.equal(summary.audit, 1);
});

test("each dropped coverage case fails with its own reason", () => {
  const mutations = [
    ["no automatic uploads", (d) => { d.automaticUploads = []; }],
    ["only the clean mode", (d) => { d.automaticUploads = d.automaticUploads.slice(0, 1); }],
    ["upload without a retained child", (d) => { d.automaticUploads[0].resultPath = ""; }],
    ["upload without a bundle", (d) => { d.automaticUploads[0].saved.bundle.bundleId = null; }],
    ["unbounded stored bundle", (d) => { d.automaticUploads[0].saved.bundle.truncated = null; }],
    ["induced failure captured nothing", (d) => { d.forcedFailure.bundleId = null; }],
    ["clean capture absent", (d) => { d.clean.bundleId = null; }],
    ["disabled tracing recorded as a capture", (d) => { d.disabled.bundleId = BUNDLE; }],
    ["disabled tracing recorded bytes", (d) => { d.disabled.bytes = 17; }],
    ["no capture audit", (d) => { d.successArtifacts.qaTraceCaptureAudit = []; }],
    ["audit entry without bytes", (d) => { d.successArtifacts.qaTraceCaptureAudit[0].bytes = 0; }],
  ];
  for (const [label, mutate] of mutations) {
    const document = liveShapedDocument();
    mutate(document);
    assert.throws(() => assertQaTraceCoverage(document), Error, `${label} must be rejected`);
  }
});

test("the contract survives the extra fields a live artifact carries", () => {
  const document = liveShapedDocument();
  // The real artifact has many more blocks than the contract reads; none of them may matter to it.
  document.firstStall = { induced: {}, bundle: { ...captured(), truncated: true } };
  document.repeatedStalls = { diagnostics: {} };
  document.uploads = Array.from({ length: 7 }, () => ({ ok: true }));
  document.controls = Array.from({ length: 13 }, () => ({ ok: true }));
  document.disabledIdentityResources = [];
  assert.doesNotThrow(() => assertQaTraceCoverage(document));
});

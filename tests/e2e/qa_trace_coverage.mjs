// Coverage contract for the QA trace drill's parent result.
//
// The drill produces several kinds of capture on purpose, and "the run passed" is not the same statement as "every
// required capture was taken". This asserts the latter, so a future change that silently stops producing one of the
// cases fails the run instead of quietly shrinking its coverage: incomplete diagnostics must stay distinct from a
// passing gameplay result.
//
// Required, all observed on a live Chrome/Firefox artifact before this contract was written:
//   - automatic uploads, for both `clean` and `dropped` delivery modes, each with a retained child result and a
//     stored bundle that is explicitly bounded (`truncated === true`);
//   - an induced failure (`forcedFailure`) that still captured a bundle, so a deliberate failure is evidence rather
//     than a gap;
//   - tracing explicitly disabled, recorded as `bundleId: null` with zero bytes - a *distinct* shape, not a pass;
//   - the joined clean capture, and the capture audit that names the bundle and its byte count.
const MODES = ["clean", "dropped"];

function requireRecord(value, label) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object, got ${JSON.stringify(value)}`);
  }
}

function requireBundleId(value, label) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${label} must name a captured bundle, got ${JSON.stringify(value)}`);
  }
}

export function assertQaTraceCoverage(document) {
  requireRecord(document, "qa trace result");

  const uploads = document.automaticUploads;
  if (!Array.isArray(uploads)) {
    throw new Error("automaticUploads must be an array");
  }
  const modes = new Set(uploads.map((entry) => entry?.mode));
  for (const mode of MODES) {
    if (!modes.has(mode)) {
      throw new Error(`automaticUploads must cover the ${mode} delivery mode; saw ${JSON.stringify([...modes])}`);
    }
  }
  for (const entry of uploads) {
    const label = `automaticUploads[${entry?.mode}]`;
    if (typeof entry?.resultPath !== "string" || entry.resultPath.length === 0) {
      throw new Error(`${label} must retain its session result path`);
    }
    requireRecord(entry.saved, `${label}.saved`);
    requireBundleId(entry.saved.bundle?.bundleId, `${label}.saved.bundle`);
    if (entry.saved.bundle.truncated !== true) {
      throw new Error(`${label}.saved.bundle must record that it is bounded (truncated: true)`);
    }
  }

  // A deliberate failure must still have captured something: an induced failure with no bundle is a coverage gap
  // wearing the same shape as a pass.
  requireRecord(document.forcedFailure, "forcedFailure");
  requireBundleId(document.forcedFailure.bundleId, "forcedFailure");
  requireBundleId(document.clean?.bundleId, "clean");

  // Explicitly disabled tracing is a first-class outcome, and its shape must stay distinguishable from both a
  // capture and a failure.
  requireRecord(document.disabled, "disabled");
  if (document.disabled.bundleId !== null) {
    throw new Error(`disabled capture must record bundleId: null, got ${JSON.stringify(document.disabled.bundleId)}`);
  }
  if (document.disabled.bytes !== 0) {
    throw new Error(`disabled capture must record zero bytes, got ${JSON.stringify(document.disabled.bytes)}`);
  }

  const audit = document.successArtifacts?.qaTraceCaptureAudit;
  if (!Array.isArray(audit) || audit.length === 0) {
    throw new Error("successArtifacts.qaTraceCaptureAudit must name at least one captured bundle");
  }
  for (const [index, entry] of audit.entries()) {
    requireBundleId(entry?.bundleId, `successArtifacts.qaTraceCaptureAudit[${index}]`);
    if (!Number.isInteger(entry.bytes) || entry.bytes <= 0) {
      throw new Error(
        `successArtifacts.qaTraceCaptureAudit[${index}] must record positive bytes, got ${JSON.stringify(entry.bytes)}`,
      );
    }
  }

  return { modes: [...modes].sort(), uploads: uploads.length, audit: audit.length };
}

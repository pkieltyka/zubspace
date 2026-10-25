import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  assertRetainedArtifactDirectoryAvailable,
  createAtomicArtifactDirectory,
} from "./artifact_paths.mjs";

test("retained artifact collisions fail before the lane starts", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-artifact-preflight-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const target = path.join(root, "lane-artifacts");

  assert.doesNotThrow(() => assertRetainedArtifactDirectoryAvailable(target));
  assert.doesNotThrow(() => assertRetainedArtifactDirectoryAvailable(""));

  fs.mkdirSync(target);
  assert.throws(
    () => assertRetainedArtifactDirectoryAvailable(target),
    /retained artifact directory already exists: .* \(each lane needs its own ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR\)/,
  );
  assert.throws(
    () => createAtomicArtifactDirectory(target),
    /retained artifact directory already exists/,
    "publication must stay atomic against the same directory",
  );
});

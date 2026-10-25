import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { execFileSync } from "node:child_process";

import {
  assertArtifactProvenanceMatches,
  comparableProvenance,
  deterministicWorkingTreeDiffHash,
  filesystemEntryIdentity,
} from "./artifact_provenance.mjs";

function fixture(overrides = {}) {
  return {
    formatVersion: 7,
    capturedAt: "2000-01-01T00:00:00.000Z",
    gitHead: "abc123",
    workingTreeDiffSha256: "diff",
    serverBinary: { sha256: "server" },
    wasm: { sha256: "wasm" },
    clientEntry: { sha256: "html" },
    clientBuildSha256: "client",
    e2eScript: { sha256: "script" },
    additionalFiles: [],
    dockerImages: [],
    ...overrides,
  };
}

test("owner-only planning content is excluded before entry capture, while its index metadata stays bound", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-owner-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const git = (...args) => execFileSync("git", args, { cwd: root });
  git("init", "--quiet"); git("config", "user.email", "test@example.test");
  git("config", "user.name", "Fixture");
  fs.mkdirSync(path.join(root, "plans"));
  // These are synthetic files in a temporary fixture, never owner documents.
  const excluded = ["plans/prod_deployment.md", "plans/release_caching_and_versioning.md"];
  for (const entry of excluded) fs.writeFileSync(path.join(root, entry), "synthetic initial");
  fs.writeFileSync(path.join(root, "source.rs"), "build input");
  git("add", "."); git("commit", "--quiet", "-m", "fixture");
  const capture = () => deterministicWorkingTreeDiffHash(root, { mutationHook({ relativePath }) {
    assert.ok(!excluded.includes(relativePath), "excluded content must never enter descriptor capture");
  } });
  const before = capture();
  for (const entry of excluded) fs.writeFileSync(path.join(root, entry), "synthetic modified");
  const after = capture();
  assert.equal(after.sha256, before.sha256);
  assert.deepEqual(after.contentExclusions, excluded);
  assert.deepEqual(after.trackedEntries.map(entry => entry.path), ["source.rs"]);
  assert.deepEqual(after.trackedMissing, []);
  git("add", "plans");
  assert.notEqual(capture().sha256, before.sha256, "indexed blob identities remain part of provenance");
  fs.writeFileSync(path.join(root, "source.rs"), "changed build input");
  assert.notEqual(capture().sha256, after.sha256);
});

test("artifact provenance verifier ignores capture time but binds every build input", () => {
  const expected = fixture();
  const recaptured = fixture({ capturedAt: "2000-01-01T01:00:00.000Z" });
  assert.equal(assertArtifactProvenanceMatches(expected, recaptured), true);
  for (const [field, replacement] of [
    ["gitHead", "other-head"],
    ["workingTreeDiffSha256", "other-diff"],
    ["clientBuildSha256", "other-client"],
  ]) {
    assert.throws(
      () => assertArtifactProvenanceMatches(expected, fixture({ [field]: replacement })),
      /provenance drifted/,
    );
  }
  assert.throws(
    () => assertArtifactProvenanceMatches(expected, fixture({
      serverBinary: { sha256: "other-server" },
    })),
    /provenance drifted/,
  );
  assert.throws(
    () => assertArtifactProvenanceMatches(expected, fixture({
      e2eScript: { sha256: "other-script" },
    })),
    /provenance drifted/,
  );
  assert.throws(
    () => assertArtifactProvenanceMatches(expected, fixture({
      additionalFiles: [{ path: "Makefile", sha256: "other-input" }],
    })),
    /provenance drifted/,
  );
  assert.throws(
    () => assertArtifactProvenanceMatches(expected, fixture({
      dockerImages: [{
        name: "zubspace/server:dev",
        id: "sha256:other-image",
        repoDigests: [],
      }],
    })),
    /provenance drifted/,
  );
});

test("comparable provenance is stable and excludes timestamps", () => {
  assert.deepEqual(comparableProvenance(fixture()), {
    formatVersion: 7,
    gitHead: "abc123",
    workingTreeDiffSha256: "diff",
    serverBinary: "server",
    wasm: "wasm",
    clientEntry: "html",
    clientBuildSha256: "client",
    e2eScript: "script",
    additionalFiles: [],
    dockerImages: [],
  });
});

test("filesystem provenance binds entry type, mode, and symlink target without following", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-entry-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.writeFileSync(path.join(root, "target-a"), "same-content");
  fs.writeFileSync(path.join(root, "target-b"), "same-content");
  fs.mkdirSync(path.join(root, "target-dir"));

  fs.symlinkSync("target-a", path.join(root, "entry"));
  const targetA = filesystemEntryIdentity(root, "entry");
  assert.equal(targetA.entryType, "symlink");
  assert.equal(targetA.linkTarget, "target-a");
  fs.unlinkSync(path.join(root, "entry"));
  fs.symlinkSync("target-b", path.join(root, "entry"));
  const targetB = filesystemEntryIdentity(root, "entry");
  assert.notEqual(
    targetA.sha256,
    targetB.sha256,
    "retargeting a symlink must change identity even when targets have identical bytes",
  );

  fs.unlinkSync(path.join(root, "entry"));
  fs.writeFileSync(path.join(root, "entry"), "target-b");
  fs.chmodSync(path.join(root, "entry"), 0o644);
  const regular = filesystemEntryIdentity(root, "entry");
  assert.equal(regular.entryType, "regular");
  assert.notEqual(regular.sha256, targetB.sha256, "regular and symlink entries differ");
  fs.chmodSync(path.join(root, "entry"), 0o755);
  const executable = filesystemEntryIdentity(root, "entry");
  assert.notEqual(regular.sha256, executable.sha256, "executable mode is provenance");

  fs.symlinkSync("target-dir", path.join(root, "directory-link"));
  const directoryLink = filesystemEntryIdentity(root, "directory-link");
  assert.equal(directoryLink.entryType, "symlink");
  assert.equal(directoryLink.linkTarget, "target-dir");
  assert.throws(
    () => filesystemEntryIdentity(root, "target-dir"),
    /is a directory; declare individual files/,
  );
});

test("filesystem provenance retries a regular-to-symlink capture race without following", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-race-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.writeFileSync(path.join(root, "target"), "target bytes");
  fs.writeFileSync(path.join(root, "entry"), "regular bytes");
  let swapped = false;
  const identity = filesystemEntryIdentity(root, "entry", {
    mutationHook({ stage, attempt, relativePath }) {
      if (stage !== "after-entry-lstat" || attempt !== 0 || relativePath !== "entry") return;
      fs.unlinkSync(path.join(root, "entry"));
      fs.symlinkSync("target", path.join(root, "entry"));
      swapped = true;
    },
  });
  assert.equal(swapped, true);
  assert.equal(identity.entryType, "symlink");
  assert.equal(identity.linkTarget, "target");
  assert.equal(identity.contentSha256, cryptoHash("target"));
  assert.notEqual(identity.contentSha256, cryptoHash("target bytes"));
});

test("filesystem provenance fails bounded capture under content, mode, and type mutation", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-race-fail-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));

  for (const [name, stage, mutate] of [
    ["content", "after-entry-payload", ({ attempt }) => {
      fs.writeFileSync(path.join(root, "content"), attempt % 2 === 0 ? "bbbb" : "aaaa");
    }],
    ["mode", "after-entry-payload", ({ attempt }) => {
      fs.chmodSync(path.join(root, "mode"), attempt % 2 === 0 ? 0o755 : 0o644);
    }],
    ["type", "after-entry-lstat", ({ entryType }) => {
      const entry = path.join(root, "type");
      fs.unlinkSync(entry);
      if (entryType === "regular") fs.symlinkSync("target", entry);
      else fs.writeFileSync(entry, "type");
    }],
  ]) {
    fs.writeFileSync(path.join(root, name), name === "content" ? "aaaa" : name);
    fs.chmodSync(path.join(root, name), 0o644);
    assert.throws(
      () => filesystemEntryIdentity(root, name, {
        entryCaptureAttempts: 3,
        mutationHook(details) {
          if (details.stage === stage && details.relativePath === name) mutate(details);
        },
      }),
      /could not capture stable entry.*after 3 attempts/,
      name,
    );
  }
});

test("working-tree provenance independently walks every non-ignored untracked node", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-repo-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  execFileSync("git", ["init", "--quiet"], { cwd: root });
  execFileSync("git", ["config", "user.email", "provenance@example.test"], { cwd: root });
  execFileSync("git", ["config", "user.name", "Provenance Fixture"], { cwd: root });
  fs.writeFileSync(path.join(root, ".gitignore"), [
    "ignored/",
    "cache/*",
    "!cache/keep/",
    "!cache/keep/**",
    "",
  ].join("\n"));
  fs.writeFileSync(path.join(root, "tracked"), "tracked\n");
  execFileSync("git", ["add", ".gitignore", "tracked"], { cwd: root });
  execFileSync("git", ["commit", "--quiet", "-m", "fixture"], { cwd: root });

  fs.mkdirSync(path.join(root, "nested"));
  fs.writeFileSync(path.join(root, "nested", "value"), "nested\n");
  fs.mkdirSync(path.join(root, "cache", "keep"), { recursive: true });
  fs.writeFileSync(path.join(root, "cache", "keep", "allowed"), "allowed\n");
  fs.mkdirSync(path.join(root, "cache", "drop"), { recursive: true });
  fs.writeFileSync(path.join(root, "cache", "drop", "ignored"), "ignored\n");
  fs.writeFileSync(path.join(root, "target-a"), "same-content");
  fs.writeFileSync(path.join(root, "target-b"), "same-content");
  fs.writeFileSync(path.join(root, "loose"), "regular");
  fs.chmodSync(path.join(root, "loose"), 0o644);

  const baseline = deterministicWorkingTreeDiffHash(root);
  assert.deepEqual(baseline.untrackedFiles, [
    "cache/keep/allowed",
    "loose",
    "nested/value",
    "target-a",
    "target-b",
  ]);
  assert.deepEqual(
    baseline.untrackedEntries
      .filter((entry) => entry.entryType !== "directory")
      .map((entry) => entry.path),
    baseline.untrackedFiles,
    "the independently walked leaf and Git-declared untracked sets must match exactly",
  );
  assert.deepEqual(baseline.untrackedDirectories, ["cache/keep", "nested"]);
  assert.equal(deterministicWorkingTreeDiffHash(root).sha256, baseline.sha256);

  fs.mkdirSync(path.join(root, "ignored"));
  fs.writeFileSync(path.join(root, "ignored", "file"), "ignored\n");
  let fifoSupported = true;
  try {
    execFileSync("mkfifo", [path.join(root, "ignored", "pipe")]);
  } catch {
    fifoSupported = false;
  }
  assert.equal(
    deterministicWorkingTreeDiffHash(root).sha256,
    baseline.sha256,
    "ignored directories and special nodes must not affect provenance",
  );

  fs.chmodSync(path.join(root, "nested"), 0o700);
  const privateDirectory = deterministicWorkingTreeDiffHash(root);
  assert.notEqual(privateDirectory.sha256, baseline.sha256, "untracked directory mode is provenance");
  fs.chmodSync(path.join(root, "nested"), 0o755);
  assert.equal(deterministicWorkingTreeDiffHash(root).sha256, baseline.sha256);

  fs.symlinkSync("../target-a", path.join(root, "nested", "literal-link"));
  const nestedLink = deterministicWorkingTreeDiffHash(root);
  const nestedLinkEvidence = nestedLink.untrackedEntries.find(
    (entry) => entry.path === "nested/literal-link",
  );
  assert.equal(nestedLinkEvidence.entryType, "symlink");
  assert.equal(nestedLinkEvidence.linkTarget, "../target-a");
  assert.equal(nestedLinkEvidence.contentSha256, cryptoHash("../target-a"));
  assert.notEqual(nestedLinkEvidence.contentSha256, cryptoHash("same-content"));
  fs.unlinkSync(path.join(root, "nested", "literal-link"));
  assert.equal(deterministicWorkingTreeDiffHash(root).sha256, baseline.sha256);

  fs.chmodSync(path.join(root, "loose"), 0o755);
  const executable = deterministicWorkingTreeDiffHash(root);
  assert.notEqual(executable.sha256, baseline.sha256);
  fs.chmodSync(path.join(root, "loose"), 0o644);
  assert.equal(deterministicWorkingTreeDiffHash(root).sha256, baseline.sha256);

  fs.unlinkSync(path.join(root, "loose"));
  fs.symlinkSync("target-a", path.join(root, "loose"));
  const targetA = deterministicWorkingTreeDiffHash(root);
  assert.notEqual(targetA.sha256, baseline.sha256, "regular and symlink identities differ");
  fs.unlinkSync(path.join(root, "loose"));
  fs.symlinkSync("target-b", path.join(root, "loose"));
  const targetB = deterministicWorkingTreeDiffHash(root);
  assert.notEqual(targetB.sha256, targetA.sha256, "literal symlink target is bound");

  fs.mkdirSync(path.join(root, "empty-untracked"));
  assert.throws(
    () => deterministicWorkingTreeDiffHash(root),
    /empty untracked directory "empty-untracked"/,
  );
  fs.rmdirSync(path.join(root, "empty-untracked"));

  if (fifoSupported) {
    execFileSync("mkfifo", [path.join(root, "untracked-pipe")]);
    assert.throws(
      () => deterministicWorkingTreeDiffHash(root),
      /unsupported filesystem type/,
    );
  } else {
    t.diagnostic("mkfifo is unavailable; FIFO provenance rejection fixture skipped");
  }
});

test("working-tree provenance requires two stable complete captures", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-tree-race-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  execFileSync("git", ["init", "--quiet"], { cwd: root });
  execFileSync("git", ["config", "user.email", "provenance@example.test"], { cwd: root });
  execFileSync("git", ["config", "user.name", "Provenance Fixture"], { cwd: root });
  fs.writeFileSync(path.join(root, "tracked"), "tracked\n");
  execFileSync("git", ["add", "tracked"], { cwd: root });
  execFileSync("git", ["commit", "--quiet", "-m", "fixture"], { cwd: root });
  fs.writeFileSync(path.join(root, "first"), "first\n");

  let added = false;
  const recovered = deterministicWorkingTreeDiffHash(root, {
    mutationHook({ stage, attempt }) {
      if (stage === "between-tree-captures" && attempt === 0) {
        fs.writeFileSync(path.join(root, "second"), "second\n");
        added = true;
      }
    },
  });
  assert.equal(added, true);
  assert.deepEqual(recovered.untrackedFiles, ["first", "second"]);

  assert.throws(
    () => deterministicWorkingTreeDiffHash(root, {
      treeCaptureAttempts: 3,
      mutationHook({ stage }) {
        if (stage !== "between-tree-captures") return;
        const entry = path.join(root, "flapping");
        if (fs.existsSync(entry)) fs.unlinkSync(entry);
        else fs.writeFileSync(entry, "flapping\n");
      },
    }),
    /could not capture a stable working tree after 4 captures/,
  );
});

test("working-tree provenance v7 anchors live tracked bytes, index, type, mode, and deletion", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-tracked-v7-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  execFileSync("git", ["init", "--quiet"], { cwd: root });
  execFileSync("git", ["config", "user.email", "provenance@example.test"], { cwd: root });
  execFileSync("git", ["config", "user.name", "Provenance Fixture"], { cwd: root });
  fs.writeFileSync(path.join(root, "tracked"), "base\n");
  execFileSync("git", ["add", "tracked"], { cwd: root });
  execFileSync("git", ["commit", "--quiet", "-m", "fixture"], { cwd: root });

  fs.writeFileSync(path.join(root, "tracked"), "state-A\n");
  const stateA = deterministicWorkingTreeDiffHash(root);
  let swapped = false;
  const recoveredB = deterministicWorkingTreeDiffHash(root, {
    mutationHook({ stage, captureAttempt }) {
      if (stage !== "after-git-state-before-filesystem" || captureAttempt !== 0 || swapped) return;
      fs.writeFileSync(path.join(root, "tracked"), "state-B\n");
      swapped = true;
    },
  });
  assert.equal(swapped, true);
  assert.notEqual(recoveredB.sha256, stateA.sha256);
  assert.equal(
    recoveredB.sha256,
    deterministicWorkingTreeDiffHash(root).sha256,
    "a mutation after the first git diff must recover to one real stable tree",
  );
  assert.equal(
    recoveredB.trackedEntries.find((entry) => entry.path === "tracked").contentSha256,
    cryptoHash("state-B\n"),
  );

  let trailingSwap = false;
  const recoveredC = deterministicWorkingTreeDiffHash(root, {
    mutationHook({ stage, captureAttempt }) {
      if (stage !== "after-trailing-git-state" || captureAttempt !== 0 || trailingSwap) return;
      fs.writeFileSync(path.join(root, "tracked"), "state-C\n");
      trailingSwap = true;
    },
  });
  assert.equal(trailingSwap, true);
  assert.equal(recoveredC.sha256, deterministicWorkingTreeDiffHash(root).sha256);
  assert.notEqual(recoveredC.sha256, recoveredB.sha256);

  const regular = recoveredC.sha256;
  fs.chmodSync(path.join(root, "tracked"), 0o755);
  const executable = deterministicWorkingTreeDiffHash(root);
  assert.notEqual(executable.sha256, regular, "tracked executable mode is bound");
  fs.chmodSync(path.join(root, "tracked"), 0o644);

  fs.unlinkSync(path.join(root, "tracked"));
  fs.symlinkSync("literal-target", path.join(root, "tracked"));
  const symlink = deterministicWorkingTreeDiffHash(root);
  assert.notEqual(symlink.sha256, regular, "tracked regular/symlink type is bound");
  assert.equal(symlink.trackedEntries[0].linkTarget, "literal-target");

  fs.unlinkSync(path.join(root, "tracked"));
  const deleted = deterministicWorkingTreeDiffHash(root);
  assert.deepEqual(deleted.trackedMissing, ["tracked"]);
  assert.notEqual(deleted.sha256, symlink.sha256);

  fs.writeFileSync(path.join(root, "renamed"), "state-B\n");
  const renamed = deterministicWorkingTreeDiffHash(root);
  assert.deepEqual(renamed.trackedMissing, ["tracked"]);
  assert.deepEqual(renamed.untrackedFiles, ["renamed"]);
  assert.notEqual(renamed.sha256, deleted.sha256);

  execFileSync("git", ["add", "-A"], { cwd: root });
  const stagedRename = deterministicWorkingTreeDiffHash(root);
  assert.equal(stagedRename.trackedMissing.length, 0);
  assert.deepEqual(stagedRename.untrackedFiles, []);
  assert.deepEqual(stagedRename.trackedEntries.map((entry) => entry.path), ["renamed"]);
  assert.notEqual(stagedRename.sha256, renamed.sha256, "the staged index state is bound");

  fs.writeFileSync(path.join(root, "intent"), "intent bytes\n");
  execFileSync("git", ["add", "-N", "intent"], { cwd: root });
  const intent = deterministicWorkingTreeDiffHash(root);
  assert.ok(intent.trackedEntries.some((entry) => entry.path === "intent"));
  assert.ok(intent.trackedIndexBytes > stagedRename.trackedIndexBytes);

  assert.throws(
    () => deterministicWorkingTreeDiffHash(root, {
      treeCaptureAttempts: 3,
      mutationHook({ stage, captureAttempt }) {
        if (stage !== "after-git-state-before-filesystem") return;
        fs.writeFileSync(
          path.join(root, "renamed"),
          captureAttempt % 2 === 0 ? "flap-A\n" : "flap-B\n",
        );
      },
    }),
    /could not capture a stable working tree after 4 captures/,
  );
});

test("working-tree provenance v7 treats gitlinks as opaque clean or uninitialized entries", (t) => {
  const source = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-submodule-source-"));
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-submodule-parent-"));
  t.after(() => fs.rmSync(source, { recursive: true, force: true }));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const configure = (repo) => {
    execFileSync("git", ["init", "--quiet"], { cwd: repo });
    execFileSync("git", ["config", "user.email", "provenance@example.test"], { cwd: repo });
    execFileSync("git", ["config", "user.name", "Provenance Fixture"], { cwd: repo });
  };
  configure(source);
  fs.writeFileSync(path.join(source, "module.txt"), "module-v1\n");
  execFileSync("git", ["add", "module.txt"], { cwd: source });
  execFileSync("git", ["commit", "--quiet", "-m", "module-v1"], { cwd: source });
  const firstObject = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: source,
    encoding: "utf8",
  }).trim();

  configure(root);
  fs.writeFileSync(path.join(root, "base"), "base\n");
  execFileSync("git", ["add", "base"], { cwd: root });
  execFileSync("git", ["commit", "--quiet", "-m", "parent-base"], { cwd: root });
  execFileSync("git", [
    "-c", "protocol.file.allow=always", "submodule", "add", "--quiet", source, "modules/child",
  ], { cwd: root });
  execFileSync("git", ["commit", "--quiet", "-am", "add-gitlink"], { cwd: root });

  const initialized = deterministicWorkingTreeDiffHash(root);
  assert.equal(initialized.gitlinks.length, 1);
  assert.deepEqual(
    {
      path: initialized.gitlinks[0].path,
      entryType: initialized.gitlinks[0].entryType,
      indexMode: initialized.gitlinks[0].indexMode,
      indexObjectId: initialized.gitlinks[0].indexObjectId,
      checkoutState: initialized.gitlinks[0].checkoutState,
      checkoutHead: initialized.gitlinks[0].checkoutHead,
    },
    {
      path: "modules/child",
      entryType: "gitlink",
      indexMode: "160000",
      indexObjectId: firstObject,
      checkoutState: "initialized-clean",
      checkoutHead: firstObject,
    },
  );
  assert.equal(deterministicWorkingTreeDiffHash(root).sha256, initialized.sha256);
  assert.ok(
    !initialized.trackedEntries.some((entry) => entry.path === "modules/child/module.txt"),
    "the superproject walker must never recurse into a gitlink",
  );

  fs.writeFileSync(path.join(root, "modules/child/module.txt"), "dirty tracked\n");
  assert.throws(
    () => deterministicWorkingTreeDiffHash(root),
    /gitlink "modules\/child" has dirty tracked or untracked nested state/,
  );
  fs.writeFileSync(path.join(root, "modules/child/module.txt"), "module-v1\n");
  fs.writeFileSync(path.join(root, "modules/child/untracked"), "dirty untracked\n");
  assert.throws(
    () => deterministicWorkingTreeDiffHash(root),
    /gitlink "modules\/child" has dirty tracked or untracked nested state/,
  );
  fs.unlinkSync(path.join(root, "modules/child/untracked"));
  assert.equal(deterministicWorkingTreeDiffHash(root).sha256, initialized.sha256);

  execFileSync("git", ["submodule", "deinit", "-f", "--", "modules/child"], { cwd: root });
  const uninitialized = deterministicWorkingTreeDiffHash(root);
  assert.equal(uninitialized.gitlinks[0].checkoutState, "uninitialized");
  assert.equal(uninitialized.gitlinks[0].checkoutHead, null);
  assert.equal(deterministicWorkingTreeDiffHash(root).sha256, uninitialized.sha256);
  assert.notEqual(uninitialized.sha256, initialized.sha256);
  fs.chmodSync(path.join(root, "modules/child"), 0o700);
  const privateUninitialized = deterministicWorkingTreeDiffHash(root);
  assert.notEqual(privateUninitialized.sha256, uninitialized.sha256, "gitlink directory mode is bound");
  fs.chmodSync(path.join(root, "modules/child"), 0o755);
  assert.equal(deterministicWorkingTreeDiffHash(root).sha256, uninitialized.sha256);

  execFileSync("git", [
    "-c", "protocol.file.allow=always", "submodule", "update", "--init", "--", "modules/child",
  ], { cwd: root });
  fs.writeFileSync(path.join(source, "module.txt"), "module-v2\n");
  execFileSync("git", ["add", "module.txt"], { cwd: source });
  execFileSync("git", ["commit", "--quiet", "-m", "module-v2"], { cwd: source });
  const secondObject = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: source,
    encoding: "utf8",
  }).trim();
  execFileSync("git", ["fetch", "--quiet", "origin"], { cwd: path.join(root, "modules/child") });
  execFileSync("git", ["checkout", "--quiet", "--detach", secondObject], {
    cwd: path.join(root, "modules/child"),
  });
  assert.throws(
    () => deterministicWorkingTreeDiffHash(root),
    /checkout HEAD .* does not match index object/,
  );
  execFileSync("git", ["add", "modules/child"], { cwd: root });
  const advanced = deterministicWorkingTreeDiffHash(root);
  assert.equal(advanced.gitlinks[0].indexObjectId, secondObject);
  assert.equal(advanced.gitlinks[0].checkoutHead, secondObject);
  assert.notEqual(advanced.sha256, initialized.sha256, "the gitlink index object is bound");

  execFileSync("git", ["submodule", "deinit", "-f", "--", "modules/child"], { cwd: root });
  fs.rmSync(path.join(root, "modules/child"), { recursive: true, force: true });
  fs.mkdirSync(path.join(root, "modules/child"), { recursive: true });
  fs.writeFileSync(path.join(root, "modules/child/replacement"), "not a checkout\n");
  assert.throws(
    () => deterministicWorkingTreeDiffHash(root),
    /nonempty replacement directory without an initialized checkout/,
  );
  fs.rmSync(path.join(root, "modules/child"), { recursive: true, force: true });
  fs.symlinkSync(source, path.join(root, "modules/child"));
  assert.throws(
    () => deterministicWorkingTreeDiffHash(root),
    /must be an initialized\/uninitialized directory|not to be a symbolic link/,
  );
  fs.unlinkSync(path.join(root, "modules/child"));
  fs.rmdirSync(path.join(root, "modules"));
  const absent = deterministicWorkingTreeDiffHash(root);
  assert.equal(absent.gitlinks[0].checkoutState, "uninitialized-absent");
  assert.equal(absent.gitlinks[0].mode, null);
  assert.equal(deterministicWorkingTreeDiffHash(root).sha256, absent.sha256);
});

test("working-tree provenance never follows an intermediate directory swapped to a symlink", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-dir-swap-"));
  const external = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-external-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  t.after(() => fs.rmSync(external, { recursive: true, force: true }));
  execFileSync("git", ["init", "--quiet"], { cwd: root });
  execFileSync("git", ["config", "user.email", "provenance@example.test"], { cwd: root });
  execFileSync("git", ["config", "user.name", "Provenance Fixture"], { cwd: root });
  fs.writeFileSync(path.join(root, "tracked"), "tracked\n");
  execFileSync("git", ["add", "tracked"], { cwd: root });
  execFileSync("git", ["commit", "--quiet", "-m", "fixture"], { cwd: root });
  fs.mkdirSync(path.join(root, "nested"));
  fs.writeFileSync(path.join(root, "nested", "inside"), "inside\n");
  fs.writeFileSync(path.join(external, "secret"), "external secret bytes\n");

  let swapped = false;
  const captured = deterministicWorkingTreeDiffHash(root, {
    mutationHook({ stage, relativePath, captureAttempt }) {
      if (
        swapped ||
        stage !== "after-child-lstat" ||
        relativePath !== "nested" ||
        captureAttempt !== 0
      ) return;
      fs.renameSync(path.join(root, "nested"), path.join(root, "nested-old"));
      fs.symlinkSync(external, path.join(root, "nested"));
      swapped = true;
    },
  });
  assert.equal(swapped, true);
  assert.ok(captured.untrackedFiles.includes("nested"));
  assert.ok(captured.untrackedFiles.includes("nested-old/inside"));
  assert.ok(!captured.untrackedFiles.some((entry) => entry.includes("secret")));
  const literal = captured.untrackedEntries.find((entry) => entry.path === "nested");
  assert.equal(literal.entryType, "symlink");
  assert.equal(literal.linkTarget, external);
  assert.notEqual(literal.contentSha256, cryptoHash("external secret bytes\n"));
});

test("working-tree directory capture recovers once or fails after bounded mutation", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-provenance-dir-race-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  execFileSync("git", ["init", "--quiet"], { cwd: root });
  execFileSync("git", ["config", "user.email", "provenance@example.test"], { cwd: root });
  execFileSync("git", ["config", "user.name", "Provenance Fixture"], { cwd: root });
  fs.writeFileSync(path.join(root, "tracked"), "tracked\n");
  execFileSync("git", ["add", "tracked"], { cwd: root });
  execFileSync("git", ["commit", "--quiet", "-m", "fixture"], { cwd: root });
  fs.mkdirSync(path.join(root, "nested"));
  fs.writeFileSync(path.join(root, "nested", "first"), "first\n");

  let changedOnce = false;
  const recovered = deterministicWorkingTreeDiffHash(root, {
    mutationHook({ stage, relativePath, captureAttempt }) {
      if (
        changedOnce ||
        stage !== "after-directory-enumeration" ||
        relativePath !== "nested" ||
        captureAttempt !== 0
      ) return;
      fs.writeFileSync(path.join(root, "nested", "second"), "second\n");
      changedOnce = true;
    },
  });
  assert.equal(changedOnce, true);
  assert.deepEqual(recovered.untrackedFiles, ["nested/first", "nested/second"]);

  assert.throws(
    () => deterministicWorkingTreeDiffHash(root, {
      treeCaptureAttempts: 3,
      mutationHook({ stage, relativePath, captureAttempt }) {
        if (stage !== "after-directory-enumeration" || relativePath !== "nested") return;
        const entry = path.join(root, "nested", `flap-${captureAttempt}`);
        fs.writeFileSync(entry, `flap-${captureAttempt}\n`);
      },
    }),
    /could not capture a stable working tree after 4 captures/,
  );
});

function cryptoHash(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

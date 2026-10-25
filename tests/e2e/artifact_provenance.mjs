import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { execFileSync, spawnSync } from "node:child_process";

import { repoRoot } from "../../tools/browser_firefox_rtc_smoke.mjs";

const DEFAULT_SERVER_BINARY = "target/debug/zubspace_server";
const DEFAULT_WASM = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const DEFAULT_CLIENT_ENTRY = "index.html";
const DEFAULT_E2E_SCRIPT = "tests/e2e/combat_replay.mjs";
const DEFAULT_ENTRY_CAPTURE_ATTEMPTS = 4;
const DEFAULT_TREE_CAPTURE_ATTEMPTS = 4;
// Owner-only planning documents are not build inputs. Bind their indexed blob
// IDs through the index manifest, but never open, diff or copy their contents.
const CONTENT_EXCLUSIONS = ["plans/prod_deployment.md", "plans/release_caching_and_versioning.md"];
const contentExcluded = relativePath => CONTENT_EXCLUSIONS.includes(relativePath);
const DIRECTORY_OPEN_FLAGS = fs.constants.O_RDONLY |
  (fs.constants.O_DIRECTORY || 0) |
  (fs.constants.O_NOFOLLOW || 0) |
  (fs.constants.O_NONBLOCK || 0);
const FILE_OPEN_FLAGS = fs.constants.O_RDONLY |
  (fs.constants.O_NOFOLLOW || 0) |
  (fs.constants.O_NONBLOCK || 0);

class ProvenanceCaptureRaceError extends Error {}

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function updateFramed(hash, value) {
  const bytes = Buffer.isBuffer(value) ? value : Buffer.from(String(value));
  hash.update(String(bytes.length));
  hash.update("\0");
  hash.update(bytes);
}

function stableStatIdentity(stat) {
  return [
    stat.dev,
    stat.ino,
    stat.mode,
    stat.size,
    stat.mtimeNs,
    stat.ctimeNs,
  ].map(String).join(":");
}

function sameStableStat(left, right) {
  return stableStatIdentity(left) === stableStatIdentity(right);
}

function captureHook(options, stage, details) {
  options.mutationHook?.({ stage, ...details });
}

function raceError(relativePath, detail) {
  return new ProvenanceCaptureRaceError(
    `provenance entry ${JSON.stringify(relativePath)} changed during capture (${detail})`,
  );
}

function lstatBigInt(absolutePath) {
  return fs.lstatSync(absolutePath, { bigint: true });
}

function descriptorPath(descriptor, childName = null) {
  const base = `/proc/self/fd/${descriptor}`;
  return childName == null ? base : path.join(base, childName);
}

function filesystemEvidence(relativePath, entryType, mode, payload, stat, extras = {}) {
  const hash = crypto.createHash("sha256");
  hash.update("zubspace-filesystem-entry-v2\0");
  updateFramed(hash, relativePath);
  updateFramed(hash, entryType);
  updateFramed(hash, mode.toString(8));
  updateFramed(hash, payload);
  return {
    path: relativePath,
    entryType,
    mode,
    bytes: payload.length,
    sha256: hash.digest("hex"),
    contentSha256: sha256(payload),
    ...extras,
    modifiedAt: new Date(Number(stat.mtimeNs / 1_000_000n)).toISOString(),
  };
}

function captureAnchoredEntryOnce(
  parentDescriptor,
  childName,
  relativePath,
  options,
  attempt,
) {
  const anchoredPath = descriptorPath(parentDescriptor, childName);
  const stat = lstatBigInt(anchoredPath);
  const mode = Number(stat.mode & 0o7777n);
  let entryType;
  let payload;
  let linkTarget = null;
  if (stat.isFile()) {
    entryType = "regular";
    captureHook(options, "after-entry-lstat", { attempt, relativePath, entryType });
    let descriptor;
    try {
      descriptor = fs.openSync(
        anchoredPath,
        FILE_OPEN_FLAGS,
      );
      const opened = fs.fstatSync(descriptor, { bigint: true });
      if (!opened.isFile() || !sameStableStat(stat, opened)) {
        throw raceError(relativePath, "pathname/open descriptor identity mismatch");
      }
      captureHook(options, "after-entry-open", { attempt, relativePath, entryType });
      payload = fs.readFileSync(descriptor);
      captureHook(options, "after-entry-payload", { attempt, relativePath, entryType });
      const descriptorAfter = fs.fstatSync(descriptor, { bigint: true });
      const pathnameAfter = lstatBigInt(anchoredPath);
      if (
        !descriptorAfter.isFile() ||
        !sameStableStat(opened, descriptorAfter) ||
        !sameStableStat(descriptorAfter, pathnameAfter)
      ) {
        throw raceError(relativePath, "regular file changed while descriptor was read");
      }
    } finally {
      if (descriptor != null) fs.closeSync(descriptor);
    }
  } else if (stat.isSymbolicLink()) {
    entryType = "symlink";
    captureHook(options, "after-entry-lstat", { attempt, relativePath, entryType });
    linkTarget = fs.readlinkSync(anchoredPath);
    payload = Buffer.from(linkTarget);
    captureHook(options, "after-entry-payload", { attempt, relativePath, entryType });
    const pathnameAfter = lstatBigInt(anchoredPath);
    if (!pathnameAfter.isSymbolicLink() || !sameStableStat(stat, pathnameAfter)) {
      throw raceError(relativePath, "symbolic link changed while target was read");
    }
  } else if (stat.isDirectory()) {
    captureHook(options, "after-entry-lstat", {
      attempt,
      relativePath,
      entryType: "directory",
    });
    const pathnameAfter = lstatBigInt(anchoredPath);
    if (!pathnameAfter.isDirectory() || !sameStableStat(stat, pathnameAfter)) {
      throw raceError(relativePath, "directory changed while classified");
    }
    throw new Error(
      `provenance entry ${JSON.stringify(relativePath)} is a directory; ` +
        "declare individual files instead",
    );
  } else {
    captureHook(options, "after-entry-lstat", {
      attempt,
      relativePath,
      entryType: "special",
    });
    const pathnameAfter = lstatBigInt(anchoredPath);
    if (!sameStableStat(stat, pathnameAfter)) {
      throw raceError(relativePath, "special entry changed while classified");
    }
    throw new Error(
      `provenance entry ${JSON.stringify(relativePath)} has unsupported filesystem type`,
    );
  }
  return filesystemEvidence(
    relativePath,
    entryType,
    mode,
    payload,
    stat,
    linkTarget == null ? {} : { linkTarget },
  );
}

function openDirectoryChain(absoluteDirectory, options, attempt) {
  const normalized = path.resolve(absoluteDirectory);
  const rootPath = path.parse(normalized).root;
  const components = path.relative(rootPath, normalized).split(path.sep).filter(Boolean);
  const descriptors = [];
  const rootStat = lstatBigInt(rootPath);
  if (!rootStat.isDirectory()) throw raceError(rootPath, "filesystem root is not a directory");
  const rootDescriptor = fs.openSync(rootPath, DIRECTORY_OPEN_FLAGS);
  const rootOpened = fs.fstatSync(rootDescriptor, { bigint: true });
  if (!rootOpened.isDirectory() || !sameStableStat(rootStat, rootOpened)) {
    fs.closeSync(rootDescriptor);
    throw raceError(rootPath, "filesystem root descriptor identity mismatch");
  }
  descriptors.push({ descriptor: rootDescriptor, stat: rootOpened, absolutePath: rootPath });

  try {
    let parentDescriptor = rootDescriptor;
    let componentPath = rootPath;
    for (const component of components) {
      componentPath = path.join(componentPath, component);
      const anchoredPath = descriptorPath(parentDescriptor, component);
      const before = lstatBigInt(anchoredPath);
      captureHook(options, "after-ancestor-lstat", {
        attempt,
        relativePath: componentPath,
        entryType: before.isDirectory() ? "directory" : "other",
      });
      if (!before.isDirectory()) {
        throw raceError(componentPath, "intermediate ancestor is not a directory");
      }
      const descriptor = fs.openSync(anchoredPath, DIRECTORY_OPEN_FLAGS);
      const opened = fs.fstatSync(descriptor, { bigint: true });
      if (!opened.isDirectory() || !sameStableStat(before, opened)) {
        fs.closeSync(descriptor);
        throw raceError(componentPath, "ancestor pathname/descriptor identity mismatch");
      }
      descriptors.push({
        descriptor,
        stat: opened,
        absolutePath: componentPath,
        parentDescriptor,
        childName: component,
      });
      parentDescriptor = descriptor;
    }
  } catch (error) {
    for (const entry of descriptors.reverse()) fs.closeSync(entry.descriptor);
    throw error;
  }

  return {
    descriptor: descriptors.at(-1).descriptor,
    revalidate() {
      for (let index = descriptors.length - 1; index >= 0; index -= 1) {
        const entry = descriptors[index];
        const descriptorAfter = fs.fstatSync(entry.descriptor, { bigint: true });
        const pathnameAfter = entry.parentDescriptor == null
          ? lstatBigInt(entry.absolutePath)
          : lstatBigInt(descriptorPath(entry.parentDescriptor, entry.childName));
        if (
          !descriptorAfter.isDirectory() ||
          !pathnameAfter.isDirectory() ||
          !sameStableStat(entry.stat, descriptorAfter) ||
          !sameStableStat(descriptorAfter, pathnameAfter)
        ) {
          throw raceError(entry.absolutePath, "ancestor changed during capture");
        }
      }
    },
    close() {
      for (const entry of descriptors.reverse()) fs.closeSync(entry.descriptor);
    },
  };
}

function captureFilesystemEntryOnce(root, relativePath, options, attempt) {
  const absolutePath = path.isAbsolute(relativePath)
    ? path.normalize(relativePath)
    : path.resolve(root, relativePath);
  const parentPath = path.dirname(absolutePath);
  const childName = path.basename(absolutePath);
  const chain = openDirectoryChain(parentPath, options, attempt);
  try {
    const evidence = captureAnchoredEntryOnce(
      chain.descriptor,
      childName,
      relativePath,
      options,
      attempt,
    );
    chain.revalidate();
    return evidence;
  } finally {
    chain.close();
  }
}

function filesystemEntryIdentity(root, relativePath, options = {}) {
  const attempts = options.entryCaptureAttempts ?? DEFAULT_ENTRY_CAPTURE_ATTEMPTS;
  if (!Number.isInteger(attempts) || attempts < 1) {
    throw new Error("entryCaptureAttempts must be a positive integer");
  }
  let lastRace = null;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      return captureFilesystemEntryOnce(root, relativePath, options, attempt);
    } catch (error) {
      const raceLikeCode = ["ELOOP", "ENOENT", "ENOTDIR", "ESTALE", "EISDIR", "EINVAL"].includes(
        error?.code,
      );
      if (!(error instanceof ProvenanceCaptureRaceError) && !raceLikeCode) throw error;
      lastRace = error;
    }
  }
  throw new Error(
    `provenance could not capture stable entry ${JSON.stringify(relativePath)} after ` +
      `${attempts} attempts: ${lastRace?.message || "entry kept changing"}`,
  );
}

function fileEvidence(relativePath) {
  return filesystemEntryIdentity(repoRoot, relativePath);
}

function dockerImageEvidence(name) {
  let raw;
  try {
    raw = execFileSync("docker", ["image", "inspect", name], {
      cwd: repoRoot,
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
    });
  } catch (error) {
    throw new Error(`cannot inspect retained Docker image ${JSON.stringify(name)}: ${error.message}`);
  }
  const [image] = JSON.parse(raw);
  if (!image?.Id) {
    throw new Error(`Docker did not return a content id for ${JSON.stringify(name)}`);
  }
  return {
    name,
    id: image.Id,
    repoDigests: [...(image.RepoDigests || [])].sort(),
    createdAt: image.Created || null,
  };
}

function commandBytes(args, root = repoRoot) {
  return execFileSync("git", args, {
    cwd: root,
    encoding: "buffer",
    maxBuffer: 128 * 1024 * 1024,
  });
}

function ignoredPaths(root, relativePaths) {
  if (relativePaths.length === 0) return new Set();
  const result = spawnSync("git", ["check-ignore", "--stdin", "-z"], {
    cwd: root,
    encoding: "buffer",
    input: Buffer.from(`${relativePaths.join("\0")}\0`),
    maxBuffer: 128 * 1024 * 1024,
  });
  if (result.error) {
    throw new Error(`cannot apply Git ignore rules during provenance walk: ${result.error.message}`);
  }
  if (result.status !== 0 && result.status !== 1) {
    throw new Error(
      `git check-ignore failed during provenance walk with status ${result.status}: ` +
        Buffer.from(result.stderr || "").toString("utf8"),
    );
  }
  return new Set(
    Buffer.from(result.stdout || "")
      .toString("utf8")
      .split("\0")
      .filter(Boolean),
  );
}

function repositoryNegationPrefixes(root, trackedPaths, declaredUntracked) {
  const candidates = [...trackedPaths, ...declaredUntracked]
    .filter((relativePath) => path.posix.basename(relativePath) === ".gitignore")
    .map((relativePath) => ({
      absolutePath: path.join(root, relativePath),
      base: path.posix.dirname(relativePath) === "." ? "" : path.posix.dirname(relativePath),
    }));
  candidates.push({ absolutePath: path.join(root, ".git/info/exclude"), base: "" });
  const configuredExclude = spawnSync("git", ["config", "--path", "--get", "core.excludesFile"], {
    cwd: root,
    encoding: "utf8",
  });
  if (configuredExclude.status === 0 && configuredExclude.stdout.trim()) {
    candidates.push({ absolutePath: configuredExclude.stdout.trim(), base: "" });
  }
  const prefixes = [];
  for (const { absolutePath, base } of candidates) {
    if (!fs.existsSync(absolutePath) || !fs.lstatSync(absolutePath).isFile()) continue;
    for (const line of fs.readFileSync(absolutePath, "utf8").split(/\r?\n/)) {
      if (!line.startsWith("!") || line.length === 1) continue;
      let pattern = line.slice(1);
      if (pattern.startsWith("/")) pattern = pattern.slice(1);
      const scopedPattern = pattern.replace(/\/+$/, "");
      const firstWildcard = pattern.search(/[?*[\\]/);
      const literal = (firstWildcard < 0 ? pattern : pattern.slice(0, firstWildcard))
        .replace(/\/+$/, "");
      const prefix = scopedPattern.includes("/")
        ? [base, literal].filter(Boolean).join("/")
        : base;
      prefixes.push(prefix);
    }
  }
  return [...new Set(prefixes)].sort();
}

function directoryEvidence(relativePath, stat, descendants) {
  const payload = Buffer.from(JSON.stringify(
    [...descendants]
      .sort((left, right) => left.path < right.path ? -1 : left.path > right.path ? 1 : 0)
      .map((entry) => [entry.path, entry.sha256]),
  ));
  return filesystemEvidence(
    relativePath,
    "directory",
    Number(stat.mode & 0o7777n),
    payload,
    stat,
  );
}

function parseIndexManifest(indexState) {
  const entries = new Map();
  for (const record of indexState.toString("utf8").split("\0").filter(Boolean)) {
    const match = /^([0-7]{6}) ([0-9a-f]+) ([0-3])\t([\s\S]+)$/.exec(record);
    if (!match) throw new Error(`cannot parse Git index entry ${JSON.stringify(record)}`);
    const [, mode, objectId, stageText, relativePath] = match;
    const stage = Number(stageText);
    if (entries.has(relativePath)) {
      throw new Error(
        `provenance does not support conflicted/multi-stage index path ${JSON.stringify(relativePath)}`,
      );
    }
    entries.set(relativePath, { mode, objectId, stage });
  }
  return entries;
}

function gitlinkEvidence(relativePath, indexEntry, checkout, stat = null) {
  const mode = stat == null ? null : Number(stat.mode & 0o7777n);
  const payload = Buffer.from(JSON.stringify({
    indexMode: indexEntry.mode,
    indexObjectId: indexEntry.objectId,
    checkoutState: checkout.state,
    checkoutHead: checkout.head,
  }));
  const hash = crypto.createHash("sha256");
  hash.update("zubspace-filesystem-entry-v2\0");
  updateFramed(hash, relativePath);
  updateFramed(hash, "gitlink");
  updateFramed(hash, mode == null ? "absent" : mode.toString(8));
  updateFramed(hash, payload);
  return {
    path: relativePath,
    entryType: "gitlink",
    mode,
    bytes: payload.length,
    sha256: hash.digest("hex"),
    contentSha256: sha256(payload),
    indexMode: indexEntry.mode,
    indexObjectId: indexEntry.objectId,
    checkoutState: checkout.state,
    checkoutHead: checkout.head,
    modifiedAt: stat == null
      ? null
      : new Date(Number(stat.mtimeNs / 1_000_000n)).toISOString(),
  };
}

function gitCommandResult(root, args) {
  const result = spawnSync("git", args, {
    cwd: root,
    encoding: "buffer",
    maxBuffer: 128 * 1024 * 1024,
  });
  if (result.error || result.status !== 0) {
    throw new Error(
      `git ${args.join(" ")} failed for gitlink ${JSON.stringify(root)}: ` +
        (result.error?.message || Buffer.from(result.stderr || "").toString("utf8")),
    );
  }
  return Buffer.from(result.stdout || "");
}

function captureGitlinkEntry(
  root,
  directoryDescriptor,
  childName,
  relativePath,
  stat,
  indexEntry,
  options,
  captureAttempt,
) {
  if (indexEntry.stage !== 0 || indexEntry.mode !== "160000") {
    throw new Error(`invalid gitlink index entry for ${JSON.stringify(relativePath)}`);
  }
  if (!stat.isDirectory()) {
    throw new Error(
      `gitlink ${JSON.stringify(relativePath)} must be an initialized/uninitialized directory, ` +
        "not a file, symlink, or special entry",
    );
  }

  const anchoredPath = descriptorPath(directoryDescriptor, childName);
  let descriptor;
  try {
    descriptor = fs.openSync(anchoredPath, DIRECTORY_OPEN_FLAGS);
    const opened = fs.fstatSync(descriptor, { bigint: true });
    if (!opened.isDirectory() || !sameStableStat(stat, opened)) {
      throw raceError(relativePath, "gitlink pathname/descriptor identity mismatch");
    }
    const firstChildren = fs.readdirSync(descriptorPath(descriptor)).sort();
    captureHook(options, "after-gitlink-enumeration", {
      attempt: captureAttempt,
      captureAttempt,
      relativePath,
      childNames: [...firstChildren],
    });

    let checkout;
    if (firstChildren.length === 0) {
      checkout = { state: "uninitialized", head: null };
    } else {
      if (!firstChildren.includes(".git")) {
        throw new Error(
          `gitlink ${JSON.stringify(relativePath)} is a nonempty replacement directory ` +
            "without an initialized checkout",
        );
      }
      const absolutePath = path.join(root, relativePath);
      const headBefore = gitCommandResult(absolutePath, ["rev-parse", "--verify", "HEAD"])
        .toString("utf8").trim();
      const superproject = path.resolve(gitCommandResult(
        absolutePath,
        ["rev-parse", "--show-superproject-working-tree"],
      ).toString("utf8").trim());
      const statusBefore = gitCommandResult(absolutePath, [
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=none",
      ]);
      const nestedSubmodules = gitCommandResult(
        absolutePath,
        ["submodule", "status", "--recursive"],
      );
      captureHook(options, "after-gitlink-status", {
        attempt: captureAttempt,
        captureAttempt,
        relativePath,
      });
      const statusAfter = gitCommandResult(absolutePath, [
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=none",
      ]);
      const headAfter = gitCommandResult(absolutePath, ["rev-parse", "--verify", "HEAD"])
        .toString("utf8").trim();
      if (superproject !== path.resolve(root)) {
        throw new Error(
          `gitlink ${JSON.stringify(relativePath)} is not owned by the captured superproject`,
        );
      }
      if (headBefore !== headAfter) {
        throw raceError(relativePath, "gitlink HEAD changed during capture");
      }
      if (headAfter !== indexEntry.objectId) {
        throw new Error(
          `gitlink ${JSON.stringify(relativePath)} checkout HEAD ${headAfter} does not match ` +
            `index object ${indexEntry.objectId}`,
        );
      }
      if (!statusBefore.equals(statusAfter)) {
        throw raceError(relativePath, "gitlink status changed during capture");
      }
      if (statusAfter.length !== 0) {
        throw new Error(
          `gitlink ${JSON.stringify(relativePath)} has dirty tracked or untracked nested state`,
        );
      }
      if (nestedSubmodules.length !== 0) {
        throw new Error(
          `gitlink ${JSON.stringify(relativePath)} contains unsupported nested submodules`,
        );
      }
      checkout = { state: "initialized-clean", head: headAfter };
    }

    const secondChildren = fs.readdirSync(descriptorPath(descriptor)).sort();
    const descriptorAfter = fs.fstatSync(descriptor, { bigint: true });
    const pathnameAfter = lstatBigInt(anchoredPath);
    if (
      JSON.stringify(firstChildren) !== JSON.stringify(secondChildren) ||
      !descriptorAfter.isDirectory() ||
      !pathnameAfter.isDirectory() ||
      !sameStableStat(opened, descriptorAfter) ||
      !sameStableStat(descriptorAfter, pathnameAfter)
    ) {
      throw raceError(relativePath, "gitlink directory changed during capture");
    }
    return gitlinkEvidence(relativePath, indexEntry, checkout, stat);
  } finally {
    if (descriptor != null) fs.closeSync(descriptor);
  }
}

function isCaptureRace(error) {
  return error instanceof ProvenanceCaptureRaceError ||
    ["ELOOP", "ENOENT", "ENOTDIR", "ESTALE", "EISDIR", "EINVAL"].includes(error?.code);
}

function walkWorkingTreeFilesystem(
  root,
  trackedPaths,
  declaredUntracked,
  gitlinks,
  options = {},
) {
  const negationPrefixes = repositoryNegationPrefixes(
    root,
    trackedPaths,
    declaredUntracked,
  );
  const mayReincludeBelow = (relativePath) => negationPrefixes.some((prefix) =>
    prefix === "" ||
    prefix === relativePath ||
    prefix.startsWith(`${relativePath}/`) ||
    relativePath.startsWith(`${prefix}/`));
  const hasDeclaredDescendant = (relativePath) => declaredUntracked.some((entry) =>
    entry.startsWith(`${relativePath}/`));
  const hasTrackedDescendant = (relativePath) => [...trackedPaths].some((entry) =>
    entry.startsWith(`${relativePath}/`));
  const captureAttempt = options.captureAttempt ?? 0;
  const rootChain = openDirectoryChain(root, options, captureAttempt);

  function walk(directoryDescriptor, relativeDirectory) {
    const directoryBefore = fs.fstatSync(directoryDescriptor, { bigint: true });
    const visibleNames = fs.readdirSync(descriptorPath(directoryDescriptor))
      .filter((childName) => childName !== ".git");
    const childNames = visibleNames.filter(childName => !contentExcluded(relativeDirectory
      ? path.posix.join(relativeDirectory, childName) : childName)).sort();
    captureHook(options, "after-directory-enumeration", {
      attempt: captureAttempt,
      captureAttempt,
      relativePath: relativeDirectory,
      childNames: [...childNames],
    });
    const children = childNames
      .map((childName) => {
        const relativePath = relativeDirectory
          ? path.posix.join(relativeDirectory, childName)
          : childName;
        const stat = lstatBigInt(descriptorPath(directoryDescriptor, childName));
        return { childName, relativePath, stat };
      });
    const ignoreQueries = children
      .filter(({ relativePath }) => !gitlinks.has(relativePath))
      .map(({ relativePath, stat }) => stat.isDirectory() ? `${relativePath}/` : relativePath);
    const ignored = ignoredPaths(root, ignoreQueries);
    // A directory containing only excluded entries is not an accidentally
    // empty untracked directory. No excluded entry is stat'ed or opened.
    let visibleDescendants = visibleNames.length - childNames.length;
    let untrackedLeafCount = 0;
    const trackedEntries = [];
    const untrackedEntries = [];

    for (let index = 0; index < children.length; index += 1) {
      const { childName, relativePath, stat } = children[index];
      const ignoreQuery = stat.isDirectory() ? `${relativePath}/` : relativePath;
      const isIgnored = !gitlinks.has(relativePath) && ignored.has(ignoreQuery);
      if (isIgnored && !trackedPaths.has(relativePath) && !(
        stat.isDirectory() && (
          mayReincludeBelow(relativePath) ||
          hasDeclaredDescendant(relativePath) ||
          hasTrackedDescendant(relativePath)
        )
      )) continue;
      if (gitlinks.has(relativePath)) {
        visibleDescendants += 1;
        trackedEntries.push(captureGitlinkEntry(
          root,
          directoryDescriptor,
          childName,
          relativePath,
          stat,
          gitlinks.get(relativePath),
          options,
          captureAttempt,
        ));
        continue;
      }
      if (stat.isDirectory()) {
        captureHook(options, "after-child-lstat", {
          attempt: captureAttempt,
          captureAttempt,
          relativePath,
          entryType: "directory",
        });
        const anchoredPath = descriptorPath(directoryDescriptor, childName);
        let childDescriptor;
        let childResult;
        try {
          childDescriptor = fs.openSync(anchoredPath, DIRECTORY_OPEN_FLAGS);
          const opened = fs.fstatSync(childDescriptor, { bigint: true });
          if (!opened.isDirectory() || !sameStableStat(stat, opened)) {
            throw raceError(relativePath, "child directory pathname/descriptor identity mismatch");
          }
          childResult = walk(childDescriptor, relativePath);
          captureHook(options, "before-child-directory-revalidate", {
            attempt: captureAttempt,
            captureAttempt,
            relativePath,
            entryType: "directory",
          });
          const descriptorAfter = fs.fstatSync(childDescriptor, { bigint: true });
          const pathnameAfter = lstatBigInt(anchoredPath);
          if (
            !descriptorAfter.isDirectory() ||
            !pathnameAfter.isDirectory() ||
            !sameStableStat(stat, descriptorAfter) ||
            !sameStableStat(descriptorAfter, pathnameAfter)
          ) {
            throw raceError(relativePath, "child directory changed during traversal");
          }
        } finally {
          if (childDescriptor != null) fs.closeSync(childDescriptor);
        }
        if (childResult.visibleDescendants === 0 && !isIgnored) {
          throw new Error(
            `provenance found empty untracked directory ${JSON.stringify(relativePath)}; ` +
              "remove or ignore it",
          );
        }
        visibleDescendants += childResult.visibleDescendants;
        untrackedLeafCount += childResult.untrackedLeafCount;
        trackedEntries.push(...childResult.trackedEntries);
        untrackedEntries.push(...childResult.untrackedEntries);
        if (childResult.untrackedLeafCount > 0 && !isIgnored) {
          untrackedEntries.push(directoryEvidence(relativePath, stat, childResult.untrackedEntries));
        }
        continue;
      }
      visibleDescendants += 1;
      const evidence = captureAnchoredEntryOnce(
        directoryDescriptor,
        childName,
        relativePath,
        options,
        captureAttempt,
      );
      if (trackedPaths.has(relativePath)) {
        trackedEntries.push(evidence);
      } else {
        untrackedLeafCount += 1;
        untrackedEntries.push(evidence);
      }
    }
    captureHook(options, "before-directory-revalidate", {
      attempt: captureAttempt,
      captureAttempt,
      relativePath: relativeDirectory,
    });
    const directoryAfter = fs.fstatSync(directoryDescriptor, { bigint: true });
    if (!directoryAfter.isDirectory() || !sameStableStat(directoryBefore, directoryAfter)) {
      throw raceError(relativeDirectory || ".", "directory changed during enumeration");
    }
    return { visibleDescendants, untrackedLeafCount, trackedEntries, untrackedEntries };
  }

  try {
    const result = walk(rootChain.descriptor, "");
    rootChain.revalidate();
    const comparePath = (left, right) => left.path < right.path ? -1 :
      left.path > right.path ? 1 : 0;
    return {
      trackedEntries: result.trackedEntries.sort(comparePath),
      untrackedEntries: result.untrackedEntries.sort(comparePath),
    };
  } finally {
    rootChain.close();
  }
}

function captureGitWorkingState(root) {
  const trackedDiff = commandBytes(
    ["diff", "--binary", "--no-ext-diff", "HEAD", "--", ".",
      ...CONTENT_EXCLUSIONS.map(relativePath => `:(exclude)${relativePath}`)],
    root,
  );
  // The index manifest binds staged changes, intent-to-add entries, executable
  // bits, symlinks, and any gitlink. The descriptor evidence below separately
  // binds the live worktree bytes/type/mode.
  const indexState = commandBytes(["ls-files", "--stage", "-z"], root);
  const trackedPathState = commandBytes(["ls-files", "-z"], root);
  const declaredUntrackedState = commandBytes(
    ["ls-files", "--others", "--exclude-standard", "-z"],
    root,
  );
  return { trackedDiff, indexState, trackedPathState, declaredUntrackedState };
}

function sameGitWorkingState(left, right) {
  return left.trackedDiff.equals(right.trackedDiff) &&
    left.indexState.equals(right.indexState) &&
    left.trackedPathState.equals(right.trackedPathState) &&
    left.declaredUntrackedState.equals(right.declaredUntrackedState);
}

function captureWorkingTree(root, options) {
  const before = captureGitWorkingState(root);
  captureHook(options, "after-git-state-before-filesystem", {
    attempt: options.captureAttempt ?? 0,
    captureAttempt: options.captureAttempt ?? 0,
    root,
  });
  const trackedPaths = new Set(before.trackedPathState
    .toString("utf8")
    .split("\0")
    .filter(entry => entry && !contentExcluded(entry)));
  const indexManifest = parseIndexManifest(before.indexState);
  const gitlinks = new Map(
    [...indexManifest].filter(([, entry]) => entry.mode === "160000"),
  );
  const declaredUntracked = before.declaredUntrackedState
    .toString("utf8")
    .split("\0")
    .filter(entry => entry && !contentExcluded(entry))
    .sort();
  const filesystem = walkWorkingTreeFilesystem(
    root, trackedPaths, declaredUntracked, gitlinks, options,
  );
  const after = captureGitWorkingState(root);
  captureHook(options, "after-trailing-git-state", {
    attempt: options.captureAttempt ?? 0,
    captureAttempt: options.captureAttempt ?? 0,
    root,
  });
  const finalState = captureGitWorkingState(root);
  if (!sameGitWorkingState(before, after) || !sameGitWorkingState(after, finalState)) {
    throw raceError(".", "Git tracked/index/untracked state changed around descriptor capture");
  }
  const trackedEntries = [...filesystem.trackedEntries];
  const seenGitlinks = new Set(
    trackedEntries.filter((entry) => entry.entryType === "gitlink").map((entry) => entry.path),
  );
  for (const [relativePath, indexEntry] of gitlinks) {
    if (seenGitlinks.has(relativePath)) continue;
    trackedEntries.push(gitlinkEvidence(
      relativePath,
      indexEntry,
      { state: "uninitialized-absent", head: null },
    ));
  }
  trackedEntries.sort((left, right) => left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
  const untrackedEntries = filesystem.untrackedEntries;
  const seenTracked = new Set(trackedEntries.map((entry) => entry.path));
  const trackedMissing = [...trackedPaths].filter((entry) => !seenTracked.has(entry)).sort();
  const untracked = untrackedEntries
    .filter((entry) => entry.entryType !== "directory")
    .map((entry) => entry.path);
  if (JSON.stringify(untracked) !== JSON.stringify(declaredUntracked)) {
    throw new Error(`independent provenance walk disagrees with Git's untracked set: ${JSON.stringify({
      filesystemWalk: untracked,
      gitDeclared: declaredUntracked,
    })}`);
  }
  return {
    trackedDiff: before.trackedDiff,
    indexState: before.indexState,
    trackedEntries,
    trackedMissing,
    untrackedEntries,
    untracked,
    untrackedDirectories: untrackedEntries
      .filter((entry) => entry.entryType === "directory")
      .map((entry) => entry.path),
    gitlinks: trackedEntries.filter((entry) => entry.entryType === "gitlink"),
  };
}

function sameWorkingTreeCapture(left, right) {
  return left.trackedDiff.equals(right.trackedDiff) &&
    left.indexState.equals(right.indexState) &&
    JSON.stringify(left.trackedEntries) === JSON.stringify(right.trackedEntries) &&
    JSON.stringify(left.trackedMissing) === JSON.stringify(right.trackedMissing) &&
    JSON.stringify(left.untrackedEntries) === JSON.stringify(right.untrackedEntries);
}

function deterministicWorkingTreeDiffHash(root = repoRoot, options = {}) {
  const attempts = options.treeCaptureAttempts ?? DEFAULT_TREE_CAPTURE_ATTEMPTS;
  if (!Number.isInteger(attempts) || attempts < 1) {
    throw new Error("treeCaptureAttempts must be a positive integer");
  }
  let stable = null;
  let previous = null;
  let lastRace = null;
  for (let captureAttempt = 0; captureAttempt <= attempts; captureAttempt += 1) {
    if (captureAttempt > 0) {
      captureHook(options, "between-tree-captures", {
        attempt: captureAttempt - 1,
        captureAttempt,
        root,
      });
    }
    let current;
    try {
      current = captureWorkingTree(root, { ...options, captureAttempt });
    } catch (error) {
      if (!isCaptureRace(error)) throw error;
      lastRace = error;
      previous = null;
      continue;
    }
    if (previous && sameWorkingTreeCapture(previous, current)) {
      stable = current;
      break;
    }
    previous = current;
  }
  if (!stable) {
    throw new Error(
      `provenance could not capture a stable working tree after ${attempts + 1} captures` +
        (lastRace ? `: ${lastRace.message}` : ""),
    );
  }

  const hash = crypto.createHash("sha256");
  hash.update("zubspace-working-tree-diff-v8\0");
  updateFramed(hash, JSON.stringify(CONTENT_EXCLUSIONS));
  hash.update(String(stable.trackedDiff.length));
  hash.update("\0");
  hash.update(stable.trackedDiff);
  updateFramed(hash, stable.indexState);
  for (const entry of stable.trackedEntries) {
    hash.update("tracked-entry\0");
    updateFramed(hash, entry.sha256);
  }
  for (const relativePath of stable.trackedMissing) {
    hash.update("tracked-missing\0");
    updateFramed(hash, relativePath);
  }
  for (const entry of stable.untrackedEntries) {
    hash.update("untracked-entry\0");
    updateFramed(hash, entry.sha256);
  }
  return {
    sha256: hash.digest("hex"),
    contentExclusions: [...CONTENT_EXCLUSIONS],
    trackedDiffBytes: stable.trackedDiff.length,
    trackedIndexBytes: stable.indexState.length,
    trackedEntries: stable.trackedEntries,
    trackedMissing: stable.trackedMissing,
    untrackedFiles: stable.untracked,
    untrackedDirectories: stable.untrackedDirectories,
    untrackedEntries: stable.untrackedEntries,
    gitlinks: stable.gitlinks,
  };
}

function collectArtifactProvenance({
  serverBinary = DEFAULT_SERVER_BINARY,
  wasm = DEFAULT_WASM,
  clientEntry = DEFAULT_CLIENT_ENTRY,
  e2eScript = DEFAULT_E2E_SCRIPT,
  additionalFiles = [],
  dockerImages = [],
  runStartedAt = null,
} = {}) {
  const server = fileEvidence(serverBinary);
  const wasmEvidence = fileEvidence(wasm);
  const clientEntryEvidence = fileEvidence(clientEntry);
  const script = fileEvidence(e2eScript);
  const workingTree = deterministicWorkingTreeDiffHash();
  const clientBuildManifest = JSON.stringify([
    [clientEntryEvidence.path, clientEntryEvidence.sha256],
    [wasmEvidence.path, wasmEvidence.sha256],
  ]);
  return {
    formatVersion: 8,
    capturedAt: new Date().toISOString(),
    runStartedAt,
    gitHead: commandBytes(["rev-parse", "HEAD"]).toString("utf8").trim(),
    workingTreeDiffSha256: workingTree.sha256,
    workingTreeTrackedDiffBytes: workingTree.trackedDiffBytes,
    workingTreeTrackedIndexBytes: workingTree.trackedIndexBytes,
    workingTreeTrackedEntries: workingTree.trackedEntries,
    workingTreeTrackedMissing: workingTree.trackedMissing,
    workingTreeUntrackedFiles: workingTree.untrackedFiles,
    workingTreeUntrackedDirectories: workingTree.untrackedDirectories,
    workingTreeUntrackedEntries: workingTree.untrackedEntries,
    workingTreeGitlinks: workingTree.gitlinks,
    workingTreeContentExclusions: workingTree.contentExclusions,
    serverBinary: server,
    wasm: wasmEvidence,
    clientEntry: clientEntryEvidence,
    clientBuildSha256: sha256(Buffer.from(clientBuildManifest)),
    e2eScript: script,
    additionalFiles: [...new Set(additionalFiles)]
      .sort()
      .map((relativePath) => fileEvidence(relativePath)),
    dockerImages: [...new Set(dockerImages)]
      .sort()
      .map((name) => dockerImageEvidence(name)),
  };
}

function comparableProvenance(provenance) {
  return {
    formatVersion: provenance?.formatVersion,
    gitHead: provenance?.gitHead,
    workingTreeDiffSha256: provenance?.workingTreeDiffSha256,
    serverBinary: provenance?.serverBinary?.sha256,
    wasm: provenance?.wasm?.sha256,
    clientEntry: provenance?.clientEntry?.sha256,
    clientBuildSha256: provenance?.clientBuildSha256,
    e2eScript: provenance?.e2eScript?.sha256,
    additionalFiles: (provenance?.additionalFiles || []).map((entry) => [
      entry.path,
      entry.sha256,
    ]),
    dockerImages: (provenance?.dockerImages || []).map((entry) => [
      entry.name,
      entry.id,
      entry.repoDigests,
    ]),
  };
}

function assertArtifactProvenanceMatches(expected, current) {
  const expectedComparable = comparableProvenance(expected);
  const currentComparable = comparableProvenance(current);
  if (JSON.stringify(expectedComparable) !== JSON.stringify(currentComparable)) {
    throw new Error(`artifact provenance drifted during or after the E2E run: ${JSON.stringify({
      expected: expectedComparable,
      current: currentComparable,
    })}`);
  }
  return true;
}

export {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
  comparableProvenance,
  deterministicWorkingTreeDiffHash,
  fileEvidence,
  filesystemEntryIdentity,
};

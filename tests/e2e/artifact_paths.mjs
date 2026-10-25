import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { validateBrowserConsoleArtifacts } from "../../tools/browser_console_evidence.mjs";

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function stableFileEvidence(absolutePath) {
  const before = fs.lstatSync(absolutePath);
  if (!before.isFile() || before.isSymbolicLink()) {
    throw new Error(`retained artifact is not a regular file: ${absolutePath}`);
  }
  const bytes = fs.readFileSync(absolutePath);
  const after = fs.lstatSync(absolutePath);
  if (
    !after.isFile() || after.isSymbolicLink() ||
    before.dev !== after.dev || before.ino !== after.ino ||
    before.mode !== after.mode || before.size !== after.size ||
    before.mtimeMs !== after.mtimeMs
  ) {
    throw new Error(`retained artifact changed while hashing: ${absolutePath}`);
  }
  return { bytes: bytes.length, sha256: sha256(bytes) };
}

function walkArtifactFiles(root, relative = "") {
  const absolute = path.join(root, relative);
  const stat = fs.lstatSync(absolute);
  if (!stat.isDirectory() || stat.isSymbolicLink()) {
    throw new Error(`retained artifact root is not a real directory: ${root}`);
  }
  const files = [];
  for (const name of fs.readdirSync(absolute).sort()) {
    const childRelative = path.posix.join(relative.split(path.sep).join(path.posix.sep), name);
    const childAbsolute = path.join(root, childRelative);
    const childStat = fs.lstatSync(childAbsolute);
    if (childStat.isSymbolicLink()) {
      throw new Error(`retained artifacts may not contain symlinks: ${childRelative}`);
    }
    if (childStat.isDirectory()) {
      files.push(...walkArtifactFiles(root, childRelative));
    } else if (childStat.isFile()) {
      files.push({
        path: childRelative,
        type: "file",
        ...stableFileEvidence(childAbsolute),
      });
    } else {
      throw new Error(`retained artifacts may not contain special entries: ${childRelative}`);
    }
  }
  return files;
}

export function artifactFileManifest(root, { exclude = ["result.json"] } = {}) {
  const excluded = new Set(exclude);
  return walkArtifactFiles(path.resolve(root))
    .filter((entry) => !excluded.has(entry.path));
}
/// Fails before a lane spends minutes in a browser when the operator already
/// pointed two lanes at the same retained success directory. Publishing stays
/// atomic; this only moves the same collision from the end of the run to its
/// start.
export function assertRetainedArtifactDirectoryAvailable(targetDirectory) {
  if (!targetDirectory) return;
  const target = path.resolve(targetDirectory);
  if (fs.existsSync(target)) {
    throw new Error(
      `retained artifact directory already exists: ${target} (each lane needs its own ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR)`,
    );
  }
}

export function createAtomicArtifactDirectory(targetDirectory) {
  const target = path.resolve(targetDirectory);
  if (fs.existsSync(target)) {
    throw new Error(`retained artifact directory already exists: ${target}`);
  }
  const parent = path.dirname(target);
  fs.mkdirSync(parent, { recursive: true });
  const temporary = fs.mkdtempSync(path.join(parent, `.${path.basename(target)}.tmp-`));
  let published = false;
  return {
    target,
    temporary,
    publish() {
      if (published) throw new Error(`retained artifact directory already published: ${target}`);
      fs.renameSync(temporary, target);
      published = true;
      return target;
    },
    cleanup() {
      if (!published && fs.existsSync(temporary)) {
        fs.rmSync(temporary, { recursive: true, force: true });
      }
    },
  };
}

function isPlainRecord(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function normalizedContainedPath(root, value, label) {
  if (typeof value !== "string" || value.length === 0 || value.includes("\0")) {
    throw new Error(`${label} must be a non-empty path string`);
  }
  if (path.isAbsolute(value)) return null;
  const absolute = path.resolve(root, value);
  if (absolute !== root && !absolute.startsWith(`${root}${path.sep}`)) {
    throw new Error(`${label} escapes retained result root: ${value}`);
  }
  return absolute;
}

function exactExternalPaths(document) {
  const entries = document.retainedExternalPaths ?? [];
  if (!Array.isArray(entries)) {
    throw new Error("retainedExternalPaths must be an array");
  }
  const external = new Map();
  for (const [index, entry] of entries.entries()) {
    if (!isPlainRecord(entry)) {
      throw new Error(`retainedExternalPaths[${index}] must be an object`);
    }
    const keys = Object.keys(entry).sort();
    if (JSON.stringify(keys) !== JSON.stringify(["bytes", "path", "sha256", "type"])) {
      throw new Error(`retainedExternalPaths[${index}] has an invalid schema`);
    }
    if (!path.isAbsolute(entry.path) || entry.type !== "file") {
      throw new Error(`retainedExternalPaths[${index}] must declare an absolute regular file`);
    }
    if (
      !Number.isInteger(entry.bytes) || entry.bytes < 0 ||
      !/^[0-9a-f]{64}$/.test(entry.sha256)
    ) {
      throw new Error(`retainedExternalPaths[${index}] has invalid size/hash evidence`);
    }
    const absolute = path.resolve(entry.path);
    if (external.has(absolute)) {
      throw new Error(`retainedExternalPaths[${index}] duplicates ${entry.path}`);
    }
    const evidence = stableFileEvidence(absolute);
    if (evidence.bytes !== entry.bytes || evidence.sha256 !== entry.sha256) {
      throw new Error(`retainedExternalPaths[${index}] size/hash evidence does not match`);
    }
    external.set(absolute, entry);
  }
  return external;
}

const RETAINED_DIRECTORY_FIELDS = new Set(["artifactDir"]);
const RETAINED_FILE_FIELDS = new Set([
  "serverLogPath",
  "staticLogPath",
  "clientUrlsPath",
  "serverEventsPath",
  "serverDamageEventsPath",
  "provenancePath",
]);
const RETAINED_FILE_ARRAY_FIELDS = new Set(["extraLogPaths", "clientStatePaths"]);

function pathFieldKind(key, parentLabel) {
  // Filesystem references have an explicit retained-result schema. Gameplay
  // and RTC evidence also uses words such as `displayPath`,
  // `authoritativePath`, and candidate `paths` for distances/topology; suffix
  // guessing would turn those numbers and histograms into false filesystem
  // claims. Top-level external and manifest entries are validated separately.
  if (RETAINED_DIRECTORY_FIELDS.has(key)) return "directory";
  if (RETAINED_FILE_FIELDS.has(key)) return "file";
  if (RETAINED_FILE_ARRAY_FIELDS.has(key)) return "file-array";
  if (key === "path" && /^result\.logs\.(?:stdout|stderr)$/.test(parentLabel)) {
    return "file";
  }
  return null;
}

export function validateArtifactPathReferences(document, resultPath) {
  if (!isPlainRecord(document)) throw new Error("retained result must be an object");
  const absoluteResult = path.resolve(resultPath);
  const root = path.dirname(absoluteResult);
  const external = exactExternalPaths(document);
  const manifests = new Map();
  const manifestArrays = [];

  function collectManifests(value, label, seen = new Set()) {
    if (!value || typeof value !== "object" || seen.has(value)) return;
    seen.add(value);
    if (Array.isArray(value.artifactFiles)) {
      manifestArrays.push([value.artifactFiles, `${label}.artifactFiles`]);
      for (const [index, entry] of value.artifactFiles.entries()) {
        if (!isPlainRecord(entry) || JSON.stringify(Object.keys(entry).sort()) !==
          JSON.stringify(["bytes", "path", "sha256", "type"])) {
          throw new Error(`${label}.artifactFiles[${index}] has an invalid schema`);
        }
        if (entry.type !== "file" || !Number.isInteger(entry.bytes) || entry.bytes < 0 ||
          !/^[0-9a-f]{64}$/.test(entry.sha256) || manifests.has(entry.path)) {
          throw new Error(`${label}.artifactFiles[${index}] is malformed or duplicated`);
        }
        manifests.set(entry.path, entry);
      }
    }
    // Validate the JSON artifact contract. Runtime-only non-enumerable helper
    // properties (for example an absolute directory used solely to format a
    // failure footer) are not embedded in result.json and therefore are not
    // retained path claims.
    for (const key of Object.keys(value)) {
      if (key === "provenance" || key === "artifactFiles" || key === "retainedExternalPaths") continue;
      const child = value[key];
      collectManifests(child, `${label}.${key}`, seen);
    }
  }
  collectManifests(document, "result");
  if (manifestArrays.length === 0) {
    throw new Error("retained result has no artifactFiles manifest");
  }

  const referencedFiles = new Set();
  const referencedExternal = new Set();
  function validateOne(value, kind, label) {
    const contained = normalizedContainedPath(root, value, label);
    const absolute = contained ?? path.resolve(value);
    if (contained == null && !external.has(absolute)) {
      throw new Error(`${label} is outside retained result root without an external declaration`);
    }
    let stat;
    try {
      stat = fs.lstatSync(absolute);
    } catch (error) {
      throw new Error(`${label} does not exist: ${value} (${error.code || error.message})`);
    }
    if (stat.isSymbolicLink()) throw new Error(`${label} may not be a symlink`);
    if (kind === "directory") {
      if (!stat.isDirectory()) throw new Error(`${label} is not a directory`);
      return;
    }
    if (!stat.isFile()) throw new Error(`${label} is not a regular file`);
    const evidence = stableFileEvidence(absolute);
    const relative = contained == null ? null : path.relative(root, absolute).split(path.sep).join("/");
    const expected = contained == null ? external.get(absolute) : manifests.get(relative);
    if (!expected) throw new Error(`${label} is not bound by an artifact manifest`);
    if (expected.type !== "file" || evidence.bytes !== expected.bytes || evidence.sha256 !== expected.sha256) {
      throw new Error(`${label} size/hash does not match its artifact manifest`);
    }
    if (relative != null) referencedFiles.add(relative);
    else referencedExternal.add(absolute);
  }

  function traverse(value, label, seen = new Set()) {
    if (!value || typeof value !== "object" || seen.has(value)) return;
    seen.add(value);
    for (const key of Object.keys(value)) {
      if (key === "provenance" || key === "retainedExternalPaths") continue;
      const child = value[key];
      const kind = pathFieldKind(key, label);
      const childLabel = `${label}.${key}`;
      if (kind === "file-array") {
        if (!Array.isArray(child)) throw new Error(`${childLabel} must be an array of paths`);
        child.forEach((entry, index) => validateOne(entry, "file", `${childLabel}[${index}]`));
      } else if (kind) {
        validateOne(child, kind, childLabel);
      }
      if (key !== "artifactFiles") traverse(child, childLabel, seen);
    }
  }
  traverse(document, "result");

  for (const manifestPath of manifests.keys()) {
    if (!referencedFiles.has(manifestPath)) {
      // Manifest-only files (screenshots and complete client states) are still
      // explicit retained references by virtue of their exact manifest path.
      validateOne(manifestPath, "file", `artifactFiles[${JSON.stringify(manifestPath)}]`);
    }
  }
  for (const externalPath of external.keys()) {
    if (!referencedExternal.has(externalPath)) {
      throw new Error(`retained external declaration is unreferenced: ${externalPath}`);
    }
  }
  const browserConsoleAudit = validateBrowserConsoleArtifacts(document, resultPath);
  return { ok: true, root, files: manifests.size, externalFiles: external.size, browserConsoleAudit };
}

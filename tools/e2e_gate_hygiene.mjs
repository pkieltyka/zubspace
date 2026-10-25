// Gate-environment hygiene for browser-based E2E lanes.
//
// Two failures cost hours when they are missing:
//   1. A lane that is interrupted (Ctrl-C, `pkill`, a crash) leaves its browsers,
//      their renderers and their Xvfb displays running. The next lane then measures
//      a machine with leaked GPU/CPU/X servers, which shows up as a gameplay
//      regression rather than as leftover processes.
//   2. A stale Xvfb or an already-answered DevTools port makes the next lane talk
//      to somebody else's browser or display.
//
// So: refuse to start on a dirty machine, and make every registered child die with
// the lane that started it, including on signals.

import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import os from "node:os";

const USER_DATA_DIR_FLAG = "--user-data-dir=";
const TEST_PROFILE_PREFIX = "/tmp/zubspace-";

function windowClass() {
  return process.env.ZUBSPACE_E2E_WINDOW_CLASS || "zubspace-e2e";
}

function optOut(env = process.env) {
  return env.ZUBSPACE_E2E_SKIP_PREFLIGHT === "1";
}

function readCmdline(procRoot, pid) {
  try {
    const raw = fs.readFileSync(path.join(procRoot, String(pid), "cmdline"), "utf8");
    return raw.split("\0").filter((entry) => entry.length > 0);
  } catch {
    return null;
  }
}

function readPpid(procRoot, pid) {
  try {
    const stat = fs.readFileSync(path.join(procRoot, String(pid), "stat"), "utf8");
    // `<pid> (<comm>) <state> <ppid> ...` - comm may contain spaces or parens.
    const tail = stat.slice(stat.lastIndexOf(")") + 1).trim().split(/\s+/);
    const ppid = Number.parseInt(tail[1] ?? "", 10);
    return Number.isInteger(ppid) ? ppid : null;
  } catch {
    return null;
  }
}

function readDisplay(procRoot, pid) {
  try {
    const environ = fs.readFileSync(path.join(procRoot, String(pid), "environ"), "utf8");
    for (const entry of environ.split("\0")) {
      if (entry.startsWith("DISPLAY=")) return entry.slice("DISPLAY=".length) || null;
    }
  } catch {
    // Not our process, or the kernel will not show it. Treating an unknown DISPLAY
    // as "not ours" is the conservative answer for a preflight about to refuse.
  }
  return null;
}

function ancestorPids(procRoot, selfPid) {
  const chain = new Set([selfPid]);
  let pid = selfPid;
  for (let depth = 0; depth < 64; depth += 1) {
    const ppid = readPpid(procRoot, pid);
    if (ppid == null || ppid <= 1 || chain.has(ppid)) break;
    chain.add(ppid);
    pid = ppid;
  }
  return chain;
}

// The processes this pid launched. A lane starts browsers on purpose, so its own
// descendants are never leftovers; only abandoned runs (re-parented orphans whose
// lane is gone) are.
function descendantPids(procRoot, selfPid) {
  const children = new Map();
  let entries;
  try {
    entries = fs.readdirSync(procRoot);
  } catch {
    return new Set([selfPid]);
  }
  for (const entry of entries) {
    if (!/^\d+$/.test(entry)) continue;
    const pid = Number.parseInt(entry, 10);
    const ppid = readPpid(procRoot, pid);
    if (ppid == null) continue;
    const siblings = children.get(ppid) ?? [];
    siblings.push(pid);
    children.set(ppid, siblings);
  }
  const reached = new Set([selfPid]);
  const queue = [selfPid];
  while (queue.length > 0) {
    for (const child of children.get(queue.shift()) ?? []) {
      if (reached.has(child)) continue;
      reached.add(child);
      queue.push(child);
    }
  }
  return reached;
}

function classifyProcess(argv) {
  const bin = path.basename(argv[0] ?? "");
  if (bin === "Xvfb") {
    const display = argv.find((arg) => /^:\d+/.test(arg)) ?? null;
    return { kind: "xvfb", display, profile: null };
  }
  if (bin === "xvfb-run") {
    return { kind: "xvfb-run", display: null, profile: null };
  }
  const userDataDir = argv
    .find((arg) => arg.startsWith(USER_DATA_DIR_FLAG))
    ?.slice(USER_DATA_DIR_FLAG.length);
  // Firefox takes its profile as a separate argument, not an `=` value.
  const profileIndex = argv.findIndex((arg) => arg === "-profile" || arg === "--profile");
  const firefoxProfile = profileIndex >= 0 ? argv[profileIndex + 1] ?? null : null;
  const profile = userDataDir ?? firefoxProfile;
  const e2eClass = argv.includes(`--class=${windowClass()}`);
  if (profile?.startsWith(TEST_PROFILE_PREFIX) || e2eClass) {
    return { kind: "browser", display: null, profile };
  }
  if (bin === "geckodriver") {
    return { kind: "driver", display: null, profile: null };
  }
  return null;
}

function scanAllProcesses({ procRoot = "/proc", selfPid = process.pid } = {}) {
  let entries;
  try {
    entries = fs.readdirSync(procRoot);
  } catch {
    return [];
  }
  const found = [];
  for (const entry of entries) {
    if (!/^\d+$/.test(entry)) continue;
    const pid = Number.parseInt(entry, 10);
    if (pid === selfPid) continue;
    const argv = readCmdline(procRoot, pid);
    if (argv == null || argv.length === 0) continue;
    const info = classifyProcess(argv);
    if (info == null) continue;
    const display =
      info.kind === "xvfb" ? info.display : readDisplay(procRoot, pid);
    found.push({ pid, ppid: readPpid(procRoot, pid), ...info, display });
  }
  return found;
}

// The roots that adopt orphans. In a systemd user session a killed lane's browser
// is re-parented to `systemd --user`, not to pid 1, so both count as abandoned.
const ADOPTERS = new Set(["init", "systemd", "launchd", "dumb-init", "tini", "docker-init"]);

// The `xvfb-run` wrapper, in either shape it appears in: its own `argv[0]`, or the script argument Nix
// installs it under (`sh /nix/store/…/bin/.xvfb-run`).
const XVFB_WRAPPER = /^\.?xvfb-run/;

function parentAdoptedProcess(procRoot, ppid) {
  if (ppid == null || ppid <= 1) return true;
  const argv = readCmdline(procRoot, ppid);
  if (argv == null || argv.length === 0) return true;
  return ADOPTERS.has(path.basename(argv[0] ?? "")) || argv.includes("--user");
}

// The abandoned leftovers of an interrupted lane. Three rules keep this from
// refusing a machine that is merely busy:
//   - only test markers match (`/tmp/zubspace-*` profile, the test window class),
//     so an operator's own Chrome/Firefox is never reported;
//   - our own process tree is never reported, in either direction (the `xvfb-run`
//     wrapper that started us, and the browsers we launched ourselves);
//   - a process with a live ordinary parent is somebody's *running* lane, so only
//     processes an init/systemd/launchd adopted count as leftovers.
// A concurrent lane is therefore never refused, and neither is a developer's
// running game server: gates bind their own free ports.
export function scanGateProcesses({
  procRoot = "/proc",
  selfPid = process.pid,
  display = process.env.DISPLAY ?? null,
} = {}) {
  const ours = new Set([...ancestorPids(procRoot, selfPid), ...descendantPids(procRoot, selfPid)]);
  let livePids;
  try {
    livePids = new Set(fs.readdirSync(procRoot).filter((entry) => /^\d+$/.test(entry)));
  } catch {
    livePids = new Set();
  }
  return scanAllProcesses({ procRoot, selfPid })
    .filter((record) => {
      if (ours.has(record.pid)) return false;
      if (record.kind === "xvfb" && display != null && record.display === display) return false;
      return parentAdoptedProcess(procRoot, record.ppid);
    })
    .sort((left, right) => left.kind.localeCompare(right.kind) || left.pid - right.pid);
}

export function listXvfbDisplays({ procRoot = "/proc", selfPid = process.pid } = {}) {
  const displays = new Set();
  for (const record of scanAllProcesses({ procRoot, selfPid })) {
    if (record.kind === "xvfb" && record.display != null) displays.add(record.display);
  }
  return [...displays];
}

/**
 * An `Xvfb` somebody started by hand: not ours, not an abandoned leftover (those are refused by
 * `scanGateProcesses`), and not under an `xvfb-run` wrapper - a wrapper means it belongs to a lane running
 * in parallel, which is a normal thing for two gates to be doing at once.
 *
 * This is deliberately a *report*, not a refusal. It is the operator's process, `xvfb-run -a` picks another
 * display regardless, and an earlier draft of a "refuse anything foreign" guard refused a developer's own
 * running game server, which is exactly the false positive that makes people disable a guard. It is surfaced
 * in the measurement note because a foreign X server can hold CPU and slow a wall-clock gate down, and a
 * failure message that hides that reads like a gameplay regression.
 */
export function foreignXDisplays({
  procRoot = "/proc",
  selfPid = process.pid,
  display = process.env.DISPLAY ?? null,
} = {}) {
  const ours = new Set([...ancestorPids(procRoot, selfPid), ...descendantPids(procRoot, selfPid)]);
  return scanAllProcesses({ procRoot, selfPid })
    .filter((record) => {
      if (record.kind !== "xvfb") return false;
      if (ours.has(record.pid)) return false;
      if (display != null && record.display === display) return false;
      if (parentAdoptedProcess(procRoot, record.ppid)) return false;
      return !hasXvfbRunAncestor(procRoot, record.pid);
    })
    .sort((left, right) => left.pid - right.pid);
}

// A wrapper shows up as itself or, the way Nix packages it, as `sh /nix/store/…/bin/.xvfb-run` - the script
// is named `.xvfb-run` and `argv[0]` is the interpreter. Matching only `argv[0] == "xvfb-run"` calls a
// parallel lane's display foreign, which is the false positive this rule exists to avoid.
function hasXvfbRunAncestor(procRoot, pid) {
  for (const ancestor of ancestorPids(procRoot, pid)) {
    const argv = readCmdline(procRoot, ancestor);
    if (argv == null) continue;
    if (argv.some((entry) => XVFB_WRAPPER.test(path.basename(entry)))) return true;
  }
  return false;
}

/** The foreign-display counterpart of `loadMeasurementNote`; "" when there is nothing to name. */
export function foreignXDisplayNote(options = {}) {
  const found = foreignXDisplays(options);
  if (found.length === 0) return "";
  const parts = found.map((record) => `${record.display ?? "(no display)"} pid ${record.pid}`);
  return `; foreign X display live during this run -> ${parts.join(", ")} (not refused: it is not ours)`;
}

// X socket locks left behind by a killed Xvfb make `xvfb-run -a` reuse a display
// that nothing is serving. The lock body is the owning pid, so trust that first;
// falling back to "no Xvfb mentions this display" would report our own live display
// as stale whenever the gate itself runs under xvfb-run.
export function scanStaleDisplayLocks({ procRoot = "/proc", tmpRoot = "/tmp" } = {}) {
  let entries;
  try {
    entries = fs.readdirSync(tmpRoot);
  } catch {
    return [];
  }
  let servingDisplays = null;
  const stale = [];
  for (const entry of entries) {
    const match = /^\.X(\w+)-lock$/.exec(entry);
    if (match == null) continue;
    const lockPath = path.join(tmpRoot, entry);
    let ownerPid = null;
    try {
      ownerPid = Number.parseInt(fs.readFileSync(lockPath, "utf8").trim(), 10);
    } catch {
      ownerPid = null;
    }
    if (Number.isInteger(ownerPid) && ownerPid > 1) {
      const argv = readCmdline(procRoot, ownerPid);
      if (argv != null && argv.length > 0) continue;
      stale.push({ display: `:${match[1]}`, path: lockPath, ownerPid });
      continue;
    }
    // No usable pid inside: fall back to whether anything serves the display.
    servingDisplays ??= listXvfbDisplays({ procRoot });
    const display = `:${Number.parseInt(match[1], 16)}`;
    if (!servingDisplays.includes(display) && !servingDisplays.includes(`:${match[1]}`)) {
      stale.push({ display, path: lockPath, ownerPid: null });
    }
  }
  return stale;
}

export function gateEnvironmentFindings(options = {}) {
  return {
    processes: scanGateProcesses(options),
    locks: scanStaleDisplayLocks(options),
  };
}

export function formatGateEnvironmentFindings(findings) {
  const lines = [];
  for (const record of findings.processes) {
    const where = record.display == null ? "" : ` display=${record.display}`;
    const profile = record.profile == null ? "" : ` profile=${record.profile}`;
    lines.push(`  pid ${record.pid}: ${record.kind}${where}${profile}`);
  }
  for (const lock of findings.locks) {
    lines.push(`  stale X lock: ${lock.path} (display ${lock.display})`);
  }
  return lines;
}

// Refuses a lane that would otherwise measure - or collide with - an abandoned run.
// `ZUBSPACE_E2E_SKIP_PREFLIGHT=1` is the escape hatch for an operator who knows why
// the machine looks dirty.
export function assertCleanGateEnvironment(stage = "gate start", options = {}) {
  if (optOut(options.env ?? process.env)) return { skipped: true, processes: [], locks: [] };
  const findings = gateEnvironmentFindings(options);
  if (findings.processes.length === 0 && findings.locks.length === 0) return findings;
  const lines = formatGateEnvironmentFindings(findings);
  throw new Error(
    [
      `${stage} refused: an interrupted browser gate left orphaned processes behind. A lane`,
      `that measures leaked browsers, X servers or stale X locks is not evidence about this change.`,
      ...lines,
      "",
      "Clear them (these markers never match your own browser or a running second lane):",
      "  pkill -f 'zubspace-combat-e2e'; pkill -f xvfb-run; pkill -f '^Xvfb'",
      "or re-run with ZUBSPACE_E2E_SKIP_PREFLIGHT=1 if you know why this is safe.",
    ].join("\n"),
  );
}

export function killProcessGroup(child, signal = "SIGKILL") {
  if (child == null || child.pid == null) return false;
  if (child.exitCode !== null || child.signalCode !== null) return false;
  try {
    // A detached child leads its own process group, so the negative pid takes the
    // browser and every renderer/GPU/zygote child with it.
    process.kill(-child.pid, signal);
    return true;
  } catch {
    try {
      child.kill(signal);
      return true;
    } catch {
      return false;
    }
  }
}

const registered = new Set();
const rememberedChildDisplays = new WeakMap();
const rememberedDisplayFiles = new Map();
let cleanupInstalled = false;
let cleaning = false;

// A process-group teardown can stop xvfb-run before its final file cleanup.
// Capture ownership while its Xvfb is still a descendant; never discover files
// to delete by scanning stale locks after the owner has disappeared.
export function captureOwnedXDisplays(child, { procRoot = "/proc", tmpRoot = "/tmp" } = {}) {
  return captureOwnedXDisplaysForChildren([child], { procRoot, tmpRoot });
}

function captureOwnedXDisplaysForChildren(children, { procRoot = "/proc", tmpRoot = "/tmp" } = {}) {
  const roots = new Set(children
    .filter(child => child != null && child.exitCode == null && child.signalCode == null)
    .map(child => child.pid)
    .filter(pid => Number.isSafeInteger(pid) && pid > 1));
  if (!roots.size) return [];
  const displays = [];
  for (const process of scanAllProcesses({ procRoot })) {
    if (process.kind !== "xvfb" ||
        !/^:\d+$/.test(process.display ?? "")) continue;
    if (![...ancestorPids(procRoot, process.pid)].some(pid => roots.has(pid))) continue;
    const number = process.display.slice(1);
    const lockPath = path.join(tmpRoot, `.X${number}-lock`);
    const socketPath = path.join(tmpRoot, ".X11-unix", `X${number}`);
    try {
      const lock = fs.lstatSync(lockPath), socket = fs.lstatSync(socketPath);
      if (!lock.isFile() || !socket.isSocket() ||
          lock.uid !== os.userInfo().uid || socket.uid !== lock.uid ||
          Number(fs.readFileSync(lockPath, "utf8").trim()) !== process.pid) continue;
      displays.push({ pid: process.pid, display: process.display, procRoot,
        lockPath, socketPath, lock: { dev: lock.dev, ino: lock.ino },
        socket: { dev: socket.dev, ino: socket.ino } });
    } catch { /* Startup/exit raced capture: leave unknown ownership alone. */ }
  }
  return displays;
}

function displayFileKey(display) {
  return `${display.lockPath}:${display.lock.dev}:${display.lock.ino}:${display.pid}`;
}

export function rememberOwnedXDisplays(child, options = {}) {
  if (child == null || typeof child !== "object") return [];
  const remembered = rememberedChildDisplays.get(child) ?? [];
  // Once the launcher exits, its numeric pid may be reused. Only ownership
  // captured before exit can survive that boundary.
  const current = child.exitCode == null && child.signalCode == null
    ? captureOwnedXDisplays(child, options) : [];
  const merged = [...new Map([...remembered, ...current]
    .map(display => [displayFileKey(display), display])).values()];
  rememberedChildDisplays.set(child, merged);
  for (const display of merged) rememberedDisplayFiles.set(displayFileKey(display), display);
  if (process.env.ZUBSPACE_E2E_DISPLAY_TRACE === "1") {
    console.error(JSON.stringify({ type: "e2e_owned_display_capture", at: new Date().toISOString(),
      launcherPid: child.pid, exitCode: child.exitCode, signalCode: child.signalCode,
      current: current.map(({ pid, display }) => ({ pid, display })),
      remembered: merged.map(({ pid, display }) => ({ pid, display })) }));
  }
  return merged;
}

function displayOwnerIsLive(display) {
  try {
    const stat = fs.readFileSync(path.join(display.procRoot, String(display.pid), "stat"), "utf8");
    const state = stat.slice(stat.lastIndexOf(")") + 1).trim().split(/\s+/)[0];
    return state !== "Z" && state !== "X";
  } catch (error) {
    // An unreadable owner is unknown, not proof that its display is dead.
    return error.code !== "ENOENT" && error.code !== "ESRCH";
  }
}

export function removeExitedOwnedXDisplayFiles(displays) {
  const removed = [];
  for (const display of displays) {
    if (displayOwnerIsLive(display)) continue;
    try {
      const lock = fs.lstatSync(display.lockPath);
      if (!lock.isFile() || lock.dev !== display.lock.dev || lock.ino !== display.lock.ino ||
          Number(fs.readFileSync(display.lockPath, "utf8").trim()) !== display.pid) continue;
      // A replacement socket belongs to another server even if its old lock
      // remains. Preserve both rather than breaking a newly claimed display.
      let socket = null;
      try {
        socket = fs.lstatSync(display.socketPath);
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
      if (socket != null) {
        if (!socket.isSocket() || socket.dev !== display.socket.dev || socket.ino !== display.socket.ino) continue;
        fs.unlinkSync(display.socketPath);
      }
      const currentLock = fs.lstatSync(display.lockPath);
      if (currentLock.dev !== display.lock.dev || currentLock.ino !== display.lock.ino ||
          Number(fs.readFileSync(display.lockPath, "utf8").trim()) !== display.pid) continue;
      fs.unlinkSync(display.lockPath);
      removed.push({ pid: display.pid, display: display.display });
    } catch { /* Already cleaned, replaced, or inaccessible: never broaden removal. */ }
  }
  return removed;
}

export async function finishOwnedXDisplayCleanup(displays) {
  const deadline = Date.now() + 1_000;
  // The launcher can exit before its X server finishes handling SIGTERM.
  while (displays.some(displayOwnerIsLive) && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  const removed = removeExitedOwnedXDisplayFiles(displays);
  if (removed.length) console.error(`e2e owned display-file cleanup: ${JSON.stringify(removed)}`);
  return removed;
}

function runGateCleanup() {
  if (cleaning) return;
  cleaning = true;
  const displays = [...new Map([
    ...rememberedDisplayFiles.values(),
    ...captureOwnedXDisplaysForChildren([...registered].map(entry => entry.child)),
  ].map(display => [displayFileKey(display), display])).values()];
  for (const entry of registered) killProcessGroup(entry.child);
  // Exit handlers must be synchronous. Bound the shared wait across all
  // displays, including large moving-peer runs; unknown/live owners survive.
  const deadline = Date.now() + 250;
  while (displays.some(displayOwnerIsLive) && Date.now() < deadline) {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
  }
  const removed = removeExitedOwnedXDisplayFiles(displays);
  if (removed.length) console.error(`e2e owned display-file cleanup: ${JSON.stringify(removed)}`);
  for (const entry of [...registered]) {
    if (entry.userDataDir != null) {
      try {
        fs.rmSync(entry.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
      } catch {
        // An undeletable profile directory is not worth losing the process kill over.
      }
    }
    registered.delete(entry);
  }
  cleaning = false;
}

function installHandlers() {
  if (cleanupInstalled) return;
  cleanupInstalled = true;
  // `exit` is the only hook that also fires after a thrown error, and it must be sync.
  process.on("exit", runGateCleanup);
  for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
    process.on(signal, () => {
      runGateCleanup();
      // Conventional status so a Makefile/wrapper still sees the signal's effect.
      process.exit(signal === "SIGINT" ? 130 : signal === "SIGHUP" ? 129 : 143);
    });
  }
}

// Call this for every child a lane launches. The first call also arms the
// signal/exit handlers, so a consumer that only registers children is covered.
export function registerGateProcess(child, { userDataDir = null } = {}) {
  installHandlers();
  if (child == null) return null;
  const entry = { child, userDataDir };
  registered.add(entry);
  child.once("exit", () => registered.delete(entry));
  return entry;
}

export function runGateCleanupForTests() {
  runGateCleanup();
}

export function registeredGateProcessCount() {
  return registered.size;
}

// A DevTools port that another Chrome already answers is the quiet cross-talk
// hazard: the new browser fails to bind, and `fetch(.../json/version)` then
// succeeds against somebody else's instance. Checked synchronously from
// /proc/net/tcp so a synchronous launcher can refuse before spawning.
export function portListeningSync(port, { procRoot = "/proc" } = {}) {
  const hex = port.toString(16).toUpperCase().padStart(4, "0");
  for (const file of ["net/tcp", "net/tcp6"]) {
    let table;
    try {
      table = fs.readFileSync(path.join(procRoot, file), "utf8");
    } catch {
      continue;
    }
    for (const line of table.split("\n").slice(1)) {
      const fields = line.trim().split(/\s+/);
      if (fields.length < 4) continue;
      if (fields[3] !== "0A") continue;
      if ((fields[1]?.split(":")[1] ?? "").toUpperCase() === hex) return true;
    }
  }
  return false;
}

export function assertDevToolsPortFree(port, options = {}) {
  if (optOut(options.env ?? process.env)) return;
  if (!portListeningSync(port, options)) return;
  throw new Error(
    `DevTools port ${port} is already answered by another listener; a lane that talks to it ` +
      `drives somebody else's browser. Pick a free port or clear the leaked test browsers ` +
      `(pkill -f 'zubspace-combat-e2e').`,
  );
}

// A timing gate measured on a saturated machine is not evidence about the product:
// the leaked-browser incident this module came from ran at load average 47 on a
// 32-thread box. Lanes call this once at entry - never per browser - because a lane
// is allowed to raise its own load (eight moving peers is the point of that gate).
export function loadHeadroom({ cores = os.cpus().length, factor = 1.5 } = {}) {
  return Math.max(2, Math.round(cores * factor));
}

export function readLoadAverage1m({ procRoot = "/proc" } = {}) {
  let raw;
  try {
    raw = fs.readFileSync(path.join(procRoot, "loadavg"), "utf8");
  } catch {
    return null;
  }
  const value = Number.parseFloat(raw.trim().split(/\s+/)[0]);
  return Number.isFinite(value) ? value : null;
}

/**
 * A one-line measurement-environment reading for a gate failure message. Wall-clock timing failures on a
 * busy machine are not evidence about the code under test, so a failure footer that omits the load reads as
 * a gameplay regression. A hand-started foreign `Xvfb` belongs in the same note: it can hold CPU, and the
 * gate must not refuse it. Returns "" when there is nothing to say, so a footer never implies a load of 0
 * or a display that is not there.
 */
export function loadMeasurementNote(options = {}) {
  const load = readLoadAverage1m(options);
  const notes = [];
  if (load != null) {
    notes.push(
      `machine load 1m=${load.toFixed(2)} (a saturated machine invalidates this measurement)`,
    );
  }
  const foreign = foreignXDisplayNote(options);
  if (foreign !== "") {
    notes.push(foreign.slice("; ".length));
  }
  if (notes.length === 0) return "";
  return `; ${notes.join("; ")}`;
}

export function assertMachineHeadroom(stage = "gate start", options = {}) {
  const env = options.env ?? process.env;
  if (optOut(env)) return;
  const load = readLoadAverage1m(options);
  if (load == null) return;
  const configured = Number.parseFloat(env.ZUBSPACE_E2E_MAX_LOAD ?? "");
  const limit = Number.isFinite(configured) ? configured : loadHeadroom(options);
  if (load <= limit) return;
  throw new Error(
    `${stage} refused: 1-minute load average ${load.toFixed(2)} exceeds the ${limit} headroom ` +
      `(${options.cores ?? os.cpus().length} cores). A timing gate measured on a saturated machine ` +
      `is not evidence about the change under test; wait for it to drain, or set ` +
      `ZUBSPACE_E2E_MAX_LOAD=<n> if you know what the machine is doing.`,
  );
}

// A browser binds AF_UNIX sockets whose paths it derives from the inherited temp root,
// and AF_UNIX paths are capped at 104 bytes. A lane launched under stacked `nix-shell`
// layers inherits a temp root of three ~35-byte components, and Chrome then dies inside
// its own singleton with `process_singleton_posix.cc Socket path too long` - which reads
// exactly like a browser startup bug and cost a complete keyboard lane
// (`/tmp/nix-shell-A/nix-shell-B/nix-shell-C/com.google.Chrome.…`). Browsers have no
// interest in the launcher's temp root, so hand them a short literal one and say so when
// the inherited one could not have worked.
export const BROWSER_TEMP_ROOT = "/tmp";
export const AF_UNIX_PATH_LIMIT_BYTES = 104;

function socketProbe(path) {
  return `${path.replace(/\/+$/, "")}/com.google.Chrome.abcdefgh`;
}

export function browserLaunchEnvironment(env = process.env, stage = "browser launch") {
  const inherited = env.TMPDIR || os.tmpdir();
  const probe = socketProbe(inherited);
  if (Buffer.byteLength(probe, "utf8") > AF_UNIX_PATH_LIMIT_BYTES) {
    console.log(
      `${stage}: inherited TMPDIR ${inherited} cannot hold an AF_UNIX socket ` +
        `(${Buffer.byteLength(probe, "utf8")} bytes > ${AF_UNIX_PATH_LIMIT_BYTES}); ` +
        `launching the browser with TMPDIR=${BROWSER_TEMP_ROOT}`,
    );
  }
  return {
    ...env,
    TMPDIR: BROWSER_TEMP_ROOT,
    TEMP: BROWSER_TEMP_ROOT,
    TMP: BROWSER_TEMP_ROOT,
  };
}

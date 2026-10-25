import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import net from "node:net";
import path from "node:path";
import test from "node:test";
import { spawn } from "node:child_process";

import {
  assertCleanGateEnvironment,
  assertDevToolsPortFree,
  assertMachineHeadroom,
  killProcessGroup,
  loadHeadroom,
  foreignXDisplayNote,
  foreignXDisplays,
  loadMeasurementNote,
  portListeningSync,
  readLoadAverage1m,
  registerGateProcess,
  registeredGateProcessCount,
  runGateCleanupForTests,
  scanGateProcesses,
  scanStaleDisplayLocks,
  browserLaunchEnvironment,
  captureOwnedXDisplays,
  rememberOwnedXDisplays,
  removeExitedOwnedXDisplayFiles,
} from "../../tools/e2e_gate_hygiene.mjs";
import { startStaticServer } from "../../tools/browser_firefox_rtc_smoke.mjs";

function fakeProc(root, records) {
  for (const record of records) {
    const dir = path.join(root, String(record.pid));
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, "cmdline"), `${record.argv.join("\0")}\0`);
    fs.writeFileSync(
      path.join(dir, "stat"),
      `${record.pid} (${record.comm ?? "proc"}) S ${record.ppid ?? 1} 1 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n`,
    );
    const environ = Object.entries(record.env ?? {})
      .map(([key, value]) => `${key}=${value}`)
      .join("\0");
    fs.writeFileSync(path.join(dir, "environ"), `${environ}\0`);
  }
  return root;
}

function makeTmp(options) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-gate-hygiene-"));
  options.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return root;
}

async function ownedDisplayFixture(t) {
  const root = makeTmp(t), procRoot = path.join(root, "proc"), tmpRoot = path.join(root, "tmp");
  const socketPath = path.join(tmpRoot, ".X11-unix", "X77");
  const lockPath = path.join(tmpRoot, ".X77-lock");
  fs.mkdirSync(path.dirname(socketPath), { recursive: true });
  fakeProc(procRoot, [
    { pid: 700, comm: "wrapper", argv: ["wrapper"], ppid: 1 },
    { pid: 701, comm: "Xvfb", argv: ["Xvfb", ":77"], ppid: 700 },
    { pid: 702, comm: "Xvfb", argv: ["Xvfb", ":78"], ppid: 799 },
  ]);
  fs.writeFileSync(lockPath, "701\n");
  // A killed Unix listener leaves the same real socket inode as a killed
  // X server, without launching an X display alongside an acceptance lane.
  const listener = spawn(process.execPath, ["-e",
    "require('node:net').createServer().listen(process.argv[1],()=>console.log('ready'))", socketPath],
  { stdio: ["ignore", "pipe", "pipe"] });
  const stop = async () => {
    if (listener.exitCode !== null || listener.signalCode !== null) return;
    const exited = new Promise(resolve => listener.once("exit", resolve));
    listener.kill("SIGKILL"); await exited;
  };
  t.after(stop);
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("socket fixture startup timeout")), 5_000);
    listener.stdout.once("data", () => { clearTimeout(timer); resolve(); });
    listener.once("error", error => { clearTimeout(timer); reject(error); });
    listener.once("exit", () => { clearTimeout(timer); reject(new Error("socket fixture exited before readiness")); });
  });
  return { procRoot, tmpRoot, socketPath, lockPath, stop,
    capture: () => captureOwnedXDisplays({ pid: 700 }, { procRoot, tmpRoot }),
    exitOwner: async () => { await stop(); fs.rmSync(path.join(procRoot, "701"), { recursive: true }); } };
}

test("owned display cleanup waits for its owner and removes only its captured stale files", async t => {
  const fixture = await ownedDisplayFixture(t), displays = fixture.capture();
  assert.equal(displays.length, 1);
  assert.deepEqual(removeExitedOwnedXDisplayFiles(displays), []);
  assert.ok(fs.existsSync(fixture.socketPath));
  await fixture.exitOwner();
  assert.deepEqual(removeExitedOwnedXDisplayFiles(displays), [{ pid: 701, display: ":77" }]);
  assert.equal(fs.existsSync(fixture.lockPath), false);
  assert.equal(fs.existsSync(fixture.socketPath), false);
  assert.deepEqual(removeExitedOwnedXDisplayFiles(displays), []);
});

test("a foreign display and an uncaptured stale lock are never cleanup candidates", async t => {
  const fixture = await ownedDisplayFixture(t);
  assert.deepEqual(captureOwnedXDisplays({ pid: 799 }, fixture), []);
  await fixture.exitOwner();
  assert.deepEqual(fixture.capture(), []);
  assert.deepEqual(removeExitedOwnedXDisplayFiles([]), []);
  assert.ok(fs.existsSync(fixture.lockPath));
  assert.ok(fs.existsSync(fixture.socketPath));
});

test("remembered display ownership survives launcher exit without rediscovering a reused pid", async t => {
  const fixture = await ownedDisplayFixture(t);
  const child = { pid: 700, exitCode: null, signalCode: null };
  const remembered = rememberOwnedXDisplays(child, fixture);
  assert.equal(remembered.length, 1);
  await fixture.exitOwner();
  child.exitCode = 0;
  fs.rmSync(path.join(fixture.procRoot, "700"), { recursive: true });
  fakeProc(fixture.procRoot, [
    { pid: 700, comm: "replacement", argv: ["replacement"], ppid: 1 },
    { pid: 703, comm: "Xvfb", argv: ["Xvfb", ":77"], ppid: 700 },
  ]);
  fs.writeFileSync(fixture.lockPath, "703\n");
  assert.deepEqual(rememberOwnedXDisplays(child, fixture), remembered);
  assert.deepEqual(removeExitedOwnedXDisplayFiles(remembered), [], "the new owner's files survive");
  assert.ok(fs.existsSync(fixture.socketPath));
});

test("display capture never rediscovers descendants through an exited launcher's reused pid", async t => {
  const fixture = await ownedDisplayFixture(t);
  assert.deepEqual(captureOwnedXDisplays({ pid: 700, exitCode: 0, signalCode: null }, fixture), []);
  assert.deepEqual(captureOwnedXDisplays({ pid: 700, exitCode: null, signalCode: "SIGTERM" }, fixture), []);
  assert.ok(fs.existsSync(fixture.lockPath));
  assert.ok(fs.existsSync(fixture.socketPath));
});

test("display cleanup preserves a live owner even when its command line is unavailable", async t => {
  const fixture = await ownedDisplayFixture(t), displays = fixture.capture();
  fs.unlinkSync(path.join(fixture.procRoot, "701", "cmdline"));
  assert.deepEqual(removeExitedOwnedXDisplayFiles(displays), []);
  fs.unlinkSync(path.join(fixture.procRoot, "701", "stat"));
  fs.mkdirSync(path.join(fixture.procRoot, "701", "stat"));
  assert.deepEqual(removeExitedOwnedXDisplayFiles(displays), [], "unknown owner state must preserve files");
  assert.ok(fs.existsSync(fixture.lockPath));
  assert.ok(fs.existsSync(fixture.socketPath));
});

for (const replacement of ["lock inode", "lock owner", "socket inode", "socket symlink"]) {
  test(`owned display cleanup preserves a replacement ${replacement}`, async t => {
    const fixture = await ownedDisplayFixture(t), displays = fixture.capture();
    await fixture.exitOwner();
    if (replacement === "lock inode") {
      fs.renameSync(fixture.lockPath, `${fixture.lockPath}.old`);
      fs.writeFileSync(fixture.lockPath, "701\n");
    } else if (replacement === "lock owner") {
      fs.writeFileSync(fixture.lockPath, "777\n");
    } else {
      fs.renameSync(fixture.socketPath, `${fixture.socketPath}.old`);
      if (replacement === "socket symlink") {
        fs.symlinkSync(`${fixture.socketPath}.missing`, fixture.socketPath);
      } else {
        fs.writeFileSync(fixture.socketPath, "replacement");
      }
    }
    assert.deepEqual(removeExitedOwnedXDisplayFiles(displays), []);
    assert.ok(fs.existsSync(fixture.lockPath));
    assert.ok(fs.lstatSync(fixture.socketPath));
  });
}

test("display capture refuses symlinked ownership files", async t => {
  const fixture = await ownedDisplayFixture(t);
  fs.renameSync(fixture.lockPath, `${fixture.lockPath}.target`);
  fs.symlinkSync(`${fixture.lockPath}.target`, fixture.lockPath);
  assert.deepEqual(fixture.capture(), []);
  assert.ok(fs.lstatSync(fixture.lockPath).isSymbolicLink());
});

const OPERATOR_CHROME = ["/opt/google/chrome/chrome", "--profile-directory=Default"];
const LEAKED_CHROME = [
  "/usr/bin/google-chrome",
  "--class=zubspace-e2e",
  "--user-data-dir=/tmp/zubspace-combat-e2e-alpha-9333-4242",
];

test("a leaked test browser is reported and an operator browser is not", (t) => {
  const proc = path.join(makeTmp(t), "proc");
  fakeProc(proc, [
    { pid: 100, comm: "node", argv: ["/usr/bin/node", "tests/e2e/combat_replay.mjs"] },
    { pid: 101, comm: "chrome", argv: OPERATOR_CHROME },
    { pid: 102, comm: "chrome", argv: LEAKED_CHROME, env: { DISPLAY: ":77" } },
    { pid: 103, comm: "Xvfb", argv: ["Xvfb", ":77", "-screen", "0", "1440x1000x24"] },
    { pid: 104, comm: "firefox", argv: ["/run/current-system/sw/bin/firefox", "-profile", "/tmp/zubspace-bot-authority-firefox-9401"] },
    { pid: 105, comm: "geckodriver", argv: ["/usr/local/bin/geckodriver", "--host", "127.0.0.1", "--port", "4445"] },
    { pid: 106, comm: "firefox", argv: ["/run/current-system/sw/bin/firefox", "--new-window", "https://example.com"] },
    { pid: 110, comm: "systemd", argv: ["/nix/store/systemd/lib/systemd/systemd", "--user"], ppid: 1 },
    // What an interrupted lane actually looks like: adopted by the user's systemd.
    { pid: 111, comm: "chrome", argv: LEAKED_CHROME, ppid: 110 },
    // A browser an operator started from a live shell on purpose: left alone.
    { pid: 112, comm: "chrome", argv: LEAKED_CHROME, ppid: 113 },
    { pid: 113, comm: "bash", argv: ["/bin/bash"], ppid: 1 },
  ]);

  const found = scanGateProcesses({ procRoot: proc, selfPid: 100, display: ":99" });
  const kinds = found.map((record) => `${record.kind}:${record.pid}`);
  assert.deepEqual(
    kinds,
    ["browser:102", "browser:104", "browser:111", "driver:105", "xvfb:103"],
    "only the abandoned run is reported, never the operator's browsers",
  );
  assert.equal(found.find((record) => record.pid === 104).profile, "/tmp/zubspace-bot-authority-firefox-9401");
  assert.equal(found.find((record) => record.pid === 102).profile, "/tmp/zubspace-combat-e2e-alpha-9333-4242");
  assert.equal(found.find((record) => record.pid === 103).display, ":77");
  assert.equal(
    found.find((record) => record.pid === 112),
    undefined,
    "a test-profile browser with a live ordinary parent is a running lane, not a leftover",
  );
});

test("the lane's own xvfb-run wrapper and display are never leftovers", (t) => {
  const proc = path.join(makeTmp(t), "proc");
  fakeProc(proc, [
    // self <- xvfb-run <- Xvfb, all on the shared display.
    { pid: 200, comm: "xvfb-run", argv: ["/nix/store/x/xvfb-run", "-a", "node", "gate.mjs"] },
    { pid: 201, comm: "Xvfb", argv: ["Xvfb", ":99", "-screen", "0", "1440x1000x24"], ppid: 200 },
    { pid: 202, comm: "node", argv: ["/usr/bin/node", "gate.mjs"], ppid: 201, env: { DISPLAY: ":99" } },
    // Somebody else's display, not on our chain.
    { pid: 203, comm: "Xvfb", argv: ["Xvfb", ":98", "-screen", "0", "1440x1000x24"] },
  ]);

  const found = scanGateProcesses({ procRoot: proc, selfPid: 202, display: ":99" });
  assert.deepEqual(found.map((record) => record.pid), [203]);
});

test("an X lock with no serving Xvfb is stale, a live one is not", (t) => {
  const root = makeTmp(t);
  const proc = path.join(root, "proc");
  const tmp = path.join(root, "tmp");
  fs.mkdirSync(tmp);
  fakeProc(proc, [
    { pid: 300, comm: "Xvfb", argv: ["Xvfb", ":9", "-screen", "0", "1440x1000x24"] },
    // The pid that owned :10 is gone (absent from the fake proc tree).
  ]);
  fs.writeFileSync(path.join(tmp, ".X9-lock"), `${String(300).padEnd(10, " ")}\n`);
  fs.writeFileSync(path.join(tmp, ".X10-lock"), `${String(987654).padEnd(10, " ")}\n`);
  fs.writeFileSync(path.join(tmp, ".X11-lock"), "garbage\n");

  const stale = scanStaleDisplayLocks({ procRoot: proc, tmpRoot: tmp });
  assert.deepEqual(
    stale.map((lock) => path.basename(lock.path)),
    [".X10-lock", ".X11-lock"],
    "a live owner is kept, a dead pid and an unserved display are reported",
  );
  assert.equal(stale[0].ownerPid, 987654);
});

test("the preflight refuses an abandoned browser but never the lane's own", (t) => {
  const root = makeTmp(t);
  const proc = path.join(root, "proc");
  const tmp = path.join(root, "tmp");
  fs.mkdirSync(tmp);
  fakeProc(proc, [
    { pid: 1, comm: "init", argv: ["/init"], ppid: 0 },
    { pid: 500, comm: "node", argv: ["/usr/bin/node", "tests/e2e/combat_replay.mjs"], ppid: 1 },
    // The lane's own alpha Chrome: launched on purpose, so a descendant. Its
    // display differs from ours to prove the descendant rule carries it alone.
    { pid: 600, comm: "chrome", argv: LEAKED_CHROME, ppid: 500, env: { DISPLAY: ":98" } },
    // A second lane running right now: its browser has a live parent of its own.
    { pid: 699, comm: "node", argv: ["/usr/bin/node", "tests/e2e/bomb_damage.mjs"], ppid: 300 },
    { pid: 700, comm: "chrome", argv: LEAKED_CHROME, ppid: 699, env: { DISPLAY: ":96" } },
  ]);
  const options = { procRoot: proc, tmpRoot: tmp, selfPid: 500, display: ":99", env: {} };
  assert.doesNotThrow(() => assertCleanGateEnvironment("gate start", options));

  // An interrupted lane re-parents its browser to init; that one is a leftover.
  fakeProc(proc, [
    { pid: 41, comm: "chrome", argv: LEAKED_CHROME, ppid: 1, env: { DISPLAY: ":77" } },
  ]);
  assert.throws(
    () => assertCleanGateEnvironment("alpha Chrome", options),
    /alpha Chrome refused:.*pid 41: browser display=:77/s,
  );
  assert.doesNotThrow(() =>
    assertCleanGateEnvironment("alpha Chrome", { ...options, env: { ZUBSPACE_E2E_SKIP_PREFLIGHT: "1" } }),
  );
});

test("cleanup kills a detached browser's whole process group", async (t) => {
  const marker = path.join(makeTmp(t), "grandchild");
  const child = spawn(
    "/bin/sh",
    ["-c", `sh -c 'sleep 120' & echo $! > ${marker}; wait`],
    { detached: true, stdio: "ignore" },
  );
  registerGateProcess(child, { userDataDir: null });
  t.after(() => killProcessGroup(child));
  assert.equal(registeredGateProcessCount(), 1);

  const deadline = Date.now() + 5_000;
  while (!fs.existsSync(marker) && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  assert.ok(fs.existsSync(marker), "the launcher wrote its child's pid");
  const grandchild = Number.parseInt(fs.readFileSync(marker, "utf8").trim(), 10);

  runGateCleanupForTests();
  await new Promise((resolve) => child.once("exit", resolve));

  assert.equal(child.signalCode, "SIGKILL", "the registered child died by group kill");
  assert.equal(registeredGateProcessCount(), 0);
  assert.ok(
    await waitForGone(grandchild, 3_000),
    "the negative-pid kill reached the launcher's own child",
  );
});

test("a DevTools port already answered by another listener is refused", async (t) => {
  const proc = makeTmp(t);
  fs.mkdirSync(path.join(proc, "net"));
  fs.writeFileSync(
    path.join(proc, "net", "tcp"),
    [
      "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode",
      "   0: 0100007F:24CB 00000000:0000 0A 00000000:00000000 00000000 00000000     0        0 1 1 0 0 0",
      "   1: 0100007F:1F90 0100007F:CCDD 01 00000000:00000000 00:00000000 00000000     0        0 2 2 0 0 0",
      "",
    ].join("\n"),
  );
  assert.equal(portListeningSync(9419, { procRoot: proc }), true, "st 0A is a listener");
  assert.equal(portListeningSync(8080, { procRoot: proc }), false, "an ESTABLISHED row is not");
  assert.throws(() => assertDevToolsPortFree(9419, { procRoot: proc, env: {} }), /already answered/);
  assert.doesNotThrow(() => assertDevToolsPortFree(8080, { procRoot: proc, env: {} }));

  // Against the live kernel table: a socket this test holds open reads as taken,
  // which is exactly the view a lane uses before it spawns a browser.
  const held = net.createServer();
  await new Promise((resolve, reject) => {
    held.once("error", reject);
    held.listen(0, "127.0.0.1", resolve);
  });
  t.after(() => held.close());
  assert.equal(portListeningSync(held.address().port), true);
});

test("a saturated machine refuses a timing gate", (t) => {
  const proc = makeTmp(t);
  fs.writeFileSync(path.join(proc, "loadavg"), "61.20 44.10 30.05 3/400 99999 1\n");
  assert.equal(readLoadAverage1m({ procRoot: proc }), 61.2);
  assert.equal(loadHeadroom({ cores: 32 }), 48);
  assert.throws(
    () => assertMachineHeadroom("combat gate", { procRoot: proc, env: {}, cores: 32 }),
    /combat gate refused: 1-minute load average 61.20 exceeds the 48 headroom/,
  );
  assert.doesNotThrow(() =>
    assertMachineHeadroom("combat gate", { procRoot: proc, env: { ZUBSPACE_E2E_MAX_LOAD: "70" }, cores: 32 }),
  );

  // A machine with ordinary background work still gets a lane.
  fs.writeFileSync(path.join(proc, "loadavg"), "12.00 9.00 6.00 1/1 1\n");
  assert.doesNotThrow(() => assertMachineHeadroom("combat gate", { procRoot: proc, env: {}, cores: 32 }));

  // No /proc/loadavg (a non-Linux harness) is not a reason to refuse.
  assert.doesNotThrow(() =>
    assertMachineHeadroom("combat gate", { procRoot: path.join(proc, "missing"), env: {}, cores: 32 }),
  );
});

test("a timing failure footer names the load that invalidated it", (t) => {
  const proc = makeTmp(t);
  fs.writeFileSync(path.join(proc, "loadavg"), "61.20 44.10 30.05 3/400 99999 1\n");
  const note = loadMeasurementNote({ procRoot: proc });
  assert.match(note, /load 1m=61\.20/);
  assert.match(note, /invalidates this measurement/);
  // Without /proc/loadavg the note is empty, so a footer never implies a load of 0.
  assert.equal(loadMeasurementNote({ procRoot: path.join(proc, "missing") }), "");
});

async function waitForGone(pid, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!fs.existsSync(`/proc/${pid}`)) return true;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  return !fs.existsSync(`/proc/${pid}`);
}


test("a hand-started Xvfb is named for the record and never refused", (t) => {
  const proc = path.join(makeTmp(t), "proc");
  fakeProc(proc, [
    // Our own lane: xvfb-run -> node (self) with the wrapper's Xvfb as a sibling.
    {
      pid: 199,
      comm: "xvfb-run",
      argv: ["/run/current-system/sw/bin/xvfb-run", "-a", "node", "tests/e2e/combat_replay.mjs"],
    },
    { pid: 200, comm: "node", argv: ["/usr/bin/node", "tests/e2e/combat_replay.mjs"], ppid: 199 },
    { pid: 201, comm: "Xvfb", argv: ["Xvfb", ":99", "-screen", "0", "1440x1000x24"], ppid: 199 },
    // A second lane running in parallel, also wrapped. Refusing this would refuse concurrency.
    { pid: 210, comm: "xvfb-run", argv: ["/run/current-system/sw/bin/xvfb-run", "-a", "node", "y.mjs"] },
    { pid: 211, comm: "Xvfb", argv: ["Xvfb", ":101"], ppid: 210 },
    // The shape Nix really produces: the wrapper is an interpreter plus a `.xvfb-run` script argument, so
    // a rule that matched only `argv[0] == "xvfb-run"` called a parallel lane's display foreign.
    {
      pid: 240,
      comm: "sh",
      argv: [
        "/nix/store/bash-5.3/bin/sh",
        "/nix/store/xyz-xvfb-run-1+g87f6705/bin/.xvfb-run",
        "-a",
        "node",
        "z.mjs",
      ],
    },
    { pid: 241, comm: "Xvfb", argv: ["Xvfb", ":102"], ppid: 240 },
    // The case this exists for: an operator's display from a live shell.
    { pid: 220, comm: "bash", argv: ["/bin/bash"] },
    { pid: 221, comm: "Xvfb", argv: ["Xvfb", ":77", "-screen", "0", "1280x1024x24"], ppid: 220 },
    // An interrupted lane's Xvfb: adopted, so the refusal path already owns it.
    { pid: 230, comm: "systemd", argv: ["/nix/store/systemd/lib/systemd/systemd", "--user"] },
    { pid: 231, comm: "Xvfb", argv: ["Xvfb", ":88"], ppid: 230 },
  ]);
  const options = { procRoot: proc, selfPid: 200, display: ":99" };

  assert.deepEqual(
    foreignXDisplays(options).map((record) => `${record.display}:${record.pid}`),
    [":77:221"],
    "only an Xvfb with a live non-wrapper parent is foreign; ours, a lane's wrapper, and an orphan are not",
  );
  assert.match(
    foreignXDisplayNote(options),
    /foreign X display live during this run -> :77 pid 221/,
  );
  assert.equal(
    foreignXDisplayNote({ ...options, display: ":77" }),
    "",
    "a display the caller claims is its own is not foreign",
  );

  // The load footer carries it too, so every existing gate failure message names it without a new call site.
  fs.writeFileSync(path.join(proc, "loadavg"), "12.00 12 12 1/1 1\n");
  const note = loadMeasurementNote(options);
  assert.match(note, /load 1m=12\.00/);
  assert.match(note, /:77 pid 221/);
});

test("a browser never inherits a temp root too long for its own socket", () => {
  // Chrome's singleton socket lives under the temp root it inherits, and AF_UNIX paths
  // stop at 104 bytes. Three stacked nix-shell layers produce exactly that failure, and
  // it surfaces as `Chrome exited before DevTools became available`.
  const nested = "/tmp/nix-shell-750531-4035700832/nix-shell-750646-1996683712/nix-shell-766365-3616348021";
  assert.ok(Buffer.byteLength(`${nested}/com.google.Chrome.abcdefgh`, "utf8") > 104);

  const launched = browserLaunchEnvironment(
    { TMPDIR: nested, DISPLAY: ":99", PATH: "/usr/bin" },
    "keyboard lane Chrome",
  );

  assert.equal(launched.TMPDIR, "/tmp");
  assert.ok(
    Buffer.byteLength(`${launched.TMPDIR}/com.google.Chrome.abcdefgh`, "utf8") <= 104,
    "the browser's socket path must fit the AF_UNIX limit",
  );
  // The override is scoped to the temp root: the lane's own environment still reaches
  // the browser, because a headed lane that loses DISPLAY dies on X11 instead.
  assert.equal(launched.DISPLAY, ":99");
  assert.equal(launched.PATH, "/usr/bin");

  const shallow = browserLaunchEnvironment({ TMPDIR: "/tmp" }, "smoke Chrome");
  assert.equal(shallow.TMPDIR, "/tmp", "a short temp root is pinned, not passed through");
});

// The build-fidelity comparison runs the same lane against two different client builds, so
// which bundle the browsers were handed has to be switchable and recorded, and the default must
// stay the repository root that `index.html` needs. Harness fixtures under `/tests/**` resolve
// against the repository either way: `dist/` is an application bundle and deliberately does not
// contain the lane's own `capacity_host.html`.
test("the static root switch serves the bundle, keeps fixtures, and reports both", async () => {
  const bundle = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-bundle-"));
  fs.writeFileSync(path.join(bundle, "index.html"), "<html>release bundle</html>");
  try {
    process.env.ZUBSPACE_E2E_STATIC_ROOT = bundle;
    const served = await startStaticServer();
    const get = async (pathname) => {
      const response = await fetch(`http://127.0.0.1:${served.port}${pathname}`);
      return { status: response.status, body: await response.text() };
    };
    try {
      assert.equal(served.appRoot, bundle);
      assert.equal((await get("/index.html")).body, "<html>release bundle</html>",
        "the browsers must load the selected bundle, not the repo-root debug client");
      const status = JSON.parse((await get("/v1/status")).body);
      assert.equal(status.root, bundle);
      assert.notEqual(status.fixtureRoot, bundle, "the repository stays a fixture root");
      const fixture = await get("/tests/e2e/capacity_host.html");
      assert.equal(fixture.status, 200, "a harness fixture must still resolve under an override");
      assert.ok(fixture.body.length > 0);
      assert.equal((await get("/zubspace.wasm")).status, 404,
        "a missing bundle asset is a 404, never a silent fallback to the repository");
    } finally {
      served.server.close();
    }
  } finally {
    delete process.env.ZUBSPACE_E2E_STATIC_ROOT;
    fs.rmSync(bundle, { recursive: true, force: true });
  }

  const defaulted = await startStaticServer();
  try {
    assert.equal(defaulted.appRoot, path.resolve(import.meta.dirname, "..", ".."));
  } finally {
    defaulted.server.close();
  }

  process.env.ZUBSPACE_E2E_STATIC_ROOT = path.join(os.tmpdir(), "zubspace-no-such-bundle");
  try {
    // The root is validated before the socket opens, so a typoed bundle fails at the call
    // rather than as a lane that serves 404s for every app asset.
    assert.throws(() => startStaticServer(), /has no index.html/);
  } finally {
    delete process.env.ZUBSPACE_E2E_STATIC_ROOT;
  }
});

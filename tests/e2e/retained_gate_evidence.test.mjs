import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  logAudit,
  parseArguments,
  parseJsonDocuments,
  runRetainedGate,
  validateArtifactPathReferences,
  validateRetainedGateDocuments,
} from "./retained_gate_evidence.mjs";
import { rtcPressureSummary } from "../../tools/rtc_gate_audit.mjs";
import { artifactFileManifest } from "./artifact_paths.mjs";

function cleanStatus() {
  return {
    rtc_inbound_packets: { game: 1, event: 2, reliable: 3 },
    rtc_ingress_execution: {limit: 4, workers: 4, in_flight: 0, waiting: 0},
    rtc_inbound_queue_packets: { game: 0, event: 0, reliable: 0 },
    rtc_inbound_peak_queue_packets: { game: 1, event: 1, reliable: 1 },
    rtc_inbound_backpressure: { game: 0, event: 0, reliable: 0 },
    position_input_queue_capacity_rejections: 0,
    realtime_event_queue_capacity_per_client: 3072,
    realtime_event_queue_current: 0,
    realtime_event_queue_peak: 7,
    realtime_event_queue_overflows: 0,
    realtime_event_actions_rejected: 0,
    realtime_event_actions_deferred: 0,
    projectile_pending_hit_capacity_total: 3072,
    projectile_pending_hit_capacity_per_owner: 128,
    projectile_pending_hit_retry_budget_per_tick: 256,
    projectile_pending_hit_current: 0,
    projectile_pending_hit_retained_bytes: 0,
    projectile_pending_damage_transactions: 0,
    projectile_pending_damage_payload_bytes: 0,
    rtc_outbound_reliable_overflows: 0,
    rtc_outbound_queue_packets: 0,
    rtc_outbound_queue_bytes: 0,
  };
}

function cleanPressure() {
  return rtcPressureSummary(cleanStatus());
}

function cleanServerLogAudit() {
  return {
    driverFull: { game: 0, event: 0, reliable: 0, unknown: 0, total: 0 },
    structuredSecurityRejectCount: 0,
    structuredSecurityRejectLines: [],
    applicationQueueFailureCount: 0,
    applicationQueueFailureLines: [],
  };
}

test("retained capacity and Compose reject handed-off work at cleanup, not during traffic", () => {
  for (const [gate, fixture] of [["webrtc-capacity-200", capacityDocument], ["compose-rtc", composeDocument]]) {
    for (const field of ["in_flight", "waiting"]) {
      const document = fixture();
      const status = cleanStatus();
      status.rtc_ingress_execution[field] = 1;
      document.pressureAudit.during = rtcPressureSummary(status);
      assert.equal(validateRetainedGateDocuments(gate, [document]).ok, true);
      document.pressureAudit.after = rtcPressureSummary(status);
      assert.equal(validateRetainedGateDocuments(gate, [document]).ok, false);
    }
  }
});

function mixedDocument(pressure = cleanPressure()) {
  return {
    ok: true,
    transport: "rtc",
    browserPair: "chrome-firefox",
    pressureAudit: pressure,
    serverLogAudit: cleanServerLogAudit(),
  };
}

function capacityDocument() {
  return {
    ok: true,
    transport: {
      connected: 200,
      snapshots: { peersReceiving: 200, idGaps: 0 },
    },
    statusDuring: { rtc_sessions: 200 },
    statusAfter: { rtc_sessions: 0 },
    pressureAudit: {
      before: cleanPressure(),
      during: cleanPressure(),
      after: cleanPressure(),
    },
    serverLogAudit: cleanServerLogAudit(),
  };
}

function composeDocument() {
  return {
    ok: true,
    channels: ["zubspace-game", "zubspace-events", "zubspace-reliable"],
    snapshot: { latest: 7, acked: 7 },
    status: {
      during: { rtcSessions: 1 },
      after: { rtcSessions: 0, clients: 0, udpPortsInUse: 0 },
    },
    pressureAudit: {
      before: cleanPressure(),
      during: cleanPressure(),
      after: cleanPressure(),
    },
    serverLogAudit: cleanServerLogAudit(),
  };
}

function channelContractDocuments() {
  const names = [
    "fully reliable game",
    "partially reliable event",
    "partially reliable reliable",
  ];
  return [
    {
      ok: true,
      transport: "rtc",
      validPressureAudit: cleanPressure(),
      mismatchResults: names.map((name, index) => ({
        name,
        status: {
          rtc_channel_protocol_violations: index + 1,
          rtc_sessions: 0,
          clients: 0,
        },
        pressureAudit: cleanPressure(),
      })),
      serverLogAudit: cleanServerLogAudit(),
    },
    {
      ok: true,
      transport: "rtc",
      browser: "firefox",
      pressureAudit: cleanPressure(),
      serverLogAudit: cleanServerLogAudit(),
    },
  ];
}

test("retained gate arguments bind command inputs, binaries, and Docker images", () => {
  assert.deepEqual(
    parseArguments([
      "--gate",
      "compose",
      "--out",
      "/tmp/compose-evidence",
      "--server-binary",
      "target/release/zubspace_server",
      "--input",
      "tools/compose_rtc_smoke.mjs",
      "--docker-image",
      "zubspace/server:dev",
      "--",
      "make",
      "test-compose-rtc",
    ]),
    {
      gate: "compose",
      outputDirectory: "/tmp/compose-evidence",
      serverBinary: "target/release/zubspace_server",
      wasm: "target/wasm32-unknown-unknown/debug/zubspace.wasm",
      clientEntry: "index.html",
      additionalFiles: ["Makefile", "tools/compose_rtc_smoke.mjs"],
      dockerImages: ["zubspace/server:dev"],
      command: ["make", "test-compose-rtc"],
    },
  );
});

test("retained gate extracts every complete JSON result despite braces in logs", () => {
  const documents = parseJsonDocuments([
    "progress {not json}",
    JSON.stringify({ ok: true, nested: { count: 3 }, text: "escaped } brace" }, null, 2),
    "another log line",
    JSON.stringify({ ok: true, browser: "firefox" }),
  ].join("\n"));
  assert.deepEqual(documents, [
    { ok: true, nested: { count: 3 }, text: "escaped } brace" },
    { ok: true, browser: "firefox" },
  ]);
});

test("retained gate log audit counts actual driver Full, rejects, and queue failures narrowly", () => {
  const audit = logAudit(
    [
      '{"rtc_outbound_reliable_overflows":0,"fullReceived":3}',
      "Failed to send DataChannelMessage to data channel 5: Full",
      "security rejection player_id=7 reason=forged",
    ].join("\n"),
    "server RTC application ingress queue overflowed; disconnecting peer",
  );
  assert.equal(audit.driverDataChannelFullCount, 1);
  assert.equal(audit.structuredSecurityRejectCount, 1);
  assert.equal(audit.applicationQueueFailureCount, 1);
  assert.equal(audit.driverDataChannelFullLines.length, 1);
});

test("retained gate parsed-result validation fails closed on missing and nonzero evidence", () => {
  assert.deepEqual(validateRetainedGateDocuments("mixed-smoke", [mixedDocument()]), {
    ok: true,
    errors: [],
  });
  assert.equal(validateRetainedGateDocuments("mixed-smoke", []).ok, false);
  assert.equal(validateRetainedGateDocuments("mixed-smoke", [{ ok: true }]).ok, false);
  for (const mutate of [
    (pressure) => { pressure.applicationBackpressure.game = 1; },
    (pressure) => { pressure.applicationQueuePackets.event = 1; },
    (pressure) => { pressure.outboundReliableOverflows = 1; },
    (pressure) => { pressure.outboundQueuePackets = 1; },
    (pressure) => { pressure.outboundQueueBytes = 12; },
  ]) {
    const pressure = cleanPressure();
    mutate(pressure);
    const validation = validateRetainedGateDocuments("mixed-smoke", [mixedDocument(pressure)]);
    assert.equal(validation.ok, false, JSON.stringify(pressure));
    assert.ok(validation.errors.length > 0);
  }
});

test("retained gate requires exact raw pressure source evidence", () => {
  const invalidPressures = [];
  for (const mutate of [
    (pressure) => { delete pressure.sourceStatus; },
    (pressure) => { delete pressure.sourceStatus.rtc_inbound_packets; },
    (pressure) => { delete pressure.sourceStatus.rtc_inbound_packets.reliable; },
    (pressure) => { pressure.sourceStatus.rtc_inbound_packets.unknown = 0; },
    (pressure) => { pressure.sourceStatus.rtc_inbound_packets.game = null; },
    (pressure) => { pressure.sourceStatus.rtc_inbound_packets.game = "1"; },
    (pressure) => { pressure.sourceStatus.rtc_inbound_packets.game = Number.NaN; },
    (pressure) => { pressure.sourceStatus.rtc_inbound_packets.game = Number.POSITIVE_INFINITY; },
    (pressure) => { pressure.sourceStatus.rtc_inbound_packets.game = -1; },
    (pressure) => { pressure.sourceStatus.rtc_inbound_packets.game = 0.5; },
    (pressure) => { delete pressure.sourceStatus.rtc_outbound_queue_bytes; },
    (pressure) => { pressure.sourceStatus.rtc_outbound_queue_bytes = null; },
    (pressure) => { pressure.sourceStatus.rtc_outbound_queue_bytes = "0"; },
    (pressure) => { pressure.sourceStatus.rtc_outbound_queue_bytes = -1; },
    (pressure) => { pressure.inboundPackets.game += 1; },
    (pressure) => { delete pressure.applicationPeakQueuePackets.event; },
    (pressure) => { pressure.applicationQueuePackets.extra = 0; },
  ]) {
    const pressure = cleanPressure();
    mutate(pressure);
    invalidPressures.push(pressure);
  }
  for (const pressure of invalidPressures) {
    const validation = validateRetainedGateDocuments("mixed-smoke", [mixedDocument(pressure)]);
    assert.equal(validation.ok, false, JSON.stringify(pressure));
    assert.ok(validation.errors.length > 0);
  }
});

test("retained gates require exact scenario pressure phases and cardinalities", () => {
  assert.equal(
    validateRetainedGateDocuments("webrtc-capacity-200", [capacityDocument()]).ok,
    true,
  );
  assert.equal(
    validateRetainedGateDocuments("compose-rtc", [composeDocument()]).ok,
    true,
  );
  assert.equal(
    validateRetainedGateDocuments("webrtc-channel-contract", channelContractDocuments()).ok,
    true,
  );

  for (const [gate, documents, mutate] of [
    ["webrtc-capacity-200", [capacityDocument()], (docs) => {
      delete docs[0].pressureAudit.during;
    }],
    ["webrtc-capacity-200", [capacityDocument()], (docs) => {
      delete docs[0].pressureAudit.after;
      docs[0].decoy = { pressureAudit: cleanPressure() };
    }],
    ["compose-rtc", [composeDocument()], (docs) => {
      docs[0].pressureAudit.unexpected = cleanPressure();
    }],
    ["compose-rtc", [composeDocument()], (docs) => {
      docs[0].duplicatePressure = cleanPressure();
    }],
    ["webrtc-channel-contract", channelContractDocuments(), (docs) => {
      docs[0].mismatchResults.splice(1, 1);
    }],
    ["webrtc-channel-contract", channelContractDocuments(), (docs) => {
      delete docs[0].mismatchResults[1].pressureAudit;
      docs[0].decoy = { pressureAudit: cleanPressure() };
    }],
    ["webrtc-channel-contract", channelContractDocuments(), (docs) => {
      docs.pop();
    }],
    ["webrtc-channel-contract", channelContractDocuments(), (docs) => {
      docs[1].unexpected = { serverLogAudit: cleanServerLogAudit() };
    }],
  ]) {
    const candidate = structuredClone(documents);
    mutate(candidate);
    const validation = validateRetainedGateDocuments(gate, candidate);
    assert.equal(validation.ok, false, `${gate}: ${JSON.stringify(validation.errors)}`);
  }
});

test("retained Compose accepts only a wrap-safe ACK inside retained snapshot history", () => {
  for (const [latest, acked] of [[8, 7], [3, 0xfffe], [33, 1]]) {
    const document = composeDocument();
    document.snapshot = { latest, acked };
    assert.equal(
      validateRetainedGateDocuments("compose-rtc", [document]).ok,
      true,
      `${latest}/${acked}`,
    );
  }

  for (const [latest, acked] of [[34, 1], [7, 8], [0xfffe, 3]]) {
    const document = composeDocument();
    document.snapshot = { latest, acked };
    assert.equal(
      validateRetainedGateDocuments("compose-rtc", [document]).ok,
      false,
      `${latest}/${acked}`,
    );
  }
});

test("retained gates reject hidden, duplicate, malformed, and prototype-ish server-log audits", () => {
  for (const [label, mutate] of [
    ["missing canonical path with beside decoy", (document) => {
      delete document.serverLogAudit;
      document.decoy = { serverLogAudit: cleanServerLogAudit() };
    }],
    ["decoy beside canonical audit", (document) => {
      document.decoy = { serverLogAudit: cleanServerLogAudit() };
    }],
    ["decoy nested inside recognized audit", (document) => {
      document.serverLogAudit.nestedDecoy = { serverLogAudit: cleanServerLogAudit() };
    }],
    ["duplicate audits in an array", (document) => {
      document.auditCopies = [cleanServerLogAudit(), cleanServerLogAudit()];
    }],
    ["duplicate alias at an unexpected path", (document) => {
      document.auditAlias = document.serverLogAudit;
    }],
    ["unexpected canonical key", (document) => {
      document.serverLogAudit.unexpected = 0;
    }],
    ["unexpected driver key", (document) => {
      document.serverLogAudit.driverFull.decoy = 0;
    }],
    ["malformed line samples", (document) => {
      document.serverLogAudit.structuredSecurityRejectLines = [null];
    }],
    ["divergent driver total", (document) => {
      document.serverLogAudit.driverFull.total = 1;
    }],
    ["prototype-ish unexpected path", (document) => {
      Object.defineProperty(document, "__proto__", {
        enumerable: true,
        configurable: true,
        value: { serverLogAudit: cleanServerLogAudit() },
      });
    }],
    ["non-plain canonical audit", (document) => {
      document.serverLogAudit = Object.assign(
        Object.create({ hidden: { serverLogAudit: cleanServerLogAudit() } }),
        cleanServerLogAudit(),
      );
    }],
    ["non-enumerable adjacent named audit", (document) => {
      Object.defineProperty(document.decoy = {}, "serverLogAudit", {
        enumerable: false,
        configurable: true,
        value: {},
      });
    }],
    ...[
      {},
      [],
      { unknownOnly: 0 },
      null,
      "not-an-audit",
      7,
    ].map((malformed, index) => [
      `malformed adjacent named audit ${index}`,
      (document) => {
        document.decoy = { serverLogAudit: malformed };
      },
    ]),
  ]) {
    const document = mixedDocument();
    mutate(document);
    const validation = validateRetainedGateDocuments("mixed-smoke", [document]);
    assert.equal(validation.ok, false, `${label}: ${JSON.stringify(validation.errors)}`);
    assert.ok(validation.errors.length > 0, label);
  }

  assert.deepEqual(validateRetainedGateDocuments("mixed-smoke", [mixedDocument()]), {
    ok: true,
    errors: [],
  });
  assert.deepEqual(
    validateRetainedGateDocuments("webrtc-channel-contract", channelContractDocuments()),
    { ok: true, errors: [] },
  );
  assert.deepEqual(
    validateRetainedGateDocuments("webrtc-capacity-200", [capacityDocument()]),
    { ok: true, errors: [] },
  );
  assert.deepEqual(validateRetainedGateDocuments("compose-rtc", [composeDocument()]), {
    ok: true,
    errors: [],
  });
});

test("exit-zero retained commands fail on Full, security, overflow, pressure, or missing JSON", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-retained-gate-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  for (const file of ["server", "client.wasm", "index.html"]) {
    fs.writeFileSync(path.join(root, file), file);
  }
  const base = {
    gate: "mixed-smoke",
    serverBinary: path.join(root, "server"),
    wasm: path.join(root, "client.wasm"),
    clientEntry: path.join(root, "index.html"),
    additionalFiles: [],
    dockerImages: [],
  };
  let attempt = 0;
  async function assertRetainedFailure({ document = mixedDocument(), stderr = "", stdout = null }) {
    attempt += 1;
    const script = [
      stdout == null
        ? `process.stdout.write(${JSON.stringify(`${JSON.stringify(document)}\n`)})`
        : `process.stdout.write(${JSON.stringify(stdout)})`,
      `process.stderr.write(${JSON.stringify(stderr)})`,
    ].join(";");
    await assert.rejects(
      runRetainedGate({
        ...base,
        outputDirectory: path.join(root, `attempt-${attempt}`),
        command: [process.execPath, "-e", script],
      }),
      /retained gate mixed-smoke failed/,
    );
    const result = JSON.parse(fs.readFileSync(
      path.join(root, `attempt-${attempt}`, "result.json"),
      "utf8",
    ));
    assert.equal(result.ok, false);
  }

  await assertRetainedFailure({
    stderr: "Failed to send DataChannelMessage to data channel 5: Full\n",
  });
  await assertRetainedFailure({ stderr: "server security reject category=position\n" });
  await assertRetainedFailure({
    stderr: "RTC application ingress queue overflowed; disconnecting peer\n",
  });
  const pressured = cleanPressure();
  pressured.applicationBackpressure.reliable = 1;
  await assertRetainedFailure({ document: mixedDocument(pressured) });
  await assertRetainedFailure({ stdout: "successful command with no JSON result\n" });
  await assertRetainedFailure({ stdout: "{malformed JSON result}\n" });
});

test("retained artifact paths are root-relative, existent, typed, and hash-bound", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-artifact-paths-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.writeFileSync(path.join(root, "server.log"), "server evidence\n");
  fs.mkdirSync(path.join(root, "states"));
  fs.writeFileSync(path.join(root, "states", "alpha.json"), "{}\n");
  const valid = {
    ok: true,
    successArtifacts: {
      artifactDir: ".",
      serverLogPath: "server.log",
      clientStatePaths: ["states/alpha.json"],
      artifactFiles: artifactFileManifest(root),
      provenance: { additionalFiles: [{ path: "/repo/source.rs" }] },
    },
    transport: {
      expectedCandidatePaths: ["host->prflx"],
      selectedCandidatePairs: { paths: { "host->prflx": 200 } },
    },
    movement: {
      displayPath: 1939.04,
      authoritativePath: 1945.54,
      attackerPath: [{ x: 1, y: 2 }],
    },
  };
  const resultPath = path.join(root, "result.json");
  Object.defineProperty(valid.successArtifacts, "absoluteArtifactDir", {
    enumerable: false,
    value: "/tmp/runtime-only-artifact-footer",
  });
  fs.writeFileSync(resultPath, `${JSON.stringify(valid)}\n`);
  assert.equal(validateArtifactPathReferences(valid, resultPath).ok, true);

  for (const [label, mutate] of [
    ["missing", (document) => { document.successArtifacts.serverLogPath = "missing.log"; }],
    ["wrong type", (document) => { document.successArtifacts.serverLogPath = "states"; }],
    ["outside absolute", (document) => {
      document.successArtifacts.serverLogPath = "/tmp/round33-stale-final2/server.log";
    }],
    ["path traversal", (document) => { document.successArtifacts.serverLogPath = "../server.log"; }],
    ["hash mismatch", (document) => {
      document.successArtifacts.artifactFiles.find((entry) => entry.path === "server.log").sha256 = "0".repeat(64);
    }],
    ["unmanifested file", (document) => {
      document.successArtifacts.artifactFiles = document.successArtifacts.artifactFiles
        .filter((entry) => entry.path !== "server.log");
    }],
  ]) {
    const candidate = structuredClone(valid);
    mutate(candidate);
    assert.throws(
      () => validateArtifactPathReferences(candidate, resultPath),
      undefined,
      label,
    );
  }
});

test("retained external declarations are exact, unique, and referenced", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-artifact-external-root-"));
  const outside = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-artifact-external-file-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  t.after(() => fs.rmSync(outside, { recursive: true, force: true }));
  fs.writeFileSync(path.join(root, "local.log"), "local\n");
  const externalPath = path.join(outside, "server.log");
  fs.writeFileSync(externalPath, "external\n");
  const externalBytes = fs.statSync(externalPath).size;
  const externalSha = crypto.createHash("sha256").update(fs.readFileSync(externalPath)).digest("hex");
  const valid = {
    successArtifacts: {
      artifactDir: ".",
      serverLogPath: externalPath,
      artifactFiles: artifactFileManifest(root),
    },
    retainedExternalPaths: [{
      path: externalPath,
      type: "file",
      bytes: externalBytes,
      sha256: externalSha,
    }],
  };
  const resultPath = path.join(root, "result.json");
  assert.equal(validateArtifactPathReferences(valid, resultPath).externalFiles, 1);

  for (const [label, mutate] of [
    ["duplicate", (document) => document.retainedExternalPaths.push({
      ...document.retainedExternalPaths[0],
    })],
    ["negative size", (document) => { document.retainedExternalPaths[0].bytes = -1; }],
    ["invalid hash", (document) => { document.retainedExternalPaths[0].sha256 = "nope"; }],
    ["invalid type", (document) => { document.retainedExternalPaths[0].type = "directory"; }],
    ["unreferenced", (document) => {
      document.successArtifacts.serverLogPath = "local.log";
    }],
  ]) {
    const candidate = structuredClone(valid);
    mutate(candidate);
    assert.throws(() => validateArtifactPathReferences(candidate, resultPath), undefined, label);
  }
});

test("successful retained gates atomically publish self-contained relative manifests", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-retained-success-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  for (const file of ["server", "client.wasm", "index.html"]) {
    fs.writeFileSync(path.join(root, file), file);
  }
  const outputDirectory = path.join(root, "retained");
  const result = await runRetainedGate({
    gate: "mixed-smoke",
    outputDirectory,
    serverBinary: path.join(root, "server"),
    wasm: path.join(root, "client.wasm"),
    clientEntry: path.join(root, "index.html"),
    additionalFiles: [],
    dockerImages: [],
    command: [
      process.execPath,
      "-e",
      `process.stdout.write(${JSON.stringify(`${JSON.stringify(mixedDocument())}\n`)})`,
    ],
  });
  assert.equal(result.ok, true);
  const resultPath = path.join(outputDirectory, "result.json");
  const retained = JSON.parse(fs.readFileSync(resultPath, "utf8"));
  assert.equal(path.isAbsolute(retained.logs.stdout.path), false);
  assert.equal(path.isAbsolute(retained.logs.stderr.path), false);
  assert.equal(validateArtifactPathReferences(retained, resultPath).ok, true);
  assert.deepEqual(
    retained.artifactFiles.map((entry) => entry.path).sort(),
    ["stderr.log", "stdout.log"],
  );
  assert.equal(
    fs.readdirSync(root).filter((name) => name.startsWith(".retained.tmp-")).length,
    0,
  );
});

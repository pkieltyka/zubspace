#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import { repoRoot } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { assertRtcPressureSummaryClean } from "../../tools/rtc_gate_audit.mjs";
import { snapshotAckWithinHistory } from "../../tools/server_events.mjs";
import {
  artifactFileManifest,
  createAtomicArtifactDirectory,
  validateArtifactPathReferences,
} from "./artifact_paths.mjs";

const scriptPath = path.relative(repoRoot, fileURLToPath(import.meta.url));

function parseArguments(argv) {
  const options = {
    gate: null,
    outputDirectory: null,
    serverBinary: "target/debug/zubspace_server",
    wasm: "target/wasm32-unknown-unknown/debug/zubspace.wasm",
    clientEntry: "index.html",
    additionalFiles: [],
    dockerImages: [],
    command: [],
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--") {
      options.command = argv.slice(index + 1);
      break;
    }
    const take = (label) => {
      index += 1;
      if (index >= argv.length) throw new Error(`${label} requires a value`);
      return argv[index];
    };
    switch (argument) {
      case "--gate":
        options.gate = take(argument);
        break;
      case "--out":
        options.outputDirectory = take(argument);
        break;
      case "--server-binary":
        options.serverBinary = take(argument);
        break;
      case "--wasm":
        options.wasm = take(argument);
        break;
      case "--client-entry":
        options.clientEntry = take(argument);
        break;
      case "--input":
        options.additionalFiles.push(take(argument));
        break;
      case "--docker-image":
        options.dockerImages.push(take(argument));
        break;
      default:
        throw new Error(`unknown retained-gate argument ${JSON.stringify(argument)}`);
    }
  }
  if (!options.gate || !options.outputDirectory || options.command.length === 0) {
    throw new Error(
      "usage: node tests/e2e/retained_gate_evidence.mjs --gate NAME --out DIR " +
        "[--input PATH] [--docker-image NAME] -- COMMAND [ARG ...]",
    );
  }
  options.additionalFiles = [...new Set(["Makefile", ...options.additionalFiles])].sort();
  return options;
}

function parseJsonDocuments(text) {
  const documents = [];
  for (let start = 0; start < text.length; start += 1) {
    if (text[start] !== "{") continue;
    let depth = 0;
    let inString = false;
    let escaped = false;
    for (let end = start; end < text.length; end += 1) {
      const character = text[end];
      if (inString) {
        if (escaped) escaped = false;
        else if (character === "\\") escaped = true;
        else if (character === '"') inString = false;
        continue;
      }
      if (character === '"') inString = true;
      else if (character === "{") depth += 1;
      else if (character === "}") {
        depth -= 1;
        if (depth === 0) {
          try {
            documents.push(JSON.parse(text.slice(start, end + 1)));
            start = end;
          } catch {
            // A log fragment can contain braces. Continue scanning for the
            // next candidate without weakening the command exit-code gate.
          }
          break;
        }
      }
    }
  }
  return documents;
}

function matchingLines(text, expression) {
  const lines = text.split(/\r?\n/).filter((line) => expression.test(line));
  return { count: lines.length, samples: lines.slice(0, 200) };
}

function logAudit(stdout, stderr) {
  const text = `${stdout}\n${stderr}`;
  const driverFull = matchingLines(
    text,
    /Failed to send DataChannelMessage to data channel \d+: Full/,
  );
  const securityRejects = matchingLines(
    text,
    /server security reject|security rejection|security_reject|"securityReject"/i,
  );
  const applicationQueueFailures = matchingLines(
    text,
    /RTC application ingress queue overflowed|RTC reliable outbound queue overflowed|outbound reliable queue overflowed|RTC application .*queue.*failed/i,
  );
  return {
    driverDataChannelFullCount: driverFull.count,
    driverDataChannelFullLines: driverFull.samples,
    structuredSecurityRejectCount: securityRejects.count,
    structuredSecurityRejectLines: securityRejects.samples,
    applicationQueueFailureCount: applicationQueueFailures.count,
    applicationQueueFailureLines: applicationQueueFailures.samples,
  };
}

const RTC_CHANNELS = ["game", "event", "reliable"];
const SERVER_LOG_AUDIT_KEYS = [
  "applicationQueueFailureCount",
  "applicationQueueFailureLines",
  "driverFull",
  "structuredSecurityRejectCount",
  "structuredSecurityRejectLines",
];

function isPlainRecord(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function validatePressureSummary(summary, label, errors, requireIngressDrained = false) {
  try {
    assertRtcPressureSummaryClean(summary, label, {requireIngressDrained});
  } catch (error) {
    errors.push(`${label} is invalid: ${error.message}`);
  }
}

function validateServerLogAudit(audit, label, errors) {
  if (!isPlainRecord(audit)) {
    errors.push(`${label} is missing a server-log audit`);
    return;
  }
  if (JSON.stringify(Object.keys(audit).sort()) !== JSON.stringify(SERVER_LOG_AUDIT_KEYS)) {
    errors.push(`${label} must contain exactly ${SERVER_LOG_AUDIT_KEYS.join("/")}`);
  }
  const full = audit.driverFull;
  if (!isPlainRecord(full)) {
    errors.push(`${label}.driverFull is missing`);
  } else {
    const expectedKeys = [...RTC_CHANNELS, "unknown", "total"].sort();
    if (JSON.stringify(Object.keys(full).sort()) !== JSON.stringify(expectedKeys)) {
      errors.push(`${label}.driverFull must contain exactly game/event/reliable/unknown/total`);
    }
    for (const channel of expectedKeys) {
      if (!Number.isFinite(full[channel]) || !Number.isInteger(full[channel]) || full[channel] < 0) {
        errors.push(`${label}.driverFull.${channel} is not a finite nonnegative integer`);
      } else if (full[channel] !== 0) {
        errors.push(`${label}.driverFull.${channel}=${full[channel]}`);
      }
    }
    if (
      expectedKeys.every((channel) => Number.isInteger(full[channel])) &&
      full.total !== full.game + full.event + full.reliable + full.unknown
    ) {
      errors.push(`${label}.driverFull.total does not equal its channel sum`);
    }
  }
  for (const prefix of ["structuredSecurityReject", "applicationQueueFailure"]) {
    const countField = `${prefix}Count`;
    const linesField = `${prefix}Lines`;
    if (!Number.isFinite(audit[countField]) || !Number.isInteger(audit[countField]) || audit[countField] < 0) {
      errors.push(`${label}.${countField} is not a finite nonnegative integer`);
    } else if (audit[countField] !== 0) {
      errors.push(`${label}.${countField}=${audit[countField]}`);
    }
    if (!Array.isArray(audit[linesField]) || !audit[linesField].every((line) => typeof line === "string")) {
      errors.push(`${label}.${linesField} must be an array of strings`);
    } else if (Number.isInteger(audit[countField]) && audit[linesField].length > audit[countField]) {
      errors.push(`${label}.${linesField} has more samples than ${countField}`);
    } else if (audit[countField] === 0 && audit[linesField].length !== 0) {
      errors.push(`${label}.${linesField} must be empty when ${countField} is zero`);
    }
  }
}

function looksLikeServerLogAudit(value) {
  return isPlainRecord(value) && SERVER_LOG_AUDIT_KEYS.some((key) => Object.hasOwn(value, key));
}

function collectNamedAudits(value, label, collected, seen = new Set(), namedServerLog = false) {
  if (namedServerLog) {
    collected.serverLogs.push([value, label]);
  }
  if (!value || typeof value !== "object") return;
  if (Object.hasOwn(value, "applicationQueuePackets")) {
    collected.pressure.push([value, label]);
  }
  if (!namedServerLog && looksLikeServerLogAudit(value)) {
    collected.serverLogs.push([value, label]);
  }
  if (seen.has(value)) return;
  seen.add(value);
  for (const key of Object.getOwnPropertyNames(value)) {
    if (Array.isArray(value) && key === "length") continue;
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    const child = descriptor && Object.hasOwn(descriptor, "value")
      ? descriptor.value
      : undefined;
    const childLabel = Array.isArray(value) ? `${label}[${key}]` : `${label}.${key}`;
    // A malformed value must not evade the audit-path contract merely because
    // it has none of the schema's recognizable fields. Property identity is
    // evidence too: every own `serverLogAudit` slot is validated at its exact
    // path, regardless of whether its value is null, primitive, an array, or
    // an empty/unknown-only object. Shape discovery below remains necessary to
    // catch copied/aliased audit objects stored under unexpected names.
    collectNamedAudits(
      child,
      childLabel,
      collected,
      seen,
      !Array.isArray(value) && key === "serverLogAudit",
    );
  }
}

function samePaths(actual, expected) {
  return JSON.stringify([...actual].sort()) === JSON.stringify([...expected].sort());
}

function validateRetainedGateDocuments(gate, documents) {
  const errors = [];
  const expectedPressure = [];
  const expectedServerLogs = [];
  const expectPressure = (value, label, requireIngressDrained = false) =>
    expectedPressure.push([value, label, requireIngressDrained]);
  const expectServerLog = (value, label) => expectedServerLogs.push([value, label]);
  const expectedDocumentCounts = {
    "mixed-smoke": 1,
    "webrtc-channel-contract": 2,
    "webrtc-capacity-200": 1,
    "compose-rtc": 1,
  };
  const expectedCount = expectedDocumentCounts[gate];
  if (expectedCount == null) {
    errors.push(`unknown retained gate ${JSON.stringify(gate)}`);
  } else if (documents.length !== expectedCount) {
    errors.push(`expected ${expectedCount} parsed JSON result(s), got ${documents.length}`);
  }
  for (const [index, document] of documents.entries()) {
    if (!document || typeof document !== "object" || Array.isArray(document)) {
      errors.push(`document[${index}] is not an object`);
    } else if (document.ok !== true) {
      errors.push(`document[${index}].ok is not true`);
    }
  }

  if (gate === "mixed-smoke") {
    const [document] = documents;
    if (document?.transport !== "rtc" || document?.browserPair !== "chrome-firefox") {
      errors.push("mixed-smoke result is missing the RTC Chrome/Firefox contract");
    }
    expectPressure(document?.pressureAudit, "document[0].pressureAudit");
    expectServerLog(document?.serverLogAudit, "document[0].serverLogAudit");
  } else if (gate === "webrtc-channel-contract") {
    const [chrome, firefox] = documents;
    const mismatchNames = chrome?.mismatchResults?.map((result) => result?.name);
    if (
      chrome?.transport !== "rtc" ||
      JSON.stringify(mismatchNames) !== JSON.stringify([
        "fully reliable game",
        "partially reliable event",
        "partially reliable reliable",
      ])
    ) {
      errors.push("channel-contract Chrome result is missing exact mismatch coverage");
    }
    if (firefox?.transport !== "rtc" || firefox?.browser !== "firefox") {
      errors.push("channel-contract Firefox result is missing");
    }
    expectPressure(chrome?.validPressureAudit, "document[0].validPressureAudit");
    for (let index = 0; index < 3; index += 1) {
      const mismatch = chrome?.mismatchResults?.[index];
      if (
        mismatch?.status?.rtc_channel_protocol_violations !== index + 1 ||
        mismatch?.status?.rtc_sessions !== 0 ||
        mismatch?.status?.clients !== 0
      ) {
        errors.push(`channel-contract mismatch leg ${index} lacks rejection/cleanup evidence`);
      }
      expectPressure(
        mismatch?.pressureAudit,
        `document[0].mismatchResults[${index}].pressureAudit`,
        true,
      );
    }
    expectServerLog(chrome?.serverLogAudit, "document[0].serverLogAudit");
    expectPressure(firefox?.pressureAudit, "document[1].pressureAudit");
    expectServerLog(firefox?.serverLogAudit, "document[1].serverLogAudit");
  } else if (gate === "webrtc-capacity-200") {
    const [document] = documents;
    if (
      document?.transport?.connected !== 200 ||
      document?.transport?.snapshots?.peersReceiving !== 200 ||
      document?.transport?.snapshots?.idGaps !== 0 ||
      document?.statusDuring?.rtc_sessions !== 200 ||
      document?.statusAfter?.rtc_sessions !== 0
    ) {
      errors.push("capacity result is missing the exact 200-session snapshot/cleanup contract");
    }
    for (const phase of ["before", "during", "after"]) {
      expectPressure(document?.pressureAudit?.[phase], `document[0].pressureAudit.${phase}`, phase !== "during");
    }
    expectServerLog(document?.serverLogAudit, "document[0].serverLogAudit");
  } else if (gate === "compose-rtc") {
    const [document] = documents;
    const channels = [...(document?.channels || [])].sort();
    if (
      JSON.stringify(channels) !== JSON.stringify([
        "zubspace-events",
        "zubspace-game",
        "zubspace-reliable",
      ]) ||
      !snapshotAckWithinHistory(
        document?.snapshot?.latest,
        document?.snapshot?.acked,
      ) ||
      document?.status?.during?.rtcSessions !== 1 ||
      document?.status?.after?.rtcSessions !== 0 ||
      document?.status?.after?.clients !== 0 ||
      document?.status?.after?.udpPortsInUse !== 0
    ) {
      errors.push("Compose result is missing the exact channel/snapshot/cleanup contract");
    }
    for (const phase of ["before", "during", "after"]) {
      expectPressure(document?.pressureAudit?.[phase], `document[0].pressureAudit.${phase}`, phase !== "during");
    }
    expectServerLog(document?.serverLogAudit, "document[0].serverLogAudit");
  }

  const discovered = { pressure: [], serverLogs: [] };
  for (const [index, document] of documents.entries()) {
    collectNamedAudits(document, `document[${index}]`, discovered);
  }
  if (!samePaths(
    discovered.pressure.map(([, label]) => label),
    expectedPressure.map(([, label]) => label),
  )) {
    errors.push(`pressure audit paths do not match the ${gate} phase contract: ${JSON.stringify({
      expected: expectedPressure.map(([, label]) => label),
      actual: discovered.pressure.map(([, label]) => label),
    })}`);
  }
  if (!samePaths(
    discovered.serverLogs.map(([, label]) => label),
    expectedServerLogs.map(([, label]) => label),
  )) {
    errors.push(`server-log audit paths do not match the ${gate} phase contract: ${JSON.stringify({
      expected: expectedServerLogs.map(([, label]) => label),
      actual: discovered.serverLogs.map(([, label]) => label),
    })}`);
  }
  for (const [audit, label, requireIngressDrained] of expectedPressure) {
    validatePressureSummary(audit, label, errors, requireIngressDrained);
  }
  for (const [audit, label] of expectedServerLogs) {
    validateServerLogAudit(audit, label, errors);
  }
  return { ok: errors.length === 0, errors };
}

async function runRetainedGate(options) {
  const outputDirectory = path.resolve(options.outputDirectory);
  const atomicDirectory = createAtomicArtifactDirectory(outputDirectory);
  const workingDirectory = atomicDirectory.temporary;
  const stdoutPath = path.join(workingDirectory, "stdout.log");
  const stderrPath = path.join(workingDirectory, "stderr.log");
  const stdoutStream = fs.createWriteStream(stdoutPath, { flags: "w" });
  const stderrStream = fs.createWriteStream(stderrPath, { flags: "w" });
  const stdoutChunks = [];
  const stderrChunks = [];
  const runStartedAt = new Date().toISOString();
  const provenanceOptions = {
    serverBinary: options.serverBinary,
    wasm: options.wasm,
    clientEntry: options.clientEntry,
    e2eScript: scriptPath,
    additionalFiles: options.additionalFiles,
    runStartedAt,
  };
  const initialProvenance = collectArtifactProvenance(provenanceOptions);
  const [command, ...args] = options.command;
  const child = spawn(command, args, {
    cwd: repoRoot,
    env: process.env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout.on("data", (chunk) => {
    stdoutChunks.push(chunk);
    stdoutStream.write(chunk);
    process.stdout.write(chunk);
  });
  child.stderr.on("data", (chunk) => {
    stderrChunks.push(chunk);
    stderrStream.write(chunk);
    process.stderr.write(chunk);
  });
  const outcome = await new Promise((resolve) => {
    child.once("error", (error) => resolve({ exitCode: null, signal: null, error }));
    child.once("exit", (exitCode, signal) => resolve({ exitCode, signal, error: null }));
  });
  await Promise.all([
    new Promise((resolve) => stdoutStream.end(resolve)),
    new Promise((resolve) => stderrStream.end(resolve)),
  ]);
  const runEndedAt = new Date().toISOString();
  const stdout = Buffer.concat(stdoutChunks).toString("utf8");
  const stderr = Buffer.concat(stderrChunks).toString("utf8");
  const finalBaseProvenance = collectArtifactProvenance(provenanceOptions);
  let provenanceDrift = null;
  try {
    assertArtifactProvenanceMatches(initialProvenance, finalBaseProvenance);
  } catch (error) {
    provenanceDrift = error.message;
  }
  const provenance = collectArtifactProvenance({
    ...provenanceOptions,
    dockerImages: options.dockerImages,
  });
  const audit = logAudit(stdout, stderr);
  const outputJsonDocuments = parseJsonDocuments(stdout);
  const validation = validateRetainedGateDocuments(options.gate, outputJsonDocuments);
  const auditClean = audit.driverDataChannelFullCount === 0 &&
    audit.structuredSecurityRejectCount === 0 &&
    audit.applicationQueueFailureCount === 0;
  const ok = outcome.exitCode === 0 && outcome.signal === null && provenanceDrift === null &&
    auditClean && validation.ok;
  const result = {
    ok,
    formatVersion: 2,
    gate: options.gate,
    runStartedAt,
    runEndedAt,
    durationMs: Date.parse(runEndedAt) - Date.parse(runStartedAt),
    command: { executable: command, args },
    outcome: {
      exitCode: outcome.exitCode,
      signal: outcome.signal,
      spawnError: outcome.error?.message || null,
      provenanceDrift,
    },
    logs: {
      stdout: { path: "stdout.log", bytes: Buffer.byteLength(stdout) },
      stderr: { path: "stderr.log", bytes: Buffer.byteLength(stderr) },
    },
    audit,
    validation,
    outputJsonDocuments,
    provenance,
    retainedExternalPaths: [],
  };
  result.artifactFiles = artifactFileManifest(workingDirectory);
  const workingResultPath = path.join(workingDirectory, "result.json");
  fs.writeFileSync(workingResultPath, `${JSON.stringify(result, null, 2)}\n`);
  validateArtifactPathReferences(result, workingResultPath);
  atomicDirectory.publish();
  if (!ok) {
    throw new Error(`retained gate ${options.gate} failed: ${JSON.stringify({
      outcome: result.outcome,
      audit: result.audit,
      validation: result.validation,
    })}`);
  }
  return result;
}

const invokedPath = process.argv[1] ? path.resolve(process.argv[1]) : null;
if (invokedPath === fileURLToPath(import.meta.url)) {
  runRetainedGate(parseArguments(process.argv.slice(2))).catch((error) => {
    console.error(error.stack || error.message);
    process.exitCode = 1;
  });
}

export {
  logAudit,
  parseArguments,
  parseJsonDocuments,
  runRetainedGate,
  validateArtifactPathReferences,
  validateRetainedGateDocuments,
};

#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import {
  artifactFileManifest,
  createAtomicArtifactDirectory,
  validateArtifactPathReferences,
} from "./artifact_paths.mjs";
import { assertSmokeQaCaptures } from "./smoke_qa_artifacts.mjs";
import { validateQaTraceArtifacts } from "./qa_trace_artifacts.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const scriptPath = path.relative(repoRoot, fileURLToPath(import.meta.url));
const smokeScript = "tools/browser_real_server_smoke.mjs";
const outputDirectory = path.resolve(
  process.env.ZUBSPACE_OBJECTIVE_AUTHORITY_ARTIFACT_DIR ||
    `/tmp/zubspace-objective-authority-${Date.now()}`,
);
const objectivePacketTypes = new Set([0x12, 0x13, 0x16, 0x2e]);
const channelNames = ["zubspace-game", "zubspace-events", "zubspace-reliable"];
const expectedChannelContracts = {
  "zubspace-game": {
    label: "zubspace-game",
    ordered: false,
    maxPacketLifeTime: 80,
    maxRetransmits: null,
  },
  "zubspace-events": {
    label: "zubspace-events",
    ordered: false,
    maxPacketLifeTime: null,
    maxRetransmits: null,
  },
  "zubspace-reliable": {
    label: "zubspace-reliable",
    ordered: true,
    maxPacketLifeTime: null,
    maxRetransmits: null,
  },
};

function u16Advanced(after, before) {
  if (!Number.isInteger(after) || !Number.isInteger(before)) return false;
  const delta = (after - before) & 0xffff;
  return delta > 0 && delta < 0x8000;
}

function objectiveStateIsEmptyAndAuthoritative(state) {
  return state?.objectiveRenderFrameObserved === true &&
    state.objectivePresentationAuthoritativeOnly === true &&
    state.objectivePresentedFlagCount === 0 &&
    state.objectivePresentedPowerballCount === 0 &&
    state.localFlagFallbackPresented === false &&
    state.localPowerballFallbackPresented === false &&
    state.networkedPlay === true &&
    state.localObjectiveFallbackAllowed === false &&
    state.authoritativeFlagCount === 0 &&
    state.authoritativePowerballCount === 0 &&
    state.localCarryingFlag === false &&
    state.localDroppedFlag === false &&
    state.localPowerballCarried === false;
}

function exactTreeProvenance(runStartedAt) {
  return collectArtifactProvenance({
    serverBinary: "target/debug/zubspace_server",
    wasm: "target/wasm32-unknown-unknown/debug/zubspace.wasm",
    clientEntry: "index.html",
    e2eScript: scriptPath,
    additionalFiles: [
      "Makefile",
      smokeScript,
      "assets/zones/chaos-zone/zone.bundle",
      "assets/zones/extreme/zone.bundle",
    ],
    runStartedAt,
  });
}

function parseSmokeDocument(label, result) {
  if (result.error || result.status !== 0) {
    throw new Error(
      `${label} focused RTC smoke failed: ${result.error?.message || result.stderr || result.stdout}`,
    );
  }
  const output = result.stdout.trim();
  const jsonStart = output.indexOf("{");
  if (jsonStart < 0) {
    throw new Error(`${label} focused RTC smoke returned no JSON document`);
  }
  return JSON.parse(output.slice(jsonStart));
}

function runZone(label, arena, expectedMap, artifactRoot, qaTraceCaptureAudit) {
  const result = spawnSync(process.execPath, [smokeScript], {
    cwd: repoRoot,
    env: {
      ...process.env,
      ZUBSPACE_SMOKE_ARENA: arena,
      ZUBSPACE_SMOKE_ZONE_ROOT: "assets/zones",
      ZUBSPACE_SMOKE_ALLOW_ZONE_OVERRIDES: "1",
      ZUBSPACE_SMOKE_STOP_AFTER_OBJECTIVE_AUTHORITY: "1",
      ZUBSPACE_SMOKE_LOG_LEVEL: "info",
      ZUBSPACE_SMOKE_QA_ARTIFACT_DIR: artifactRoot,
      ZUBSPACE_SMOKE_QA_LABEL: label,
    },
    encoding: "utf8",
    maxBuffer: 128 * 1024 * 1024,
  });
  fs.writeFileSync(path.join(artifactRoot, `${label}.stdout.log`), result.stdout || "");
  fs.writeFileSync(path.join(artifactRoot, `${label}.stderr.log`), result.stderr || "");
  const indexPath = path.join(artifactRoot, `${label}-qa-index.json`);
  const captures = fs.existsSync(indexPath)
    ? JSON.parse(fs.readFileSync(indexPath, "utf8")).qaTraceCaptureAudit : [];
  qaTraceCaptureAudit.push(...captures);
  const document = parseSmokeDocument(label, result);
  assertSmokeQaCaptures({ label, captures });
  const status = document.objectiveAuthorityStatus;
  if (
    document.ok !== true ||
    document.transport !== "rtc" ||
    document.arena !== arena ||
    document.expectedMap !== expectedMap ||
    status?.objective_profile_disabled !== true ||
    status.objective_flags !== 0 ||
    status.objective_powerballs !== 0 ||
    !["game", "event", "reliable"].every(
      (channel) => status.rtc_inbound_packets?.[channel] > 0,
    ) ||
    document.serverSecurityRejects?.length !== 0 ||
    document.objectiveAuthoritySecurityAudit?.serverSecurityRejects?.length !== 0 ||
    document.objectiveAuthoritySecurityAudit?.rtcChannelProtocolViolations !== 0
  ) {
    throw new Error(`${label} server objective authority evidence is incomplete: ${JSON.stringify({
      ok: document.ok,
      transport: document.transport,
      arena: document.arena,
      expectedMap: document.expectedMap,
      status,
      serverSecurityRejects: document.serverSecurityRejects,
    })}`);
  }

  const clients = {};
  for (const clientName of ["alpha", "beta"]) {
    const client = document[clientName];
    const state = client?.uiMetrics?.localGameplayState;
    const frames = document.objectiveAuthorityFrames?.[clientName] || [];
    const channelDeliveryAudit = client?.channelDeliveryAudit;
    const channelContractAudit = client?.channelContractAudit;
    const bidirectionalChannels = channelNames.every(
      (channel) =>
        channelDeliveryAudit?.[channel]?.sent > 0 &&
        channelDeliveryAudit?.[channel]?.received > 0,
    );
    const exactChannelContracts =
      JSON.stringify(channelContractAudit) === JSON.stringify(expectedChannelContracts);
    const movementObserved = frames.some((frame) => {
      const local = frame.localGameplayState;
      return Math.hypot(local?.velocityX || 0, local?.velocityY || 0) > 1;
    });
    const receivedObjectivePacketTypes = (client?.receivedReliableTypes || [])
      .filter((packetType) => objectivePacketTypes.has(packetType));
    if (
      !client?.mapFilenames?.includes(expectedMap) ||
      !objectiveStateIsEmptyAndAuthoritative(state) ||
      frames.length !== 3 ||
      frames.some(
        (frame) =>
          !objectiveStateIsEmptyAndAuthoritative(frame.localGameplayState) ||
          frame.screenshotNonBlackPixels < 100,
      ) ||
      !u16Advanced(frames.at(-1)?.clientInputSeq, frames[0]?.clientInputSeq) ||
      !(frames.at(-1)?.sentPositionCount > frames[0]?.sentPositionCount) ||
      !movementObserved ||
      receivedObjectivePacketTypes.length !== 0 ||
      !bidirectionalChannels ||
      !exactChannelContracts
    ) {
      throw new Error(`${label} ${clientName} objective authority evidence is incomplete: ${JSON.stringify({
        mapFilenames: client?.mapFilenames,
        state,
        frames,
        movementObserved,
        receivedObjectivePacketTypes,
        channelDeliveryAudit,
        channelContractAudit,
      })}`);
    }
    clients[clientName] = {
      networkedPlay: state.networkedPlay,
      localObjectiveFallbackAllowed: state.localObjectiveFallbackAllowed,
      authoritativeFlagCount: state.authoritativeFlagCount,
      authoritativePowerballCount: state.authoritativePowerballCount,
      localCarryingFlag: state.localCarryingFlag,
      localDroppedFlag: state.localDroppedFlag,
      localPowerballCarried: state.localPowerballCarried,
      renderFrames: frames.map((frame) => ({
        clientInputSeq: frame.clientInputSeq,
        sentPositionCount: frame.sentPositionCount,
        screenshotNonBlackPixels: frame.screenshotNonBlackPixels,
        objectiveRenderFrameObserved: frame.localGameplayState.objectiveRenderFrameObserved,
        objectivePresentationAuthoritativeOnly:
          frame.localGameplayState.objectivePresentationAuthoritativeOnly,
        objectivePresentedFlagCount:
          frame.localGameplayState.objectivePresentedFlagCount,
        objectivePresentedPowerballCount:
          frame.localGameplayState.objectivePresentedPowerballCount,
        localFlagFallbackPresented: frame.localGameplayState.localFlagFallbackPresented,
        localPowerballFallbackPresented:
          frame.localGameplayState.localPowerballFallbackPresented,
        positionX: frame.localGameplayState.positionX,
        positionY: frame.localGameplayState.positionY,
        velocityX: frame.localGameplayState.velocityX,
        velocityY: frame.localGameplayState.velocityY,
      })),
      receivedObjectivePacketTypes,
      channelDeliveryAudit,
      channelContractAudit,
    };
  }

  return {
    arena,
    expectedMap,
    server: {
      profileDisabled: status.objective_profile_disabled,
      flagCount: status.objective_flags,
      powerballCount: status.objective_powerballs,
      inboundGamePackets: status.rtc_inbound_packets.game,
      inboundEventPackets: status.rtc_inbound_packets.event,
      inboundReliablePackets: status.rtc_inbound_packets.reliable,
      securityAudit: document.objectiveAuthoritySecurityAudit,
    },
    clients,
  };
}

function main() {
  const artifactDirectory = createAtomicArtifactDirectory(outputDirectory);
  const runStartedAt = new Date().toISOString();
  const initialProvenance = exactTreeProvenance(runStartedAt);
  const qaTraceCaptureAudit = [];
  fs.writeFileSync(path.join(artifactDirectory.temporary, "provenance.json"), `${JSON.stringify(initialProvenance, null, 2)}\n`);
  try {
    const zones = {
      chaos: runZone(
        "chaos-zone",
        "chaos-zone",
        "bdhadar.lvl",
        artifactDirectory.temporary,
        qaTraceCaptureAudit,
      ),
      extreme: runZone(
        "extreme",
        "extreme",
        "doomsy8.lvl",
        artifactDirectory.temporary,
        qaTraceCaptureAudit,
      ),
    };
    const finalProvenance = exactTreeProvenance(runStartedAt);
    assertArtifactProvenanceMatches(initialProvenance, finalProvenance);
    fs.writeFileSync(
      path.join(artifactDirectory.temporary, "provenance.json"),
      `${JSON.stringify(finalProvenance, null, 2)}\n`,
    );
    const result = {
      ok: true,
      formatVersion: 1,
      runStartedAt,
      runEndedAt: new Date().toISOString(),
      browser: "chrome",
      transport: "rtc",
      scope: "objective-authority-keyboard-render-frames-before-lifecycle-plus-three-channel-audit",
      zones,
      provenancePath: "provenance.json",
      provenance: finalProvenance,
      qaTraceCaptureAudit,
      retainedExternalPaths: [],
    };
    result.artifactFiles = artifactFileManifest(artifactDirectory.temporary);
    const resultPath = path.join(artifactDirectory.temporary, "result.json");
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
    validateArtifactPathReferences(result, resultPath);
    validateQaTraceArtifacts(result, resultPath);
    const published = artifactDirectory.publish();
    console.log(JSON.stringify({ ok: true, artifactDir: published, zones }, null, 2));
  } catch (error) {
    fs.writeFileSync(
      path.join(artifactDirectory.temporary, "failure.txt"),
      `${error.stack || error.message}\n`,
    );
    fs.writeFileSync(path.join(artifactDirectory.temporary, "result.json"), `${JSON.stringify({
      ok: false, provenance: initialProvenance, qaTraceCaptureAudit,
      artifactFiles: artifactFileManifest(artifactDirectory.temporary),
    }, null, 2)}\n`);
    const failed = artifactDirectory.publish();
    throw new Error(`${error.message}; failure artifacts: ${failed}`);
  }
}

try {
  main();
} catch (error) {
  console.error(error.stack || error.message);
  process.exit(1);
}

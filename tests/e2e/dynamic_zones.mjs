#!/usr/bin/env node

import crypto from "node:crypto";
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
const applicationRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const scriptPath = path.relative(repoRoot, fileURLToPath(import.meta.url));
const smokeScript = "tools/browser_real_server_smoke.mjs";
const outputDirectory = path.resolve(
  process.env.ZUBSPACE_DYNAMIC_ZONE_ARTIFACT_DIR ||
    `/tmp/zubspace-dynamic-zones-${Date.now()}`,
);
const extremeSettings = {
  bulletDamage: 210000,
  bombDamage: 5600000,
  bulletSpeed: 4100,
  initialSpeed: 3200,
  maxFrequency: 10000,
  xradarWeight: 23,
  antiwarpWeight: 10,
  multiprizeWeight: 5,
  portalWeight: 0,
  shipRadiusWire: [0, 0, 0, 0, 0, 0, 0, 0],
};

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function singleFileLvz(filename, bytes) {
  const archiveHeader = Buffer.alloc(8);
  archiveHeader.write("CONT", 0, "ascii");
  archiveHeader.writeUInt32LE(1, 4);
  const sectionHeader = Buffer.alloc(16);
  sectionHeader.write("CONT", 0, "ascii");
  sectionHeader.writeUInt32LE(bytes.length, 4);
  sectionHeader.writeUInt32LE(1, 8);
  sectionHeader.writeUInt32LE(bytes.length, 12);
  return Buffer.concat([
    archiveHeader,
    sectionHeader,
    Buffer.from(`${filename}\0`, "ascii"),
    bytes,
  ]);
}

function createTemporaryZoneBundle(root) {
  const directory = path.join(root, "test-zone");
  fs.mkdirSync(directory, { recursive: true });
  const map = fs.readFileSync(path.join(repoRoot, "assets/zones/extreme/doomsy8.lvl"));
  const settings = fs.readFileSync(path.join(repoRoot, "assets/zones/extreme/arena.conf"));
  const news = Buffer.from("Temporary dynamic zone\n");
  const chatSound = fs.readFileSync(path.join(repoRoot, "assets/sound/bong1.wa2"));
  const dynamicLvz = singleFileLvz("bong27.wa2", chatSound);
  fs.writeFileSync(path.join(directory, "test.lvl"), map);
  fs.writeFileSync(path.join(directory, "arena.conf"), settings);
  fs.writeFileSync(path.join(directory, "news.txt"), news);
  fs.writeFileSync(path.join(directory, "dynamic.lvz"), dynamicLvz);
  fs.writeFileSync(
    path.join(directory, "zone.bundle"),
    [
      "version=1",
      "id=test-zone",
      "display_name=Test Zone",
      "map=test.lvl",
      `map_sha256=${sha256(map)}`,
      "settings=arena.conf",
      `settings_sha256=${sha256(settings)}`,
      "news=news.txt",
      `news_sha256=${sha256(news)}`,
      `lvz=dynamic.lvz,${sha256(dynamicLvz)}`,
      "",
    ].join("\n"),
  );
  return {
    root,
    manifestSha256: sha256(fs.readFileSync(path.join(directory, "zone.bundle"))),
    mapSha256: sha256(map),
    settingsSha256: sha256(settings),
    dynamicLvzSha256: sha256(dynamicLvz),
    chatSoundSha256: sha256(chatSound),
  };
}

function runSmoke(label, environment, artifactRoot, qaTraceCaptureAudit) {
  const result = spawnSync(process.execPath, [smokeScript], {
    cwd: repoRoot,
    env: {
      ...process.env,
      ZUBSPACE_SMOKE_STOP_AFTER_POSITION: "1",
      ZUBSPACE_SMOKE_POSITION_SAMPLE_MS: "3000",
      ZUBSPACE_SMOKE_LOG_LEVEL: "info",
      ZUBSPACE_SMOKE_REQUIRE_THREE_CHANNEL_TRAFFIC: "1",
      ZUBSPACE_SMOKE_QA_ARTIFACT_DIR: artifactRoot,
      ZUBSPACE_SMOKE_QA_LABEL: label,
      ...environment,
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
  if (result.error || result.status !== 0) {
    throw new Error(
      `${label} Chrome smoke failed: ${result.error?.message || result.stderr || result.stdout}`,
    );
  }
  const text = result.stdout.trim();
  const document = JSON.parse(text.slice(text.indexOf("{")));
  if (document.ok !== true || document.transport !== "rtc") {
    throw new Error(`${label} did not return successful RTC evidence`);
  }
  assertSmokeQaCaptures({ label, captures,
    cacheRejoin: environment.ZUBSPACE_SMOKE_REQUIRE_MAP_CACHE_REJOIN === "1",
    discovery: environment.ZUBSPACE_SMOKE_ARENA_SOURCE === "discovery" });
  return document;
}

function assertChannelAudit(label, document) {
  const totals = new Map();
  for (const client of [document.alpha, document.beta]) {
    for (const [channel, counts] of Object.entries(client.channelDeliveryAudit || {})) {
      const total = totals.get(channel) || { sent: 0, received: 0 };
      total.sent += counts.sent || 0;
      total.received += counts.received || 0;
      totals.set(channel, total);
    }
  }
  for (const [clientName, client] of [["alpha", document.alpha], ["beta", document.beta]]) {
    for (const channel of ["zubspace-game", "zubspace-events", "zubspace-reliable"]) {
      const counts = client.channelDeliveryAudit?.[channel];
      if (!counts || counts.sent === 0 || counts.received === 0) {
        throw new Error(`${label} ${clientName} lacks bidirectional ${channel} delivery evidence: ${JSON.stringify(client.channelDeliveryAudit)}`);
      }
    }
  }
  const expectedContracts = {
    "zubspace-game": { ordered: false, maxPacketLifeTime: 80 },
    "zubspace-events": { ordered: false, maxPacketLifeTime: null },
    "zubspace-reliable": { ordered: true, maxPacketLifeTime: null },
  };
  const contracts = {};
  for (const [clientName, client] of [["alpha", document.alpha], ["beta", document.beta]]) {
    const actual = client.channelContractAudit || {};
    for (const [channel, expected] of Object.entries(expectedContracts)) {
      const contract = actual[channel];
      if (
        !contract ||
        contract.ordered !== expected.ordered ||
        contract.maxPacketLifeTime !== expected.maxPacketLifeTime ||
        contract.maxRetransmits !== null
      ) {
        throw new Error(`${label} ${clientName} has invalid ${channel} contract: ${JSON.stringify(contract)}`);
      }
    }
    contracts[clientName] = actual;
  }
  return {
    delivery: Object.fromEntries(totals),
    contracts,
    eventDeliveryExpected: true,
  };
}

function assertZoneEvidence(label, document, expectedArena, expectedMap) {
  if (
    document.arena !== expectedArena ||
    document.expectedMap !== expectedMap ||
    !document.alpha.mapFilenames.includes(expectedMap) ||
    !document.beta.mapFilenames.includes(expectedMap) ||
    document.serverSecurityRejects.length !== 0
  ) {
    throw new Error(`${label} zone/map/security evidence is incomplete: ${JSON.stringify(document)}`);
  }
  for (const [clientName, client] of [["alpha", document.alpha], ["beta", document.beta]]) {
    const policy = client.mapCachePolicyEvidence;
    if (
      !policy?.identityIsolation ||
      !policy?.totalBudgetLru ||
      !policy?.quotaRetry ||
      !policy?.corruptStaleCleanup
    ) {
      throw new Error(`${label} ${clientName} lacks adversarial cache-policy evidence: ${JSON.stringify(policy)}`);
    }
  }
}

function assertDisabledObjectiveEvidence(label, document) {
  const evidence = {};
  for (const [clientName, client] of [["alpha", document.alpha], ["beta", document.beta]]) {
    const state = client.uiMetrics?.localGameplayState;
    if (
      state?.networkedPlay !== true ||
      state.localObjectiveFallbackAllowed !== false ||
      state.authoritativeFlagCount !== 0 ||
      state.authoritativePowerballCount !== 0 ||
      state.localCarryingFlag !== false ||
      state.localDroppedFlag !== false ||
      state.localPowerballCarried !== false
    ) {
      throw new Error(
        `${label} ${clientName} did not remain authoritative-objective empty: ${JSON.stringify(state)}`,
      );
    }
    evidence[clientName] = {
      networkedPlay: state.networkedPlay,
      localObjectiveFallbackAllowed: state.localObjectiveFallbackAllowed,
      authoritativeFlagCount: state.authoritativeFlagCount,
      authoritativePowerballCount: state.authoritativePowerballCount,
      localCarryingFlag: state.localCarryingFlag,
      localDroppedFlag: state.localDroppedFlag,
      localPowerballCarried: state.localPowerballCarried,
    };
  }
  return evidence;
}

function assertFreshDownloadEvidence(label, document) {
  for (const [clientName, client] of [["alpha", document.alpha], ["beta", document.beta]]) {
    if (
      (client.mapCacheStats?.misses || 0) < 1 ||
      (client.mapCacheStats?.writes || 0) < 1 ||
      !client.sentReliableTypes.includes(0x0c)
    ) {
      throw new Error(`${label} ${clientName} did not prove fresh 0x0c download/write: ${JSON.stringify({
        cache: client.mapCacheStats,
        reliable: client.sentReliableTypes,
      })}`);
    }
  }
}

function assertWeaponCarrierEvidence(label, document) {
  const current = document.currentGenerationWeaponEvidence;
  if (
    !current ||
    !Array.isArray(current.visualSpawnDeltas) ||
    current.visualSpawnDeltas.length !== 2 ||
    current.visualSpawnDeltas.some((delta) => delta !== 1)
  ) {
    throw new Error(`${label} lacks exactly-once current-generation visual evidence: ${JSON.stringify(current)}`);
  }
  const evidence = {};
  for (const [clientName, client] of [["alpha", document.alpha], ["beta", document.beta]]) {
    const weapons = current[clientName];
    if (
      !weapons ||
      weapons.uniqueFacts < 1 ||
      weapons.dualGameEventFacts !== weapons.uniqueFacts ||
      weapons.exactlyDualDeliveredFacts !== weapons.uniqueFacts ||
      weapons.immutableSourceFacts !== weapons.uniqueFacts
    ) {
      throw new Error(`${label} ${clientName} lacks exactly-once dual-channel weapon evidence: ${JSON.stringify({
        weapons,
        visualSpawnDelta: current.visualSpawnDeltas[clientName === "alpha" ? 0 : 1],
      })}`);
    }
    evidence[clientName] = {
      uniqueFacts: weapons.uniqueFacts,
      dualGameEventFacts: weapons.dualGameEventFacts,
      exactlyDualDeliveredFacts: weapons.exactlyDualDeliveredFacts,
      immutableSourceFacts: weapons.immutableSourceFacts,
      renderedRemoteProjectileSpawns: current.visualSpawnDeltas[clientName === "alpha" ? 0 : 1],
      channels: weapons.channels,
    };
  }
  return evidence;
}

function assertProjectileLifecycleEvidence(label, document) {
  const evidence = document.projectileLifecycleEvidence;
  if (
    !evidence ||
    evidence.formatVersion !== 2 ||
    evidence.kind !== "mine" ||
    evidence.transition !== "intentional-ship-cycle" ||
    !evidence.removedAfterLifecycleReplacement ||
    !evidence.cancelledSourceSessionId ||
    !evidence.cancelledSourceLifeSeq ||
    !evidence.replacementLifeSeq ||
    evidence.cancelledSourceLifeSeq === evidence.replacementLifeSeq ||
    evidence.sourceLifeBeforeReset !== evidence.cancelledSourceLifeSeq ||
    !evidence.immutableFields ||
    evidence.cancellation?.source !== evidence.sourcePlayerId ||
    evidence.cancellation.sourceSessionId !== evidence.cancelledSourceSessionId ||
    BigInt(evidence.cancellation.throughShotGroupId) < BigInt(evidence.cancelledShotGroupId)
  ) {
    throw new Error(
      `${label} lacks exact-source intentional weapon cancellation evidence: ${JSON.stringify(evidence)}`,
    );
  }
  return evidence;
}

function assertQueueDrainEvidence(label, document) {
  const evidence = document.queueDrainEvidence;
  const pressure = evidence?.serverPressure;
  const log = evidence?.serverLogAudit;
  if (
    !evidence ||
    evidence.rtcSessions !== 0 ||
    evidence.clients !== 0 ||
    evidence.browserReliableQueues?.alpha !== 0 ||
    evidence.browserReliableQueues?.beta !== 0 ||
    pressure?.outboundQueuePackets !== 0 ||
    pressure?.outboundQueueBytes !== 0 ||
    pressure?.outboundReliableOverflows !== 0 ||
    Object.values(pressure?.applicationQueuePackets || {}).some((value) => value !== 0) ||
    Object.values(pressure?.applicationBackpressure || {}).some((value) => value !== 0) ||
    log?.driverFull?.total !== 0 ||
    log?.structuredSecurityRejectCount !== 0 ||
    log?.applicationQueueFailureCount !== 0
  ) {
    throw new Error(`${label} did not retain clean drained-queue evidence: ${JSON.stringify(evidence)}`);
  }
  return evidence;
}

function exactTreeProvenance(runStartedAt) {
  return collectArtifactProvenance({
    serverBinary: "target/debug/zubspace_server",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT
      ? path.relative(repoRoot, path.join(applicationRoot, "zubspace.wasm"))
      : "target/wasm32-unknown-unknown/debug/zubspace.wasm",
    clientEntry: path.relative(repoRoot, path.join(applicationRoot, "index.html")),
    e2eScript: scriptPath,
    additionalFiles: [
      "Makefile",
      smokeScript,
      "tools/dynamic_zone_weapon_schedule.mjs",
      "tools/realtime_event_evidence.mjs",
      "js/zubspace_zone_catalog.js",
      "assets/zones/chaos-zone/zone.bundle",
      "assets/zones/extreme/zone.bundle",
    ],
    runStartedAt,
  });
}

function main() {
  const artifactDirectory = createAtomicArtifactDirectory(outputDirectory);
  const runStartedAt = new Date().toISOString();
  const initialProvenance = exactTreeProvenance(runStartedAt);
  const qaTraceCaptureAudit = [];
  fs.writeFileSync(path.join(artifactDirectory.temporary, "provenance.json"), `${JSON.stringify(initialProvenance, null, 2)}\n`);
  try {
    const temporaryBundle = createTemporaryZoneBundle(
      path.join(artifactDirectory.temporary, "temporary-zone-root"),
    );
    const diskEnvironment = {
      ZUBSPACE_SMOKE_ZONE_ROOT: "assets/zones",
      ZUBSPACE_SMOKE_ALLOW_ZONE_OVERRIDES: "1",
    };
    const chaos = runSmoke(
      "chaos-disk",
      {
        ...diskEnvironment,
        ZUBSPACE_SMOKE_ARENA: "chaos-zone",
        ZUBSPACE_SMOKE_FORCE_FRESH_MAP_CACHE: "1",
        ZUBSPACE_SMOKE_REQUIRE_PROJECTILE_LIFECYCLE_IDENTITY: "1",
      },
      artifactDirectory.temporary,
      qaTraceCaptureAudit,
    );
    const extreme = runSmoke(
      "extreme-disk",
      {
        ...diskEnvironment,
        ZUBSPACE_SMOKE_ARENA: "extreme",
        ZUBSPACE_SMOKE_FORCE_FRESH_MAP_CACHE: "1",
      },
      artifactDirectory.temporary,
      qaTraceCaptureAudit,
    );
    const temporary = runSmoke(
      "temporary-zone",
      {
        ZUBSPACE_SMOKE_ZONE_ROOT: temporaryBundle.root,
        ZUBSPACE_SMOKE_ARENA: "test-zone",
        ZUBSPACE_SMOKE_ARENA_SOURCE: "discovery",
        ZUBSPACE_SMOKE_EXPECTED_MAP: "test.lvl",
        ZUBSPACE_SMOKE_EXPECTED_STORED_ARENA: "test-zone",
        ZUBSPACE_SMOKE_EXPECTED_SETTINGS_JSON: JSON.stringify(extremeSettings),
        ZUBSPACE_SMOKE_REQUIRE_MAP_CACHE_REJOIN: "1",
        ZUBSPACE_SMOKE_REQUIRE_DYNAMIC_LVZ_CHAT_SOUND: "1",
      },
      artifactDirectory.temporary,
      qaTraceCaptureAudit,
    );

    assertZoneEvidence("Chaos disk bundle", chaos, "chaos-zone", "bdhadar.lvl");
    assertZoneEvidence("Extreme disk bundle", extreme, "extreme", "doomsy8.lvl");
    assertZoneEvidence("temporary disk bundle", temporary, "test-zone", "test.lvl");
    assertFreshDownloadEvidence("Chaos disk bundle", chaos);
    assertFreshDownloadEvidence("Extreme disk bundle", extreme);
    const disabledObjectiveEvidence = {
      chaos: assertDisabledObjectiveEvidence("Chaos disk bundle", chaos),
      extreme: assertDisabledObjectiveEvidence("Extreme disk bundle", extreme),
      temporary: assertDisabledObjectiveEvidence("temporary disk bundle", temporary),
    };
    const discoveryEvidence = temporary.zoneDiscoveryEvidence;
    if (
      discoveryEvidence?.source !== "server" ||
      discoveryEvidence.statusCode !== 200 ||
      discoveryEvidence.publicSelectorAbsent !== true ||
      discoveryEvidence.selectionContract !== "private-zone-url" ||
      discoveryEvidence.browserRequestCount < 2 ||
      discoveryEvidence.publicDefault?.currentArena !== "chaos-zone" ||
      discoveryEvidence.publicDefault?.storedArena !== "chaos-zone" ||
      discoveryEvidence.publicDefault?.receivedJoinGameCount < 1 ||
      !discoveryEvidence.publicDefault?.receivedMapFilenames?.includes("bdhadar.lvl") ||
      /[?&](?:arena|zone)=/.test(discoveryEvidence.publicDefault?.currentUrl) ||
      !discoveryEvidence.catalog?.some(
        (zone) =>
          zone.id === "test-zone" &&
          zone.display_name === "Test Zone" &&
          zone.map === "test.lvl",
      ) ||
      ![discoveryEvidence.selections?.alpha, discoveryEvidence.selections?.beta].every(
        selection => selection?.selectedArena === "test-zone" && selection.input === "mouse",
      )
    ) {
      throw new Error(
        `temporary zone did not prove server discovery plus private URL and real Play input: ${JSON.stringify(discoveryEvidence)}`,
      );
    }
    if (![temporary.alpha, temporary.beta].some((client) =>
      (client.uiMetrics?.dynamicLvzChatSoundSources || 0) > 0 &&
      (client.uiMetrics?.dynamicLvzChatSoundsLoaded || 0) > 0
    )) {
      throw new Error(`temporary zone did not install post-build LVZ chat audio: ${JSON.stringify({
        alpha: temporary.alpha.uiMetrics,
        beta: temporary.beta.uiMetrics,
      })}`);
    }
    const channelAudits = {
      chaos: assertChannelAudit("Chaos disk bundle", chaos),
      extreme: assertChannelAudit("Extreme disk bundle", extreme),
      temporary: assertChannelAudit("temporary disk bundle", temporary),
    };
    const weaponCarrierEvidence = {
      chaos: assertWeaponCarrierEvidence("Chaos disk bundle", chaos),
      extreme: assertWeaponCarrierEvidence("Extreme disk bundle", extreme),
      temporary: assertWeaponCarrierEvidence("temporary disk bundle", temporary),
    };
    const projectileLifecycleEvidence = assertProjectileLifecycleEvidence(
      "Chaos disk bundle",
      chaos,
    );
    const queueDrainEvidence = {
      chaos: assertQueueDrainEvidence("Chaos disk bundle", chaos),
      extreme: assertQueueDrainEvidence("Extreme disk bundle", extreme),
      temporary: assertQueueDrainEvidence("temporary disk bundle", temporary),
    };
    const cacheRejoin = temporary.cacheRejoinEvidence || {};
    if (
      !["alpha", "beta"].includes(cacheRejoin.clientName) ||
      (cacheRejoin.cacheStats?.hits || 0) < 1 ||
      !cacheRejoin.sentReliableTypes?.includes(0x7c) ||
      (cacheRejoin.downloadCacheStats?.writes || 0) < 1 ||
      !cacheRejoin.initialSentReliableTypes?.includes(0x0c)
    ) {
      throw new Error(`temporary zone did not prove request/download/cache-check paths: ${JSON.stringify({
        cacheRejoin,
      })}`);
    }

    const finalProvenance = exactTreeProvenance(runStartedAt);
    assertArtifactProvenanceMatches(initialProvenance, finalProvenance);
    fs.writeFileSync(
      path.join(artifactDirectory.temporary, "provenance.json"),
      `${JSON.stringify(finalProvenance, null, 2)}\n`,
    );
    const result = {
      ok: true,
      formatVersion: 2,
      runStartedAt,
      runEndedAt: new Date().toISOString(),
      browser: "chrome",
      transport: "rtc",
      zones: {
        chaos,
        extreme,
        temporary,
      },
      temporaryBundle: {
        retainedPath: "temporary-zone-root/test-zone",
        manifestSha256: temporaryBundle.manifestSha256,
        mapSha256: temporaryBundle.mapSha256,
        settingsSha256: temporaryBundle.settingsSha256,
        dynamicLvzSha256: temporaryBundle.dynamicLvzSha256,
        chatSoundSha256: temporaryBundle.chatSoundSha256,
      },
      cacheEvidence: {
        downloader: cacheRejoin.clientName,
        firstDownload: cacheRejoin.downloadCacheStats,
        returningCacheHit: cacheRejoin.cacheStats,
        chaosFreshDownloads: {
          alpha: chaos.alpha.mapCacheStats,
          beta: chaos.beta.mapCacheStats,
        },
        extremeFreshDownloads: {
          alpha: extreme.alpha.mapCacheStats,
          beta: extreme.beta.mapCacheStats,
        },
        adversarialPolicy: temporary.alpha.mapCachePolicyEvidence,
        mapRequestType: 0x0c,
        cachedMapCheckType: 0x7c,
      },
      zoneDiscoveryEvidence: discoveryEvidence,
      channelAudits,
      weaponCarrierEvidence,
      projectileLifecycleEvidence,
      disabledObjectiveEvidence,
      queueDrainEvidence,
      securityAudit: {
        chaosRejects: chaos.serverSecurityRejects,
        extremeRejects: extreme.serverSecurityRejects,
        temporaryRejects: temporary.serverSecurityRejects,
      },
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
    console.log(JSON.stringify({
      ok: true,
      artifactDir: published,
      cacheEvidence: result.cacheEvidence,
      channelAudits,
      disabledObjectiveEvidence,
    }, null, 2));
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

main();

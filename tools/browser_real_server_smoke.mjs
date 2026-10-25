#!/usr/bin/env node
import { acknowledgeControlsWelcome } from "./browser_controls_welcome.mjs";
import { fetchGameStatusResponse } from "./game_status.mjs";
import { dispatchChromeKey, keyEvent } from "../tests/e2e/combat_replay.mjs";
import { isDeepStrictEqual } from "node:util";

import fs from "node:fs";
import { hudLineCountMatchesMode } from "./smoke_ui_contract.mjs";
import { prepareSmokeWeaponEncounter } from "../tests/e2e/smoke_encounter_preparation.mjs";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import zlib from "node:zlib";
import { assertCleanGateEnvironment, assertMachineHeadroom, browserLaunchEnvironment, killProcessGroup, registerGateProcess } from "./e2e_gate_hygiene.mjs";

import {
  isExactPlayerEnteringPayload,
  cancelledProjectileAfterRosterReplacement,
  moveOutOfSafeZoneForWeaponProof,
  mutualRosterConverged,
  orderedZoneFilesSettledForLifecycle,
  selfJoinBootstrapSettled,
  shipResetLifecycleSettled,
  lifecycleBaselineWireEvidence,
  shipResetBefore,
  initialSpawnLifecycleEvidence,
  assertInitialSpawnPositionStream,
} from "./dynamic_zone_weapon_schedule.mjs";
import {
  assertRtcPressureClean,
  assertRtcServerLogClean,
} from "./rtc_gate_audit.mjs";
import { parseRealtimeWeaponEvent, parseRealtimeOwnerWeaponCancellation } from "./realtime_event_evidence.mjs";
import { createSmokeQaRecorder } from "../tests/e2e/smoke_qa_artifacts.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const chromeBin = process.env.CHROME_BIN || "google-chrome";
const applicationRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const wasmPath = process.env.ZUBSPACE_E2E_STATIC_ROOT
  ? path.relative(repoRoot, path.join(applicationRoot, "zubspace.wasm"))
  : "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const serverBin = "target/debug/zubspace_server";
const smokeArenaSetting = process.env.ZUBSPACE_SMOKE_ARENA || "chaos-zone";
const smokeArenaSource = process.env.ZUBSPACE_SMOKE_ARENA_SOURCE || "query";
const smokeTickRate = process.env.ZUBSPACE_SMOKE_TICK_RATE || "60";
const smokeBotCount = process.env.ZUBSPACE_SMOKE_BOT_COUNT || "";
const smokeLogLevel = process.env.ZUBSPACE_SMOKE_LOG_LEVEL || "debug";
const smokeZoneRoot = process.env.ZUBSPACE_SMOKE_ZONE_ROOT || "";
const smokeAllowZoneOverrides = process.env.ZUBSPACE_SMOKE_ALLOW_ZONE_OVERRIDES === "1";
const stopAfterRemoteWeaponSmoke = process.env.ZUBSPACE_SMOKE_STOP_AFTER_REMOTE_WEAPON === "1";
const stopAfterMapSmoke = process.env.ZUBSPACE_SMOKE_STOP_AFTER_MAP === "1";
const stopAfterObjectiveAuthoritySmoke =
  process.env.ZUBSPACE_SMOKE_STOP_AFTER_OBJECTIVE_AUTHORITY === "1";
const stopAfterPositionSmoke = process.env.ZUBSPACE_SMOKE_STOP_AFTER_POSITION === "1";
const requireMapCacheRejoin = process.env.ZUBSPACE_SMOKE_REQUIRE_MAP_CACHE_REJOIN === "1";
const requireThreeChannelTraffic =
  process.env.ZUBSPACE_SMOKE_REQUIRE_THREE_CHANNEL_TRAFFIC === "1";
const requireProjectileLifecycleIdentity =
  process.env.ZUBSPACE_SMOKE_REQUIRE_PROJECTILE_LIFECYCLE_IDENTITY === "1";
const forceFreshMapCache = process.env.ZUBSPACE_SMOKE_FORCE_FRESH_MAP_CACHE === "1";
const requireDynamicLvzChatSound =
  process.env.ZUBSPACE_SMOKE_REQUIRE_DYNAMIC_LVZ_CHAT_SOUND === "1";
const qaArtifactDirectory = process.env.ZUBSPACE_SMOKE_QA_ARTIFACT_DIR || null;
const qaArtifactLabel = process.env.ZUBSPACE_SMOKE_QA_LABEL || "smoke";
const positionSampleMs = Number(process.env.ZUBSPACE_SMOKE_POSITION_SAMPLE_MS || "3000");
const zonePublicationTimeoutMs = Number(
  process.env.ZUBSPACE_SMOKE_ZONE_PUBLICATION_TIMEOUT_MS || "180000",
);
const serverBotIdBase = 0x0300;
const expectedMapByArena = new Map([
  ["chaos-zone", "bdhadar.lvl"],
  ["Chaos Zone", "bdhadar.lvl"],
  ["extreme", "doomsy8.lvl"],
  ["Extreme", "doomsy8.lvl"],
]);
const expectedStoredArenaByArena = new Map([
  ["chaos-zone", "chaos-zone"],
  ["Chaos Zone", "chaos-zone"],
  ["extreme", "extreme"],
  ["Extreme", "extreme"],
]);
const expectedSettingsByArena = new Map([
  ["chaos-zone", { bulletDamage: 200000, bombDamage: 750000, bulletSpeed: 2000, initialSpeed: 2010, maxFrequency: 10000, xradarWeight: 40, antiwarpWeight: 25, multiprizeWeight: 130, portalWeight: 60, shipRadiusWire: [14, 14, 14, 14, 14, 14, 14, 14] }],
  ["Chaos Zone", { bulletDamage: 200000, bombDamage: 750000, bulletSpeed: 2000, initialSpeed: 2010, maxFrequency: 10000, xradarWeight: 40, antiwarpWeight: 25, multiprizeWeight: 130, portalWeight: 60, shipRadiusWire: [14, 14, 14, 14, 14, 14, 14, 14] }],
  ["extreme", { bulletDamage: 210000, bombDamage: 5600000, bulletSpeed: 4100, initialSpeed: 3200, maxFrequency: 10000, xradarWeight: 23, antiwarpWeight: 10, multiprizeWeight: 5, portalWeight: 0, shipRadiusWire: [0, 0, 0, 0, 0, 0, 0, 0] }],
  ["Extreme", { bulletDamage: 210000, bombDamage: 5600000, bulletSpeed: 4100, initialSpeed: 3200, maxFrequency: 10000, xradarWeight: 23, antiwarpWeight: 10, multiprizeWeight: 5, portalWeight: 0, shipRadiusWire: [0, 0, 0, 0, 0, 0, 0, 0] }],
]);
const lvzChatSoundSmokeArenas = new Set(["chaos-zone", "Chaos Zone"]);
const remoteWeaponSmokeArenas =
  process.env.ZUBSPACE_SMOKE_REMOTE_WEAPONS === "1"
    ? new Set(["chaos-zone", "Chaos Zone"])
    : new Set();

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function mimeType(filename) {
  if (filename.endsWith(".html")) return "text/html; charset=utf-8";
  if (filename.endsWith(".js")) return "text/javascript; charset=utf-8";
  if (filename.endsWith(".wasm")) return "application/wasm";
  if (filename.endsWith(".png")) return "image/png";
  if (filename.endsWith(".wav") || filename.endsWith(".wa2")) return "audio/wav";
  return "application/octet-stream";
}

function paethPredictor(left, up, upperLeft) {
  const p = left + up - upperLeft;
  const pa = Math.abs(p - left);
  const pb = Math.abs(p - up);
  const pc = Math.abs(p - upperLeft);
  if (pa <= pb && pa <= pc) return left;
  if (pb <= pc) return up;
  return upperLeft;
}

function pngNonBlackMetrics(png) {
  const signature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  if (!png.subarray(0, signature.length).equals(signature)) {
    throw new Error("screenshot was not a PNG");
  }

  let offset = signature.length;
  let width = 0;
  let height = 0;
  let bitDepth = 0;
  let colorType = 0;
  let interlace = 0;
  const idat = [];

  while (offset + 12 <= png.length) {
    const length = png.readUInt32BE(offset);
    const type = png.subarray(offset + 4, offset + 8).toString("ascii");
    const data = png.subarray(offset + 8, offset + 8 + length);
    offset += 12 + length;

    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
      interlace = data[12];
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
  }

  if (bitDepth !== 8 || interlace !== 0 || ![2, 6].includes(colorType)) {
    throw new Error(
      `unsupported screenshot PNG layout: bitDepth=${bitDepth} colorType=${colorType} interlace=${interlace}`,
    );
  }

  const channels = colorType === 6 ? 4 : 3;
  const stride = width * channels;
  const inflated = zlib.inflateSync(Buffer.concat(idat));
  let sourceOffset = 0;
  let previous = Buffer.alloc(stride);
  let nonBlackPixels = 0;
  let sampledPixels = 0;

  for (let y = 0; y < height; y += 1) {
    const filter = inflated[sourceOffset];
    sourceOffset += 1;
    const row = Buffer.from(inflated.subarray(sourceOffset, sourceOffset + stride));
    sourceOffset += stride;

    for (let x = 0; x < stride; x += 1) {
      const left = x >= channels ? row[x - channels] : 0;
      const up = previous[x];
      const upperLeft = x >= channels ? previous[x - channels] : 0;
      if (filter === 1) row[x] = (row[x] + left) & 0xff;
      else if (filter === 2) row[x] = (row[x] + up) & 0xff;
      else if (filter === 3) row[x] = (row[x] + Math.floor((left + up) / 2)) & 0xff;
      else if (filter === 4) row[x] = (row[x] + paethPredictor(left, up, upperLeft)) & 0xff;
      else if (filter !== 0) throw new Error(`unsupported PNG row filter ${filter}`);
    }

    for (let x = 0; x < width; x += 1) {
      const pixel = x * channels;
      const alpha = channels === 4 ? row[pixel + 3] : 255;
      if (alpha > 0 && row[pixel] + row[pixel + 1] + row[pixel + 2] > 10) {
        nonBlackPixels += 1;
      }
      sampledPixels += 1;
    }
    previous = row;
  }

  return { nonBlackPixels, sampledPixels };
}

function fixedCString(buffer) {
  const zero = buffer.indexOf(0);
  return buffer.subarray(0, zero < 0 ? buffer.length : zero).toString("utf8");
}

function isReliable(packet) {
  return packet.length >= 6 && packet[0] === 0x00 && packet[1] === 0x03;
}

function reliablePayload(packet) {
  return isReliable(packet) ? packet.subarray(6) : null;
}

function reliableMessageId(packet) {
  return isReliable(packet) ? packet.readUInt32LE(2) : null;
}

const POSITION_SEQUENCE_MAGIC = 0x5a5a0000;
const POSITION_SEQUENCE_MASK = 0xffff0000;

function clientPositionSequence(packet) {
  if (!packet || packet.length < 32) {
    return null;
  }
  const items = packet.readUInt32LE(28);
  return (items & POSITION_SEQUENCE_MASK) === POSITION_SEQUENCE_MAGIC ? items & 0xffff : null;
}

function latestClientPositionSequence(packets) {
  let latest = null;
  for (const packet of packets) {
    if (packet[0] !== 0x03 || packet.length < 32) {
      continue;
    }
    const sequence = clientPositionSequence(packet);
    if (sequence !== null) {
      latest = latest === null ? sequence : Math.max(latest, sequence);
    }
  }
  return latest;
}

function clientPositionPacketWithSequence(packet, sequence) {
  const source = Buffer.from(packet);
  const output = Buffer.alloc(Math.max(source.length, 32));
  source.copy(output);
  output.writeUInt32LE(0, 2);
  output.writeUInt32LE((POSITION_SEQUENCE_MAGIC | (sequence & 0xffff)) >>> 0, 28);
  return output;
}

function parseRoster(payload) {
  if (!isExactPlayerEnteringPayload(payload)) return null;
  return {
    id: payload.readUInt16LE(51),
    name: fixedCString(payload.subarray(3, 23)),
    ship: payload[1],
    frequency: payload.readUInt16LE(53),
    wins: payload.readUInt16LE(55),
    losses: payload.readUInt16LE(57),
  };
}

function parseShipFrequencyChange(payload) {
  if (!payload || payload[0] !== 0x1d || payload.length < 6) return null;
  return {
    id: payload.readUInt16LE(2),
    ship: payload[1],
    frequency: payload.readUInt16LE(4),
  };
}

function parseLoginName(payload) {
  if (!payload || payload[0] !== 0x24 || payload.length < 34) return null;
  return fixedCString(payload.subarray(2, 34));
}

function parseChatRequest(payload) {
  if (!payload || payload[0] !== 0x06 || payload.length < 6) return null;
  return {
    chatType: payload[1],
    sound: payload[2],
    target: payload.readUInt16LE(3),
    message: fixedCString(payload.subarray(5)),
  };
}

function encodeChatRequest({ chatType, sound = 0, target = 0, message = "" }) {
  const messageBytes = Buffer.from(message, "utf8");
  const packet = Buffer.alloc(6 + messageBytes.length);
  packet[0] = 0x06;
  packet[1] = chatType;
  packet[2] = sound;
  packet.writeUInt16LE(target, 3);
  messageBytes.copy(packet, 5);
  packet[5 + messageBytes.length] = 0;
  return packet;
}

function parseChatPacket(payload) {
  if (!payload || payload[0] !== 0x07 || payload.length < 6) return null;
  return {
    chatType: payload[1],
    sound: payload[2],
    senderId: payload.readUInt16LE(3),
    message: fixedCString(payload.subarray(5)),
  };
}

function parseSetCoordinates(payload) {
  if (!payload || payload[0] !== 0x32 || payload.length < 5) return null;
  return {
    x: payload.readUInt16LE(1),
    y: payload.readUInt16LE(3),
  };
}

function parseMapInformation(payload) {
  if (!payload || payload[0] !== 0x29 || payload.length < 25) return null;
  if (payload.subarray(1, 5).equals(Buffer.from("ZMAP"))) {
    if (payload.length < 80 || payload[5] !== 1) return null;
    return {
      zoneId: fixedCString(payload.subarray(6, 22)),
      filename: fixedCString(payload.subarray(22, 38)),
      checksum: payload.readUInt32LE(38),
      compressedSize: payload.readUInt32LE(42),
      sha256: payload.subarray(46, 78).toString("hex"),
    };
  }
  return {
    filename: fixedCString(payload.subarray(1, 17)),
    checksum: payload.readUInt32LE(17),
    compressedSize: payload.readUInt32LE(21),
  };
}

function logicalReliablePayloads(packets) {
  const payloads = [];
  const smallChunks = [];
  const seenReliableIds = new Set();
  let documentOrigin = null;
  for (const packet of packets) {
    const origin = packet.zubspaceDetail?.documentOrigin ?? 0;
    if (origin !== documentOrigin) {
      // Reliable IDs and fragment assembly belong to one page/session.
      smallChunks.length = 0;
      documentOrigin = origin;
    }
    const payload = reliablePayload(packet);
    if (!payload) continue;
    const reliableId = reliableMessageId(packet);
    const scopedId = `${origin}:${reliableId}`;
    if (seenReliableIds.has(scopedId)) {
      continue;
    }
    seenReliableIds.add(scopedId);
    if (payload.length >= 2 && payload[0] === 0x00 && payload[1] === 0x08) {
      smallChunks.push(payload.subarray(2));
      continue;
    }
    if (payload.length >= 2 && payload[0] === 0x00 && payload[1] === 0x09) {
      smallChunks.push(payload.subarray(2));
      payloads.push(Buffer.concat(smallChunks));
      smallChunks.length = 0;
      continue;
    }
    payloads.push(payload);
  }
  return payloads;
}

function parseReceivedPositionUpdates(packet) {
  const at = packet.zubspaceDetail?.at ?? null;
  const channel = packet.zubspaceDetail?.channel ?? null;
  const realtimeWeapon = parseRealtimeWeaponEvent(packet);
  if (realtimeWeapon) {
    return [{
      ...realtimeWeapon,
      timestamp: realtimeWeapon.serverTick & 0xffff,
      timestampMask: 0xffff,
      channel,
      at,
    }];
  }
  if (packet[0] === 0x05 && packet.length >= 23) {
    return [
      {
        source: packet.readUInt16LE(8),
        timestamp: packet.readUInt16LE(2),
        timestampMask: 0xffff,
        x: packet.readUInt16LE(4),
        y: packet.readUInt16LE(15),
        velocityX: packet.readInt16LE(10),
        velocityY: packet.readInt16LE(6),
        weapon: packet.readUInt16LE(19),
        channel,
        at,
      },
    ];
  }
  if (packet[0] === 0x39 && packet.length >= 11 && (packet.length - 1) % 10 === 0) {
    return parseBatchedPositionUpdates(packet, 10, false, at);
  }
  if (packet[0] === 0x3a && packet.length >= 12 && (packet.length - 1) % 11 === 0) {
    return parseBatchedPositionUpdates(packet, 11, true, at);
  }
  if (packet[0] === 0x70 && packet.length >= 21) {
    return parseSnapshotPositionUpdates(packet, at, channel);
  }
  return [];
}

function compactShipResetMetrics(metrics) {
  const lifecycle = metrics?.localGameplayState || {};
  return {
    lifecycleWireEvidence: metrics?.lifecycleWireEvidence ?? null,
    playerLifeSeq: metrics?.playerLifeSeq ?? null,
    playerX: metrics?.playerX ?? null,
    playerY: metrics?.playerY ?? null,
    playerVelocityX: metrics?.playerVelocityX ?? null,
    playerVelocityY: metrics?.playerVelocityY ?? null,
    playerAlive: metrics?.playerAlive ?? null,
    serverPlayerAlive: metrics?.serverPlayerAlive ?? null,
    authorityPhase: lifecycle.authorityPhase ?? null,
    authorityTransitionKind: lifecycle.authorityTransitionKind ?? null,
    authorityApplicationBarrierKind: lifecycle.authorityApplicationBarrierKind ?? null,
    authorityApplicationBarrierCount: lifecycle.authorityApplicationBarrierCount ?? null,
    pendingShipResetPosition: lifecycle.pendingShipResetPosition ?? null,
    pendingShipResetDeliveryId: lifecycle.pendingShipResetDeliveryId ?? null,
    pendingShipResetLifeSeq: lifecycle.pendingShipResetLifeSeq ?? null,
    pendingShipResetSequence: lifecycle.pendingShipResetSequence ?? null,
    pendingFrequencyBaselineDeliveryId:
      lifecycle.pendingFrequencyBaselineDeliveryId ?? null,
    lifecycleBarrierElapsed: lifecycle.lifecycleBarrierElapsed ?? null,
    lifecycleBaselineReceiptsSeen: lifecycle.lifecycleBaselineReceiptsSeen ?? null,
    lifecycleBaselineReceiptsMatched: lifecycle.lifecycleBaselineReceiptsMatched ?? null,
    lifecycleBaselineReceiptsUnmatched: lifecycle.lifecycleBaselineReceiptsUnmatched ?? null,
    authorityLastTransitionSource: lifecycle.authorityLastTransitionSource ?? null,
    authorityLastTransitionFact: lifecycle.authorityLastTransitionFact ?? null,
    authorityLastTransitionDisposition: lifecycle.authorityLastTransitionDisposition ?? null,
    authorityLastTransitionResulting: lifecycle.authorityLastTransitionResulting ?? null,
  };
}

// Phase 4 legacy retirement: the snapshot stream (`0x70`) replaced
// the legacy `0x05` / `0x3a` LARGE_POSITION + batched fan-out paths.
// The smoke watches `receivedPositionSources.includes(peer.selfId)`
// to confirm two clients exchange position state; without a snapshot
// parser the assertion times out on the production-default path.
// Wire format mirrors `src/bin/zubspace_server/snapshot.rs::build_snapshot`:
//   byte 0       : 0x70
//   byte 1       : flags (bit 0 = FULL)
//   bytes 2-3    : u16 snapshot_id (LE)
//   bytes 4-5    : u16 base_id (LE)
//   bytes 6-9    : u32 tick (LE)
//   bytes 10-17  : u64 arena_stream_id (LE)
//   byte 18      : u8  near_count
//   byte 19      : u8  mid_count
//   byte 20      : u8  event_count
//   then near_count + mid_count entity entries:
//     u16 entity_id, u16 field_mask, optional fields per mask bit
//   then event_count event entries (variable; we skip past them).
const SNAPSHOT_FIELD_LEN = {
  0x0001: 2,   // X u16
  0x0002: 2,   // Y u16
  0x0004: 2,   // VELOCITY_X i16
  0x0008: 2,   // VELOCITY_Y i16
  0x0010: 1,   // DIRECTION u8
  0x0020: 2,   // ENERGY u16
  0x0040: 2,   // WEAPON u16
  0x0080: 1,   // STATUS u8
  0x0100: 2,   // BOUNTY u16
  0x0200: 1,   // PING u8
  0x0400: 2,   // FLAG_TIMER u16
  0x0800: 4,   // LIFE_SEQ u32
  0x1000: 5,   // SAMPLE_TIME u32 tick + u8 thirds
  0x2000: 4,   // MOTION_SEGMENT u32
};

function parseSnapshotPositionUpdates(packet, at, channel) {
  if (packet.length < 21) return [];
  const tick = packet.readUInt32LE(6);
  const snapshotId = packet.readUInt16LE(2);
  const nearCount = packet[18];
  const midCount = packet[19];
  // packet[20] is event_count — the smoke ignores the event region
  // since `parseReceivedPositionUpdates` only cares about position
  // sources, not WATCH_DAMAGE / other event payloads.
  const updates = [];
  let cursor = 21;
  const total = nearCount + midCount;
  for (let i = 0; i < total; i++) {
    if (cursor + 4 > packet.length) return updates;
    const entityId = packet.readUInt16LE(cursor);
    const mask = packet.readUInt16LE(cursor + 2);
    cursor += 4;
    let x = 0;
    let y = 0;
    let velocityX = 0;
    let velocityY = 0;
    let weapon = 0;
    for (let bit = 1; bit <= 0x4000; bit <<= 1) {
      if ((mask & bit) === 0) continue;
      let len = bit === 0x2000 && (packet[1] & 0x08) !== 0 ? 8 : SNAPSHOT_FIELD_LEN[bit];
      // Bit 0x4000: x and y are signed one-byte offsets from the acknowledged
      // base. Without base tracking they are reported as offsets, which still
      // change frame to frame while a ship moves.
      const offsets = (mask & 0x4000) !== 0;
      if (offsets && (bit === 0x0001 || bit === 0x0002)) len = 1;
      if (bit === 0x4000) continue;
      // Compact frames (flag 0x10): one byte, or 0xff then the five-byte form.
      if (bit === 0x1000 && (packet[1] & 0x10) !== 0) len = packet[cursor] === 0xff ? 6 : 1;
      if (len == null || cursor + len > packet.length) return updates;
      switch (bit) {
        case 0x0001: x = offsets ? packet.readInt8(cursor) : packet.readUInt16LE(cursor); break;
        case 0x0002: y = offsets ? packet.readInt8(cursor) : packet.readUInt16LE(cursor); break;
        case 0x0004: velocityX = packet.readInt16LE(cursor); break;
        case 0x0008: velocityY = packet.readInt16LE(cursor); break;
        case 0x0040: weapon = packet.readUInt16LE(cursor); break;
        default: break;
      }
      cursor += len;
    }
    updates.push({
      source: entityId,
      timestamp: tick & 0xffff,
      timestampMask: 0xffff,
      x,
      y,
      velocityX,
      velocityY,
      weapon,
      snapshotId,
      channel,
      at,
    });
  }
  return updates;
}

function parseBatchedPositionUpdates(packet, entryLen, includesTogglables, at) {
  const updates = [];
  for (let offset = 1; offset < packet.length; offset += entryLen) {
    let source;
    let cursor;
    if (includesTogglables) {
      source = packet.readUInt16LE(offset) & 0x03ff;
      cursor = offset + 2;
    } else {
      source = packet[offset];
      cursor = offset + 1;
    }
    const packedHeading = packet.readUInt16LE(cursor);
    const packedPos = packet.readUInt32LE(cursor + 2);
    const packedVelocity = packet.readUInt16LE(cursor + 6);
    const velocityY = signExtend(packedVelocity & 0x3fff, 14);
    const multiplier = packet.readInt8(cursor + 8);
    const velocityX = (((packedVelocity >> 14) + multiplier * 4) * 16) + (packedPos >>> 28);
    updates.push({
      source,
      timestamp: packedHeading & 0x03ff,
      timestampMask: 0x03ff,
      x: packedPos & 0x3fff,
      y: (packedPos >>> 14) & 0x3fff,
      velocityX,
      velocityY,
      weapon: 0,
      at,
    });
  }
  return updates;
}

function signExtend(value, bits) {
  const sign = 1 << (bits - 1);
  return (value & sign) ? value - (1 << bits) : value;
}

function parseReceivedPositionSources(packet) {
  return parseReceivedPositionUpdates(packet).map((update) => update.source);
}

function parseArenaSettingsFingerprint(packet) {
  if (packet[0] !== 0x0f || packet.length !== 1428) return null;
  const shipOffset = 4;
  const globalOffset = 4 + 8 * 144;
  const prizeWeightOffset = globalOffset + 20 * 4 + 4 * 4 + 58 * 2 + 32;
  return {
    packetLength: packet.length,
    bulletDamage: packet.readInt32LE(globalOffset),
    bombDamage: packet.readInt32LE(globalOffset + 4),
    bulletSpeed: packet.readUInt16LE(shipOffset + 24),
    initialSpeed: packet.readUInt16LE(shipOffset + 54),
    maxFrequency: packet.readInt32LE(globalOffset + 28),
    xradarWeight: packet[prizeWeightOffset + 5],
    antiwarpWeight: packet[prizeWeightOffset + 19],
    multiprizeWeight: packet[prizeWeightOffset + 24],
    portalWeight: packet[prizeWeightOffset + 27],
    shipRadiusWire: Array.from({ length: 8 }, (_, ship) => {
      const visibility = packet.readUInt16LE(shipOffset + ship * 144 + 28);
      return (visibility >> 3) & 0xff;
    }),
  };
}

function summarizeFrames(frames) {
  const sentPayloads = logicalReliablePayloads(frames.sent);
  const receivedPayloads = logicalReliablePayloads(frames.received);
  const rosters = receivedPayloads.map(parseRoster).filter(Boolean);
  const shipChanges = receivedPayloads.map(parseShipFrequencyChange).filter(Boolean);
  const setCoordinates = receivedPayloads.map(parseSetCoordinates).filter(Boolean);
  const settingsProfiles = receivedPayloads.map(parseArenaSettingsFingerprint).filter(Boolean);
  const playerIds = receivedPayloads
    .filter((payload) => payload[0] === 0x01 && payload.length === 3)
    .map((payload) => payload.readUInt16LE(1));
  const receivedPositionUpdates = frames.received.flatMap(parseReceivedPositionUpdates);
  const receivedPositionSources = frames.received.flatMap(parseReceivedPositionSources);
  const receivedPositionWeapons = frames.received
    .flatMap(parseReceivedPositionUpdates)
    .filter((update) => update.weapon !== 0)
    .map((update) => ({
      source: update.source,
      weapon: update.weapon,
      eventId: update.eventId ?? null,
      serverTick: update.serverTick ?? null,
      snapshotId: update.snapshotId ?? null,
      x: update.x,
      y: update.y,
      velocityX: update.velocityX,
      velocityY: update.velocityY,
      arenaStreamId: update.arenaStreamId ?? null,
      sourceSessionId: update.sourceSessionId ?? null,
      sourceLifeSeq: update.sourceLifeSeq ?? null,
      shotGroupId: update.shotGroupId ?? null,
      orientation: update.orientation ?? null,
      frequency: update.frequency ?? null,
      ship: update.ship ?? null,
      channel: update.channel ?? null,
    }));
  const sentPositionWeapons = frames.sent
    .filter((packet) => packet[0] === 0x03 && packet.length >= 22)
    .map((packet) => ({
      weapon: packet.readUInt16LE(20),
      channel: packet.zubspaceDetail?.channel ?? null,
    }));
  const sentPositions = frames.sent
    .filter((packet) => packet[0] === 0x03 && packet.length >= 22)
    .map((packet) => ({
      at: packet.zubspaceDetail?.at ?? null,
      timestamp: packet.readUInt32LE(2),
      x: packet.readUInt16LE(12),
      y: packet.readUInt16LE(8),
      velocityX: packet.readInt16LE(6),
      velocityY: packet.readInt16LE(14),
      weapon: packet.readUInt16LE(20),
      channel: packet.zubspaceDetail?.channel ?? null,
    }));

  return {
    selfId: playerIds.at(-1) ?? null,
    sentReliableTypes: sentPayloads.map((payload) => payload[0]),
    receivedReliableTypes: receivedPayloads.map((payload) => payload[0]),
    sentChatRequests: sentPayloads.map(parseChatRequest).filter(Boolean),
    receivedChatPackets: receivedPayloads.map(parseChatPacket).filter(Boolean),
    sentLoginNames: sentPayloads.map(parseLoginName).filter(Boolean),
    sentLoginCount: sentPayloads.filter((payload) => payload[0] === 0x24).length,
    sentArenaLoginCount: sentPayloads.filter((payload) => payload[0] === 0x01).length,
    sentPositionCount: frames.sent.filter((packet) => packet[0] === 0x03 && packet.length >= 22)
      .length,
    receivedPasswordAccepts: receivedPayloads.filter((payload) => payload[0] === 0x0a).length,
    receivedJoinGameCount: receivedPayloads.filter((payload) => payload[0] === 0x02).length,
    receivedMapInfoCount: receivedPayloads.filter((payload) => payload[0] === 0x29).length,
    receivedMapFilenames: receivedPayloads.map(parseMapInformation).filter(Boolean).map((info) => info.filename),
    receivedFileTransferCount: receivedPayloads.filter((payload) => payload[0] === 0x2a).length,
    rosters,
    shipChanges,
    setCoordinates,
    settingsProfiles,
    rosterNames: rosters.map((roster) => roster.name),
    receivedPositionUpdates,
    receivedPositionSources,
    receivedPositionWeapons,
    sentPositionWeapons,
    sentPositions,
  };
}

function nonzeroPositionWeapons(summary) {
  // The browser renders only remote weapon facts. Keep the transport proof
  // aligned with that consumer so a server echo of the local player's shot
  // can never satisfy the cross-channel delivery assertion.
  return summary.receivedPositionWeapons.filter(
    (entry) => entry.weapon !== 0 && entry.source !== summary.selfId,
  );
}

function positionTrafficSummary(summary, elapsedMs) {
  const elapsedSeconds = Math.max(elapsedMs / 1000, 0.001);
  return {
    sent: summary.sentPositionCount,
    received: summary.receivedPositionSources.length,
    sentPerSecond: Number((summary.sentPositionCount / elapsedSeconds).toFixed(1)),
    receivedPerSecond: Number((summary.receivedPositionSources.length / elapsedSeconds).toFixed(1)),
    receivedTimestampGaps: positionTimestampGapSummary(summary),
  };
}

function percentile(sortedValues, ratio) {
  if (sortedValues.length === 0) return null;
  const index = Math.min(
    sortedValues.length - 1,
    Math.max(0, Math.round((sortedValues.length - 1) * ratio)),
  );
  return sortedValues[index];
}

function positionTimestampGapSummary(summary) {
  const lastBySource = new Map();
  const gapsMs = [];
  let largeGapCount = 0;
  for (const update of summary.receivedPositionUpdates) {
    const last = lastBySource.get(update.source);
    if (last && last.timestampMask === update.timestampMask) {
      const mask = update.timestampMask;
      const delta = (update.timestamp - last.timestamp) & mask;
      const halfRange = (mask + 1) / 2;
      if (delta > 0 && delta < halfRange) {
        const gapMs = delta * 10;
        if (gapMs > 250) {
          largeGapCount += 1;
        } else {
          gapsMs.push(gapMs);
        }
      }
    }
    lastBySource.set(update.source, {
      ...update,
      timestamp: update.timestamp & update.timestampMask,
    });
  }
  gapsMs.sort((a, b) => a - b);
  const average = gapsMs.length
    ? gapsMs.reduce((sum, value) => sum + value, 0) / gapsMs.length
    : null;
  return {
    count: gapsMs.length,
    avgMs: average === null ? null : Number(average.toFixed(1)),
    p50Ms: percentile(gapsMs, 0.5),
    p95Ms: percentile(gapsMs, 0.95),
    maxMs: gapsMs.at(-1) ?? null,
    largeGapsOver250Ms: largeGapCount,
  };
}

function compactPositionSources(sources) {
  const counts = {};
  for (const source of sources) {
    counts[source] = (counts[source] || 0) + 1;
  }
  return {
    count: sources.length,
    counts,
    sample: sources.slice(0, 24),
  };
}

function compactWeaponEntries(entries) {
  const counts = {};
  const channels = {};
  for (const entry of entries) {
    counts[entry.weapon] = (counts[entry.weapon] || 0) + 1;
    if (entry.channel) {
      channels[entry.channel] = (channels[entry.channel] || 0) + 1;
    }
  }
  const deliveryByFact = new Map();
  for (const entry of entries) {
    const key = `${entry.arenaStreamId ?? "legacy"}:${entry.eventId ?? `legacy-${entry.snapshotId}`}:${entry.source}:${entry.weapon}`;
    const fact = deliveryByFact.get(key) || { channels: new Set(), entries: [] };
    if (entry.channel) fact.channels.add(entry.channel);
    fact.entries.push(entry);
    deliveryByFact.set(key, fact);
  }
  const immutableSourceSignature = (entry) => JSON.stringify([
    entry.serverTick,
    entry.arenaStreamId,
    entry.source,
    entry.sourceSessionId,
    entry.sourceLifeSeq,
    entry.shotGroupId,
    entry.x,
    entry.y,
    entry.velocityX,
    entry.velocityY,
    entry.orientation,
    entry.frequency,
    entry.ship,
    entry.weapon,
  ]);
  return {
    count: entries.length,
    counts,
    channels,
    uniqueFacts: deliveryByFact.size,
    dualGameEventFacts: [...deliveryByFact.values()].filter(
      (fact) => fact.channels.has("zubspace-game") && fact.channels.has("zubspace-events"),
    ).length,
    exactlyDualDeliveredFacts: [...deliveryByFact.values()].filter(
      (fact) =>
        fact.entries.length === 2 &&
        fact.channels.has("zubspace-game") &&
        fact.channels.has("zubspace-events"),
    ).length,
    immutableSourceFacts: [...deliveryByFact.values()].filter(
      (fact) =>
        new Set(fact.entries.map(immutableSourceSignature)).size === 1,
    ).length,
    sample: entries.slice(0, 16),
  };
}

function compactFrameSummary(summary) {
  const receivedWeapons = nonzeroPositionWeapons(summary);
  const sentWeapons = summary.sentPositionWeapons.filter((entry) => entry.weapon !== 0);
  return {
    selfId: summary.selfId,
    sentReliableTypes: summary.sentReliableTypes,
    receivedReliableTypes: summary.receivedReliableTypes,
    sentChatRequests: summary.sentChatRequests,
    receivedChatPackets: summary.receivedChatPackets,
    sentLoginNames: summary.sentLoginNames,
    sentLoginCount: summary.sentLoginCount,
    sentArenaLoginCount: summary.sentArenaLoginCount,
    sentPositionCount: summary.sentPositionCount,
    receivedPasswordAccepts: summary.receivedPasswordAccepts,
    receivedJoinGameCount: summary.receivedJoinGameCount,
    receivedMapInfoCount: summary.receivedMapInfoCount,
    receivedMapFilenames: summary.receivedMapFilenames,
    receivedFileTransferCount: summary.receivedFileTransferCount,
    rosters: summary.rosters,
    shipChanges: summary.shipChanges,
    setCoordinates: summary.setCoordinates,
    settingsProfiles: summary.settingsProfiles,
    rosterNames: summary.rosterNames,
    receivedPositionSources: compactPositionSources(summary.receivedPositionSources),
    receivedTimestampGaps: positionTimestampGapSummary(summary),
    receivedPositionWeapons: compactWeaponEntries(receivedWeapons),
    sentPositionWeapons: compactWeaponEntries(sentWeapons),
    sentPositionSamples: summary.sentPositions.slice(0, 8),
  };
}

// Cold evidence only. Retain weapon requests and every authenticated weapon
// event kind, including recovery and cancellation, without copying asset data.
function retainedSmokeWeaponPackets(frames, limit = 4096) {
  const packets = [];
  let total = 0, truncatedPackets = 0;
  for (const direction of ["sent", "received"]) {
    for (const packet of frames[direction]) {
      const weaponEvent = direction === "received" &&
        (packet[0] === 0x7d || (packet[0] === 0 && packet[1] === 3 && packet[6] === 0x7d));
      const truncated = packet.zubspaceDetail?.bytesTruncated === true ||
        (packet.zubspaceDetail?.byteLength ?? packet.length) > packet.length;
      const weaponRequest = direction === "sent" && packet[0] === 3 &&
        (packet.length >= 22 ? packet.readUInt16LE(20) !== 0 : truncated);
      if (!weaponEvent && !weaponRequest) continue;
      total++;
      if (truncated) truncatedPackets++;
      if (packets.length < limit) packets.push({ direction,
        detail: packet.zubspaceDetail || null, byteLength: packet.zubspaceDetail?.byteLength ?? packet.length,
        retainedByteLength: packet.length, bytesTruncated: truncated, bytes: [...packet] });
    }
  }
  return { scope: "C2S nonzero or unclassifiably truncated positions and S2C authenticated weapon events, including reliable cancellations; asset traffic excluded",
    complete: total <= limit && truncatedPackets === 0, truncatedPackets,
    total, omitted: Math.max(0, total - limit), limit, packets };
}

function channelDeliveryAudit(frames) {
  const audit = {};
  for (const direction of ["sent", "received"]) {
    for (const frame of frames[direction]) {
      const channel = frame.zubspaceDetail?.channel || "unknown";
      audit[channel] ||= { sent: 0, received: 0 };
      audit[channel][direction] += 1;
    }
  }
  return audit;
}

function channelContractAudit(pageState) {
  return Object.fromEntries(
    (pageState.networkLog || [])
      .filter((entry) => entry.message === "rtc data channel open")
      .map((entry) => [entry.detail?.label || "unknown", entry.detail || {}]),
  );
}

function assertArenaSettingsProfile(label, summary, smokeArena, expectedSettings) {
  const actualSettings = summary.settingsProfiles.at(-1);
  if (JSON.stringify(actualSettings) !== JSON.stringify({ packetLength: 1428, ...expectedSettings })) {
    throw new Error(
      `${label} did not receive expected ${smokeArena} arena settings: expected=${JSON.stringify(expectedSettings)} actual=${JSON.stringify(actualSettings)}`,
    );
  }
}


function assertCoastingPositionStream(label, positions, releaseAt) {
  const afterRelease = positions.filter(
    (position) =>
      position.weapon === 0 &&
      typeof position.at === "number" &&
      position.at >= releaseAt + 250,
  );
  if (afterRelease.length < 5) {
    throw new Error(
      `${label} did not keep sending movement positions after key release: ${JSON.stringify({
        releaseAt,
        afterRelease: afterRelease.length,
        latest: positions.slice(-8),
      })}`,
    );
  }
  // Motion evidence: count consecutive samples whose (x, y) actually
  // changed. After Phase 4 the snapshot delta stream only carries
  // velocity when it *changes*, so a coasting peer's reported
  // velocity drops out of the delta and reads as zero — but their
  // position keeps advancing. Counting position deltas across
  // consecutive samples works for both legacy LARGE_POSITION (carries
  // velocity reliably) and 0x70 snapshots (carries position reliably).
  const motionEvents = countPositionMotionEvents(afterRelease);
  const uniquePositions = new Set(afterRelease.map((position) => `${position.x},${position.y}`));
  if (motionEvents < 3 || uniquePositions.size < 2) {
    throw new Error(
      `${label} sent post-release positions but they did not show coasting motion: ${JSON.stringify({
        motionEvents,
        uniquePositions: uniquePositions.size,
        sample: afterRelease.slice(0, 12),
      })}`,
    );
  }
}

function countPositionMotionEvents(positions) {
  let events = 0;
  for (let i = 1; i < positions.length; i += 1) {
    const prev = positions[i - 1];
    const curr = positions[i];
    if (curr.x !== prev.x || curr.y !== prev.y) {
      events += 1;
    }
  }
  return events;
}

function assertCoastingPositionRelay(label, summary, sourceId, releaseAt) {
  const afterRelease = summary.receivedPositionUpdates.filter(
    (position) =>
      position.source === sourceId &&
      typeof position.at === "number" &&
      position.at >= releaseAt + 250,
  );
  if (afterRelease.length < 5) {
    throw new Error(
      `${label} did not receive movement positions after remote key release: ${JSON.stringify({
        sourceId,
        releaseAt,
        afterRelease: afterRelease.length,
        latest: summary.receivedPositionUpdates.filter((position) => position.source === sourceId).slice(-8),
      })}`,
    );
  }
  // Motion evidence: position deltas across consecutive samples
  // rather than reported velocity. See `countPositionMotionEvents`
  // for why — snapshot delta encoding (`0x70`) only includes
  // velocity when it changes, so a coasting peer's velocity reads
  // as zero in the smoke's parsed updates even though they're
  // clearly moving.
  const motionEvents = countPositionMotionEvents(afterRelease);
  const uniquePositions = new Set(afterRelease.map((position) => `${position.x},${position.y}`));
  if (motionEvents < 3 || uniquePositions.size < 2) {
    throw new Error(
      `${label} received post-release positions but they did not show remote coasting motion: ${JSON.stringify({
        sourceId,
        motionEvents,
        uniquePositions: uniquePositions.size,
        sample: afterRelease.slice(0, 12),
      })}`,
    );
  }
}

function latestRosterById(summary, playerId) {
  for (let index = summary.rosters.length - 1; index >= 0; index -= 1) {
    const roster = summary.rosters[index];
    if (roster.id === playerId) return roster;
  }
  return null;
}

function observedPlayerInShip(summaries, playerId) {
  return summaries.some((summary) => {
    for (let index = summary.shipChanges.length - 1; index >= 0; index -= 1) {
      const change = summary.shipChanges[index];
      if (change.id === playerId) return change.ship < 8;
    }
    const roster = latestRosterById(summary, playerId);
    return roster && roster.ship < 8;
  });
}

function assertNoDuplicateRosterIds(label, summary) {
  const counts = new Map();
  for (const roster of summary.rosters) {
    counts.set(roster.id, (counts.get(roster.id) || 0) + 1);
  }
  const duplicates = [...counts.entries()]
    .filter(([, count]) => count > 1)
    .map(([id, count]) => ({ id, count }));
  if (duplicates.length > 0) {
    throw new Error(
      `${label} received duplicate initial roster entries: ${JSON.stringify({
        duplicates,
        rosters: summary.rosters,
      })}`,
    );
  }
}

async function freePort() {
  const server = net.createServer();
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const port = server.address().port;
      server.close(() => resolve(port));
    });
  });
}

async function startStaticServer(upstreamPort) {
  const startedAtMs = Date.now();
  const requests = [];
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    const requestAudit = {
      authorization: request.headers.authorization || null,
      method: request.method,
      path: url.pathname,
      statusCode: null,
    };
    requests.push(requestAudit);
    if (upstreamPort && (["/v1/rtc/offer", "/v1/status", "/v1/zones", "/v1/players", "/v1/ping"].includes(url.pathname) ||
        /^\/v1\/zones\/[a-z0-9-]+(?:\/(?:players|status))?$/.test(url.pathname))) {
      const upstream = http.request(
        {
          hostname: "127.0.0.1",
          port: upstreamPort,
          path: `${url.pathname}${url.search}`,
          method: request.method,
          headers: request.headers,
        },
        (upstreamResponse) => {
          requestAudit.statusCode = upstreamResponse.statusCode || 502;
          response.writeHead(upstreamResponse.statusCode || 502, upstreamResponse.headers);
          upstreamResponse.pipe(response);
        },
      );
      upstream.on("error", (error) => {
        response.writeHead(502, { "content-type": "text/plain; charset=utf-8" });
        response.end(`upstream error: ${error.message}`);
      });
      request.pipe(upstream);
      return;
    }
    if (url.pathname === "/v1/status" || url.pathname === "/client-status") {
      const body = JSON.stringify({
        ok: true,
        service: "zubspace-client",
        uptime_seconds: Math.floor((Date.now() - startedAtMs) / 1000),
        started_at_ms: startedAtMs,
        root: repoRoot,
      });
      response.writeHead(200, {
        "content-type": "application/json; charset=utf-8",
        "content-length": Buffer.byteLength(body),
        "cache-control": "no-store",
        "access-control-allow-origin": "*",
      });
      response.end(request.method === "HEAD" ? undefined : body);
      return;
    }
    const relativePath = decodeURIComponent(url.pathname === "/" ? "/index.html" : url.pathname);
    const resolved = path.resolve(applicationRoot, `.${relativePath}`);
    if (!resolved.startsWith(applicationRoot + path.sep)) {
      response.writeHead(403);
      response.end("forbidden");
      return;
    }
    fs.readFile(resolved, (error, data) => {
      if (error) {
        response.writeHead(404);
        response.end("not found");
        return;
      }
      response.writeHead(200, { "content-type": mimeType(resolved) });
      response.end(data);
    });
  });

  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({ requests, server, port: server.address().port });
    });
  });
}

async function waitForServerListening(port, process, logs) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (process.exitCode !== null) {
      throw new Error("zubspace_server exited before accepting connections");
    }
    if (logs.join("").includes(`rtc=http://127.0.0.1:${port}/v1/rtc/offer`)) {
      return;
    }
    await sleep(100);
  }
  throw new Error(`timed out waiting for zubspace_server on 127.0.0.1:${port}`);
}

async function waitForChrome(port, process) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (process.exitCode !== null) {
      throw new Error("Chrome exited before DevTools became available");
    }
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (response.ok) return;
    } catch {
      await sleep(100);
    }
  }
  throw new Error("timed out waiting for Chrome DevTools");
}

function startRealServer(port) {
  const binary = path.join(repoRoot, serverBin);
  if (!fs.existsSync(binary)) {
    throw new Error(`missing ${serverBin}; run cargo build --bin zubspace_server first`);
  }
  const logs = [];
  const args = [`127.0.0.1:${port}`, "--tick-rate", smokeTickRate, "--log-level", smokeLogLevel];
  if (smokeZoneRoot) {
    args.push("--zone-root", smokeZoneRoot);
  }
  if (smokeAllowZoneOverrides) {
    args.push("--allow-zone-overrides");
  }
  if (smokeBotCount) {
    args.push("--bot-count", smokeBotCount);
  }
  const child = spawn(binary, args, { cwd: repoRoot });
  child.stdout.on("data", (chunk) => logs.push(chunk.toString("utf8")));
  child.stderr.on("data", (chunk) => logs.push(chunk.toString("utf8")));
  registerGateProcess(child);
  return { child, logs };
}

// Pull the file-transfer stall markers out of the whole in-memory log rather than the retained tail. The server runs with
// ICE/DTLS/SCTP DEBUG chatter, so the 4 KB tail a failing run keeps is entirely transport noise and cannot contain these
// INFO lines even when they fired. Absence from the tail proved nothing; this filter is what makes the instruments usable.
function serverTransferStallDiagnostics(logs) {
  const markers = /transfer_window_full|transfer_inactive|reliable_capacity_blocked|session_audit|acks_received|rtc_lane_send|ack_id_not_in_transfer_window|ack_id_seen|route_unroutable|rtc_ack_inbound|transfer_window_stalled_terminal|zone_file_request_redrive|zone_file_request_refused|zone_file_transfer_complete|zone_file_request_accepted|dropped before registration|rtc_ack_door_for_player|dropped pre-login packet|acks_dropped_session_mismatch|acks_dropped_no_client_handle|acks_applied/;
  return logs.filter((line) => markers.test(String(line)));
}

function serverSecurityRejects(logs) {
  return logs
    .join("")
    .split(/\r?\n/)
    .filter((line) => line.includes("server security reject"));
}

function serverSecurityContext(logs) {
  return logs
    .join("")
    .split(/\r?\n/)
    .filter(
      (line) =>
        line.includes("server security reject") ||
        line.includes("noncanonical ship-life reset details"),
    );
}

function serverWeaponContext(logs) {
  return logs
    .join("")
    .split(/\r?\n/)
    .filter(
      (line) =>
        line.includes("server weapon") ||
        line.includes("previous-life position before embedded input admission"),
    );
}

function startChrome(port) {
  const userDataDir = `/tmp/zubspace-real-server-smoke-${port}-${process.pid}`;
  const child = spawn(chromeBin, [
    "--headless=new",
    "--enable-unsafe-swiftshader",
    "--use-gl=angle",
    "--use-angle=swiftshader",
    "--password-store=basic",
    "--disable-extensions",
    "--disable-background-timer-throttling",
    "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding",
    "--autoplay-policy=no-user-gesture-required",
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${userDataDir}`,
    "about:blank",
  ], {
    env: browserLaunchEnvironment(process.env, "real-server smoke Chrome"),
    // Own process group so a lane that dies takes its renderers with it.
    detached: true,
  });
  registerGateProcess(child, { userDataDir });
  child.stdout.on("data", () => {});
  child.stderr.on("data", () => {});
  return { child, userDataDir };
}

const audioProbeScript = `(() => {
  if (window.__zubAudioProbeInstalled) return;
  window.__zubAudioProbeInstalled = true;
  const RealAudioContext = window.AudioContext || window.webkitAudioContext;
  window.__zubAudioProbe = {
    available: !!RealAudioContext,
    constructed: 0,
    decoded: 0,
    decodeFailed: 0,
    starts: 0,
    resumes: 0,
    states: [],
  };
  if (!RealAudioContext) return;

  class ProbeAudioContext extends RealAudioContext {
    constructor(...args) {
      super(...args);
      window.__zubAudioProbe.constructed += 1;
      window.__zubAudioProbe.states.push(this.state);
    }

    decodeAudioData(arrayBuffer, successCallback, errorCallback) {
      const onSuccess = (buffer) => {
        window.__zubAudioProbe.decoded += 1;
        if (successCallback) return successCallback(buffer);
        return buffer;
      };
      const onFailure = (error) => {
        window.__zubAudioProbe.decodeFailed += 1;
        if (errorCallback) return errorCallback(error);
        throw error;
      };

      if (successCallback || errorCallback) {
        return super.decodeAudioData(arrayBuffer, onSuccess, onFailure);
      }
      return super.decodeAudioData(arrayBuffer).then(onSuccess, onFailure);
    }

    createBufferSource(...args) {
      const source = super.createBufferSource(...args);
      const start = source.start.bind(source);
      source.start = (...startArgs) => {
        window.__zubAudioProbe.starts += 1;
        return start(...startArgs);
      };
      return source;
    }

    resume(...args) {
      window.__zubAudioProbe.resumes += 1;
      return super.resume(...args);
    }
  }

  window.AudioContext = ProbeAudioContext;
  window.webkitAudioContext = ProbeAudioContext;
})()`;

function keyEventForChar(ch) {
  if (ch === " ") return { key: " ", code: "Space", keyCode: 32 };
  if (ch === "/") return { key: "/", code: "Slash", keyCode: 191 };
  if (ch >= "0" && ch <= "9") return { key: ch, code: `Digit${ch}`, keyCode: ch.charCodeAt(0) };
  const upper = ch.toUpperCase();
  return { key: ch, code: `Key${upper}`, keyCode: upper.charCodeAt(0) };
}

async function stopProcess(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise((resolve) => child.once("exit", resolve));
  killProcessGroup(child, "SIGTERM");
  await Promise.race([exited, sleep(2_000)]);
  if (child.exitCode === null && child.signalCode === null) {
    killProcessGroup(child, "SIGKILL");
    await Promise.race([exited, sleep(1_000)]);
  }
}

async function removeDirectory(directory) {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    try {
      fs.rmSync(directory, { recursive: true, force: true });
      return;
    } catch (error) {
      if (attempt === 9) throw error;
      await sleep(100);
    }
  }
}

async function openClient(chromePort, pageUrl, options = {}) {
  const target = await (
    await fetch(`http://127.0.0.1:${chromePort}/json/new?${encodeURIComponent("about:blank")}`, {
      method: "PUT",
    })
  ).json();
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  let nextId = 1;
  const pending = new Map();
  const events = [];
  const frames = { sent: [], received: [] };
  let initialSpawnEvidence = null;
  const lifecycleResetEvidence = [];
  let lifecycleResetEvidenceDropped = 0;
  const rememberLifecycleReset = (before, metrics) => {
    lifecycleResetEvidence.push({ before, after: compactShipResetMetrics(metrics) });
    if (lifecycleResetEvidence.length > 64) {
      lifecycleResetEvidence.shift(); lifecycleResetEvidenceDropped++;
    }
  };
  const useBrowserPacketLog = pageUrl.includes("rtc=");
  let browserPacketLogSeq = 0;
  let browserDocumentOrigin = null;
  let packetPollTimer = null;
  let packetPollInFlight = false;

  ws.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) {
      const { resolve, reject, timeout } = pending.get(message.id);
      pending.delete(message.id);
      clearTimeout(timeout);
      if (message.error) reject(new Error(JSON.stringify(message.error)));
      else resolve(message.result || {});
      return;
    }

    if (message.method === "Runtime.consoleAPICalled") {
      const type = message.params.type;
      const text = (message.params.args || [])
        .map((arg) => arg.value ?? arg.description ?? "")
        .join(" ");
      if (["error", "warning", "assert"].includes(type) || text === "fix") {
        events.push({ type, text });
      }
    } else if (message.method === "Runtime.exceptionThrown") {
      events.push({
        type: "exception",
        text: message.params.exceptionDetails?.exception?.description ||
          message.params.exceptionDetails?.text || "exception",
      });
    } else if (message.method === "Log.entryAdded") {
      const entry = message.params.entry;
      if (["error", "warning"].includes(entry.level)) {
        events.push({ type: entry.level, text: entry.url ? `${entry.text} (${entry.url})` : entry.text });
      }
    }
  };

  ws.onclose = () => {
    for (const request of pending.values()) {
      clearTimeout(request.timeout);
      request.reject(new Error("DevTools WebSocket closed"));
    }
    pending.clear();
  };

  const send = (method, params = {}, timeoutMs = 5_000) => {
    const id = nextId++;
    ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`DevTools command timed out: ${method}`));
      }, timeoutMs);
      pending.set(id, { resolve, reject, timeout });
    });
  };

  function pushFrame(targetFrames, packet, detail = null) {
    const frame = Buffer.isBuffer(packet) ? Buffer.from(packet) : Buffer.from(packet || []);
    const frameDetail = { at: Date.now() };
    if (detail && typeof detail === "object") {
      Object.assign(frameDetail, detail);
    }
    frame.zubspaceDetail = frameDetail;
    targetFrames.push(frame);
  }

  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
    setTimeout(() => reject(new Error("DevTools WebSocket timeout")), 5_000);
  });

  await send("Runtime.enable");
  await send("Page.enable");
  await send("Log.enable");
  await send("Network.enable");
  if (options.preNavigationScript) {
    await send("Page.addScriptToEvaluateOnNewDocument", {
      source: options.preNavigationScript,
    });
  }
  if (options.mobileIdentity) {
    await send("Emulation.setUserAgentOverride", { userAgent: "iPhone Mobile", platform: "iPhone" });
  }
  const viewport = options.viewport || {};
  await send("Emulation.setDeviceMetricsOverride", {
    width: viewport.width || 1280,
    height: viewport.height || 720,
    deviceScaleFactor: viewport.deviceScaleFactor || 1,
    mobile: viewport.mobile || false,
  });
  await send("Page.navigate", { url: pageUrl });
  await send("Page.bringToFront").catch(() => {});
  await waitUntil(
    `page baseline for ${pageUrl}`,
    async () => {
      const state = await send("Runtime.evaluate", {
        returnByValue: true,
        expression: `(() => ({
          canvas: !!document.querySelector('canvas'),
          hasWasm: performance.getEntriesByType('resource').some(e => /zubspace\\.wasm/.test(e.name)),
        }))()`,
      });
      return (
        state.result.value.canvas &&
        (options.waitForWasm === false || state.result.value.hasWasm)
      );
    },
    15_000,
  );
  await send("Runtime.evaluate", {
    expression: `document.querySelector('canvas')?.focus()`,
  }).catch(() => {});

  const syncBrowserPacketLog = async () => {
    if (!useBrowserPacketLog || packetPollInFlight || ws.readyState !== WebSocket.OPEN) {
      return;
    }
    packetPollInFlight = true;
    try {
      const result = await send(
        "Runtime.evaluate",
        {
          returnByValue: true,
          expression: `(() => {
            const log = window.__zubspacePacketLog || [];
            const maxSeq = log.reduce((max, entry) => Math.max(max, entry.seq || 0), 0);
            const documentOrigin = performance.timeOrigin;
            const entries = documentOrigin !== ${JSON.stringify(browserDocumentOrigin)} || maxSeq < ${browserPacketLogSeq}
              ? log
              : log.filter((entry) => entry.seq > ${browserPacketLogSeq});
            return JSON.stringify({ maxSeq, entries, documentOrigin });
          })()`,
        },
        2_000,
      );
      const parsed = JSON.parse(result.result.value || "{}");
      if (parsed.documentOrigin !== browserDocumentOrigin || (parsed.maxSeq || 0) < browserPacketLogSeq) {
        browserPacketLogSeq = 0;
      }
      browserDocumentOrigin = parsed.documentOrigin;
      const entries = Array.isArray(parsed) ? parsed : parsed.entries || [];
      for (const entry of entries) {
        browserPacketLogSeq = Math.max(browserPacketLogSeq, entry.seq || 0);
        const detail = {
          ...(entry.detail && typeof entry.detail === "object" ? entry.detail : {}),
          at: entry.at || Date.now(),
          documentOrigin: browserDocumentOrigin,
          packetSequence: entry.seq ?? null,
          byteLength: entry.byteLength ?? entry.bytes?.length ?? 0,
          bytesTruncated: entry.bytesTruncated === true,
        };
        if (entry.direction === "sent") {
          pushFrame(frames.sent, entry.bytes || [], detail);
        } else if (entry.direction === "received") {
          pushFrame(frames.received, entry.bytes || [], detail);
        }
      }
    } finally {
      packetPollInFlight = false;
    }
  };

  if (useBrowserPacketLog) {
    packetPollTimer = setInterval(() => {
      syncBrowserPacketLog().catch(() => {});
    }, 100);
  }

  const focusCanvas = async () => {
    await send("Page.bringToFront").catch(() => {});
    await send("Runtime.evaluate", {
      expression: `document.querySelector('canvas')?.focus()`,
    }).catch(() => {});
  };
  const pressKey = async (key, code, keyCode, holdMs = 0) => {
    await send("Input.dispatchKeyEvent", {
      type: "keyDown",
      key,
      code,
      windowsVirtualKeyCode: keyCode,
      nativeVirtualKeyCode: keyCode,
    });
    if (holdMs > 0) await sleep(holdMs);
    await send("Input.dispatchKeyEvent", {
      type: "keyUp",
      key,
      code,
      windowsVirtualKeyCode: keyCode,
      nativeVirtualKeyCode: keyCode,
    }).catch(() => {});
  };
  const enterShip = async () => {
    await focusCanvas();
    for (let attempt = 0; attempt < 4; attempt += 1) {
      await pressKey("Escape", "Escape", 27, 60);
      await pressKey("2", "Digit2", 50);
      await sleep(350);
    }
  };

  return {
    frames,
    events,
    send,
    async metricsState() {
      return { uiMetrics: await this.uiMetrics() };
    },
    async press(key, holdMs = 60) {
      // Navigation uses the existing WASD aliases. Avoid bringToFront here:
      // joint real-key steering must not blur the other pilot's held input.
      const event = keyEvent(key);
      await dispatchChromeKey(send, "keyDown", event);
      try { await sleep(holdMs); }
      finally { await dispatchChromeKey(send, "keyUp", event); }
    },
    async keyDown(key) { await dispatchChromeKey(send, "keyDown", keyEvent(key)); },
    async keyUp(key) { await dispatchChromeKey(send, "keyUp", keyEvent(key)); },
    async releaseKeys() {
      for (const key of ["w", "a", "s", "d", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Control", "Shift"])
        await this.keyUp(key);
    },
    async syncFrames() {
      await syncBrowserPacketLog().catch(() => {});
    },
    async mapCacheStats() {
      await syncBrowserPacketLog().catch(() => {});
      const state = await send("Runtime.evaluate", {
        returnByValue: true,
        expression: `window.__zubspaceMapCacheStats || null`,
      });
      return state.result.value;
    },
    async startupFormState() {
      const state = await send("Runtime.evaluate", {
        returnByValue: true,
        expression: `(() => {
          const form = document.querySelector('#player-name-form');
          const input = document.querySelector('#player-name-input');
          const select = document.querySelector('#arena-select');
          const button = document.querySelector('#player-name-submit');
          const formRect = form?.getBoundingClientRect();
          const inputRect = input?.getBoundingClientRect();
          const selectRect = select?.getBoundingClientRect();
          const buttonRect = button?.getBoundingClientRect();
          const visibleRects = [
            input && !input.classList.contains('hidden') ? inputRect : null,
            selectRect,
            buttonRect,
          ].filter((rect) => rect && rect.width > 0 && rect.height > 0);
          const controlsFitText = [input, select, button]
            .filter((element) => element && !element.classList.contains('hidden'))
            .every((element) => element.scrollWidth <= element.clientWidth + 1);
          const controlsInsideForm =
            !!formRect &&
            visibleRects.every((rect) => rect.left >= formRect.left - 1 && rect.right <= formRect.right + 1);
          const noControlOverlap = visibleRects.every((rect, index) =>
            visibleRects.slice(index + 1).every((other) =>
              rect.right <= other.left + 1 ||
              other.right <= rect.left + 1 ||
              rect.bottom <= other.top + 1 ||
              other.bottom <= rect.top + 1
            )
          );
          const stackedColumn =
            visibleRects.length > 1 &&
            visibleRects.every((rect, index) => index === 0 || rect.top >= visibleRects[index - 1].bottom + 6);
          const style = form ? getComputedStyle(form) : null;
          return {
            formVisible: !!form && form.classList.contains('visible'),
            identityText: document.querySelector('#loading-identity-text')?.textContent || '',
            inputHidden: !!input && input.classList.contains('hidden'),
            inputVisible: !!input && getComputedStyle(input).visibility !== 'hidden' && inputRect.width > 0 && inputRect.height > 0,
            selectedArena: window.__zubspaceArena || null,
            mobile: window.zubspaceMenu?.mobile === true,
            desktopMessageVisible: !document.querySelector('#mobile-join-message')?.hidden,
            hasWasm: performance.getEntriesByType('resource').some(e => /zubspace\\.wasm/.test(e.name)),
            selectValue: select?.value || '',
            selectOptions: select ? Array.from(select.options, (option) => ({
              value: option.value,
              text: option.textContent || '',
            })) : [],
            zoneCatalogSource: window.__zubspaceZoneCatalogSource || null,
            zoneCatalog: window.__zubspaceZoneCatalog || null,
            selectVisible: !!select && selectRect.width > 0 && selectRect.height > 0,
            buttonVisible: !!button && getComputedStyle(button).visibility !== 'hidden' && buttonRect.width > 0 && buttonRect.height > 0,
            buttonDisabled: !!button && button.disabled,
            buttonText: button?.textContent || '',
            columnCount: style ? style.gridTemplateColumns.split(' ').filter(Boolean).length : 0,
            controlsInsideForm,
            controlsFitText,
            noControlOverlap,
            stackedColumn,
            singleRow:
              !!formRect &&
              visibleRects.length > 1 &&
              visibleRects.every((rect) => Math.abs(rect.top - visibleRects[0].top) < 2) &&
              visibleRects.every((rect, index) => index === 0 || visibleRects[index - 1].right <= rect.left + 1) &&
              visibleRects[visibleRects.length - 1].right <= formRect.right + 1,
          };
        })()`,
      });
      return state.result.value;
    },
    async runtimeBootstrapState() {
      const state = await send("Runtime.evaluate", {
        returnByValue: true,
        expression: `({
          hasWasm: performance.getEntriesByType('resource').some(
            (entry) => /zubspace\\.wasm/.test(entry.name)
          ),
          storedArena: localStorage.getItem('zubspace.arena.v1'),
          currentArena: window.__zubspaceArena || null,
          selectedArena: window.__zubspaceArena || null,
          currentUrl: window.location.href,
        })`,
      });
      return state.result.value;
    },
    async submitStartupForm() {
      await send("Runtime.evaluate", {
        expression: `document.querySelector('#player-name-form')?.requestSubmit()`,
      });
      await acknowledgeControlsWelcome(send, async expression =>
        (await send("Runtime.evaluate", { expression, returnByValue: true })).result.value);
    },
    async joinMenuWithMouse() {
      const target = await send("Runtime.evaluate", { returnByValue: true,
        expression: `(() => { const button = document.querySelector('#player-name-submit');
          const rect = button?.getBoundingClientRect();
          return { disabled: button?.disabled, x: rect?.x + rect?.width / 2, y: rect?.y + rect?.height / 2 }; })()` });
      const { disabled, x, y } = target.result.value;
      if (disabled || !Number.isFinite(x) || !Number.isFinite(y)) throw new Error("Play control unavailable");
      for (const type of ["mousePressed", "mouseReleased"]) {
        await send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
      }
      await acknowledgeControlsWelcome(send, async expression =>
        (await send("Runtime.evaluate", { expression, returnByValue: true })).result.value);
      return { input: "mouse", selectedArena: (await this.runtimeBootstrapState()).selectedArena };
    },
    async assertMenuKeyboardIsolation() {
      await pressKey("ArrowUp", "ArrowUp", 38, 50);
      const state = await this.runtimeBootstrapState();
      if (state.hasWasm) throw new Error("menu arrow key started gameplay");
    },
    async enterShip() {
      await enterShip();
    },
    async waitForFullEnergy() {
      await waitUntil(`full energy for ship menu on ${pageUrl}`, async () => {
        const metrics = await this.uiMetrics();
        return metrics?.playerAlive === true && metrics.serverPlayerAlive === true &&
          metrics.playerMaxEnergy > 0 && metrics.playerEnergy >= metrics.playerMaxEnergy &&
          metrics.serverPlayerEnergy >= metrics.playerMaxEnergy &&
          metrics.localGameplayState?.authorityPhase === "Live";
      }, 30_000);
    },
    async enterShipWithLifecycleBarrier() {
      await this.waitForFullEnergy();
      const before = shipResetBefore(await this.shipResetMetrics());
      await focusCanvas();
      await pressKey("Escape", "Escape", 27, 60);
      await pressKey("2", "Digit2", 50, 40);
      await waitUntil(
        `single keyboard ship change to establish weapon authority for ${pageUrl}`,
        async () => {
          const metrics = await this.shipResetMetrics();
          const settled = shipResetLifecycleSettled(before, metrics);
          if (settled) rememberLifecycleReset(before, metrics);
          return settled;
        },
        30_000,
      );
      return this.uiMetrics();
    },
    async uiMetrics() {
      const state = await send("Runtime.evaluate", {
        returnByValue: true,
        expression: `window.__zubspaceUiMetrics || null`,
      });
      return state.result.value;
    },
    async shipResetMetrics() {
      const state = await send("Runtime.evaluate", {
        returnByValue: true,
        expression: `(() => {
          const metrics = window.__zubspaceUiMetrics;
          if (!metrics) return null;
          const entries = [...(window.__zubspacePacketLogFirst || []), ...(window.__zubspacePacketLog || [])];
          return { ...metrics, lifecycleWireEvidence: (${lifecycleBaselineWireEvidence.toString()})(entries, metrics.playerLifeSeq) };
        })()`,
      });
      return state.result.value;
    },
    get initialSpawnEvidence() { return initialSpawnEvidence; },
    async captureInitialSpawnEvidence() {
      initialSpawnEvidence = initialSpawnLifecycleEvidence(await this.shipResetMetrics());
      return initialSpawnEvidence;
    },
    async resetMotionWithShipCycle() {
      await focusCanvas();
      for (const [key, code, keyCode] of [
        ["3", "Digit3", 51],
        ["2", "Digit2", 50],
      ]) {
        await this.waitForFullEnergy();
        const before = shipResetBefore(await this.shipResetMetrics());
        let latestMetrics = null;
        await pressKey("Escape", "Escape", 27, 60);
        await pressKey(key, code, keyCode, 40);
        try {
          await waitUntil(
            `keyboard ship ${key} transition to reset motion for ${pageUrl}`,
            async () => {
              latestMetrics = await this.shipResetMetrics();
              const settled = shipResetLifecycleSettled(before, latestMetrics);
              if (settled) rememberLifecycleReset(before, latestMetrics);
              return settled;
            },
            10_000,
          );
        } catch (error) {
          error.message = `${error.message}; lifecycle evidence=${JSON.stringify({
            requestedShipKey: key,
            before,
            latest: compactShipResetMetrics(latestMetrics),
          })}`;
          throw error;
        }
      }
      return this.uiMetrics();
    },
    async drive() {
      await enterShip();
      // A ship-change lifecycle receipt can land after the repeated Digit2
      // presses. Refocus the canvas and hold long enough for both a delayed
      // receipt and real post-barrier keyboard frames on slower headless runs.
      await focusCanvas();
      await send("Input.dispatchKeyEvent", {
        type: "keyDown",
        key: "ArrowUp",
        code: "ArrowUp",
        windowsVirtualKeyCode: 38,
        nativeVirtualKeyCode: 38,
      });
      await send("Input.dispatchKeyEvent", {
        type: "keyDown",
        key: "ArrowRight",
        code: "ArrowRight",
        windowsVirtualKeyCode: 39,
        nativeVirtualKeyCode: 39,
      });
      await sleep(1200);
      await send("Input.dispatchKeyEvent", {
        type: "keyUp",
        key: "ArrowRight",
        code: "ArrowRight",
        windowsVirtualKeyCode: 39,
        nativeVirtualKeyCode: 39,
      }).catch(() => {});
      await send("Input.dispatchKeyEvent", {
        type: "keyUp",
        key: "ArrowUp",
        code: "ArrowUp",
        windowsVirtualKeyCode: 38,
        nativeVirtualKeyCode: 38,
      }).catch(() => {});
      return { releasedAt: Date.now() };
    },
    async cruise(durationMs = 1800) {
      await focusCanvas();
      await send("Input.dispatchKeyEvent", {
        type: "keyDown",
        key: "ArrowUp",
        code: "ArrowUp",
        windowsVirtualKeyCode: 38,
        nativeVirtualKeyCode: 38,
      });
      await sleep(durationMs);
      await send("Input.dispatchKeyEvent", {
        type: "keyUp",
        key: "ArrowUp",
        code: "ArrowUp",
        windowsVirtualKeyCode: 38,
        nativeVirtualKeyCode: 38,
      }).catch(() => {});
    },
    async firePrimary() {
      await focusCanvas();
      // This helper proves one logical shot across the three RTC carriers. The
      // fastest authored Extreme ships can fire every 50 ms, so the former
      // 120 ms hold correctly produced two or three shots there. Keep the
      // genuine DevTools key chord long enough to cross a render frame while
      // remaining below every authored bullet cooldown; continuous fire has
      // its own browser acceptance scenarios.
      await pressKey("Control", "ControlLeft", 17, 30);
      await sleep(350);
    },
    async fireMine() {
      await focusCanvas();
      await pressKey("m", "KeyM", 77, 120);
      await sleep(350);
    },
    async turnLeftAndCruise(durationMs = 650) {
      await focusCanvas();
      for (const [key, code, keyCode] of [
        ["ArrowLeft", "ArrowLeft", 37],
        ["ArrowUp", "ArrowUp", 38],
      ]) {
        await send("Input.dispatchKeyEvent", {
          type: "keyDown",
          key,
          code,
          windowsVirtualKeyCode: keyCode,
          nativeVirtualKeyCode: keyCode,
        });
      }
      await sleep(durationMs);
      for (const [key, code, keyCode] of [
        ["ArrowUp", "ArrowUp", 38],
        ["ArrowLeft", "ArrowLeft", 37],
      ]) {
        await send("Input.dispatchKeyEvent", {
          type: "keyUp",
          key,
          code,
          windowsVirtualKeyCode: keyCode,
          nativeVirtualKeyCode: keyCode,
        }).catch(() => {});
      }
    },
    latestClientPositionPacketForTest() {
      return [...frames.sent]
        .reverse()
        .find(
          (packet) =>
            packet[0] === 0x03 &&
            packet.length >= 22 &&
            packet.readUInt16LE(20) === 0,
        );
    },
    async sendRawForTest(packet, label = "raw network") {
      const bytes = [...packet];
      const sent = await send("Runtime.evaluate", {
        returnByValue: true,
        expression: `window.__zubspaceSendRawForTest?.(${JSON.stringify(bytes)}) === true`,
      });
      if (sent.result.value !== true) {
        throw new Error(`${name}: browser ${label} test hook did not send`);
      }
    },
    async sendPositionWeaponForTest(weapon, count = 6) {
      const fallbackPacket = Buffer.from([
        0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0x03, 0x00, 0x00,
      ]);
      for (let index = 0; index < count; index += 1) {
        const latestPosition = this.latestClientPositionPacketForTest();
        const basePacket = Buffer.from(latestPosition || fallbackPacket);
        const latestSequence = latestClientPositionSequence(frames.sent) ?? clientPositionSequence(basePacket) ?? 0;
        const packet = clientPositionPacketWithSequence(basePacket, (latestSequence + 20) & 0xffff);
        packet.writeUInt16LE(weapon, 20);
        await this.sendRawForTest(packet, "weapon");
        await sleep(140);
      }
    },
    async sendPrizeForTest(prizeId) {
      const latestPosition = this.latestClientPositionPacketForTest();
      if (!latestPosition) {
        throw new Error("cannot send prize test request before a client position packet");
      }
      const y = latestPosition.readUInt16LE(8);
      const x = latestPosition.readUInt16LE(12);
      const packet = Buffer.alloc(11);
      packet[0] = 0x07;
      packet.writeUInt32LE(0, 1);
      packet.writeUInt16LE(x, 5);
      packet.writeUInt16LE(y, 7);
      packet.writeInt16LE(prizeId, 9);
      await this.sendRawForTest(packet, "prize");
    },
    async sendChatRequestForTest(request) {
      const payload = encodeChatRequest(request);
      const sent = await send("Runtime.evaluate", {
        returnByValue: true,
        expression: `window.__zubspaceSendReliablePayloadForTest?.(${JSON.stringify([...payload])}) === true`,
      });
      if (sent.result.value !== true) {
        throw new Error(`${name}: browser reliable chat test hook did not send`);
      }
      await sleep(120);
    },
    async unlockAudioForTest() {
      await send("Runtime.evaluate", {
        expression: `window.__zubspaceUnlockAudioForTest?.()`,
      }).catch(() => {});
      await sleep(120);
    },
    async setViewport(viewport) {
      await send("Emulation.setDeviceMetricsOverride", {
        width: viewport.width,
        height: viewport.height,
        deviceScaleFactor: viewport.deviceScaleFactor || 1,
        mobile: viewport.mobile || false,
      });
      await send("Runtime.evaluate", {
        expression: `window.dispatchEvent(new Event('resize')); document.querySelector('canvas')?.focus()`,
      }).catch(() => {});
      await sleep(250);
    },
    async sendChatInput(text) {
      await focusCanvas();
      await pressKey("Escape", "Escape", 27, 30);
      await sleep(80);
      await pressKey("Enter", "Enter", 13, 50);
      await sleep(160);
      for (const ch of text) {
        if (ch === "\t") {
          await pressKey("Tab", "Tab", 9, 30);
          await sleep(100);
          continue;
        }
        const event = keyEventForChar(ch);
        await send("Input.dispatchKeyEvent", {
          type: "keyDown",
          key: event.key,
          code: event.code,
          text: ch,
          unmodifiedText: ch,
          windowsVirtualKeyCode: event.keyCode,
          nativeVirtualKeyCode: event.keyCode,
        });
        await send("Input.dispatchKeyEvent", {
          type: "keyUp",
          key: event.key,
          code: event.code,
          windowsVirtualKeyCode: event.keyCode,
          nativeVirtualKeyCode: event.keyCode,
        }).catch(() => {});
        await sleep(25);
      }
      await sleep(160);
      await pressKey("Enter", "Enter", 13, 60);
      await sleep(120);
    },
    async evaluate(expression) {
      const result = await send("Runtime.evaluate", {
        returnByValue: true,
        awaitPromise: true,
        expression,
      });
      if (result.exceptionDetails) throw new Error(result.exceptionDetails.text || "browser evaluation failed");
      return result.result.value;
    },
    async freezeQaTrace() {
      const result = await send("Runtime.evaluate", {
        returnByValue: true,
        awaitPromise: true,
        expression: `(() => {
          if (window.__zubspaceQaTraceStateForTest?.().enabled === false) return null;
          if (typeof window.__zubspaceMarkQaIssue !== 'function') throw new Error('QA marker unavailable');
          return window.__zubspaceMarkQaIssue({ postTailMs: 0, download: false, upload: false });
        })()`,
      });
      if (result.exceptionDetails) throw new Error(result.exceptionDetails.text || "QA marker failed");
      return result.result.value;
    },
    async screenshot() {
      return (await send("Page.captureScreenshot", { format: "png", fromSurface: true })).data;
    },
    async pageState() {
      await syncBrowserPacketLog().catch(() => {});
      const state = await send("Runtime.evaluate", {
        returnByValue: true,
        awaitPromise: true,
        expression: `(async () => {
          await window.__zubspaceZoneCache?.flush();
          const canvas = document.querySelector('canvas');
          let nonBlackPixels = 0;
          let sampledPixels = 0;
          let canvasSampleError = null;
          if (canvas) {
            try {
              const sample = document.createElement('canvas');
              sample.width = Math.min(canvas.width, 320);
              sample.height = Math.min(canvas.height, 180);
              const context = sample.getContext('2d', { willReadFrequently: true });
              context.drawImage(canvas, 0, 0, sample.width, sample.height);
              const data = context.getImageData(0, 0, sample.width, sample.height).data;
              sampledPixels = sample.width * sample.height;
              for (let offset = 0; offset < data.length; offset += 4) {
                if (data[offset + 3] > 0 && data[offset] + data[offset + 1] + data[offset + 2] > 10) {
                  nonBlackPixels += 1;
                }
              }
            } catch (error) {
              canvasSampleError = String(error);
            }
          }
          return {
            canvas: !!canvas,
            canvasWidth: canvas ? canvas.width : 0,
            canvasHeight: canvas ? canvas.height : 0,
            // Observe the runtime-owned context. Calling getContext here can
            // claim WebGL2 before Miniquad requests WebGL1 and break startup.
            webgl: !!(canvas && typeof gl !== 'undefined' && gl && gl.canvas === canvas),
            hasWasm: performance.getEntriesByType('resource').some(e => /zubspace\\.wasm/.test(e.name)),
            storedIdentity: localStorage.getItem('zubspace.identity.v1'),
            storedArena: localStorage.getItem('zubspace.arena.v1'),
            zoneCatalogSource: window.__zubspaceZoneCatalogSource || null,
            zoneCatalog: window.__zubspaceZoneCatalog || null,
            zoneOptions: Array.from(
              document.querySelector('#arena-select')?.options || [],
              (option) => ({ value: option.value, text: option.textContent || '' }),
            ),
            audioProbe: window.__zubAudioProbe || null,
            audioUnlock: window.__zubspaceAudioUnlockState || null,
            networkLog: window.__zubspaceNetworkLog || [],
            uiMetrics: window.__zubspaceUiMetrics || null,
            qaTraceState: window.__zubspaceQaTraceStateForTest?.() || null,
            qaTraceBundle: window.__zubspaceLastQaTraceBundle || null,
            mapCacheStats: window.__zubspaceMapCacheStats || null,
            mapCachePolicyEvidence:
              await window.__zubspaceMapCacheTest?.exercisePolicy?.() || null,
            currentUrl: window.location.href,
            nonBlackPixels,
            sampledPixels,
            canvasSampleError,
          };
        })()`,
      });
      const screenshot = await send("Page.captureScreenshot", {
        format: "png",
        fromSurface: true,
      });
      const screenshotMetrics = pngNonBlackMetrics(Buffer.from(screenshot.data, "base64"));
      return {
        ...state.result.value,
        weaponPacketEvidence: retainedSmokeWeaponPackets(frames),
        smokeEncounterTrace: this.smokeEncounterTrace || [],
        rendezvousTrace: this.rendezvousTrace || [],
        initialSpawnEvidence,
        lifecycleResetEvidence: lifecycleResetEvidence.slice(-64),
        lifecycleResetEvidenceDropped,
        screenshotNonBlackPixels: screenshotMetrics.nonBlackPixels,
        screenshotSampledPixels: screenshotMetrics.sampledPixels,
      };
    },
    async requestNameChange(name) {
      await focusCanvas(send);
      await pressKey("Escape", "Escape", 27, 70);
      await pressKey("q", "KeyQ", 81, 70);
      await waitUntil("leave arena to editable join menu", async () => {
        const form = await this.startupFormState();
        return form.formVisible && form.inputVisible && form.buttonVisible;
      });
      await send("Runtime.evaluate", { expression: `document.querySelector('#player-name-input').focus(); document.querySelector('#player-name-input').select();` });
      await send("Input.insertText", { text: name });
      await this.joinMenuWithMouse();
      await sleep(500);
    },
    async close() {
      if (packetPollTimer) {
        clearInterval(packetPollTimer);
        packetPollTimer = null;
      }
      await syncBrowserPacketLog().catch(() => {});
      await send("Runtime.evaluate", {
        expression: `window.__zubspaceSendDisconnectForTest?.()`,
      }).catch(() => {});
      await sleep(250);
      await send("Page.close").catch(() => {});
      ws.close();
    },
  };
}

async function waitUntil(description, predicate, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${description}`);
}

function assertPage(name, pageState, events) {
  if (!pageState.canvas || !pageState.hasWasm || !pageState.webgl) {
    throw new Error(`${name}: page did not render the WASM WebGL canvas: ${JSON.stringify(pageState)}`);
  }
  if (pageState.screenshotNonBlackPixels < 100) {
    throw new Error(
      `${name}: rendered page looked blank: ${pageState.screenshotNonBlackPixels}/${pageState.screenshotSampledPixels} non-black screenshot pixels`,
    );
  }
  if (!pageState.audioProbe?.available) {
    throw new Error(`${name}: browser audio context was not available: ${JSON.stringify(pageState.audioProbe)}`);
  }
  if (pageState.audioProbe.decodeFailed !== 0) {
    throw new Error(`${name}: browser audio decode failures: ${JSON.stringify(pageState.audioProbe)}`);
  }
  const networkMessages = new Set((pageState.networkLog || []).map((entry) => entry.message));
  const connected =
    networkMessages.has("connected") ||
    (networkMessages.has("rtc answer accepted") && networkMessages.has("rtc data channel open"));
  if (!networkMessages.has("connecting") || !connected) {
    throw new Error(`${name}: browser network connection logs missing: ${JSON.stringify(pageState.networkLog)}`);
  }
  const seriousEvents = events.filter((event) => event.type === "error" || event.type === "exception");
  if (seriousEvents.length > 0) {
    throw new Error(`${name}: browser errors: ${JSON.stringify(seriousEvents)}`);
  }
  const strayDebugEvents = events.filter((event) => event.text === "fix");
  if (strayDebugEvents.length > 0) {
    throw new Error(`${name}: stray browser debug logs: ${JSON.stringify(strayDebugEvents)}`);
  }
}

function assertUiMetrics(name, metrics, expected) {
  if (!metrics) {
    throw new Error(`${name}: WASM did not publish UI metrics`);
  }
  if (metrics.screenWidth !== expected.width || metrics.screenHeight !== expected.height) {
    throw new Error(`${name}: unexpected UI metric viewport: ${JSON.stringify(metrics)}`);
  }
  if (
    !metrics.energyInside ||
    metrics.energyCenterDelta > 1 ||
    !metrics.energyDigitsInside ||
    !metrics.energyDigitsRightAligned ||
    !metrics.hudTextInside ||
    metrics.hudRadarOverlap ||
    !metrics.radarBottomRight ||
    !metrics.radarLabelInside ||
    !metrics.rightStatusPanelInside ||
    !metrics.rightStatusPanelClearOfRadar ||
    !metrics.leftItemPanelInside ||
    !metrics.shipLabelInside ||
    !metrics.notificationInside ||
    !metrics.textPositiveScale ||
    metrics.hudTextScale > 1 ||
    metrics.shipLabelScale > 1 ||
    metrics.notificationScale > 1 ||
    !hudLineCountMatchesMode(metrics)
  ) {
    throw new Error(`${name}: in-game UI text is not fitted and centered: ${JSON.stringify(metrics)}`);
  }
}

async function assertCommonViewportUiMetrics(client) {
  const viewports = [
    { name: "desktop 1366x768", width: 1366, height: 768 },
    { name: "desktop 1024x768", width: 1024, height: 768 },
    { name: "compact 800x600", width: 800, height: 600 },
    { name: "mobile 414x896", width: 414, height: 896, mobile: true },
    { name: "mobile 360x640", width: 360, height: 640, mobile: true },
  ];
  const metrics = [];
  for (const viewport of viewports) {
    await client.setViewport(viewport);
    await waitUntil(`${viewport.name} UI metrics`, async () => {
      const state = await client.pageState();
      return (
        state.uiMetrics?.screenWidth === viewport.width &&
        state.uiMetrics?.screenHeight === viewport.height
      );
    }, 5_000);
    const state = await client.pageState();
    assertPage(viewport.name, state, client.events);
    assertUiMetrics(viewport.name, state.uiMetrics, viewport);
    metrics.push({ name: viewport.name, uiMetrics: state.uiMetrics });
  }
  return metrics;
}

function assertStartupFormLayout(name, form, expectedArena, { mobile = false } = {}) {
  if (form.selectedArena !== expectedArena || form.selectVisible || form.mobile !== mobile) {
    throw new Error(`${name}: incorrect single-zone menu: ${JSON.stringify(form)}`);
  }
  if (mobile) {
    if (!form.desktopMessageVisible || form.formVisible || !form.buttonDisabled || form.hasWasm) {
      throw new Error(`${name}: mobile admission was not blocked: ${JSON.stringify(form)}`);
    }
  } else if (!form.formVisible || !form.inputVisible || !form.buttonVisible ||
      !form.controlsInsideForm || !form.controlsFitText || !form.noControlOverlap) {
    throw new Error(`${name}: desktop controls do not fit: ${JSON.stringify(form)}`);
  }
}

async function runSmoke(smokeArena) {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(
      `missing ${wasmPath}; run cargo build --target wasm32-unknown-unknown --bin zubspace first`,
    );
  }
  const expectedMap = process.env.ZUBSPACE_SMOKE_EXPECTED_MAP || expectedMapByArena.get(smokeArena);
    const expectedStoredArena = process.env.ZUBSPACE_SMOKE_EXPECTED_STORED_ARENA ||
      expectedStoredArenaByArena.get(smokeArena) || smokeArena;
    const expectedFirstRunArena =
      expectedStoredArena;
  const expectedSettings = process.env.ZUBSPACE_SMOKE_EXPECTED_SETTINGS_JSON
    ? JSON.parse(process.env.ZUBSPACE_SMOKE_EXPECTED_SETTINGS_JSON)
    : expectedSettingsByArena.get(smokeArena);
  if (!["query", "storage", "discovery"].includes(smokeArenaSource)) {
    throw new Error(
      `unsupported ZUBSPACE_SMOKE_ARENA_SOURCE=${JSON.stringify(smokeArenaSource)}; expected "query", "storage", or "discovery"`,
    );
  }
  if (!expectedMap || !expectedSettings) {
    throw new Error(
      `unsupported ZUBSPACE_SMOKE_ARENA=${JSON.stringify(smokeArena)}; expected one of ${JSON.stringify([...expectedMapByArena.keys()])}`,
    );
  }

  const serverPort = await freePort();
  const alphaChromePort = await freePort();
  const betaChromePort = await freePort();
  const server = startRealServer(serverPort);
  const alphaChrome = startChrome(alphaChromePort);
  const betaChrome = startChrome(betaChromePort);
  const alphaStatic = await startStaticServer(serverPort);
  const betaStatic = await startStaticServer(serverPort);
  let alpha = null;
  let beta = null;
  let startup = null;
  let returning = null;
  let sameOriginDefault = null;
  let partialPeerFailure = null;
  let signalingTokenOriginPolicy = null;
  let zoneDiscoveryEvidence = null;
  let publicDefaultEvidence = null;
  const qaRecorder = createSmokeQaRecorder({
    directory: qaArtifactDirectory,
    label: qaArtifactLabel,
    provenance: qaArtifactDirectory
      ? JSON.parse(fs.readFileSync(path.join(qaArtifactDirectory, "provenance.json"), "utf8")) : null,
    serverLogs: server.logs,
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await Promise.all([
      waitForChrome(alphaChromePort, alphaChrome.child),
      waitForChrome(betaChromePort, betaChrome.child),
    ]);

    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const transportQuery = `rtc=${encodeURIComponent(rtcUrl)}`;
    const testPacketLogParam = `&packetlog=1&metrics=1${qaArtifactDirectory ? "&trace=1" : ""}`;
    const arenaParam = encodeURIComponent(smokeArena);
    const pageArenaParam = `&zone=${arenaParam}`;
    const arenaPreNavigationScript =
      smokeArenaSource === "storage"
        ? `localStorage.setItem('zubspace.arena.v1', ${JSON.stringify(expectedStoredArena)});`
        : "";
    const freshMapCachePreNavigationScript = forceFreshMapCache
      ? "window.__zubspaceZoneCacheReadDisabledForTest = true;"
      : "";
    const preNavigationScript = `${audioProbeScript}\n${arenaPreNavigationScript}\n${freshMapCachePreNavigationScript}`;
    const browserBootstrapToken = "same-origin-browser-bootstrap-token";
    sameOriginDefault = await openClient(
      alphaChromePort,
      `http://127.0.0.1:${alphaStatic.port}/index.html?name=sameorigin&password=secret${pageArenaParam}`,
      {
        preNavigationScript: `${preNavigationScript}\nwindow.__zubspaceRtcToken = ${JSON.stringify(browserBootstrapToken)};`,
      },
    );
    await waitUntil("same-origin packaged RTC default to connect", async () => {
      const result = await sameOriginDefault.send("Runtime.evaluate", {
        returnByValue: true,
        expression: `({
          status: window.__zubspaceRtcStatusForTest?.(),
          offerUrl: (window.__zubspaceNetworkLog || []).find(
            (entry) => entry.message === "connecting",
          )?.detail?.offerUrl,
        })`,
      });
      return (
        result.result.value.status === 2 &&
        result.result.value.offerUrl ===
          `http://127.0.0.1:${alphaStatic.port}/v1/rtc/offer`
      );
    });
    const authenticatedSameOriginOffer = alphaStatic.requests.find(
      (request) =>
        request.method === "POST" &&
        request.path === "/v1/rtc/offer" &&
        request.authorization === `Bearer ${browserBootstrapToken}`,
    );
    if (!authenticatedSameOriginOffer) {
      throw new Error(
        `same-origin RTC signaling did not receive the injected Bearer token: ${JSON.stringify(alphaStatic.requests.filter((request) => request.path === "/v1/rtc/offer"))}`,
      );
    }
    await sameOriginDefault.close();
    sameOriginDefault = null;
    await waitUntil("same-origin default peer cleanup", async () => {
      const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
      if (!response.ok) return false;
      const status = await response.json();
      return status.rtc_sessions === 0 && status.clients === 0;
    });

    const attackerPath = "/capture-browser-bootstrap-token";
    const attackerRtcUrl = `http://127.0.0.1:${betaStatic.port}${attackerPath}`;
    returning = await openClient(
      alphaChromePort,
      `http://127.0.0.1:${alphaStatic.port}/index.html?rtc=${encodeURIComponent(attackerRtcUrl)}&name=originpolicy${pageArenaParam}`,
      {
        preNavigationScript: `${preNavigationScript}\nwindow.__zubspaceRtcToken = ${JSON.stringify(browserBootstrapToken)};`,
      },
    );
    await waitUntil("cross-origin bootstrap token rejection", async () => {
      const result = await returning.send("Runtime.evaluate", {
        returnByValue: true,
        expression: `({
          status: window.__zubspaceRtcStatusForTest?.(),
          hasPeer: window.__zubspaceRtcHasPeerForTest?.(),
          error: window.__zubspaceRtcLastErrorForTest?.(),
        })`,
      });
      signalingTokenOriginPolicy = result.result.value;
      return signalingTokenOriginPolicy.status === 4;
    });
    if (
      signalingTokenOriginPolicy.hasPeer ||
      !signalingTokenOriginPolicy.error?.includes("refusing to send a signaling Bearer token")
    ) {
      throw new Error(
        `cross-origin bootstrap token rejection was not explicit and clean: ${JSON.stringify(signalingTokenOriginPolicy)}`,
      );
    }
    const attackerRequests = betaStatic.requests.filter(
      (request) => request.path === attackerPath,
    );
    if (attackerRequests.length > 0) {
      throw new Error(
        `cross-origin RTC override received a request carrying privileged browser state: ${JSON.stringify(attackerRequests)}`,
      );
    }
    await returning.close();
    returning = null;

    const malformedAnswerUrl = `http://127.0.0.1:${alphaStatic.port}/missing-rtc-answer`;
    returning = await openClient(
      alphaChromePort,
      `http://127.0.0.1:${alphaStatic.port}/index.html?rtc=${encodeURIComponent(malformedAnswerUrl)}&name=failedsignal${pageArenaParam}`,
      { preNavigationScript },
    );
    await waitUntil("post-peer RTC signaling failure to reach an error state", async () => {
      const result = await returning.send("Runtime.evaluate", {
        returnByValue: true,
        expression: `({
          status: window.__zubspaceRtcStatusForTest?.(),
          hasPeer: window.__zubspaceRtcHasPeerForTest?.(),
          closedState: window.__zubspaceRtcLastClosedStateForTest?.(),
          networkLog: window.__zubspaceNetworkLog || [],
        })`,
      });
      partialPeerFailure = result.result.value;
      return partialPeerFailure.status === 4;
    });
    if (partialPeerFailure.hasPeer || partialPeerFailure.closedState !== "closed") {
      throw new Error(`failed RTC signaling retained an open partial peer: ${JSON.stringify(partialPeerFailure)}`);
    }
    if (!partialPeerFailure.networkLog.some((entry) => entry.message === "rtc connect failed")) {
      throw new Error(`failed RTC signaling was not logged: ${JSON.stringify(partialPeerFailure)}`);
    }
    if (returning.events.some((event) => event.type === "exception")) {
      throw new Error(`failed RTC signaling caused an unhandled rejection: ${JSON.stringify(returning.events)}`);
    }
    await returning.send("Runtime.evaluate", {
      expression: `localStorage.removeItem('zubspace.identity.v1'); localStorage.removeItem('zubspace.arena.v1');`,
    });
    await returning.close();
    returning = null;

    startup = await openClient(
      alphaChromePort,
      `http://127.0.0.1:${alphaStatic.port}/index.html?${transportQuery}&password=secret${testPacketLogParam}${pageArenaParam}`,
      { preNavigationScript, waitForWasm: false },
    );
    await waitUntil("first-run name and arena form", async () => {
      const form = await startup.startupFormState();
      return form.formVisible && form.inputVisible && form.buttonVisible;
    });
    const startupForm = await startup.startupFormState();
    assertStartupFormLayout("desktop first-run loader", startupForm, expectedFirstRunArena);
    await startup.assertMenuKeyboardIsolation();
    await startup.close();
    startup = null;

    startup = await openClient(
      alphaChromePort,
      `http://127.0.0.1:${alphaStatic.port}/index.html?${transportQuery}&password=secret${testPacketLogParam}${pageArenaParam}`,
      {
        preNavigationScript: `${preNavigationScript}\nObject.defineProperty(navigator, 'userAgent', {get: () => 'iPhone Mobile'});`,
        waitForWasm: false,
        viewport: { width: 390, height: 720, mobile: true },
      },
    );
    await waitUntil("mobile first-run name and arena form", async () => {
      const form = await startup.startupFormState();
      return form.mobile && form.desktopMessageVisible && form.selectedArena === expectedFirstRunArena;
    });
    assertStartupFormLayout("mobile first-run loader", await startup.startupFormState(), expectedFirstRunArena, {
      mobile: true,
    });
    await startup.close();
    startup = null;

    returning = await openClient(
      alphaChromePort,
      `http://127.0.0.1:${alphaStatic.port}/index.html?${transportQuery}&password=secret${testPacketLogParam}`,
      {
        preNavigationScript: `localStorage.setItem('zubspace.identity.v1', ${JSON.stringify(
          JSON.stringify({ name: "returner", playerId: "123456789012" }),
        )}); localStorage.setItem('zubspace.arena.v1', ${JSON.stringify(expectedStoredArena)});`,
        waitForWasm: false,
      },
    );
    await waitUntil("returning-player join form", async () => {
      const form = await returning.startupFormState();
      return form.formVisible && form.inputVisible && form.buttonVisible;
    });
    const returningForm = await returning.startupFormState();
    assertStartupFormLayout("desktop returning-player loader", returningForm, "chaos-zone");
    if (returningForm.buttonDisabled || returningForm.buttonText !== "Play") {
      throw new Error(`desktop returning-player loader: join button was not ready: ${JSON.stringify(returningForm)}`);
    }
    await returning.submitStartupForm();
    await waitUntil("returning-player join to start WASM", async () => {
      const state = await returning.pageState();
      return state.hasWasm && state.storedArena === "chaos-zone";
    });
    if (smokeArenaSource === "discovery") {
      await waitUntil("public default Chaos join is fully ready", async () => {
        const metrics = await returning.uiMetrics();
        return metrics?.localGameplayState?.authorityPhase === "Live" &&
          metrics.localGameplayState.zoneFilesReady === true &&
          summarizeFrames(returning.frames).receivedMapFilenames.includes("bdhadar.lvl");
      }, zonePublicationTimeoutMs);
      const summary = summarizeFrames(returning.frames);
      publicDefaultEvidence = { ...await returning.runtimeBootstrapState(),
        receivedJoinGameCount: summary.receivedJoinGameCount,
        receivedMapFilenames: summary.receivedMapFilenames };
      if (publicDefaultEvidence.currentArena !== "chaos-zone" || /[?&](?:arena|zone)=/.test(publicDefaultEvidence.currentUrl)) {
        throw new Error(`public default did not remain Chaos without a zone hint: ${JSON.stringify(publicDefaultEvidence)}`);
      }
      await qaRecorder.capture("default-chaos", returning);
    }
    await returning.close();
    returning = null;
    await waitUntil("returning-player peer cleanup", async () => {
      const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
      if (!response.ok) return false;
      const status = await response.json();
      return status.rtc_sessions === 0 && status.clients === 0;
    });

    returning = await openClient(
      alphaChromePort,
      `http://127.0.0.1:${alphaStatic.port}/index.html?${transportQuery}&password=secret${testPacketLogParam}`,
      {
        preNavigationScript: `localStorage.setItem('zubspace.identity.v1', ${JSON.stringify(
          JSON.stringify({ name: "returner", playerId: "123456789012" }),
        )}); localStorage.setItem('zubspace.arena.v1', ${JSON.stringify(expectedStoredArena)});`,
        waitForWasm: false,
        viewport: { width: 390, height: 720, mobile: true },
        mobileIdentity: true,
      },
    );
    await waitUntil("mobile returning-player join form", async () => {
      const form = await returning.startupFormState();
      return form.mobile && form.desktopMessageVisible && form.selectedArena === "chaos-zone";
    });
    const mobileReturningForm = await returning.startupFormState();
    assertStartupFormLayout("mobile returning-player loader", mobileReturningForm, "chaos-zone", { mobile: true });
    await returning.close();
    returning = null;

    if (smokeArenaSource === "discovery") {
      // Discovery still supplies the complete runtime catalog. The public
      // menu remains Chaos-only; a private URL hint selects custom zones.
      const identityScript = name => `${preNavigationScript}
        localStorage.setItem('zubspace.identity.v1', JSON.stringify({name: ${JSON.stringify(name)}, playerId: ${JSON.stringify(name === "alpha" ? "111111111111" : "222222222222")} }));`;
      const clients = [];
      for (const [name, port, staticPort] of [["alpha", alphaChromePort, alphaStatic.port], ["beta", betaChromePort, betaStatic.port]]) {
        const client = await openClient(port,
          `http://127.0.0.1:${staticPort}/index.html?${transportQuery}&password=secret${testPacketLogParam}&zone=${arenaParam}`,
          { preNavigationScript: identityScript(name), waitForWasm: false });
        clients.push(client);
        if (name === "alpha") alpha = client; else beta = client;
        await waitUntil(`${name} runtime catalog ready`, async () => {
          const form = await client.startupFormState();
          return form.zoneCatalogSource === "server" && form.inputVisible && !form.buttonDisabled && form.selectedArena === expectedStoredArena;
        });
      }
      const authoritativeResponse = await fetch(`http://127.0.0.1:${serverPort}/v1/zones`);
      const authoritativeCatalog = await authoritativeResponse.json();
      const forms = await Promise.all(clients.map(client => client.startupFormState()));
      for (const form of forms) {
        if (form.selectVisible || !isDeepStrictEqual(form.zoneCatalog, authoritativeCatalog.zones)) {
          throw new Error(`private URL catalog mismatch: ${JSON.stringify(form)}`);
        }
      }
      const selections = await Promise.all(clients.map(client => client.joinMenuWithMouse()));
      await waitUntil("both discovery-selected clients to start WASM", async () => {
        const states = await Promise.all(clients.map(client => client.runtimeBootstrapState()));
        return states.every(state => state.hasWasm && state.currentArena === expectedStoredArena && state.storedArena === expectedStoredArena);
      });
      const browserRequests = [...alphaStatic.requests, ...betaStatic.requests].filter(request => request.path === "/v1/zones");
      if (!authoritativeResponse.ok || browserRequests.length < 2 || browserRequests.some(request => request.statusCode !== 200)) {
        throw new Error(`runtime /v1/zones discovery failed: ${JSON.stringify(browserRequests)}`);
      }
      zoneDiscoveryEvidence = { source: "server", statusCode: 200, browserRequestCount: browserRequests.length,
        catalog: authoritativeCatalog.zones, publicSelectorAbsent: true, selectionContract: "private-zone-url",
        publicDefault: publicDefaultEvidence,
        selections: { alpha: selections[0], beta: selections[1] } };
    } else {
      alpha = await openClient(
        alphaChromePort,
        `http://127.0.0.1:${alphaStatic.port}/index.html?${transportQuery}&name=alpha&password=secret${testPacketLogParam}${pageArenaParam}`,
        { preNavigationScript },
      );
      if (stopAfterObjectiveAuthoritySmoke) {
        // Serialize the two authenticated map publications in the focused
        // gate. Each browser still establishes an independent real RTC
        // session and validates the production map digest, but a second
        // first-load transfer cannot contend with the first map's reliable
        // bootstrap and make this objective-only boundary depend on timing.
        await waitUntil("focused alpha client to join before beta bootstrap", () =>
          selfJoinBootstrapSettled(summarizeFrames(alpha.frames), "alpha"),
        );
        await waitUntil(
          "focused alpha client to publish its authenticated map before beta bootstrap",
          async () =>
            (await alpha.uiMetrics())?.localGameplayState?.zoneMapReady === true,
          zonePublicationTimeoutMs,
        );
      }
      beta = await openClient(
        betaChromePort,
        `http://127.0.0.1:${betaStatic.port}/index.html?${transportQuery}&name=beta&password=secret${testPacketLogParam}${pageArenaParam}`,
        { preNavigationScript, viewport: { width: 390, height: 720, mobile: true } },
      );
    }

    await waitUntil("both browser clients to join the real server", () => {
      const alphaSummary = summarizeFrames(alpha.frames);
      const betaSummary = summarizeFrames(beta.frames);
      return (
        selfJoinBootstrapSettled(alphaSummary, "alpha") &&
        selfJoinBootstrapSettled(betaSummary, "beta")
      );
    });

    if (forceFreshMapCache) {
      await waitUntil(
        "both isolated clients to complete fresh 0x0c map downloads and cache writes",
        async () => {
          const [alphaCache, betaCache] = await Promise.all([
            alpha.mapCacheStats(),
            beta.mapCacheStats(),
          ]);
          return [
            [alpha, alphaCache],
            [beta, betaCache],
          ].every(
            ([client, cache]) =>
              summarizeFrames(client.frames).sentReliableTypes.includes(0x0c) &&
              (cache?.misses || 0) >= 1 &&
              (cache?.writes || 0) >= 1,
          );
        },
        zonePublicationTimeoutMs,
      );
    }

    await waitUntil(
      "both clients to publish the authenticated map before gameplay",
      async () => {
        const [alphaMetrics, betaMetrics] = await Promise.all([
          alpha.uiMetrics(),
          beta.uiMetrics(),
        ]);
        return [alphaMetrics, betaMetrics].every(
          (metrics) => metrics?.localGameplayState?.zoneMapReady === true,
        );
      },
      zonePublicationTimeoutMs,
    );
    // A peer may request a large reliable map immediately after its own join
    // bootstrap. Another simultaneously joining player's reliable roster
    // fanout can then follow that already-sequenced transfer on this peer's
    // channel. It remains lossless, but cross-peer roster convergence is not
    // part of the first client's self-bootstrap deadline. Require convergence
    // at the same pre-gameplay map boundary instead of misclassifying legal
    // reliable head-of-line ordering as a failed join.
    await waitUntil(
      "both joined clients to converge on the complete peer roster before gameplay",
      () => {
        const alphaSummary = summarizeFrames(alpha.frames);
        const betaSummary = summarizeFrames(beta.frames);
        return mutualRosterConverged(alphaSummary, betaSummary);
      },
      zonePublicationTimeoutMs,
    );
    assertNoDuplicateRosterIds("alpha initial join", summarizeFrames(alpha.frames));
    assertNoDuplicateRosterIds("beta initial join", summarizeFrames(beta.frames));

    if (stopAfterObjectiveAuthoritySmoke) {
      // This is the focused multiplayer-objective boundary. It deliberately
      // does not wait for optional LVZ drain or perform lifecycle input. It
      // holds normal movement keys so objective update and render policy are
      // sampled across real client/server gameplay frames. The independent
      // channel audit below runs only after those focused frames are complete.
      const captureObjectiveFrame = async (client) => {
        const state = await client.pageState();
        const summary = summarizeFrames(client.frames);
        return {
          clientInputSeq: state.uiMetrics?.clientInputSeq ?? null,
          sentPositionCount: summary.sentPositionCount,
          screenshotNonBlackPixels: state.screenshotNonBlackPixels,
          screenshotSampledPixels: state.screenshotSampledPixels,
          localGameplayState: state.uiMetrics?.localGameplayState ?? null,
        };
      };
      const objectiveAuthorityFrames = {
        alpha: [await captureObjectiveFrame(alpha)],
        beta: [await captureObjectiveFrame(beta)],
      };
      const movement = Promise.all([alpha.cruise(1200), beta.cruise(1200)]);
      await sleep(500);
      objectiveAuthorityFrames.alpha.push(await captureObjectiveFrame(alpha));
      objectiveAuthorityFrames.beta.push(await captureObjectiveFrame(beta));
      await movement;
      await waitUntil("focused objective clients to emit keyboard movement frames", () => {
        const alphaSummary = summarizeFrames(alpha.frames);
        const betaSummary = summarizeFrames(beta.frames);
        return alphaSummary.sentPositionCount > objectiveAuthorityFrames.alpha[0].sentPositionCount &&
          betaSummary.sentPositionCount > objectiveAuthorityFrames.beta[0].sentPositionCount;
      });
      await sleep(250);
      objectiveAuthorityFrames.alpha.push(await captureObjectiveFrame(alpha));
      objectiveAuthorityFrames.beta.push(await captureObjectiveFrame(beta));

      // The event channel intentionally carries authenticated one-shot
      // gameplay facts. After retaining the objective-only render frames,
      // complete the normal reliable lifecycle barrier and fire one ordinary
      // keyboard shot per peer as a bounded transport audit stimulus.
      await waitUntil(
        "focused objective clients to finish authenticated zone publication before event probe",
        async () => {
          const states = await Promise.all([alpha.uiMetrics(), beta.uiMetrics()]);
          return states.every(orderedZoneFilesSettledForLifecycle);
        },
        zonePublicationTimeoutMs,
      );
      await Promise.all([
        alpha.enterShipWithLifecycleBarrier(),
        beta.enterShipWithLifecycleBarrier(),
      ]);
      await Promise.all([
        moveOutOfSafeZoneForWeaponProof(alpha),
        moveOutOfSafeZoneForWeaponProof(beta),
      ]);
      await Promise.all([alpha.firePrimary(), beta.firePrimary()]);
      await waitUntil("focused objective clients to exchange all three RTC channels", async () => {
        await Promise.all([alpha.syncFrames(), beta.syncFrames()]);
        return [alpha, beta].every((client) => {
          const audit = channelDeliveryAudit(client.frames);
          return ["zubspace-game", "zubspace-events", "zubspace-reliable"].every(
            (channel) =>
              (audit[channel]?.sent || 0) > 0 &&
              (audit[channel]?.received || 0) > 0,
          );
        });
      });
      const [alphaState, betaState, statusResponse] = await Promise.all([
        alpha.pageState(),
        beta.pageState(),
        fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`),
      ]);
      if (!statusResponse.ok) {
        throw new Error(`objective authority status returned ${statusResponse.status}`);
      }
      const objectiveAuthorityStatus = await statusResponse.json();
      const alphaSummary = summarizeFrames(alpha.frames);
      const betaSummary = summarizeFrames(beta.frames);
      const securityRejects = serverSecurityRejects(server.logs);
      const objectiveAuthoritySecurityAudit = {
        serverSecurityRejects: securityRejects,
        rtcChannelProtocolViolations:
          objectiveAuthorityStatus.rtc_channel_protocol_violations,
      };
      await qaRecorder.capture("alpha-final", alpha);
      await qaRecorder.capture("beta-final", beta);
      console.log(JSON.stringify({
        ok: true,
        transport: "rtc",
        arena: smokeArena,
        expectedMap,
        objectiveAuthorityStatus,
        objectiveAuthorityFrames,
        serverSecurityRejects: securityRejects,
        objectiveAuthoritySecurityAudit,
        qaTraceCaptureAudit: qaRecorder.audit(),
        alpha: {
          mapFilenames: alphaSummary.receivedMapFilenames,
          receivedReliableTypes: alphaSummary.receivedReliableTypes,
          channelDeliveryAudit: channelDeliveryAudit(alpha.frames),
          channelContractAudit: channelContractAudit(alphaState),
          uiMetrics: alphaState.uiMetrics,
        },
        beta: {
          mapFilenames: betaSummary.receivedMapFilenames,
          receivedReliableTypes: betaSummary.receivedReliableTypes,
          channelDeliveryAudit: channelDeliveryAudit(beta.frames),
          channelContractAudit: channelContractAudit(betaState),
          uiMetrics: betaState.uiMetrics,
        },
      }, null, 2));
      return;
    }

    // Digit2 opens a reliable ship-life application barrier. Starting that
    // boundary while the same ordered channel still carries optional LVZ
    // chunks can leave gameplay correctly paused behind the transfer tail.
    // The map-ready marker remains independently observable above; wait only
    // before intentional lifecycle input so this harness measures gameplay,
    // not reliable head-of-line scheduling.
    await waitUntil(
      "both clients to finish ordered zone files before intentional ship lifecycle input",
      async () => {
        const states = await Promise.all([alpha.uiMetrics(), beta.uiMetrics()]);
        return states.every(orderedZoneFilesSettledForLifecycle);
      },
      zonePublicationTimeoutMs,
    );

    for (const [label, client] of [["alpha", alpha], ["beta", beta]]) {
      // Legacy set-coordinates can precede the final random position. The
      // current-life v3 receipt is the accepted pose; retain its original
      // bytes before intentional keys or cache rejoin can advance the life.
      if (!summarizeFrames(client.frames).setCoordinates.length) {
        throw new Error(`${label} did not receive server set-coordinates spawn packet`);
      }
      await waitUntil(`${label} has an exact accepted initial spawn`,
        async () => Boolean(await client.captureInitialSpawnEvidence()), zonePublicationTimeoutMs);
    }

    if (stopAfterMapSmoke) {
      await sleep(Number.isFinite(positionSampleMs) ? Math.max(positionSampleMs, 0) : 1500);
      const alphaState = await alpha.pageState();
      const betaState = await beta.pageState();
      const alphaSummary = summarizeFrames(alpha.frames);
      const betaSummary = summarizeFrames(beta.frames);
      if (
        !alphaSummary.receivedMapFilenames.includes(expectedMap) ||
        !betaSummary.receivedMapFilenames.includes(expectedMap)
      ) {
        throw new Error(
          `expected ${smokeArena} to serve ${expectedMap}; alpha maps=${JSON.stringify(alphaSummary.receivedMapFilenames)} beta maps=${JSON.stringify(betaSummary.receivedMapFilenames)}`,
        );
      }
      const securityRejects = serverSecurityRejects(server.logs);
      if (securityRejects.length > 0) {
        throw new Error(`normal browser/server map smoke triggered server security rejects: ${JSON.stringify(serverSecurityContext(server.logs))}`);
      }
      console.log(
        JSON.stringify(
          {
            ok: true,
            transport: "rtc",
            server: rtcUrl,
            arena: smokeArena,
            arenaSource: smokeArenaSource,
            zoneBundleRoot: smokeZoneRoot || null,
            allowZoneOverrides: smokeAllowZoneOverrides,
            expectedMap,
            expectedSettings,
            serverSecurityRejects: securityRejects,
            tickRate: Number(smokeTickRate),
            alpha: {
              selfId: alphaSummary.selfId,
              settingsProfiles: alphaSummary.settingsProfiles,
              sentPositions: alphaSummary.sentPositionCount,
              mapCacheStats: alphaState.mapCacheStats,
              mapCachePolicyEvidence: alphaState.mapCachePolicyEvidence,
              sentReliableTypes: alphaSummary.sentReliableTypes,
              mapFilenames: alphaSummary.receivedMapFilenames,
              channelDeliveryAudit: channelDeliveryAudit(alpha.frames),
              channelContractAudit: channelContractAudit(alphaState),
            },
            beta: {
              selfId: betaSummary.selfId,
              settingsProfiles: betaSummary.settingsProfiles,
              sentPositions: betaSummary.sentPositionCount,
              mapCacheStats: betaState.mapCacheStats,
              mapCachePolicyEvidence: betaState.mapCachePolicyEvidence,
              sentReliableTypes: betaSummary.sentReliableTypes,
              mapFilenames: betaSummary.receivedMapFilenames,
              channelDeliveryAudit: channelDeliveryAudit(beta.frames),
              channelContractAudit: channelContractAudit(betaState),
            },
          },
          null,
          2,
        ),
      );
      return;
    }

    // drive() begins with genuine Digit2 ship selection and a new random
    // life. Certify the initial receipt's stream before that replacement,
    // rather than comparing its later-life samples to the retired spawn.
    // The application receipt can precede the first ordinary position by a
    // render step. Wait for actual traffic, keeping the oracle's nonempty
    // requirement rather than treating an empty stream as successful.
    await waitUntil("both initial lives to send ordinary position samples", () =>
      summarizeFrames(alpha.frames).sentPositionCount > 0 &&
      summarizeFrames(beta.frames).sentPositionCount > 0, 10_000);
    assertInitialSpawnPositionStream("alpha", summarizeFrames(alpha.frames), alpha.initialSpawnEvidence);
    assertInitialSpawnPositionStream("beta", summarizeFrames(beta.frames), beta.initialSpawnEvidence);
    const positionTrafficStartMs = Date.now();
    let positionExchangeMs = null;
    const [alphaDrive, betaDrive] = await Promise.all([alpha.drive(), beta.drive()]);
    await waitUntil("both clients to exchange real-server position packets", () => {
      const alphaSummary = summarizeFrames(alpha.frames);
      const betaSummary = summarizeFrames(beta.frames);
      const exchanged =
        alphaSummary.sentPositionCount > 0 &&
        betaSummary.sentPositionCount > 0 &&
        alphaSummary.receivedPositionSources.includes(betaSummary.selfId) &&
        betaSummary.receivedPositionSources.includes(alphaSummary.selfId);
      if (exchanged && positionExchangeMs === null) {
        positionExchangeMs = Date.now() - positionTrafficStartMs;
      }
      return exchanged;
    });
    if (requireMapCacheRejoin) {
      // Preserve the real coasting proof, then stop both ships through normal
      // keyboard lifecycle input before a potentially slow LVZ transfer and
      // cache rejoin. Otherwise the surviving peer can coast into geometry
      // while the downloader is replaced, making its later event-channel shot
      // legitimately fail server position validation.
      await sleep(750);
      const alphaBeforeReset = summarizeFrames(alpha.frames);
      const betaBeforeReset = summarizeFrames(beta.frames);
      assertCoastingPositionStream(
        "alpha local before cache rejoin",
        alphaBeforeReset.sentPositions,
        alphaDrive.releasedAt,
      );
      assertCoastingPositionStream(
        "beta local before cache rejoin",
        betaBeforeReset.sentPositions,
        betaDrive.releasedAt,
      );
      assertCoastingPositionRelay(
        "alpha receiving beta before cache rejoin",
        alphaBeforeReset,
        betaBeforeReset.selfId,
        betaDrive.releasedAt,
      );
      assertCoastingPositionRelay(
        "beta receiving alpha before cache rejoin",
        betaBeforeReset,
        alphaBeforeReset.selfId,
        alphaDrive.releasedAt,
      );
      await Promise.all([
        alpha.resetMotionWithShipCycle(),
        beta.resetMotionWithShipCycle(),
      ]);
    }
    assertArenaSettingsProfile("alpha", summarizeFrames(alpha.frames), smokeArena, expectedSettings);
    assertArenaSettingsProfile("beta", summarizeFrames(beta.frames), smokeArena, expectedSettings);
    if (stopAfterPositionSmoke) {
      let currentGenerationWeaponEvidence = null;
      let projectileLifecycleEvidence = null;
      if (requireDynamicLvzChatSound) {
        await waitUntil("an authenticated client to decode and install dynamic LVZ chat audio", async () =>
          Promise.all([alpha.pageState(), beta.pageState()]).then((states) =>
            states.some(
              (state) =>
                (state.uiMetrics?.dynamicLvzChatSoundSources || 0) > 0 &&
                (state.uiMetrics?.dynamicLvzChatSoundsLoaded || 0) > 0,
            ),
          ),
        );
      }
      if (requireThreeChannelTraffic) {
        // Random spawns and the genuine coasting proof do not establish Near
        // membership. Stage real stopped peers before the original-carrier
        // assertion; recovery events must not satisfy it by accident.
        await prepareSmokeWeaponEncounter(alpha, beta, { ...server, zoneRoot: smokeZoneRoot });
        const preFireMetrics = await Promise.all([
          moveOutOfSafeZoneForWeaponProof(alpha),
          moveOutOfSafeZoneForWeaponProof(beta),
        ]);
        const fireOneRemoteVisual = async (
          source,
          target,
          targetBeforeMetrics,
          label,
        ) => {
          const baseline = targetBeforeMetrics?.remoteProjectileSpawns ?? 0;
          const sentWeaponBaseline = summarizeFrames(source.frames).sentPositionWeapons.length;
          await source.firePrimary();
          await waitUntil(`${label} to emit one keyboard weapon position on all RTC channels`, () => {
            const sent = summarizeFrames(source.frames).sentPositionWeapons
              .slice(sentWeaponBaseline)
              .filter((entry) => entry.weapon !== 0);
            if (sent.length > 3) {
              throw new Error(`${label} emitted duplicate keyboard weapon positions: ${JSON.stringify(sent)}`);
            }
            return sent.length === 3 &&
              new Set(sent.map((entry) => entry.channel)).size === 3 &&
              ["zubspace-game", "zubspace-events", "zubspace-reliable"]
                .every((channel) => sent.some((entry) => entry.channel === channel));
          });
          await waitUntil(`${label} to render one accepted remote weapon fact`, async () => {
            const metrics = await target.uiMetrics();
            const current = metrics?.remoteProjectileSpawns ?? 0;
            if (current > baseline + 1) {
              throw new Error(
                `${label} rendered a duplicated remote weapon fact: ${baseline} -> ${current}`,
              );
            }
            return current === baseline + 1;
          });
        };
        await fireOneRemoteVisual(beta, alpha, preFireMetrics[0], "beta to alpha");
        await fireOneRemoteVisual(alpha, beta, preFireMetrics[1], "alpha to beta");
        await waitUntil("both clients to send and receive all three RTC channels", () =>
          [alpha, beta].every((client) => {
            const audit = channelDeliveryAudit(client.frames);
            return ["zubspace-game", "zubspace-events", "zubspace-reliable"].every(
              (channel) =>
                (audit[channel]?.sent || 0) > 0 &&
                (audit[channel]?.received || 0) > 0,
            );
          }),
        );
        await waitUntil("both clients to receive duplicated game/event weapon carriers", () =>
          [alpha, beta].every((client) => {
            const weapons = compactWeaponEntries(
              nonzeroPositionWeapons(summarizeFrames(client.frames)),
            );
            return (
              weapons.uniqueFacts >= 1 &&
              weapons.dualGameEventFacts === weapons.uniqueFacts &&
              weapons.exactlyDualDeliveredFacts === weapons.uniqueFacts &&
              weapons.immutableSourceFacts === weapons.uniqueFacts
            );
          }),
        );
        await waitUntil("both clients to render one current-generation remote weapon fact", async () => {
          const states = await Promise.all([alpha.pageState(), beta.pageState()]);
          return states.every(
            (state, index) =>
              (state.uiMetrics?.remoteProjectileSpawns ?? 0) >=
              (preFireMetrics[index]?.remoteProjectileSpawns ?? 0) + 1,
          );
        });
        const postFireStates = await Promise.all([alpha.pageState(), beta.pageState()]);
        const visualSpawnDeltas = postFireStates.map(
          (state, index) =>
            (state.uiMetrics?.remoteProjectileSpawns ?? 0) -
            (preFireMetrics[index]?.remoteProjectileSpawns ?? 0),
        );
        if (visualSpawnDeltas.some((delta) => delta !== 1)) {
          throw new Error(
            `duplicated current-generation weapon fact was not rendered exactly once: ${JSON.stringify(visualSpawnDeltas)}`,
          );
        }
        currentGenerationWeaponEvidence = {
          visualSpawnDeltas,
          alpha: compactWeaponEntries(
            nonzeroPositionWeapons(summarizeFrames(alpha.frames)),
          ),
          beta: compactWeaponEntries(
            nonzeroPositionWeapons(summarizeFrames(beta.frames)),
          ),
        };
      }
      if (requireProjectileLifecycleIdentity) {
        const target = alpha;
        const source = beta;
        const sourceId = summarizeFrames(source.frames).selfId;
        if (sourceId == null) {
          throw new Error("projectile lifecycle proof lacks the source player id");
        }
        await source.resetMotionWithShipCycle();
        // A lifecycle reset randomizes the source again. Re-establish real
        // Near membership and a stopped256px separation, outside mine prox.
        await prepareSmokeWeaponEncounter(target, source, { ...server, zoneRoot: smokeZoneRoot });
        const sourceBefore = await source.uiMetrics();
        const sourceWeaponBaseline = summarizeFrames(source.frames).sentPositionWeapons.length;
        await source.fireMine();
        await waitUntil("source to emit one keyboard mine position on all RTC channels", () => {
          const sent = summarizeFrames(source.frames).sentPositionWeapons
            .slice(sourceWeaponBaseline)
            .filter((entry) => (entry.weapon & 0x8000) !== 0);
          if (sent.length > 3) {
            throw new Error(`source emitted duplicate keyboard mine positions: ${JSON.stringify(sent)}`);
          }
          return sent.length === 3 &&
            new Set(sent.map((entry) => entry.channel)).size === 3 &&
            ["zubspace-game", "zubspace-events", "zubspace-reliable"]
              .every((channel) => sent.some((entry) => entry.channel === channel));
        });
        let oldMine = null;
        await waitUntil("target to retain an authenticated remote mine source", async () => {
          const metrics = await target.uiMetrics();
          oldMine = (metrics?.remoteProjectileSources || []).find(
            (entry) =>
              entry.kind === "mine" &&
              entry.playerId === sourceId &&
              entry.authenticated === true,
          );
          return !!oldMine;
        });
        await source.resetMotionWithShipCycle();
        let replacementLife = null;
        await waitUntil("target to observe the source replacement life", async () => {
          const metrics = await target.uiMetrics();
          replacementLife = (metrics?.remoteActorPresentation || []).find(
            (entry) => entry.id === sourceId,
          )?.lifeSeq ?? null;
          return replacementLife != null && replacementLife !== oldMine.lifeSeq;
        });
        let cancellation = null;
        await waitUntil("ship cycle to cancel the exact old source without retargeting", async () => {
          const after = await target.uiMetrics();
          cancellation = cancelledProjectileAfterRosterReplacement(oldMine, replacementLife,
            after?.remoteProjectileSources,
            target.frames.received.map(parseRealtimeOwnerWeaponCancellation).filter(Boolean));
          return cancellation != null;
        });
        projectileLifecycleEvidence = {
          formatVersion: 2,
          transition: "intentional-ship-cycle",
          sourcePlayerId: sourceId,
          sourceLifeBeforeReset: sourceBefore?.playerLifeSeq ?? null,
          cancelledSourceSessionId: oldMine.sessionId,
          cancelledSourceLifeSeq: oldMine.lifeSeq,
          cancelledShotGroupId: oldMine.shotGroupId,
          replacementLifeSeq: replacementLife,
          kind: oldMine.kind,
          immutableFields: {
            fireX: oldMine.fireX, fireY: oldMine.fireY,
            fireVelocityX: oldMine.fireVelocityX, fireVelocityY: oldMine.fireVelocityY,
            fireOrientation: oldMine.fireOrientation, frequency: oldMine.frequency,
            ship: oldMine.ship, bombGravity: oldMine.bombGravity, bombBounces: oldMine.bombBounces,
          },
          cancellation,
          removedAfterLifecycleReplacement: true,
        };
      }
      await sleep(Number.isFinite(positionSampleMs) ? Math.max(positionSampleMs, 0) : 3000);
      let alphaState = await alpha.pageState();
      let betaState = await beta.pageState();
      let alphaSummary = summarizeFrames(alpha.frames);
      let betaSummary = summarizeFrames(beta.frames);
      if (
        !alphaSummary.receivedMapFilenames.includes(expectedMap) ||
        !betaSummary.receivedMapFilenames.includes(expectedMap)
      ) {
        throw new Error(
          `expected ${smokeArena} to serve ${expectedMap}; alpha maps=${JSON.stringify(alphaSummary.receivedMapFilenames)} beta maps=${JSON.stringify(betaSummary.receivedMapFilenames)}`,
        );
      }
      assertCoastingPositionStream("alpha local", alphaSummary.sentPositions, alphaDrive.releasedAt);
      assertCoastingPositionStream("beta local", betaSummary.sentPositions, betaDrive.releasedAt);
      assertCoastingPositionRelay("alpha receiving beta", alphaSummary, betaSummary.selfId, betaDrive.releasedAt);
      assertCoastingPositionRelay("beta receiving alpha", betaSummary, alphaSummary.selfId, alphaDrive.releasedAt);
      let cacheRejoinEvidence = null;
      if (requireMapCacheRejoin) {
        const rejoinAlpha = (alphaState.mapCacheStats?.writes || 0) >= 1;
        if (!rejoinAlpha && (betaState.mapCacheStats?.writes || 0) < 1) {
          throw new Error(`neither client persisted the downloaded map before cache rejoin: ${JSON.stringify({
            alpha: alphaState.mapCacheStats,
            beta: betaState.mapCacheStats,
          })}`);
        }
        const rejoinName = rejoinAlpha ? "alpha" : "beta";
        const initialCacheStats = rejoinAlpha
          ? alphaState.mapCacheStats
          : betaState.mapCacheStats;
        const initialSentReliableTypes = rejoinAlpha
          ? alphaSummary.sentReliableTypes
          : betaSummary.sentReliableTypes;
        await qaRecorder.capture("cache-predecessor", rejoinAlpha ? alpha : beta);
        if (rejoinAlpha) {
          await alpha.close();
          alpha = null;
        } else {
          await beta.close();
          beta = null;
        }
        await waitUntil("download client cleanup before map-cache rejoin", async () => {
          const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
          if (!response.ok) return false;
          const status = await response.json();
          return status.rtc_sessions === 1 && status.clients === 1;
        }, 60_000);
        const returningClient = await openClient(
          rejoinAlpha ? alphaChromePort : betaChromePort,
          `http://127.0.0.1:${rejoinAlpha ? alphaStatic.port : betaStatic.port}/index.html?${transportQuery}&name=${rejoinName}&password=secret${testPacketLogParam}${pageArenaParam}`,
          rejoinAlpha
            ? { preNavigationScript }
            : { preNavigationScript, viewport: { width: 390, height: 720, mobile: true } },
        );
        if (rejoinAlpha) alpha = returningClient;
        else beta = returningClient;
        const peerClient = rejoinAlpha ? beta : alpha;
        await waitUntil("returning client authenticated map-cache attestation", async () => {
          const summary = summarizeFrames(returningClient.frames);
          const state = await returningClient.pageState();
          return (
            summary.selfId !== null &&
            summary.receivedJoinGameCount >= 1 &&
            summary.receivedMapFilenames.includes(expectedMap) &&
            summary.sentReliableTypes.includes(0x7c) &&
            (state.mapCacheStats?.hits || 0) >= 1
          );
        });
        const returningSummaryBeforeDrive = summarizeFrames(returningClient.frames);
        const peerReceivedReturningBefore = summarizeFrames(peerClient.frames)
          .receivedPositionSources.filter(
            (source) => source === returningSummaryBeforeDrive.selfId,
          ).length;
        await returningClient.drive();
        await waitUntil("returning cache client to resume authenticated gameplay", () => {
          const rejoined = summarizeFrames(returningClient.frames);
          const currentPeer = summarizeFrames(peerClient.frames);
          return (
            rejoined.sentPositionCount > 0 &&
            currentPeer.receivedPositionSources.filter(
              (source) => source === rejoined.selfId,
            ).length > peerReceivedReturningBefore
          );
        });
        if (requireDynamicLvzChatSound) {
          await waitUntil("returning client to reinstall dynamic LVZ chat audio", async () => {
            const state = await returningClient.pageState();
            return (
              (state.uiMetrics?.dynamicLvzChatSoundSources || 0) > 0 &&
              (state.uiMetrics?.dynamicLvzChatSoundsLoaded || 0) > 0
            );
          });
        }
        if (requireThreeChannelTraffic) {
          await prepareSmokeWeaponEncounter(returningClient, peerClient, { ...server, zoneRoot: smokeZoneRoot });
          // The returning client proves keyboard C2S event delivery. The
          // surviving peer fires concurrently so the returning client also
          // receives a server-routed dedicated weapon event even if its shot is
          // rejected by a just-applied lifecycle/cooldown boundary.
          await Promise.all([returningClient.firePrimary(), peerClient.firePrimary()]);
          await waitUntil("returning client to send and receive all RTC channels", () => {
            const audit = channelDeliveryAudit(returningClient.frames);
            return ["zubspace-game", "zubspace-events", "zubspace-reliable"].every(
              (channel) =>
                (audit[channel]?.sent || 0) > 0 &&
                (audit[channel]?.received || 0) > 0,
            );
          });
          await waitUntil("returning client to receive a duplicated game/event weapon carrier", () => {
            const weapons = compactWeaponEntries(
              nonzeroPositionWeapons(summarizeFrames(returningClient.frames)),
            );
            return (
              weapons.uniqueFacts >= 1 &&
              weapons.dualGameEventFacts === weapons.uniqueFacts &&
              weapons.exactlyDualDeliveredFacts === weapons.uniqueFacts &&
              weapons.immutableSourceFacts === weapons.uniqueFacts
            );
          });
        }
        if (rejoinAlpha) {
          alphaState = await alpha.pageState();
          alphaSummary = summarizeFrames(alpha.frames);
        } else {
          betaState = await beta.pageState();
          betaSummary = summarizeFrames(beta.frames);
        }
        const returningState = rejoinAlpha ? alphaState : betaState;
        const returningSummary = rejoinAlpha ? alphaSummary : betaSummary;
        cacheRejoinEvidence = {
          clientName: rejoinName,
          downloadCacheStats: initialCacheStats,
          initialSentReliableTypes,
          cacheStats: returningState.mapCacheStats,
          cachedMapCheckType: 0x7c,
          sentReliableTypes: returningSummary.sentReliableTypes,
          resumedPositions: returningSummary.sentPositionCount,
        };
      }
      const positionTrafficElapsedMs = Date.now() - positionTrafficStartMs;
      const securityRejects = serverSecurityRejects(server.logs);
      if (securityRejects.length > 0) {
        throw new Error(`normal browser/server smoke triggered server security rejects: ${JSON.stringify(serverSecurityContext(server.logs))}`);
      }
      const browserReliableQueues = {
        alpha: alphaState.uiMetrics?.localGameplayState?.reliableQueuedPackets,
        beta: betaState.uiMetrics?.localGameplayState?.reliableQueuedPackets,
      };
      const alphaChannelDeliveryAudit = channelDeliveryAudit(alpha.frames);
      const betaChannelDeliveryAudit = channelDeliveryAudit(beta.frames);
      if (Object.values(browserReliableQueues).some((queued) => queued !== 0)) {
        throw new Error(
          `browser reliable receive queues did not drain: ${JSON.stringify(browserReliableQueues)}`,
        );
      }
      // Finish the active sessions before retaining the server pressure
      // sample. This proves the complete logical-asset and gameplay tails
      // drained; a clean mid-session instant could otherwise hide a queued
      // transfer that resumes on the next server tick.
      await qaRecorder.capture("alpha-final", alpha);
      await qaRecorder.capture("beta-final", beta);
      await Promise.all([alpha.close(), beta.close()]);
      alpha = null;
      beta = null;
      let cleanupStatus = null;
      await waitUntil("browser clients and RTC queues to drain after evidence capture", async () => {
        const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
        if (!response.ok) return false;
        cleanupStatus = await response.json();
        return cleanupStatus.rtc_sessions === 0 && cleanupStatus.clients === 0;
      }, 60_000);
      const queueDrainEvidence = {
        browserReliableQueues,
        rtcSessions: cleanupStatus.rtc_sessions,
        clients: cleanupStatus.clients,
        serverPressure: assertRtcPressureClean(
          cleanupStatus,
          `${smokeArena} post-client cleanup`,
        ),
        serverLogAudit: assertRtcServerLogClean(
          server.logs,
          `${smokeArena} post-client cleanup`,
        ),
      };
      console.log(
        JSON.stringify(
          {
            ok: true,
            transport: "rtc",
            server: rtcUrl,
            arena: smokeArena,
            arenaSource: smokeArenaSource,
            zoneBundleRoot: smokeZoneRoot || null,
            allowZoneOverrides: smokeAllowZoneOverrides,
            expectedMap,
            expectedSettings,
            serverSecurityRejects: securityRejects,
            tickRate: Number(smokeTickRate),
            positionExchangeMs,
            positionTrafficElapsedMs,
            zoneDiscoveryEvidence,
            cacheRejoinEvidence,
            currentGenerationWeaponEvidence,
            projectileLifecycleEvidence,
            queueDrainEvidence,
            qaTraceCaptureAudit: qaRecorder.audit(),
            alpha: {
              selfId: alphaSummary.selfId,
              settingsProfiles: alphaSummary.settingsProfiles,
              sentPositions: alphaSummary.sentPositionCount,
              positionTraffic: positionTrafficSummary(alphaSummary, positionTrafficElapsedMs),
              mapCacheStats: alphaState.mapCacheStats,
              mapCachePolicyEvidence: alphaState.mapCachePolicyEvidence,
              sentReliableTypes: alphaSummary.sentReliableTypes,
              mapFilenames: alphaSummary.receivedMapFilenames,
              channelDeliveryAudit: alphaChannelDeliveryAudit,
              channelContractAudit: channelContractAudit(alphaState),
              receivedPositionWeapons: compactWeaponEntries(
                nonzeroPositionWeapons(alphaSummary),
              ),
              uiMetrics: alphaState.uiMetrics,
            },
            beta: {
              selfId: betaSummary.selfId,
              settingsProfiles: betaSummary.settingsProfiles,
              sentPositions: betaSummary.sentPositionCount,
              positionTraffic: positionTrafficSummary(betaSummary, positionTrafficElapsedMs),
              mapCacheStats: betaState.mapCacheStats,
              mapCachePolicyEvidence: betaState.mapCachePolicyEvidence,
              sentReliableTypes: betaSummary.sentReliableTypes,
              mapFilenames: betaSummary.receivedMapFilenames,
              channelDeliveryAudit: betaChannelDeliveryAudit,
              channelContractAudit: channelContractAudit(betaState),
              receivedPositionWeapons: compactWeaponEntries(
                nonzeroPositionWeapons(betaSummary),
              ),
              uiMetrics: betaState.uiMetrics,
            },
          },
          null,
          2,
        ),
      );
      return;
    }

    if (remoteWeaponSmokeArenas.has(smokeArena)) {
      await waitUntil("both clients to be in ships for remote-weapon smoke", () => {
        const alphaSummary = summarizeFrames(alpha.frames);
        const betaSummary = summarizeFrames(beta.frames);
        return (
          alphaSummary.selfId !== null &&
          betaSummary.selfId !== null &&
          observedPlayerInShip([alphaSummary, betaSummary], alphaSummary.selfId) &&
          observedPlayerInShip([alphaSummary, betaSummary], betaSummary.selfId)
        );
      });
      await waitUntil("both clients to apply remote players before remote-weapon smoke", async () => {
        const [alphaState, betaState] = await Promise.all([alpha.pageState(), beta.pageState()]);
        // The client's own scope readiness is required here, not only the peer count. A client whose transport is live
        // and whose zone files are complete can still have no granted gameplay scope - in which case it never emits a
        // position, is never offered to its peers, and reports remoteVisiblePlayers from a world it cannot act in. The
        // wait below would then time out on a symptom; this names the cause and reaches the failure artifact.
        return (
          (alphaState.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1 &&
          (betaState.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1 &&
          alphaState.uiMetrics?.localGameplayState?.gameplayScopeReady === true &&
          betaState.uiMetrics?.localGameplayState?.gameplayScopeReady === true
        );
      });
      const lowWeaponTypeMatches = (weaponTypes) => (entry) =>
        weaponTypes.includes(entry.weapon & 0x001f);
      const bombWeaponMatches = (entry) =>
        [3, 4].includes(entry.weapon & 0x001f) && (entry.weapon & 0x8000) === 0;
      const mineWeaponMatches = (entry) =>
        [3, 4].includes(entry.weapon & 0x001f) && (entry.weapon & 0x8000) !== 0;

      async function assertRemoteWeapon({ label, weapon, matchesWeapon, metric }) {
        const betaStateBefore = await beta.pageState();
        const betaAudioStartsBefore = betaStateBefore.audioProbe?.starts ?? 0;
        const betaMetricBefore = betaStateBefore.uiMetrics?.[metric] ?? 0;
        for (let attempt = 0; attempt < 4; attempt += 1) {
          await alpha.sendPositionWeaponForTest(weapon);
          await sleep(250);
          const betaState = await beta.pageState();
          if ((betaState.uiMetrics?.[metric] ?? 0) > betaMetricBefore) {
            break;
          }
        }
        if (
          !summarizeFrames(alpha.frames).sentPositionWeapons.some(matchesWeapon)
        ) {
          throw new Error(`alpha did not send a ${label} weapon in a position packet`);
        }
        const sentWeapons = summarizeFrames(alpha.frames).sentPositionWeapons.filter(matchesWeapon);
        if (!sentWeapons.some((entry) => entry.channel === "zubspace-events")) {
          throw new Error(
            `alpha did not send ${label} weapon over zubspace-events: ${JSON.stringify(sentWeapons)}`,
          );
        }
        await waitUntil(`beta to apply alpha remote ${label}`, async () => {
          const betaState = await beta.pageState();
          return (betaState.uiMetrics?.[metric] ?? 0) > betaMetricBefore;
        });
        await waitUntil(`beta to hear alpha remote ${label} sound`, async () => {
          const betaAudioStarts = (await beta.pageState()).audioProbe?.starts ?? 0;
          return betaAudioStarts > betaAudioStartsBefore;
        });
      }

      await assertRemoteWeapon({
        label: "bullet",
        weapon: 2,
        matchesWeapon: lowWeaponTypeMatches([1, 2]),
        metric: "remoteBulletSpawns",
      });
      await assertRemoteWeapon({
        label: "bomb",
        weapon: 3,
        matchesWeapon: bombWeaponMatches,
        metric: "remoteBombSpawns",
      });
      await assertRemoteWeapon({
        label: "mine",
        weapon: 0x8003,
        matchesWeapon: mineWeaponMatches,
        metric: "remoteMineSpawns",
      });
      await alpha.sendPrizeForTest(21);
      await sleep(250);
      await assertRemoteWeapon({
        label: "repel",
        weapon: 5,
        matchesWeapon: lowWeaponTypeMatches([5]),
        metric: "remoteRepelSpawns",
      });
      if (stopAfterRemoteWeaponSmoke) {
        console.log(
          JSON.stringify(
            {
              ok: true,
              transport: "rtc",
              arena: smokeArena,
              stoppedAfter: "remote-weapon",
              alpha: compactFrameSummary(summarizeFrames(alpha.frames)),
              beta: compactFrameSummary(summarizeFrames(beta.frames)),
              betaUiMetrics: (await beta.pageState()).uiMetrics,
            },
            null,
            2,
          ),
        );
        return;
      }
    }

    await alpha.requestNameChange("alpha2");
    await waitUntil("alpha to reconnect with changed browser player name", async () => {
      await Promise.all([alpha.syncFrames(), beta.syncFrames()]);
      const alphaSummary = summarizeFrames(alpha.frames);
      const betaSummary = summarizeFrames(beta.frames);
      return (
        alphaSummary.sentLoginNames.includes("alpha2") &&
        alphaSummary.rosterNames.includes("alpha2") &&
        betaSummary.rosterNames.includes("alpha2")
      );
    });
    await alpha.unlockAudioForTest();
    let betaIdForPrivate = summarizeFrames(beta.frames).selfId;
    if (betaIdForPrivate === null) {
      throw new Error("beta did not have a player id for private-chat smoke");
    }
    await alpha.sendChatRequestForTest({ chatType: 2, sound: 5, message: "bong" });
    await sleep(300);
    await alpha.sendChatRequestForTest({ chatType: 3, message: "teamgo" });
    await sleep(300);
    await alpha.sendChatRequestForTest({
      chatType: 5,
      target: betaIdForPrivate,
      message: "privatehi",
    });
    await waitUntil("alpha chat commands to encode or route sound, team, and private packets", async () => {
      await Promise.all([alpha.syncFrames(), beta.syncFrames()]);
      const alphaChat = summarizeFrames(alpha.frames).sentChatRequests;
      const betaChat = summarizeFrames(beta.frames).receivedChatPackets;
      return (
        (alphaChat.some((chat) => chat.chatType === 2 && chat.sound === 5 && chat.message === "bong") ||
          betaChat.some((chat) => chat.chatType === 2 && chat.sound === 5 && chat.message === "bong")) &&
        alphaChat.some((chat) => chat.chatType === 3 && chat.sound === 0 && chat.message === "teamgo") &&
        (alphaChat.some(
          (chat) =>
            chat.chatType === 5 && chat.target === betaIdForPrivate && chat.message === "privatehi",
        ) ||
          betaChat.some((chat) => chat.chatType === 5 && chat.message === "privatehi"))
      );
    });
    await waitUntil("beta to receive private chat routed by the real server", () => {
      return summarizeFrames(beta.frames).receivedChatPackets.some(
        (chat) => chat.chatType === 5 && chat.message === "privatehi",
      );
    });

    if (lvzChatSoundSmokeArenas.has(smokeArena)) {
      await sleep(900);
      await beta.sendChatRequestForTest({ chatType: 2, sound: 60, message: "lvzsound" });
      await waitUntil("Chaos LVZ chat sound to route", () => {
        const alphaSummaryForLvzChat = summarizeFrames(alpha.frames);
        const betaSummaryForLvzChat = summarizeFrames(beta.frames);
        const routedLvzChatSound = [alphaSummaryForLvzChat, betaSummaryForLvzChat].some((summary) =>
          summary.receivedChatPackets.some(
            (chat) => chat.chatType === 2 && chat.sound === 60 && chat.message === "lvzsound",
          ),
        );
        return (
          betaSummaryForLvzChat.sentChatRequests.some(
            (chat) => chat.chatType === 2 && chat.sound === 60 && chat.message === "lvzsound",
          ) &&
          routedLvzChatSound
        );
      });
    }

    let alphaState = await alpha.pageState();
    let betaState = await beta.pageState();
    await waitUntil("both clients to reach render-visible remote player metrics", async () => {
      alphaState = await alpha.pageState();
      betaState = await beta.pageState();
      return (
        (alphaState.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1 &&
        (betaState.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1 &&
        alphaState.uiMetrics?.localGameplayState?.gameplayScopeReady === true &&
        betaState.uiMetrics?.localGameplayState?.gameplayScopeReady === true
      );
    });
    const alphaSummary = summarizeFrames(alpha.frames);
    const betaSummary = summarizeFrames(beta.frames);
    const positionTrafficElapsedMs = Date.now() - positionTrafficStartMs;
    assertPage("alpha", alphaState, alpha.events);
    assertPage("beta", betaState, beta.events);
    assertUiMetrics("alpha desktop", alphaState.uiMetrics, { width: 1280, height: 720 });
    assertUiMetrics("beta mobile", betaState.uiMetrics, { width: 390, height: 720 });
    if ((alphaState.uiMetrics?.localBotCount ?? -1) !== 0 || (betaState.uiMetrics?.localBotCount ?? -1) !== 0) {
      throw new Error(
        `networked browser clients should not spawn private local bots: ${JSON.stringify({
          alpha: alphaState.uiMetrics,
          beta: betaState.uiMetrics,
        })}`,
      );
    }
    if (Number(smokeBotCount) > 0) {
      const alphaBots = new Map(alphaSummary.rosters.filter(row => row.id >= serverBotIdBase).map(row => [row.id, row.name]));
      const betaBots = new Map(betaSummary.rosters.filter(row => row.id >= serverBotIdBase).map(row => [row.id, row.name]));
      const expectedCount = Math.min(Number(smokeBotCount), 256);
      if (alphaBots.size !== expectedCount || betaBots.size !== expectedCount ||
          new Set(alphaBots.values()).size !== expectedCount ||
          [...alphaBots].some(([id, name]) => !name || betaBots.get(id) !== name)) {
        throw new Error(`configured unique bot identities were not delivered consistently: ${JSON.stringify({alpha:[...alphaBots],beta:[...betaBots],expectedCount})}`);
      }
      if (
        !alphaSummary.receivedPositionSources.some((id) => id >= serverBotIdBase) ||
        !betaSummary.receivedPositionSources.some((id) => id >= serverBotIdBase)
      ) {
        throw new Error(
          `configured server bot positions were not delivered to both clients: ${JSON.stringify({
            alpha: alphaSummary.receivedPositionSources,
            beta: betaSummary.receivedPositionSources,
          })}`,
        );
      }
    }
    const commonViewportUiMetrics = await assertCommonViewportUiMetrics(alpha);

    if (alphaSummary.selfId === betaSummary.selfId) {
      throw new Error(`server assigned duplicate player ids: ${alphaSummary.selfId}`);
    }
    if (
      !alphaSummary.receivedMapFilenames.includes(expectedMap) ||
      !betaSummary.receivedMapFilenames.includes(expectedMap)
    ) {
      throw new Error(
        `expected ${smokeArena} to serve ${expectedMap}; alpha maps=${JSON.stringify(alphaSummary.receivedMapFilenames)} beta maps=${JSON.stringify(betaSummary.receivedMapFilenames)}`,
      );
    }
    if (alphaState.storedArena !== expectedStoredArena || betaState.storedArena !== expectedStoredArena) {
      throw new Error(
        `expected stored arena ${expectedStoredArena}; alpha=${alphaState.storedArena} beta=${betaState.storedArena}`,
      );
    }
    if (
      ["storage", "discovery"].includes(smokeArenaSource) &&
      (/[?&]arena=/.test(alphaState.currentUrl) || /[?&]arena=/.test(betaState.currentUrl))
    ) {
      throw new Error(
        `storage arena smoke unexpectedly kept arena URL override: alpha=${alphaState.currentUrl} beta=${betaState.currentUrl}`,
      );
    }
    const alphaIdentity = JSON.parse(alphaState.storedIdentity || "null");
    if (alphaIdentity?.name !== "alpha2" || /[?&]name=/.test(alphaState.currentUrl)) {
      throw new Error(`name change did not persist without URL override: ${JSON.stringify(alphaState)}`);
    }
    if (
      !alphaState.audioUnlock?.installed ||
      !alphaState.audioUnlock.gestureSeen ||
      alphaState.audioUnlock.trackedContexts < 1 ||
      alphaState.audioUnlock.resumeFailures > 0
    ) {
      throw new Error(`audio unlock bridge did not track and unlock browser audio: ${JSON.stringify(alphaState.audioUnlock)}`);
    }
    if (
      alphaState.audioProbe.decoded < 10 ||
      alphaState.audioProbe.starts < 1 ||
      betaState.audioProbe.decoded < 10 ||
      betaState.audioProbe.starts < 1
    ) {
      throw new Error(
        `browser audio did not decode and start original sounds: alpha=${JSON.stringify(alphaState.audioProbe)} beta=${JSON.stringify(betaState.audioProbe)}`,
      );
    }
    const securityRejects = serverSecurityRejects(server.logs);
    if (securityRejects.length > 0) {
      throw new Error(`normal browser/server smoke triggered server security rejects: ${JSON.stringify(securityRejects)}`);
    }

    console.log(
      JSON.stringify(
        {
          ok: true,
          transport: "rtc",
          server: rtcUrl,
          arena: smokeArena,
          arenaSource: smokeArenaSource,
          tickRate: Number(smokeTickRate),
          botCount: smokeBotCount ? Number(smokeBotCount) : 0,
          partialPeerFailure: {
            status: partialPeerFailure?.status,
            hasPeer: partialPeerFailure?.hasPeer,
            closedState: partialPeerFailure?.closedState,
          },
          signalingTokenOriginPolicy,
          positionExchangeMs,
          positionTrafficElapsedMs,
          expectedMap,
          expectedStoredArena,
          expectedSettings,
          zoneDiscoveryEvidence,
          serverSecurityRejects: securityRejects,
          alpha: {
            selfId: alphaSummary.selfId,
            rosters: alphaSummary.rosters,
            sentPositions: alphaSummary.sentPositionCount,
            positionTraffic: positionTrafficSummary(alphaSummary, positionTrafficElapsedMs),
            sentPositionSamples: alphaSummary.sentPositions.slice(0, 5),
            receivedPositionSources: compactPositionSources(alphaSummary.receivedPositionSources),
            receivedPositionWeapons: nonzeroPositionWeapons(alphaSummary),
            mapInfoCount: alphaSummary.receivedMapInfoCount,
            mapFilenames: alphaSummary.receivedMapFilenames,
            settingsProfiles: alphaSummary.settingsProfiles,
            fileTransferCount: alphaSummary.receivedFileTransferCount,
            sentLoginNames: alphaSummary.sentLoginNames,
            storedIdentity: alphaIdentity,
            storedArena: alphaState.storedArena,
            audioProbe: alphaState.audioProbe,
            audioUnlock: alphaState.audioUnlock,
            commonViewportUiMetrics,
            nonBlackPixels: alphaState.nonBlackPixels,
            screenshotNonBlackPixels: alphaState.screenshotNonBlackPixels,
          },
          beta: {
            selfId: betaSummary.selfId,
            rosters: betaSummary.rosters,
            sentPositions: betaSummary.sentPositionCount,
            positionTraffic: positionTrafficSummary(betaSummary, positionTrafficElapsedMs),
            sentPositionSamples: betaSummary.sentPositions.slice(0, 5),
            receivedPositionSources: compactPositionSources(betaSummary.receivedPositionSources),
            receivedPositionWeapons: nonzeroPositionWeapons(betaSummary),
            mapInfoCount: betaSummary.receivedMapInfoCount,
            mapFilenames: betaSummary.receivedMapFilenames,
            settingsProfiles: betaSummary.settingsProfiles,
            fileTransferCount: betaSummary.receivedFileTransferCount,
            storedArena: betaState.storedArena,
            audioProbe: betaState.audioProbe,
            nonBlackPixels: betaState.nonBlackPixels,
            screenshotNonBlackPixels: betaState.screenshotNonBlackPixels,
          },
        },
        null,
        2,
      ),
    );
  } catch (error) {
    const qaCaptureErrors = [];
    for (const [role, client] of Object.entries({ alpha, beta, startup, returning, sameOriginDefault })) {
      await qaRecorder.capture(`failure-${role.toLowerCase()}`, client, { required: false })
        .catch((captureError) => qaCaptureErrors.push(String(captureError.message).slice(0, 512)));
    }
    const details = {
      qaTraceCaptureAudit: qaRecorder.audit(),
      qaCaptureErrors,
      alpha: alpha ? compactFrameSummary(summarizeFrames(alpha.frames)) : null,
      beta: beta ? compactFrameSummary(summarizeFrames(beta.frames)) : null,
      alphaState: alpha ? await alpha.pageState().catch((stateError) => String(stateError)) : null,
      betaState: beta ? await beta.pageState().catch((stateError) => String(stateError)) : null,
      alphaEvents: alpha?.events ?? null,
      betaEvents: beta?.events ?? null,
      alphaLanes: alpha
        ? await alpha
            .evaluate("JSON.stringify(window.__zubspaceLaneIngress || {})")
            .catch(() => null)
        : null,
      betaLanes: beta
        ? await beta
            .evaluate("JSON.stringify(window.__zubspaceLaneIngress || {})")
            .catch(() => null)
        : null,
      serverFileTransferTally: (() => {
        // Tally in the harness, from the full in-memory server log, rather than reading marker lines back out of the
        // rendered artifact. The artifact is a truncated string, so a per-session file list reconstructed from it can be an
        // undercount - and an undercount is exactly how "the client stopped requesting at file 4" was nearly concluded from a
        // rendering limit instead of from the server.
        const lines = (server?.logs ?? []).map((line) => String(line));
        const filesBySession = (marker) => {
          const map = new Map();
          for (const line of lines) {
            if (!line.includes(marker)) continue;
            const session = /session=PlayerSessionId\((\d+)\)/.exec(line)?.[1] ?? "?";
            const file = /file_index=(\d+)/.exec(line)?.[1];
            if (!file) continue;
            if (!map.has(session)) map.set(session, new Set());
            map.get(session).add(Number(file));
          }
          return Object.fromEntries(
            [...map].map(([session, files]) => [session, [...files].sort((left, right) => left - right)]),
          );
        };
        const requestedBySession = filesBySession("zone_file_request_accepted");
        const completedBySession = filesBySession("zone_file_transfer_complete");
        const missing = {};
        for (const [session, requested] of Object.entries(requestedBySession)) {
          const completed = new Set(completedBySession[session] ?? []);
          missing[session] = requested.filter((file) => !completed.has(file));
        }
        // Slow and stuck look identical in a file list: both end with the last two files never finishing. What separates
        // them is time - whether the session was still completing files moments before the run gave up or had fallen silent
        // long before. The server writes RFC 3339 timestamps and this process shares the clock, so the gap is computed here
        // instead of being guessed from the order of markers.
        const epochMs = (line) => {
          const stamp = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z)/.exec(line)?.[1];
          return stamp ? Date.parse(stamp) : null;
        };
        const timingBySession = new Map();
        for (const line of lines) {
          const isRequest = line.includes("zone_file_request_accepted");
          const isComplete = line.includes("zone_file_transfer_complete");
          if (!isRequest && !isComplete) continue;
          const session = /session=PlayerSessionId\((\d+)\)/.exec(line)?.[1] ?? "?";
          const when = epochMs(line);
          if (when === null) continue;
          if (!timingBySession.has(session)) {
            timingBySession.set(session, { firstRequestMs: null, lastCompleteMs: null });
          }
          const entry = timingBySession.get(session);
          if (isRequest && (entry.firstRequestMs === null || when < entry.firstRequestMs)) {
            entry.firstRequestMs = when;
          }
          if (isComplete && (entry.lastCompleteMs === null || when > entry.lastCompleteMs)) {
            entry.lastCompleteMs = when;
          }
        }
        const nowMs = Date.now();
        const timing = Object.fromEntries(
          [...timingBySession].map(([session, entry]) => [
            session,
            {
              firstRequestToLastCompleteMs:
                entry.firstRequestMs !== null && entry.lastCompleteMs !== null
                  ? entry.lastCompleteMs - entry.firstRequestMs
                  : null,
              msSinceLastComplete:
                entry.lastCompleteMs !== null ? nowMs - entry.lastCompleteMs : null,
            },
          ]),
        );
        return {
          requestedBySession,
          completedBySession,
          timingBySession: timing,
          requestedButNotCompleted: missing,
          refusals: lines.filter((line) => line.includes("zone_file_request_refused")).length,
          stalledTerminals: lines.filter((line) => line.includes("transfer_window_stalled_terminal")).length,
          capacityBlocked: lines.filter((line) => line.includes("reliable_capacity_blocked")).length,
        };
      })(),
      alphaTransport: alpha
        ? await alpha
            .evaluate(
              "(async () => JSON.stringify(await (window.__zubspaceRtcStatsForTest " +
                "? window.__zubspaceRtcStatsForTest() : { error: 'no accessor' })))()",
            )
            .catch((statsError) => String(statsError))
        : null,
      betaTransport: beta
        ? await beta
            .evaluate(
              "(async () => JSON.stringify(await (window.__zubspaceRtcStatsForTest " +
                "? window.__zubspaceRtcStatsForTest() : { error: 'no accessor' })))()",
            )
            .catch((statsError) => String(statsError))
        : null,
      serverTransferStallDiagnostics: serverTransferStallDiagnostics(server.logs),
      serverSecurity: serverSecurityContext(server.logs),
      serverWeapons: serverWeaponContext(server.logs),
      serverLogs: server.logs.join("").slice(-4_000),
    };
    error.message = `${error.message}\n${JSON.stringify(details, null, 2)}`;
    throw error;
  } finally {
    if (sameOriginDefault) await sameOriginDefault.close().catch(() => {});
    if (startup) await startup.close().catch(() => {});
    if (returning) await returning.close().catch(() => {});
    if (alpha) await alpha.close().catch(() => {});
    if (beta) await beta.close().catch(() => {});
    alphaStatic.server.close();
    betaStatic.server.close();
    await stopProcess(server.child);
    await stopProcess(alphaChrome.child);
    await stopProcess(betaChrome.child);
    await removeDirectory(alphaChrome.userDataDir);
    await removeDirectory(betaChrome.userDataDir);
  }
}

async function main() {
  assertCleanGateEnvironment("real-server smoke", {});
  assertMachineHeadroom("real-server smoke", {});
  const arenas =
    smokeArenaSetting === "all"
      ? ["chaos-zone", "extreme"]
      : [smokeArenaSetting];
  for (const arena of arenas) {
    await runSmoke(arena);
  }
}

main().catch((error) => {
  console.error(error);
  // Let the diagnostic stream drain. Forced exit truncated the retained
  // dynamic-zone failure at328,896 bytes, in the middle of its JSON body.
  process.exitCode = 1;
});

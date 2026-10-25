import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const limitsPath = path.join(repoRoot, "src/bin/zubspace_server/limits.rs");

export function maxPlayerClientsFromRustLimits(source) {
  const match = source.match(
    /pub\(crate\) const SERVER_BOT_ID_BASE:\s*u16\s*=\s*(0x[0-9a-f]+|[0-9]+)\s*;/i,
  );
  if (!match) {
    throw new Error("could not read SERVER_BOT_ID_BASE from the authoritative Rust limits");
  }
  const value = Number.parseInt(match[1], match[1].toLowerCase().startsWith("0x") ? 16 : 10);
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new Error(`invalid SERVER_BOT_ID_BASE in authoritative Rust limits: ${match[1]}`);
  }
  return value;
}

export const MAX_PLAYER_CLIENTS = maxPlayerClientsFromRustLimits(
  fs.readFileSync(limitsPath, "utf8"),
);

export function parseCapacityClientCount(raw) {
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value < 1 || value > MAX_PLAYER_CLIENTS) {
    throw new Error(
      `ZUBSPACE_RTC_LOAD_CLIENTS must be an integer in 1..${MAX_PLAYER_CLIENTS}, got ${value}`,
    );
  }
  return value;
}

const candidateTypes = new Set(["host", "srflx", "prflx", "relay"]);

export function parseExpectedCandidatePaths(raw, fallback = "host->host") {
  const paths = (raw || fallback)
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
  if (paths.length === 0) {
    throw new Error("at least one expected ICE candidate path is required");
  }
  for (const candidatePath of paths) {
    const [localType, remoteType, extra] = candidatePath.split("->");
    if (extra !== undefined || !candidateTypes.has(localType) || !candidateTypes.has(remoteType)) {
      throw new Error(
        `invalid ICE candidate path ${JSON.stringify(candidatePath)}; expected local->remote using host|srflx|prflx|relay`,
      );
    }
  }
  return [...new Set(paths)];
}

export function capacityExpectedCandidatePaths(raw, { icePolicy = "all", remoteMode = false } = {}) {
  if (remoteMode && !raw) {
    throw new Error(
      "ZUBSPACE_RTC_LOAD_OFFER_URL requires ZUBSPACE_RTC_LOAD_EXPECT_CANDIDATE_PATHS so a remote run cannot pass without proving its selected ICE path",
    );
  }
  return parseExpectedCandidatePaths(
    raw,
    icePolicy === "relay" ? "relay->relay" : "host->prflx",
  );
}

export function capacityBrowserUrl(raw, { remoteMode = false } = {}) {
  const value = (raw || "").trim();
  if (remoteMode && !value) {
    throw new Error(
      "ZUBSPACE_RTC_LOAD_OFFER_URL requires ZUBSPACE_RTC_LOAD_BROWSER_URL on an allowed browser origin",
    );
  }
  return value || "about:blank";
}

export function assertRtcUdpPortOwnership(status, expected, phase) {
  const actual = status?.rtc_udp_ports_in_use;
  if (!Number.isSafeInteger(actual) || actual < 0) {
    throw new Error(
      `fixed UDP port ownership invariant failed ${phase}: expected rtc_udp_ports_in_use=${expected}, but /v1/status reported ${JSON.stringify(actual)}; the capacity server must expose a non-negative integer and use one exclusive fixed UDP port per RTC peer`,
    );
  }
  if (actual !== expected) {
    throw new Error(
      `fixed UDP port ownership invariant failed ${phase}: expected rtc_udp_ports_in_use=${expected} (one exclusive fixed UDP port per RTC peer), got ${actual}: ${JSON.stringify(status)}`,
    );
  }
  return status;
}

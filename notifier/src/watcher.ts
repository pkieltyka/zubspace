// Polls a /v1/players list and announces human players joining or leaving a
// zone, with how long a leaving player stayed. Bots and names matching
// `ignore_names` are never announced.
import type { Config } from "./config.ts";
import type { Send } from "./providers.ts";

export interface Presence {
  name: string;
  zone: string;
}

export interface Change extends Presence {
  kind: "joined" | "left";
  /** A leave's time in the zone; unknown for players already online at the baseline. */
  seconds?: number;
}

export interface WatcherDeps {
  fetch?: typeof fetch;
  send: Send;
  log: (line: string) => void;
  now?: () => number;
}

const REQUEST_TIMEOUT_MS = 10_000;
const ZONE_NAMES_RETRY_MS = 60_000;

/** Human players in a /v1/players body, keyed by zone and name (live names are unique). */
export function humans(body: unknown, ignore: readonly RegExp[]): Map<string, Presence> {
  const players = (body as { players?: unknown } | null)?.players;
  if (!Array.isArray(players)) throw new Error("player list response has no players array");
  const result = new Map<string, Presence>();
  for (const player of players) {
    const { name, zone, bot } = player ?? {};
    if (typeof name !== "string" || typeof zone !== "string" || typeof bot !== "boolean") {
      throw new Error("player list response has a malformed player");
    }
    if (bot || ignore.some((pattern) => pattern.test(name))) continue;
    result.set(presenceKey({ name, zone }), { name, zone });
  }
  return result;
}

function presenceKey({ name, zone }: Presence): string {
  return `${zone}\n${name}`;
}

/** Leaves first, then joins, each in name order. */
export function diff(before: Map<string, Presence>, after: Map<string, Presence>): Change[] {
  const missing = (from: Map<string, Presence>, to: Map<string, Presence>, kind: Change["kind"]) =>
    [...from]
      .filter(([key]) => !to.has(key))
      .map(([, presence]): Change => ({ kind, ...presence }))
      .sort((a, b) => a.name.localeCompare(b.name));
  return [...missing(before, after, "left"), ...missing(after, before, "joined")];
}

export function message(change: Change, zoneName: string): string {
  const stay = change.seconds === undefined ? "" : ` after ${minutesAndSeconds(change.seconds)}`;
  return `${change.name} has ${change.kind} ${zoneName}${stay}`;
}

/** 754 -> "12m 34s"; minutes keep counting past the hour. */
export function minutesAndSeconds(seconds: number): string {
  const whole = Math.max(0, Math.round(seconds));
  return `${Math.floor(whole / 60)}m ${whole % 60}s`;
}

/** "chaos-zone" -> "Chaos Zone"; used only when /v1/zones has no display name. */
export function titleCase(id: string): string {
  return id
    .split("-")
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}

export function describe(error: unknown): string {
  if (!(error instanceof Error)) return String(error);
  const cause = error.cause instanceof Error ? `: ${error.cause.message}` : "";
  return `${error.message}${cause}`;
}

export function createWatcher(config: Config, deps: WatcherDeps) {
  const fetchImpl = deps.fetch ?? fetch;
  const now = deps.now ?? Date.now;
  const zonesUrl = new URL("/v1/zones", config.endpoint);
  const zoneNames = new Map<string, string>();
  let zoneNamesFetchedAt = -Infinity;
  let known: Map<string, Presence> | undefined;
  // When each announced join was seen, so its leave can say how long it lasted.
  const joinedAt = new Map<string, number>();

  async function getJson(url: URL, signal: AbortSignal): Promise<unknown> {
    const response = await fetchImpl(url, {
      headers: { accept: "application/json" },
      signal: AbortSignal.any([signal, AbortSignal.timeout(REQUEST_TIMEOUT_MS)]),
    });
    if (!response.ok) {
      await response.body?.cancel();
      throw new Error(`${url.href} answered HTTP ${response.status}`);
    }
    return response.json();
  }

  // Display names are best effort: without them a zone id is title-cased, and
  // an unknown zone retries the lookup at most once a minute.
  async function refreshZoneNames(signal: AbortSignal): Promise<void> {
    zoneNamesFetchedAt = now();
    try {
      const zones = ((await getJson(zonesUrl, signal)) as { zones?: unknown } | null)?.zones;
      if (!Array.isArray(zones)) throw new Error("zone list response has no zones array");
      for (const zone of zones) {
        if (typeof zone?.id === "string" && typeof zone.display_name === "string") {
          zoneNames.set(zone.id, zone.display_name);
        }
      }
    } catch (error) {
      if (signal.aborted) throw error;
      deps.log(`zone names unavailable, using zone ids: ${describe(error)}`);
    }
  }

  return {
    /** One poll. Throws when the player list cannot be read; the last roster is kept. */
    async poll(signal: AbortSignal): Promise<void> {
      const current = humans(await getJson(config.endpoint, signal), config.ignoreNames);
      if (!known) {
        // The first roster is the baseline: players already online when the
        // notifier starts (or restarts) are not announced.
        known = current;
        deps.log(`baseline: ${current.size} human player(s) online`);
        await refreshZoneNames(signal);
        return;
      }
      const changes = diff(known, current);
      known = current;
      if (changes.length === 0) return;
      // Times are as seen by polls, so a stay is accurate to one poll interval.
      const seenAt = now();
      for (const change of changes) {
        const key = presenceKey(change);
        if (change.kind === "joined") {
          joinedAt.set(key, seenAt);
          continue;
        }
        const since = joinedAt.get(key);
        if (since !== undefined) change.seconds = (seenAt - since) / 1000;
        joinedAt.delete(key);
      }
      if (
        changes.some((change) => !zoneNames.has(change.zone)) &&
        now() - zoneNamesFetchedAt >= ZONE_NAMES_RETRY_MS
      ) {
        await refreshZoneNames(signal);
      }
      // Changes seen in one poll share a notification, one line each.
      const text = changes
        .map((change) => message(change, zoneNames.get(change.zone) ?? titleCase(change.zone)))
        .join("\n");
      deps.log(`notify: ${text.replaceAll("\n", " | ")}`);
      try {
        await deps.send(text);
      } catch (error) {
        deps.log(`notification failed: ${describe(error)}`);
      }
    },
  };
}

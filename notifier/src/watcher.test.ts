import assert from "node:assert/strict";
import { test } from "node:test";
import { parseConfig } from "./config.ts";
import { createWatcher, diff, humans, minutesAndSeconds, titleCase } from "./watcher.ts";

const player = (id: number, name: string, bot = false, zone = "chaos-zone") => ({ id, name, bot, zone });
const bots = [player(768, "Luna Tick", true), player(769, "Pinocchio", true)];

test("humans skips bots and ignored names and rejects malformed lists", () => {
  const roster = humans({ total_players: 4, players: [...bots, player(1, "Zub"), player(2, "probe42")] }, [/^probe\d{1,3}$/]);
  assert.deepEqual([...roster.values()], [{ name: "Zub", zone: "chaos-zone" }]);
  assert.throws(() => humans({ total_players: 0 }, []), /no players array/);
  assert.throws(() => humans(null, []), /no players array/);
  assert.throws(() => humans({ players: [{ id: 1, name: "Zub" }] }, []), /malformed player/);
});

test("diff lists leaves before joins, each by name", () => {
  const before = humans({ players: [player(1, "Ann"), player(2, "Cid"), player(3, "Bo")] }, []);
  const after = humans({ players: [player(1, "Ann"), player(4, "Zed"), player(5, "Al"), player(6, "Bo", false, "extreme")] }, []);
  assert.deepEqual(diff(before, after), [
    { kind: "left", name: "Bo", zone: "chaos-zone" },
    { kind: "left", name: "Cid", zone: "chaos-zone" },
    { kind: "joined", name: "Al", zone: "chaos-zone" },
    { kind: "joined", name: "Bo", zone: "extreme" },
    { kind: "joined", name: "Zed", zone: "chaos-zone" },
  ]);
});

test("titleCase turns zone ids into names", () => {
  assert.equal(titleCase("chaos-zone"), "Chaos Zone");
  assert.equal(titleCase("extreme"), "Extreme");
});

test("minutesAndSeconds rounds to whole seconds and keeps counting minutes", () => {
  assert.equal(minutesAndSeconds(0), "0m 0s");
  assert.equal(minutesAndSeconds(42.4), "0m 42s");
  assert.equal(minutesAndSeconds(59.6), "1m 0s");
  assert.equal(minutesAndSeconds(754), "12m 34s");
  assert.equal(minutesAndSeconds(3725), "62m 5s");
});

const zoneList = { zones: [{ id: "chaos-zone", display_name: "Chaos Zone" }, { id: "extreme", display_name: "Extreme" }] };

/** A fake game server whose roster and zone list the test changes between polls. */
function harness({ zones = zoneList as unknown, failSends = false } = {}) {
  const server = { players: [...bots] as ReturnType<typeof player>[], zones, down: false };
  const clock = { ms: 1_000_000 };
  const sent: string[] = [];
  const logs: string[] = [];
  const send = async (text: string) => {
    if (failSends) throw new Error("ntfy down");
    sent.push(text);
  };
  const fetch = (async (input: string | URL | Request) => {
    const path = new URL(String(input)).pathname;
    if (server.down) throw new TypeError("fetch failed");
    if (path === "/v1/players") return Response.json({ total_players: server.players.length, players: server.players });
    if (path === "/v1/zones" && server.zones) return Response.json(server.zones);
    return new Response("", { status: 404 });
  }) as typeof globalThis.fetch;
  const { config } = parseConfig({
    endpoint: "https://server.test/v1/players",
    ignore_names: ["^probe\\d{1,3}$"],
    provider: "ntfy",
    ntfy: { topic: "zub" },
  });
  const watcher = createWatcher(config, { fetch, send, log: (line) => logs.push(line), now: () => clock.ms });
  const poll = () => watcher.poll(new AbortController().signal);
  return { server, sent, logs, poll, clock };
}

test("players online at startup are a silent baseline; later joins and leaves are announced", async () => {
  const { server, sent, poll, clock } = harness();
  server.players.push(player(1, "Early"));
  await poll();
  assert.deepEqual(sent, []);

  server.players.push(player(2, "Zub"), player(3, "probe7"));
  await poll();
  assert.deepEqual(sent, ["Zub has joined Chaos Zone"]);

  await poll();
  assert.equal(sent.length, 1, "an unchanged roster sends nothing");

  // Early's join was never seen, so only Zub's leave can say how long it stayed.
  clock.ms += 754_000;
  server.players = server.players.filter((p) => p.name !== "Early" && p.name !== "Zub");
  server.players.push(player(4, "Kim", false, "extreme"));
  await poll();
  assert.deepEqual(sent.slice(1), [
    "Early has left Chaos Zone\nZub has left Chaos Zone after 12m 34s\nKim has joined Extreme",
  ]);
});

test("a stay survives a failed poll and starts again in a new zone or on rejoin", async () => {
  const { server, sent, poll, clock } = harness();
  await poll();
  server.players.push(player(1, "Zub"));
  await poll();

  clock.ms += 30_000;
  server.down = true;
  await assert.rejects(poll(), /fetch failed/);
  server.down = false;
  clock.ms += 65_000;
  server.players = [...bots, player(1, "Zub", false, "extreme")];
  await poll();
  assert.equal(sent.at(-1), "Zub has left Chaos Zone after 1m 35s\nZub has joined Extreme");

  clock.ms += 5_000;
  server.players = [...bots];
  await poll();
  clock.ms += 60_000;
  server.players.push(player(2, "Zub", false, "extreme"));
  await poll();
  clock.ms += 9_000;
  server.players = [...bots];
  await poll();
  assert.deepEqual(sent.slice(-3), [
    "Zub has left Extreme after 0m 5s",
    "Zub has joined Extreme",
    "Zub has left Extreme after 0m 9s",
  ]);
});

test("a failed poll keeps the last roster instead of announcing everyone as gone", async () => {
  const { server, sent, poll } = harness();
  server.players.push(player(1, "Zub"));
  await poll();
  server.down = true;
  await assert.rejects(poll(), /fetch failed/);
  server.down = false;
  await poll();
  assert.deepEqual(sent, []);
});

test("without /v1/zones the zone id is title-cased", async () => {
  const { server, sent, logs, poll } = harness({ zones: null });
  await poll();
  server.players.push(player(1, "Zub"));
  await poll();
  assert.deepEqual(sent, ["Zub has joined Chaos Zone"]);
  assert.ok(logs.some((line) => line.startsWith("zone names unavailable")));
});

test("a failed notification is logged, not retried on every later poll", async () => {
  const { server, logs, poll } = harness({ failSends: true });
  await poll();
  server.players.push(player(1, "Zub"));
  await poll();
  await poll();
  assert.deepEqual(
    logs.filter((line) => line.startsWith("notif")),
    ["notify: Zub has joined Chaos Zone", "notification failed: ntfy down"],
  );
});

import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import assert from "node:assert/strict";
import { setTimeout as sleep } from "node:timers/promises";

export function agedServerJoinOptions(env = process.env, arena) {
  const seconds = Number(env.ZUBSPACE_E2E_JOIN_MIN_UPTIME_SECONDS || 0);
  assert.ok(Number.isInteger(seconds) && seconds >= 0 && seconds <= 900,
    "join minimum uptime must be an integer in 0..900 seconds");
  return {
    minimumUptimeSeconds: seconds,
    initialZone: env.ZUBSPACE_E2E_INITIAL_SERVER_ZONE || arena,
  };
}

// This deliberately ages the REAL server before opening the browser. No
// gameplay clock, packet, or application state is injected or advanced.
export async function waitForAgedServerJoin(server, port, options) {
  const started = Date.now();
  const deadline = started + (options.minimumUptimeSeconds + 30) * 1000;
  let lastReport = started;
  for (;;) {
    assert.equal(server.child.exitCode, null, "server must remain live while aging");
    const response = await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`, {
      signal: AbortSignal.timeout(5000),
    });
    assert.ok(response.ok, "server age status must be available");
    const status = await response.json();
    assert.equal(status.zone, options.initialZone);
    assert.ok(Number.isFinite(status.uptime));
    if (status.uptime >= options.minimumUptimeSeconds) {
      return { ...options, observedUptimeSeconds: status.uptime,
        waitedMs: Date.now() - started, bots: status.botPlayers, clients: status.clients };
    }
    assert.ok(Date.now() < deadline, "server did not reach required age");
    if (Date.now() - lastReport >= 30_000) {
      console.log(`Server age: ${status.uptime}/${options.minimumUptimeSeconds}s`);
      lastReport = Date.now();
    }
    await sleep(1000);
  }
}

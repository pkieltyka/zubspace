import assert from "node:assert/strict";
import test from "node:test";
import { serverLogField, serverLogMessage } from "../tools/server_log_fields.mjs";
import { parseZubspaceEvents, parseServerDeathEvents } from "../tools/server_events.mjs";
import { assertSecurityRejectAudit, parseStructuredSecurityRejects, driverDataChannelFullSummary } from "./e2e/combat_replay.mjs";
import { rtcServerLogAudit } from "../tools/rtc_gate_audit.mjs";
const envelope = fields => JSON.stringify({ timestamp: "2000-01-01T23:00:00.123456Z", level: "INFO", target: "zubspace_server", fields });

test("committed deaths accept production INFO fields and deduplicate legacy DEBUG copies", () => {
  const fields = { damage_seq: 5, projectile_id: 19, server_tick: 1234, killer_id: 7,
    victim_id: 8, victim_life_seq: 3, victim_next_life_seq: 4, killer_life_seq: 2,
    weapon: 66, energy_before: 120, energy_after: 0, killed_bounty: 30,
    flag_transfer: 0, green_id: -1, green_collectible_id: 0, respawn_delay_ms: 2000 };
  const info = envelope({ ...fields, respawn_delay_ms: "2000", message: "server player death" });
  const legacy = `DEBUG zubspace_event ${JSON.stringify({ type: "server_player_death", ...fields })}`;
  const first = parseServerDeathEvents(info);
  assert.equal(first.length, 1);
  assert.equal(first[0].killedBounty, 30);
  assert.equal(first[0].damageSeq, 5);
  assert.equal(first[0].respawnDelayMs, 2000);
  for (const log of [legacy, `${info}\n${legacy}\n`, `${legacy}\n${info}\n`]) {
    assert.deepEqual(parseServerDeathEvents(log), first);
  }
  const split = `${info}\n${legacy}\n`;
  assert.deepEqual(parseServerDeathEvents([split.slice(0, 47), split.slice(47)]), first);
  assert.throws(() => parseServerDeathEvents(`${info}\n${envelope({ ...fields,
    killed_bounty: 99, message: "server player death" })}`), /conflicting committed death/);
  assert.deepEqual(parseServerDeathEvents(envelope({ message: "server player death", damage_seq: null })), []);
  assert.throws(() => parseServerDeathEvents(envelope({ ...fields,
    killed_bounty: "invalid", message: "server player death" })), /killed_bounty is not finite/);
});

test("combat events preserve payload and offsets in both tracing formats", () => {
  const event = { type: "server_projectile_spawned", owner_id: 7, note: 'quoted "value"' };
  const text = `INFO zubspace_event ${JSON.stringify(event)}`;
  const json = envelope({ message: `zubspace_event ${JSON.stringify(event)}` });
  const parsed = parseZubspaceEvents([`${text}\n${json}\n{\"fields\": malformed unrelated transport line\n`]);
  assert.deepEqual(parsed.map(row => row.event), [event, event]);
  assert.equal(parsed[1].logIndex, text.length + 1);
  assert.throws(() => parseZubspaceEvents(envelope({ message: "zubspace_event {broken" })), /malformed zubspace_event/);
});

test("JSON security rejects retain exact category, reason, identity and whitelist counts", () => {
  const line = envelope({ message: "server security reject", player_id: 7, category: "position", reason: "future_reason" });
  const server = { logs: [line] };
  assert.deepEqual(parseStructuredSecurityRejects(server), [{ playerId: 7, category: "position", reason: "future_reason", line }]);
  assert.throws(() => assertSecurityRejectAudit(server), /fail-closed/);
  assert.equal(assertSecurityRejectAudit(server, 0, [{ playerId: 7, category: "position", reason: "future_reason", count: 1 }]).total, 1);
  assert.throws(() => assertSecurityRejectAudit({ logs: [envelope({ message: "server security reject", player_id: 7, category: "position" })] }), /missing-reason/);
});

test("JSON channel announcements still attribute full driver queues", () => {
  const server = { logs: [envelope({ message: "server rtc data channel announced", channel_id: 5, label: "zubspace-game" }), "\nFailed to send DataChannelMessage to data channel 5: Full"] };
  const expected = { game: 1, event: 0, reliable: 0, unknown: 0, total: 1 };
  assert.deepEqual(driverDataChannelFullSummary(server), expected);
  assert.deepEqual(rtcServerLogAudit(server.logs).driverFull, expected);
});

test("structured readers use own typed fields and reject malformed envelopes", () => {
  const line = envelope({ message: "ready", rtc: "http://127.0.0.1:9001/v1/rtc/offer", player_id: 3 });
  assert.equal(serverLogField(line, "player_id"), 3);
  assert.equal(serverLogField(line, "constructor"), undefined);
  assert.equal(serverLogMessage(line), "ready");
  assert.equal(serverLogMessage("plain server message"), "plain server message");
  assert.throws(() => serverLogField('{"fields":[]}', "rtc"), /malformed/);
});

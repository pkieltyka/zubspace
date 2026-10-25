import assert from "node:assert/strict";
import test from "node:test";

import {
  assertSecurityRejectAudit,
  parseStructuredSecurityRejects,
} from "./combat_replay.mjs";

function fixtureServer(lines) {
  return { logs: [`prefix\n${lines.join("\n")}\n`] };
}

test("security audit fails closed on an unknown dynamic reason", () => {
  const server = fixtureServer([
    'WARN server security reject player_id=7 category="position" reason="future_new_reason"',
  ]);
  assert.deepEqual(parseStructuredSecurityRejects(server), [{
    playerId: 7,
    category: "position",
    reason: "future_new_reason",
    line: 'WARN server security reject player_id=7 category="position" reason="future_new_reason"',
  }]);
  assert.throws(
    () => assertSecurityRejectAudit(server),
    /fail-closed server security rejection audit failed/,
  );
});

test("security audit whitelist is exact in reason count and player context", () => {
  const server = fixtureServer([
    'WARN server security reject player_id=11 category="weapon" reason="deliberate_fixture"',
  ]);
  assert.equal(assertSecurityRejectAudit(server, 0, [{
    category: "weapon",
    reason: "deliberate_fixture",
    playerId: 11,
    count: 1,
  }]).total, 1);
  assert.throws(() => assertSecurityRejectAudit(server, 0, [{
    category: "weapon",
    reason: "deliberate_fixture",
    playerId: 12,
    count: 1,
  }]), /fail-closed/);
  assert.throws(() => assertSecurityRejectAudit(server, 0, [{
    category: "weapon",
    reason: "deliberate_fixture",
    playerId: 11,
    count: 2,
  }]), /fail-closed/);
});

test("security audit rejects malformed structured records rather than skipping them", () => {
  const server = fixtureServer([
    "WARN server security reject player_id=5 category=position",
    "INFO server security reject summary position/teleport=1 total=1",
  ]);
  const records = parseStructuredSecurityRejects(server);
  assert.equal(records.length, 1);
  assert.equal(records[0].reason, "<missing-reason>");
  assert.throws(() => assertSecurityRejectAudit(server), /missing-reason/);
});

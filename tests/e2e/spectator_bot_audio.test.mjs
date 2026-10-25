import assert from 'node:assert/strict';
import test from 'node:test';
import {parseBotShots,createBotShotReader} from './spectator_bot_audio.mjs';
const row=(session,group)=>`{"timestamp":"2000-01-01T17:00:00Z","fields":{"event":"bot_weapon_committed","owner_id":768,"owner_life_seq":1,"owner_session_id":${session},"shot_group_id":${group}}}\n`;

test('bot audio evidence preserves adjacent u64 identities that JSON Number aliases',()=>{
  const rows=parseBotShots([row('18446744073709551614','9007199254740992'),row('18446744073709551615','9007199254740993')]);
  assert.notEqual(rows[0].owner_session_id,rows[1].owner_session_id);
  assert.equal(rows[1].owner_session_id,'18446744073709551615');
  assert.ok(BigInt(rows[1].shot_group_id)>BigInt(rows[0].shot_group_id));
});

test('partial log chunks cannot invent or truncate a completed autonomous shot',()=>{
  const text=row('18446744073709551615','7');
  assert.equal(parseBotShots([text.slice(0,-2)]).length,0);
  assert.equal(parseBotShots([text.slice(0,40),text.slice(40)]).length,1);
});
test('incremental shot evidence retains exact IDs and consumes each complete line once',()=>{
  const logs=[],read=createBotShotReader(logs),text=row('18446744073709551615','9007199254740993');
  assert.equal(read().length,0);logs.push(text.slice(0,37));assert.equal(read().length,0);
  logs.push(text.slice(37));assert.equal(read().length,1);assert.equal(read().length,1);
  logs.push(row('18446744073709551614','9007199254740994'));
  assert.deepEqual(read().map(r=>r.shot_group_id),['9007199254740993','9007199254740994']);
  logs.length=0;assert.throws(read,/append-only/);
});

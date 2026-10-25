import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import {outsideAoiRespawnEvidence} from './bot_respawn_aoi.mjs';
const death={serverTick:5000,victimId:768,victimNextLifeSeq:3,respawnDelayMs:2000};
const observed={latestSnapshotServerTick:6000,playerId:768,lifeSeq:3};
function fields(tick,distance=4000){return {message:'server bot movement trace',server_tick:tick,
  bot_id:768,life_seq:3,x:0,y:0,velocity_x:0,velocity_y:0,
  target_player_id:0,target_x:distance,target_y:0,assigned_human_status:0};}
function trace(){return Array.from({length:82},(_,i)=>fields(5200+i*10,i<75?4000:2000));}
const json=rows=>rows.map(fields=>JSON.stringify({level:'INFO',fields})).join('\n');
const evaluate=rows=>outsideAoiRespawnEvidence(json(rows),death,observed,0);
test('far respawn retains the original timer and bounds presentation from mandatory near entry',()=>{
 const e=evaluate(trace());assert.ok(e);assert.equal(e.firstAliveTrace.elapsedTicks,200);
 assert.equal(e.firstMandatoryNearTrace.serverTick,5950);assert.equal(e.observedElapsedTicks,1000);
 assert.equal(e.traceSamples,82);
 const early=trace();early[0]=fields(5199);assert.equal(evaluate(early),null);
 assert.equal(evaluate(trace().filter(x=>x.server_tick>5350)),null,'late spawn remains a failure');
 const late=trace().map(x=>({...x,target_x:x.server_tick>=5800?2000:4000}));
 assert.equal(evaluate(late),null,'a late in-range presentation cannot use the out-of-range exception');
 const near=trace();near[0].target_x=2306;assert.equal(evaluate(near),null,'birth must exceed the worst possible near envelope');
 const fast=trace();fast[0].target_x=2500;fast[0].velocity_x=2000;
 assert.equal(evaluate(fast),null,'fast closing motion must enlarge the conservative near envelope');
});
test('AOI proof requires uninterrupted exact-life observations of this same live observer',()=>{
 for(const change of [rows=>rows.splice(5,4),rows=>{rows[5].target_player_id=1;},
   rows=>{rows[5].assigned_human_status=1;},rows=>{rows[5].target_x=NaN;},
   rows=>{delete rows[5].velocity_x;},
   rows=>rows.splice(70),rows=>{rows[5].life_seq=4;}]){
  const rows=trace();change(rows);assert.equal(evaluate(rows),null);
 }
 assert.equal(outsideAoiRespawnEvidence(json(trace()),death,{...observed,lifeSeq:4},0),null);
 assert.equal(outsideAoiRespawnEvidence(json(trace()),death,{...observed,playerId:769},0),null);
 assert.equal(outsideAoiRespawnEvidence(json(trace()),death,{...observed,latestSnapshotServerTick:5199},0),null);
});
test('typed text and JSON logs yield the same proof, while quoted text cannot spoof one',()=>{
 const rows=trace();const text=rows.map(r=>'2000-01-01T00:00:00Z  INFO server bot movement trace '+
 Object.entries(r).filter(([k])=>k!=='message').map(([k,v])=>`${k}=${v}`).join(' ')).join('\n');
 assert.deepEqual(outsideAoiRespawnEvidence(text,death,observed,0),evaluate(rows));
 assert.equal(outsideAoiRespawnEvidence(JSON.stringify({level:'INFO',fields:{message:text}}),death,observed,0),null);
});
test('the test envelope remains bound to committed AOI constants and preserves the 150-tick slack',()=>{
 const source=fs.readFileSync('src/bin/zubspace_server/visibility.rs','utf8');
 assert.match(source,/NEAR_AOI_RADIUS_PX: f32 = 2_048\.0/);
 assert.match(source,/MID_AOI_RADIUS_PX: f32 = 2_560\.0/);
 assert.match(source,/AOI_HYSTERESIS_MARGIN_PX: f32 = 256\.0/);
 assert.match(source,/AOI_PREDICTIVE_LEAD_MAX_PX: f32 = QUADRANT_SIZE_PX as f32/);
 assert.match(source,/QUADRANT_SIZE_PX: u16 = 1_024/);
 const caller=fs.readFileSync('tests/e2e/bot_combat_evidence.mjs','utf8');
 assert.match(caller,/BOT_RESPAWN_PRESENTATION_SLACK_TICKS = 150/);
});
test('native Continuum31-bit tick wrap preserves the same elapsed deadlines',()=>{
 const shift=value=>(value+0x7fff_ff00-5000)&0x7fff_ffff;
 const rows=trace().map(r=>({...r,server_tick:shift(r.server_tick)}));
 const e=outsideAoiRespawnEvidence(json(rows),{...death,serverTick:shift(death.serverTick)},
   {...observed,latestSnapshotServerTick:shift(observed.latestSnapshotServerTick)},0);
 assert.ok(e);assert.equal(e.firstAliveTrace.elapsedTicks,200);assert.equal(e.observedElapsedTicks,1000);
});

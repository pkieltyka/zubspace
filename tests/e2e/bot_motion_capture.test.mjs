import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import {installBrowserBotMotionCapture} from './bot_motion_capture.mjs';
import {captureBotPresentationState,remoteActorMotionAcrossStatesEvidence} from './bot_combat_evidence.mjs';
import {reenteredDoorReceipt} from './bot_motion_boundaries.mjs';

const frame=(n,{initial=0,snaps=0,present=true,reason=0,x=8000}={})=>({
 at:1000+n*16,monotonicMs:n*16,renderOrdinal:n,
 combatViewTimeline:{generation:1,gameAppliedServerTick:100+n*2,presentationRenderTick:95+n*2},
 remoteActorPresentation:[{id:769,lifeSeq:1,initialCount:initial,snapCount:snaps,
  lastSnapReason:reason,positionPresent:present,renderPositionPresent:present,lastSeenSnapshotId:n+1}],
 remotePlayers:present?[{id:769,lifeSeq:1,positionX:x,positionY:8000,displayX:x,displayY:8000,
  lastServerPositionTick:100+n*2,velocityX:0,velocityY:0}]:[]});
const context=()=>vm.createContext({window:{__zubspaceMetricsTrace:[frame(0)]},performance,Date,structuredClone});
const install=(c,limit=4096)=>vm.runInContext(`(${installBrowserBotMotionCapture.toString()})([769],${limit})`,c);
const actor=c=>({evaluate:async expression=>vm.runInContext(expression,c)});

test('boundary capture preserves push and eviction, and observes AOI and relocation before ring expiry',async()=>{
 const c=context();install(c);const ring=c.window.__zubspaceMetricsTrace;
 const frames=[frame(1,{present:false}),frame(2,{initial:1,reason:1}),
  frame(3,{initial:1,snaps:1,reason:3,x:8500})];
 for(const f of frames){assert.equal(ring.push(f),2);ring.splice(0,1);}
 for(let n=4;n<100;n++){ring.push(frame(n,{initial:1,snaps:1,reason:3,x:8500}));ring.splice(0,1);}
 assert.equal(ring.length,1);assert.equal(ring[0].renderOrdinal,99);
 const captured=await captureBotPresentationState(actor(c),[769]);
 assert.deepEqual(Array.from(captured.motionBoundaryTrace,f=>f.renderOrdinal),[0,1,2,3]);
 assert.equal(captured.metricsTrace.length,1);
 const door={playerId:769,lifeSeq:1,serverTick:106,destinationX:8500,destinationY:8000};
 assert.equal(remoteActorMotionAcrossStatesEvidence([captured],[769],{doorRelocations:[door]}).ok,true);
 assert.equal(remoteActorMotionAcrossStatesEvidence([captured],[769]).ok,false,
  'captured boundaries must expose an unexplained snap even after ordinary ring eviction');
});
test('bounded observation fails closed on overflow or a replaced metrics ring',async()=>{
 for(const replace of [false,true]){
  const c=context();install(c,2);const ring=c.window.__zubspaceMetricsTrace;
  ring.push(frame(1,{present:false}));ring.push(frame(2,{initial:1,reason:1}));
  if(replace)c.window.__zubspaceMetricsTrace=[];
  await assert.rejects(captureBotPresentationState(actor(c),[769]),/lost coverage/);
 }
});
test('observer has no timers, RPCs or changes to metric values and rejects double installation',()=>{
 const c=context();install(c);const f=frame(1);const before=JSON.stringify(f);
 c.window.__zubspaceMetricsTrace.push(f);
 assert.equal(JSON.stringify(f),before);assert.equal(c.window.__zubspaceBotMotionCapture.observed,1);
 const clone=structuredClone(c.window.__zubspaceMetricsTrace);
 assert.equal(clone.push,Array.prototype.push);
 assert.equal(JSON.stringify(clone),JSON.stringify(c.window.__zubspaceMetricsTrace));
 assert.equal(c.window.__zubspaceBotMotionCapture.frames.length,0);
 assert.throws(()=>install(c),/already installed/);
});

test('a door jump across AOI reentry needs exact native relocation and every invisible boundary',()=>{
 const previous={renderOrdinal:5037,positionTick:9119,initialCount:1,snapCount:0,combatViewTimeline:{generation:1}};
 const current={renderOrdinal:5206,positionTick:9421,initialCount:2,snapCount:1,combatViewTimeline:{generation:1}};
 const snap={previous,current,lifeSeq:3,snapDelta:1,snapReason:3};
 const boundary=(ordinal,initialCount,snapCount,positionPresent,lastSnapReason=1)=>({renderOrdinal:ordinal,
  combatViewTimeline:{generation:1},remotePlayers:[],remoteActorPresentation:[{id:768,lifeSeq:3,
   initialCount,snapCount,positionPresent,renderPositionPresent:false,lastSnapReason}]});
 const frames=[boundary(5038,1,0,false),boundary(5134,2,0,true),boundary(5203,2,1,true,3)];
 const event={playerId:768,lifeSeq:3,serverTick:9121,destinationX:8584,destinationY:8840};
 assert.equal(reenteredDoorReceipt(snap,768,[event],frames).kind,'door-relocation-across-unrenderable-reentry');
 for(const events of [[],[event,event],[{...event,playerId:769}],[{...event,lifeSeq:4}],
   [{...event,serverTick:9119}],[{...event,serverTick:9422}]])
  assert.equal(reenteredDoorReceipt(snap,768,events,frames),null);
 for(let missing=0;missing<3;missing++)
  assert.equal(reenteredDoorReceipt(snap,768,[event],frames.filter((_,i)=>i!==missing)),null);
 for(const field of ['renderPositionPresent','positionPresent']){
  const changed=structuredClone(frames);changed[0].remoteActorPresentation[0][field]=true;
  assert.equal(reenteredDoorReceipt(snap,768,[event],changed),null);
 }
 const changed=structuredClone(frames);changed[1].combatViewTimeline.generation=2;
 assert.equal(reenteredDoorReceipt(snap,768,[event],changed),null);
 assert.equal(reenteredDoorReceipt({...snap,current:{...current,initialCount:3}},768,[event],frames),null);
});

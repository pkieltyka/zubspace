import assert from 'node:assert/strict';
import test from 'node:test';
import {loadRendezvousMap} from './player_rendezvous.mjs';
import {prepareKeyboardLifecycleOrigin} from './keyboard_lifecycle_origin.mjs';
const {geometry}=loadRendezvousMap('chaos-zone',[new URL('../../assets/zones',import.meta.url).pathname]);
function fixture({clear=false,reject=false,changeLife=false,remainBlocked=false,pose=null,destination={x:7272,y:8824}}={}) {
  const keys=[],server={logs:[]};let warped=false,reads=0;
  const m={selfPlayerId:0,playerLifeSeq:1,playerAlive:true,serverPlayerAlive:true,
    playerX:clear?7272:7784,playerY:clear?8824:8959.97,
    playerVelocityX:0,playerVelocityY:clear?0:-15.89,
    playerEnergy:1700,serverPlayerEnergy:1700,playerMaxEnergy:1700,
    combatViewTimeline:{weaponReady:true},localGameplayState:{
      zoneFilesReady:true,zoneMapReady:true,pendingShipResetPosition:false,
      lifecycleBaselineReceiptsMatched:1,lifecycleBaselineReceiptsUnmatched:0,
      authorityScopeServerSessionId:'10',authorityScopeArenaStreamId:'20',pendingPortalWarp:false}};
  if(pose)Object.assign(m,pose);
  const actor={
    metricsState:async()=>{if(warped&&++reads>=2)m.playerEnergy=m.serverPlayerEnergy=1700;return{uiMetrics:structuredClone(m)};},
    releaseKeys:async()=>{keys.push('release');},keyDown:async k=>{keys.push(`down:${k}`);},
    keyUp:async k=>{keys.push(`up:${k}`);},press:async k=>{
      keys.push(`press:${k}`);assert.equal(k,'0');warped=true;reads=0;
      if(!remainBlocked){m.playerX=destination.x;m.playerY=destination.y;}
      m.playerVelocityX=m.playerVelocityY=0;m.playerEnergy=m.serverPlayerEnergy=1;
      if(changeLife)m.playerLifeSeq=2;
      server.logs.push(`INFO zubspace_event ${JSON.stringify({type:'server_portal_warp_terminal',player_id:0,
        life_seq:1,action_id:keys.length,position_sequence:9,server_tick:100,used_portal:false,
        position_x:m.playerX,position_y:m.playerY,energy:1,outcome:reject?2:1,reason:reject?'blocked':'applied'})}\n`);
    }};
  return {actor,server,keys};
}
// Unit fixtures complete synchronously; keep rejection polling quick. The real
// browser uses the unchanged180s total encounter budget.
const run=f=>prepareKeyboardLifecycleOrigin({...f,geometry,playerId:0,initialLifeSeq:1,deadline:Date.now()+200});
test('the retained tight origin uses ordinary random-Warp keys, keeps life and restores full energy',async()=>{
  const f=fixture(),r=await run(f);assert.deepEqual(f.keys,['release','down:Shift','press:0','up:Shift']);
  assert.equal(r.warps.length,1);assert.equal(r.warps[0].outcome,1);assert.equal(r.final.lifeSeq,1);
  assert.equal(r.final.energy,r.final.maxEnergy);assert.equal(r.final.serverEnergy,r.final.maxEnergy);
  assert.equal(geometry.authoredHullIsTraversable(r.final.x,r.final.y,20),true);
});
test('a clear stopped origin needs no warp or changed life',async()=>{
  const f=fixture({clear:true}),r=await run(f);assert.deepEqual(f.keys,['release']);assert.equal(r.warps.length,0);
});
test('a rejected authoritative Warp cannot establish an origin',async()=>{
  await assert.rejects(run(fixture({reject:true})),/Warp rejected/);
});
test('a lifecycle change during origin staging fails',async()=>{
  await assert.rejects(run(fixture({changeLife:true})),/life\/scope/);
});
test('two unusable accepted destinations fail instead of repeating indefinitely',async()=>{
  const f=fixture({remainBlocked:true});await assert.rejects(run(f),/still lacks/);
  assert.equal(f.keys.filter(k=>k==='press:0').length,2);
});

test('the Chrome-style actor releases the preceding chord through genuine key-up calls',async()=>{
  const f=fixture();delete f.actor.releaseKeys;
  const r=await run(f);
  assert.deepEqual(f.keys.slice(0,6),['up:ArrowUp','up:ArrowDown','up:ArrowLeft','up:ArrowRight','up:Control','up:Shift']);
  assert.equal(r.warps.length,1);assert.equal(r.final.lifeSeq,1);
});


test('retained Extreme coast cannot invalidate the20px origin during rendezvous preflight',async()=>{
  const {geometry:extreme}=loadRendezvousMap('extreme',[new URL('../../assets/zones',import.meta.url).pathname]);
  const pose={playerX:8267.5,playerY:7224.5,playerVelocityX:2.67,playerVelocityY:-1.82};
  assert.equal(extreme.authoredHullIsTraversable(pose.playerX,pose.playerY,20),true);
  const f=fixture({clear:true,pose,destination:{x:8200,y:8200}});
  const r=await prepareKeyboardLifecycleOrigin({...f,geometry:extreme,playerId:0,initialLifeSeq:1,deadline:Date.now()+200});
  assert.equal(r.stop.braked,true);assert.equal(r.warps.length,1);
  assert.equal(r.final.lifeSeq,1);assert.equal(r.final.energy,r.final.maxEnergy);
  assert.equal(r.handoffCoastSeconds,12);
});

test('the same narrow Extreme origin at actual rest needs no Warp',async()=>{
  const {geometry:extreme}=loadRendezvousMap('extreme',[new URL('../../assets/zones',import.meta.url).pathname]);
  const f=fixture({clear:true,pose:{playerX:8267.5,playerY:7224.5,playerVelocityX:0,playerVelocityY:0}});
  const r=await prepareKeyboardLifecycleOrigin({...f,geometry:extreme,playerId:0,initialLifeSeq:1,deadline:Date.now()+200});
  assert.deepEqual(f.keys,['release']);assert.equal(r.warps.length,0);
});

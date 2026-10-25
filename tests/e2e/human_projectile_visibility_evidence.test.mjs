import test from 'node:test';
import assert from 'node:assert/strict';
import {humanProjectileVisibilityEvidence as audit} from './human_projectile_visibility_evidence.mjs';

function fixture(extra=[]) {
  const damage=[...[100,150,160,200].map(serverTick=>({serverTick})),...extra]
    .map((hit,i)=>({damageSeq:i+1,projectileId:i+1,victimId:1,victimLifeSeq:3,
      shooterId:768,shooterLifeSeq:7,weapon:1,damage:100,...hit}));
  const damageTrace=damage.map(h=>({...h,at:h.serverTick*10,forSelf:true,attackerId:h.shooterId}));
  const collisionWitnesses=damage.map(h=>({projectile_id:h.projectileId,owner_id:h.shooterId,owner_life_seq:h.shooterLifeSeq,
    target_id:1,target_life_seq:3,adjusted_damage:h.damage,shot_group_id:String(h.projectileId),
    harmful_eligibility:{harmful:true}}));
  const frames=[];
  for(let at=600;at<=2100;at+=10)frames.push({at,monotonicMs:at,renderOrdinal:at/10,
    projectiles:{overflow:0,entries:damage.filter(h=>at>=h.serverTick*10-150&&at<h.serverTick*10)
      .map(h=>({drawn:true,authorityProjectileId:h.projectileId,sourcePlayerId:h.shooterId,sourceLifeSeq:h.shooterLifeSeq,
        sourceSessionId:'9007199254741001',shotGroupId:String(h.projectileId),
        screenX:100+h.projectileId*4,screenY:200}))}});
  return {playerId:1,startTick:100,endTick:200,frames,damageTrace,damage,collisionWitnesses,
    shotCommits:damage.map(h=>({owner_id:h.shooterId,owner_life_seq:h.shooterLifeSeq,
      shot_group_id:String(h.projectileId),owner_session_id:'9007199254741001',
      server_tick:h.fireTick??Math.max(1,h.serverTick-128)})),
    respawns:[{player_id:1,life_seq:3,server_tick:95}]};
}
test('audits all local hits, a rapid pair, and exact early-respawn life',()=>{
  const r=audit(fixture());assert.equal(r.complete,true);assert.equal(r.hits,4);
  assert.equal(r.cases.length,4);assert.equal(r.rapidPairs.length,1);
  assert.equal(r.rapidPairs[0].gapMs,100);assert.equal(r.earlyRespawnHits,4);
});
test('a completely undrawn hit cannot be skipped in favor of other visible hits',()=>{
  const f=fixture();for(const r of f.frames)r.projectiles.entries=r.projectiles.entries.filter(p=>p.authorityProjectileId!==2);
  const r=audit(f);assert.equal(r.complete,false);assert.equal(r.hits,4);
  assert.equal(r.missing[0].reason,'no-drawn-exact-projectile-before-local-damage');
});
test('wrong source life, group, sibling or offscreen submission cannot prove a local hit',()=>{
  for(const [key,value] of [['sourceLifeSeq',8],['sourceSessionId','9007199254741000'],['shotGroupId','999'],['authorityProjectileId',999],['drawn',false]]){
    const f=fixture();for(const row of f.frames)for(const p of row.projectiles.entries)if(p.authorityProjectileId===2)p[key]=value;
    assert.equal(audit(f).complete,false,key);
  }
});
test('missing receipt, wrong victim, amount or collision cannot be passed',()=>{
  let f=fixture();f.damageTrace[0].forSelf=false;assert.equal(audit(f).complete,false);
  f=fixture();f.damageTrace[0].victimLifeSeq++;assert.equal(audit(f).complete,false);
  f=fixture();f.damageTrace[0].damage++;assert.throws(()=>audit(f),/amount/);
  f=fixture();f.collisionWitnesses[0].target_id=769;assert.equal(audit(f).complete,false);
  f=fixture();delete f.collisionWitnesses[0].harmful_eligibility;assert.equal(audit(f).complete,false);
});
test('truncated or discontinuous pre-damage captures remain incomplete',()=>{
  let f=fixture();f.frames[10].projectiles.overflow=1;assert.equal(audit(f).complete,false);
  f=fixture();f.frames=f.frames.filter(r=>r.at<800||r.at>900);assert.equal(audit(f).complete,false);
});
test('an unrelated respawn does not establish early-respawn coverage',()=>{
  const f=fixture();f.respawns[0].life_seq=2;assert.equal(audit(f).complete,false);
});
test('same projectile and different source/target lives do not create rapid pairs',()=>{
  let f=fixture();f.damage[2].shooterLifeSeq++;f.damageTrace[2].shooterLifeSeq++;
  f.collisionWitnesses[2].owner_life_seq++;f.shotCommits[2].owner_life_seq++;
  for(const row of f.frames)for(const p of row.projectiles.entries)if(p.authorityProjectileId===3)p.sourceLifeSeq++;
  assert.equal(audit(f).rapidPairs.length,0);
});
test('only the explicit capture interval is claimed, including tick wrap',()=>{
  const f=fixture();f.endTick=150;assert.equal(audit(f).hits,2);
  for(const h of f.damage)h.serverTick=(h.serverTick+0x7fffff80)&0x7fffffff;
  for(const h of f.damageTrace)h.serverTick=(h.serverTick+0x7fffff80)&0x7fffffff;
  f.startTick=(100+0x7fffff80)&0x7fffffff;f.endTick=(200+0x7fffff80)&0x7fffffff;
  assert.equal(audit(f).hits,4);
});

test('first-collision closed readiness is retained and never mistaken for the later damage commit',()=>{
  const f=fixture();f.collisionWitnesses[0].harmful_eligibility={harmful:false,reason:'awaiting_projectile_lead',projectile_harmful_open_tick:110};
  const r=audit(f);assert.equal(r.complete,true);assert.equal(r.cases[0].stagedReadiness.harmful,false);
  assert.equal(r.cases[0].stagedReadiness.projectile_harmful_open_tick,110);
});

test('production-shaped converging bullets and two-shooter bullet/bomb pairs keep separate identities',()=>{
  const f=fixture([{serverTick:151,fireTick:70,weapon:2},
    {serverTick:155,shooterId:769,weapon:4,damage:344}]);
  const r=audit(f);assert.equal(r.complete,true);assert.equal(r.hits,6);
  const converging=r.allRapidPairs.find(p=>p.first===2&&p.second===5);
  assert.equal(converging.gapMs,10);assert.equal(converging.signedFireGapTicks,48);
  assert.equal(converging.sameSource,true);assert.equal(converging.sameActivation,false);
  assert.equal(converging.shape,'bullet-bullet');
  assert.ok(r.allRapidPairs.some(p=>p.twoShooters&&p.shape==='bullet-bomb'&&p.gapMs<=120));
  assert.ok(r.pairShapes.sameSourceWithin50ms>0);
  assert.ok(r.pairShapes.twoShooterWithin120ms>0);
  assert.ok(r.pairShapes.bulletBombWithin200ms>0);
});
test('an undrawn bomb cannot hide behind four successfully drawn bullet hits',()=>{
  const f=fixture([{serverTick:162,weapon:4}]);
  for(const frame of f.frames)frame.projectiles.entries=
    frame.projectiles.entries.filter(p=>p.authorityProjectileId!==5);
  const r=audit(f);assert.equal(r.complete,false);assert.equal(r.bulletHits,4);
  assert.ok(r.missing.some(m=>m.hit.projectileId===5&&m.reason==='no-drawn-exact-projectile-before-local-damage'));
});
test('bomb contacts cannot substitute for the existing four-bullet coverage floor',()=>{
  const f=fixture();f.damage[0].weapon=4;f.damageTrace[0].weapon=4;
  const r=audit(f);assert.equal(r.complete,false);assert.equal(r.bulletHits,3);
  assert.equal(r.missing.length,0);
});
test('same-frame coincident centers are reported without claiming two perceptually distinct bullets',()=>{
  const f=fixture([{serverTick:151,weapon:2}]);
  for(const frame of f.frames)for(const p of frame.projectiles.entries){p.screenX=500;p.screenY=400;}
  const pair=audit(f).allRapidPairs.find(p=>p.first===2&&p.second===5);
  assert.ok(pair.concurrentDrawFrames>0);assert.equal(pair.minimumCenterSeparationPx,0);
  assert.equal(pair.withinOnePixelFrames,pair.concurrentDrawFrames);
});
test('different victim lives never become one rapid-hit pair',()=>{
  const f=fixture([{serverTick:151,victimLifeSeq:4}]);
  f.damageTrace.at(-1).victimLifeSeq=4;f.collisionWitnesses.at(-1).target_life_seq=4;
  const r=audit(f);
  assert.ok(!r.allRapidPairs.some(p=>p.first===5||p.second===5));
});

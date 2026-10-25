import test from "node:test";
import assert from "node:assert/strict";
import {botProjectilePresentationEvidence as audit,botProjectileObservationWindow} from "./bot_projectile_presentation_evidence.mjs";
function fixture(){
 const bytes=Buffer.alloc(122);bytes.set([125,2,4]);bytes.writeBigUInt64LE(1n,3);bytes.writeBigUInt64LE(2n,11);bytes.writeUInt32LE(9,19);
 bytes.writeUInt32LE(100,23);bytes.writeUInt16LE(768,27);bytes.writeBigUInt64LE(33n,29);bytes.writeUInt32LE(4,37);bytes.writeBigUInt64LE(7n,41);
 bytes.writeUInt32LE(8,74);bytes.writeUInt16LE(66,83);bytes[105]=4;
 const p={sourcePlayerId:768,sourceSessionId:"33",sourceLifeSeq:4,shotGroupId:"7",authorityProjectileId:8,drawn:true};
 const entries=[{id:768,ship:4,lifeSeq:4,drawn:true},{id:769,lifeSeq:3,drawn:true,renderedX:100,renderedY:100}];
 const frames=[{at:990,entries,projectiles:{overflow:0,entries:[p,{...p,sourceProjectileIndex:1}]}}];
 for(let at=1050;at<=1600;at+=16)frames.push({at,entries,projectiles:{overflow:0,entries:[]}});
 const hit={projectileId:8,shooterId:768,shooterLifeSeq:4,victimId:769,victimLifeSeq:3,weapon:66,serverTick:100,damage:400,lethal:true};
 const witness={projectile_id:8,owner_id:768,owner_life_seq:4,target_id:769,target_life_seq:3,collision_tick:100,adjusted_damage:400,
  shot_group_id:"7",contact_reason:"direct",history_source:"unified_exact_segment",presentation_geometry:{status:"server_native"},target_radius:14,projectile_x:100,projectile_y:100,collision_anchor_x:100,collision_anchor_y:100};
 return{state:{packetLog:[{direction:"received",at:1000,bytes:[...bytes]}],remoteSpriteTrace:frames},damage:[hit],collisionWitnesses:[witness],minimumCases:1,minimumShipTypes:1};
}
test("exact drawn linked group retires after a native bot collision",()=>assert.equal(audit(fixture()).complete,true));
test("visible surviving sibling or resurrected group fails",()=>{const f=fixture();f.state.remoteSpriteTrace[5].projectiles.entries=[f.state.remoteSpriteTrace[0].projectiles.entries[1]];assert.throws(()=>audit(f),/reappeared/);});
test("another session/life/group cannot satisfy pre-terminal drawing",()=>{for(const field of ["sourceSessionId","sourceLifeSeq","shotGroupId"]){const f=fixture();f.state.remoteSpriteTrace[0].projectiles.entries.forEach(p=>p[field]=field==="sourceLifeSeq"?5:"99");assert.equal(audit(f).complete,false);}});
test("truncated, short or gapped render tails are not absence proof",()=>{for(const mode of ["overflow","short","gap"]){const f=fixture();if(mode==="overflow")f.state.remoteSpriteTrace[5].projectiles.overflow=1;if(mode==="short")f.state.remoteSpriteTrace.length=8;if(mode==="gap")f.state.remoteSpriteTrace.splice(10,10);assert.equal(audit(f).complete,false);}});
test("exact collision geometry is required and contradictory geometry fails",()=>{const f=fixture();f.collisionWitnesses=[];assert.equal(audit(f).complete,false);const g=fixture();g.collisionWitnesses[0].presentation_geometry.status="observer_presentation";assert.throws(()=>audit(g));});
test("new source-life projectiles survive an older exact terminal",()=>{const f=fixture();f.state.remoteSpriteTrace[5].projectiles.entries=[{...f.state.remoteSpriteTrace[0].projectiles.entries[0],sourceLifeSeq:5}];assert.equal(audit(f).complete,true);});

test("a drawn sibling alone cannot prove the consumed child was visible",()=>{const f=fixture();f.state.remoteSpriteTrace[0].projectiles.entries.forEach(p=>p.authorityProjectileId=99);assert.equal(audit(f).complete,false);});

test("a late-starting tail does not prove the half-second after delivery",()=>{const f=fixture();f.state.remoteSpriteTrace=f.state.remoteSpriteTrace.filter(x=>x.at<1000||x.at>=1250);assert.equal(audit(f).complete,false);});

test("adjacent real captures can jointly prove a lethal tail without inventing missing frames",()=>{
 const f=fixture(),frames=f.state.remoteSpriteTrace;
 const first={...f.state,remoteSpriteTrace:frames.filter(r=>r.at<1300)};
 const second={...f.state,remoteSpriteTrace:frames.filter(r=>r.at>=1250)};
 assert.equal(audit({...f,state:first}).complete,false);
 assert.equal(audit({...f,state:second}).complete,false);
 const retained={frames:new Map(),packets:new Map()};
 for(const state of [first,second]){
  for(const row of state.remoteSpriteTrace)retained.frames.set(row.at,row);
  for(const row of state.packetLog)retained.packets.set(row.at,row);
 }
 assert.equal(audit({...f,state:botProjectileObservationWindow(retained,1600)}).complete,true);
 for(const at of [...retained.frames.keys()])if(at>=1200&&at<1400)retained.frames.delete(at);
 assert.equal(audit({...f,state:botProjectileObservationWindow(retained,1600)}).complete,false);
 assert.equal(botProjectileObservationWindow(retained,6000).packetLog.length,0);
 assert.equal(botProjectileObservationWindow(retained,6000).remoteSpriteTrace.length,0);
});


test("native box contacts outside the old circle require an exact valid witness",()=>{
 const f=fixture();f.requireOutsideLegacyCircle=true;
 assert.equal(audit(f).complete,false,"a centre hit cannot prove the corrected corner path");
 f.collisionWitnesses[0].projectile_x=118;f.collisionWitnesses[0].projectile_y=118;
 const result=audit(f);assert.equal(result.complete,true);assert.equal(result.outsideLegacyCircleCases,1);
 f.collisionWitnesses[0].projectile_x=119;
 assert.throws(()=>audit(f),/reference game's ship-first weapon boxes/);
});

test("a native witness at the reference's strict lower pixel edge is invalid",()=>{
 const f=fixture();f.collisionWitnesses[0].projectile_x=82;
 assert.throws(()=>audit(f),/reference game's ship-first weapon boxes/);
});

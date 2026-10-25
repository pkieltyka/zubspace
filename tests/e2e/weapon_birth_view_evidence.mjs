import assert from 'node:assert/strict';
import {parseRealtimeWeaponEvent} from '../../tools/realtime_event_evidence.mjs';

const diff=(a,b)=>((a-b)<<1)>>1;
const identity=p=>`${p.sourcePlayerId}/${p.sourceSessionId}/${p.sourceLifeSeq}/${p.shotGroupId}/${p.sourceProjectileIndex}`;
const matches=(p,s)=>p.sourcePlayerId===s.source&&p.sourceSessionId===s.sourceSessionId&&
  p.sourceLifeSeq===s.sourceLifeSeq&&p.shotGroupId===s.shotGroupId;

// A draw intent is stronger than current simulation state, but does not prove
// the first GPU-visible pixel: the reference game's guns begin at the ship centre and the
// ship sprite occludes the earliest part of the shot.
export function weaponBirthViewEvidence({frames,packetLog,shots,minCases=3}) {
  assert.ok(frames.length>=30,'continuous submitted draw evidence required');
  const groups=new Map(),sources=new Map(),terminals=new Map();
  let previous=null;
  for(const frame of frames){
    assert.ok(Number.isFinite(frame.monotonicMs)&&Number.isInteger(frame.renderOrdinal));
    if(previous){assert.ok(frame.monotonicMs>previous.monotonicMs,'monotonic draw order');
      assert.ok(frame.renderOrdinal>previous.renderOrdinal,'unique draw ordinal');}
    previous=frame;
    assert.equal(frame.projectiles.overflow,0,'complete bounded projectile draw set');
    for(const p of frame.projectiles.entries){
      if(p.sourcePlayerId!==768||p.texture!=='bullets')continue;
      const key=identity(p);if(!groups.has(key))groups.set(key,[]);
      groups.get(key).push({frame,p});
    }
  }
  for(const row of packetLog){
    if(row.direction!=='received')continue;
    const bytes=Buffer.from(row.bytes||[]),s=parseRealtimeWeaponEvent(bytes);
    if(s&&s.source===768&&[1,2].includes(s.weapon&31)){
      const key=`${s.source}/${s.sourceSessionId}/${s.sourceLifeSeq}/${s.shotGroupId}`;
      const old=sources.get(key);if(old)assert.deepEqual(old.source,s,'immutable duplicate source');
      else sources.set(key,{source:s,receivedAt:row.at});
    }
    if(bytes.length>=122&&bytes[0]===125&&bytes[1]===2&&bytes[2]===4&&(bytes[105]&4)&&
      bytes.readUInt16LE(27)===768&&[1,2].includes(bytes.readUInt16LE(83)&31)){
      const key=`${bytes.readBigUInt64LE(3)}/${bytes.readBigUInt64LE(11)}/${bytes.readUInt32LE(19)}`;
      const fact={at:row.at,session:bytes.readBigUInt64LE(29).toString(),life:bytes.readUInt32LE(37),group:bytes.readBigUInt64LE(41).toString(),bytes:bytes.toString('hex')};
      const previous=terminals.get(key);
      if(previous)assert.equal(previous.bytes,fact.bytes,'duplicate terminal copies must agree');
      if(!previous||fact.at<previous.at)terminals.set(key,fact);
    }
  }
  const cases=[],classification={missingOriginal:0,currentFallback:0,initiallyOffscreen:0,missingCompleteConvergence:0};
  for(const [key,draws] of groups){
    const first=draws[0],s=sources.get(key.split('/').slice(0,4).join('/'))?.source;
    if(!s){classification.missingOriginal++;continue;}
    const committed=shots.find(row=>String(row.shot_group_id)===s.shotGroupId&&
      String(row.owner_session_id)===s.sourceSessionId&&row.owner_life_seq===s.sourceLifeSeq);
    assert.ok(committed,'exact server commit owns the source');
    assert.equal(committed.server_tick,s.serverTick);
    for(const [wire,server] of [['x','x'],['y','y'],['velocityX','velocity_x'],['velocityY','velocity_y'],['orientation','orientation']])
      assert.ok(Math.abs(s[wire]-committed[server])<0.002,'source geometry must match server commit');
    for(const {frame,p} of draws){
      assert.equal(p.sourceServerTick,s.serverTick);
      assert.ok(diff(p.poseTick,s.serverTick)>=0&&diff(frame.serverTick,p.poseTick)>=0,'draw cannot precede birth or exceed current time');
      assert.ok(p.posePartialSeconds>=0&&p.posePartialSeconds<=0.010001);
    }
    // The complete trace also retains culled draw intents. A shot entering the
    // viewport later cannot prove its first visible birth; require other full
    // visible births instead of treating that partial window as a fresh shot.
    if(!first.p.drawn){classification.initiallyOffscreen++;continue;}
    if(first.p.poseTick===first.frame.serverTick){classification.currentFallback++;continue;}
    assert.equal(first.p.poseTick,first.frame.presentationRenderTick,'fresh first draw shares source actor cursor');
    assert.equal(first.p.posePartialSeconds,0,'first frame does not extrapolate ahead of its source');
    const current=draws.find(({frame,p})=>p.poseTick===frame.serverTick);
    if(!current){classification.missingCompleteConvergence++;continue;}
    assert.ok(diff(current.frame.serverTick,s.serverTick)<=28,'converges before original minimum harm deadline');
    let caught=false;
    for(const {frame,p} of draws){
      if(p.poseTick===frame.serverTick)caught=true;
      if(caught)assert.equal(p.poseTick,frame.serverTick,'converged shot cannot rewind with source actor');
    }
    const actor=first.frame.entries.find(p=>p.id===s.source&&p.sourceSessionId===s.sourceSessionId&&p.lifeSeq===s.sourceLifeSeq&&p.drawn);
    assert.ok(actor,'same-life drawn source actor required');
    let sourceDistance=null;
    if(first.p.poseTick===s.serverTick){
      assert.ok(Math.hypot(first.p.x-Math.round(s.x),first.p.y-Math.round(s.y))<=0.02,'single gun begins at immutable source centre');
      sourceDistance=Math.hypot(first.p.x-actor.renderedX,first.p.y-actor.renderedY);
      assert.ok(sourceDistance<=1,'first birth sample aligns with actual drawn source centre');
      const firingFrame=Math.floor(Math.fround(Math.fround(s.orientation)*40))%40;
      assert.equal(actor.shipFrame,firingFrame,'fresh birth uses authoritative firing heading sprite');
      assert.ok(Number.isFinite(actor.displayOrientation)&&
        Math.abs(actor.displayOrientation-s.orientation)<0.000001,
        'fresh birth uses exact authoritative firing heading');
    }
    cases.push({key,source:s,serverCommit:committed,first,current,drawFrames:draws.length,sourceDistance});
  }
  const terminalProofs=[];
  for(const t of terminals.values()){
    const tail=frames.filter(f=>f.at>=t.at+50&&f.at<=t.at+650);
    if(tail.length<12||tail[0].at>t.at+100||tail.at(-1).at<t.at+550)continue;
    for(let i=1;i<tail.length;i++)assert.ok(tail[i].monotonicMs-tail[i-1].monotonicMs<=60,'continuous terminal tail without observation gaps');
    for(const frame of tail)assert.ok(!frame.projectiles.entries.some(p=>p.sourcePlayerId===768&&p.sourceSessionId===t.session&&p.sourceLifeSeq===t.life&&p.shotGroupId===t.group),'terminal must remove the submitted group immediately');
    terminalProofs.push({terminal:t,frames:tail.length,spanMs:tail.at(-1).at-tail[0].at});
  }
  const exactBirth=cases.filter(c=>c.sourceDistance!==null);
  const moving=exactBirth.filter(c=>Math.hypot(c.source.velocityX,c.source.velocityY)>10);
  const headings=new Set(moving.map(c=>Math.floor(c.source.orientation*40)%40));
  return {complete:cases.length>=minCases&&moving.length>=minCases&&headings.size>=2&&terminalProofs.length>0,
    cases,exactMovingBirths:moving.length,headingBins:[...headings],terminalProofs,classification,
    scope:'fresh Warbird submitted draws during ordinary moving/turning bot combat, source/clock convergence and complete terminal tails; not pixel-level or two-perspective combat acceptance'};
}

import test from 'node:test';
import assert from 'node:assert/strict';
import {weaponBirthViewEvidence} from './weapon_birth_view_evidence.mjs';

function fixture(){
  const frames=[],packetLog=[],shots=[];
  const session='9007199254741001';
  for(let i=0;i<3;i++){
    const birth=100+i*50,group=String(i+1),orientation=i/10;
    const p=Buffer.alloc(74);p[0]=125;p[1]=2;p[2]=1;
    p.writeBigUInt64LE(77n,3);p.writeBigUInt64LE(88n,11);p.writeUInt32LE(i+1,19);p.writeUInt32LE(birth,23);
    p.writeUInt16LE(768,27);p.writeBigUInt64LE(BigInt(session),29);p.writeUInt32LE(7,37);p.writeBigUInt64LE(BigInt(group),41);
    p.writeFloatLE(100.25,49);p.writeFloatLE(199.75,53);p.writeFloatLE(100,57);p.writeFloatLE(0,61);p.writeFloatLE(orientation,65);p.writeUInt16LE(i===1?2:1,72);
    packetLog.push({at:birth*10,direction:'received',bytes:[...p]});
    const terminal=Buffer.alloc(122);p.copy(terminal,0,0,74);terminal[2]=4;terminal[105]=4;
    terminal.writeUInt32LE(birth+40,23);terminal.writeUInt16LE(i===1?2:1,83);
    packetLog.push({at:(birth+40)*10,direction:'received',bytes:[...terminal]});
    shots.push({server_tick:birth,owner_session_id:session,owner_life_seq:7,shot_group_id:group,
      x:100.25,y:199.75,velocity_x:100,velocity_y:0,orientation});
  }
  for(let now=90;now<=330;now++){
    const frame={at:now*10,monotonicMs:now*10,renderOrdinal:now,serverTick:now,presentationRenderTick:now-7,
      entries:[{id:768,sourceSessionId:session,lifeSeq:7,drawn:true,renderedX:100,renderedY:200}],projectiles:{overflow:0,entries:[]}};
    for(let i=0;i<3;i++){
      const birth=100+i*50;
      if(now<birth+7||now>=birth+40)continue;
      const pose=now<birth+14?birth+(now-birth-7)*2:now;
      frame.entries[0].displayOrientation=Math.fround(i/10);
      frame.entries[0].shipFrame=Math.floor(Math.fround(Math.fround(i/10)*40))%40;
      frame.projectiles.entries.push({sourcePlayerId:768,sourceSessionId:session,sourceLifeSeq:7,shotGroupId:String(i+1),sourceProjectileIndex:0,
        sourceServerTick:birth,poseTick:pose,posePartialSeconds:0,texture:'bullets',drawn:true,x:100+(pose-birth)*3,y:200});
    }
    frames.push(frame);
  }
  return {frames,packetLog,shots};
}

test('requires moving/turning births, exact u64 source commits, convergence and real terminal tails',()=>{
  const r=weaponBirthViewEvidence(fixture());assert.equal(r.complete,true);assert.equal(r.cases.length,3);
  assert.equal(r.exactMovingBirths,3);assert.ok(r.terminalProofs.length>=3);
});
test('a source-clock mismatch or invented muzzle offset fails',()=>{
  let f=fixture();f.frames.find(r=>r.serverTick===107).projectiles.entries[0].poseTick++;
  assert.throws(()=>weaponBirthViewEvidence(f),/source actor cursor/);
  f=fixture();f.frames.find(r=>r.serverTick===107).projectiles.entries[0].x++;
  assert.throws(()=>weaponBirthViewEvidence(f),/immutable source centre/);
});
test('server identity and convergence deadlines cannot be replaced with plausible poses',()=>{
  let f=fixture();f.shots[0].owner_session_id='9007199254741000';assert.throws(()=>weaponBirthViewEvidence(f),/server commit/);
  f=fixture();for(const r of f.frames.filter(r=>r.serverTick>=114&&r.serverTick<130)){
    const p=r.projectiles.entries.find(p=>p.shotGroupId==='1');if(p)p.poseTick=r.serverTick-1;
  }
  assert.throws(()=>weaponBirthViewEvidence(f),/minimum harm deadline/);
});
test('converged clocks cannot rewind and terminals cannot retain historical copies',()=>{
  let f=fixture();f.frames.find(r=>r.serverTick===120).projectiles.entries[0].poseTick--;
  assert.throws(()=>weaponBirthViewEvidence(f),/cannot rewind/);
  f=fixture();f.frames.find(r=>r.serverTick===146).projectiles.entries.push({...f.frames.find(r=>r.serverTick===110).projectiles.entries[0],poseTick:146});
  assert.throws(()=>weaponBirthViewEvidence(f),/terminal must remove/);
});
test('missing complete draw windows cannot become a passing proof',()=>{
  const f=fixture();f.frames=f.frames.slice(0,22);assert.throws(()=>weaponBirthViewEvidence(f),/continuous submitted/);
});
test('terminal absence cannot hide a missing interval',()=>{
  const f=fixture();f.frames=f.frames.filter(row=>row.serverTick<270||row.serverTick>280);
  assert.throws(()=>weaponBirthViewEvidence(f),/observation gaps/);
});
test('retransmitted terminal copies do not inflate distinct proofs',()=>{
  const f=fixture();const terminal=f.packetLog.find(row=>row.bytes[2]===4);
  f.packetLog.push({...terminal,at:terminal.at+1});
  assert.equal(weaponBirthViewEvidence(f).terminalProofs.length,3);
  const conflicting={...terminal,bytes:[...terminal.bytes]};conflicting.bytes[85]=1;
  f.packetLog.push(conflicting);
  assert.throws(()=>weaponBirthViewEvidence(f),/duplicate terminal copies/);
});
test('an initially culled shot cannot count as a witnessed visible birth',()=>{
  const f=fixture();f.frames.find(row=>row.serverTick===107).projectiles.entries[0].drawn=false;
  const report=weaponBirthViewEvidence(f);
  assert.equal(report.complete,false);
  assert.equal(report.cases.length,2);
  assert.equal(report.classification.initiallyOffscreen,1);
});

test('a bomb terminal does not retire its legitimate shrapnel children under the gun-tail oracle',()=>{
  const f=fixture();
  const terminal=Buffer.from(f.packetLog.find(p=>p.bytes[2]===4).bytes);
  terminal.writeUInt32LE(99,19);terminal.writeUInt32LE(180,23);terminal.writeBigUInt64LE(99n,41);terminal.writeUInt16LE(8612,83);
  f.packetLog.push({at:1800,direction:'received',bytes:[...terminal]});
  for(const frame of f.frames.filter(f=>f.serverTick>=185&&f.serverTick<=245)){
    frame.projectiles.entries.push({sourcePlayerId:768,sourceSessionId:'9007199254741001',sourceLifeSeq:7,
      shotGroupId:'99',sourceProjectileIndex:6,texture:'shrapnel',drawn:true});
  }
  const report=weaponBirthViewEvidence(f);
  assert.equal(report.complete,true);
  assert.equal(report.terminalProofs.length,3,'only the three gun terminals belong to this oracle');
});

test('aligned source centers do not excuse a lagging firing heading',()=>{
  let f=fixture();
  f.frames.find(r=>r.serverTick===157).entries[0].shipFrame=3;
  assert.throws(()=>weaponBirthViewEvidence(f),/authoritative firing heading sprite/);
  f=fixture();
  f.frames.find(r=>r.serverTick===157).entries[0].displayOrientation=0.09;
  assert.throws(()=>weaponBirthViewEvidence(f),/exact authoritative firing heading/);
});

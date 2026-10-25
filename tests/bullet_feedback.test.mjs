import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import fs from 'node:fs';
import {authoredMapGeometry} from './e2e/authored_map_geometry.mjs';
import {projectBulletFeedback, readBulletFeedback} from './e2e/bullet_feedback.mjs';
import {assertEnergyTraceWindow,findPrematureVisibleFullSamples,ownerRecoveryRunway} from './e2e/single_bullet_damage.mjs';

test('owner recovery avoids the real Chaos spawn door without assuming its current open phase', () => {
  const geometry=authoredMapGeometry(fs.readFileSync(new URL('../assets/zones/chaos-zone/bdhadar.lvl',import.meta.url)));
  assert.equal(geometry.tileAt(8200,8120),169);
  const runway=ownerRecoveryRunway(geometry,{playerX:8200,playerY:8200,playerOrientation:0});
  assert.equal(runway.key,'ArrowDown');
  assert.equal(runway.dy,1);
  assert.equal(runway.runwayPx,512);
  for(let traveled=0;traveled<=512;traveled+=1)
    assert.equal(geometry.authoredHullIsOpen(8200,8200+traveled),true);
});

test('owner recovery requires a full hull-clear runway and rejects unusable geometry', () => {
  const metrics={playerX:0,playerY:0,playerOrientation:0.25};
  assert.equal(ownerRecoveryRunway({authoredHullIsOpen:x=>x>=0},metrics).key,'ArrowUp');
  assert.throws(()=>ownerRecoveryRunway({authoredHullIsOpen:x=>Math.abs(x)<120},metrics),/no clear/);
  assert.throws(()=>ownerRecoveryRunway({authoredHullIsOpen:()=>true},{...metrics,playerX:NaN}),/finite/);
});

test('bullet feedback preserves all rows, complete damage identities and current metrics', () => {
  const row={at:123,selfPlayerId:7,lastSelfDamageServerSeq:9,lastSelfDamageServerTick:99,
    lastSelfDamageVictimLifeSeq:2,lastSelfDamageEnergy:1300,lastSelfDamageSeq:1,
    playerEnergy:1301,serverPlayerEnergy:1301,visibleEnergyDigits:1301,localShipLabelEnergy:1301,
    remotePlayers:[{id:8,energy:1300,serverEnergy:1300}],snapshotReceiptTiers:{unused:'x'.repeat(10000)}};
  const scope={__zubspaceUiMetrics:{arbitraryCurrentMetric:42},
    __zubspaceDamageTrace:[{damageSeq:9,projectileId:2,arbitraryIdentity:123}],
    __zubspaceMetricsTrace:Array.from({length:1500},(_,i)=>({...row,at:i}))};
  const result=projectBulletFeedback(scope);
  assert.equal(result.metricsTrace.length,1500);
  assert.deepEqual(result.damageTrace,scope.__zubspaceDamageTrace);
  assert.deepEqual(result.uiMetrics,scope.__zubspaceUiMetrics);
  const {snapshotReceiptTiers,...expected}=row;
  for(let i=0;i<1500;i++)assert.deepEqual(result.metricsTrace[i],{...expected,at:i});
  assert(scope.__zubspaceMetricsTrace.every(r=>r.snapshotReceiptTiers===snapshotReceiptTiers));
});
test('browser feedback expression is self-contained and preserves missing values', async () => {
  const window={__zubspaceMetricsTrace:[{at:1,playerEnergy:0},{at:2}],__zubspaceDamageTrace:[]};
  const result=await readBulletFeedback({evaluate:expression=>JSON.parse(JSON.stringify(vm.runInNewContext(expression,{window})))});
  assert.deepEqual(result,{uiMetrics:null,damageTrace:[],metricsTrace:[{at:1,playerEnergy:0},{at:2}]});
});

test('real energy assertions retain positive and negative verdicts after projection',async()=>{
  const event={victimId:1,shooterId:2,damageSeq:3,projectileId:4,serverTick:5,victimLifeSeq:6,
    shooterLifeSeq:7,weapon:2,damage:400,energyBefore:1700,energyAfter:1300};
  const damage={...event,attackerId:2,at:100,lastSelfDamageSeq:1};
  const rows=Array.from({length:6},(_,i)=>({at:100+i*50,selfPlayerId:1,
    lastSelfDamageSeq:1,lastSelfDamageServerSeq:3,lastSelfDamageServerTick:5,lastSelfDamageVictimLifeSeq:6,
    playerEnergy:1300+i,serverPlayerEnergy:1300+i,localShipLabelEnergy:1300+i,visibleEnergyDigits:1300+i,
    playerMaxEnergy:1700,lastSelfDamageEnergy:1300,unusedHistory:{big:'unused'}}));
  for(const fault of ['none','missing-samples','wrong-damage','energy-dip','premature-full']) {
    const scope={__zubspaceMetricsTrace:structuredClone(rows),__zubspaceDamageTrace:[{...damage}]};
    if(fault==='missing-samples')scope.__zubspaceMetricsTrace.splice(2);
    if(fault==='wrong-damage')scope.__zubspaceDamageTrace[0].damageSeq=999;
    if(fault==='energy-dip')scope.__zubspaceMetricsTrace[2].playerEnergy=1200;
    if(fault==='premature-full')scope.__zubspaceMetricsTrace[2].playerEnergy=1700;
    const projected=projectBulletFeedback(scope);
    const full={uiMetrics:null,metricsTrace:scope.__zubspaceMetricsTrace,damageTrace:scope.__zubspaceDamageTrace};
    for(const state of [full,projected]) {
      const check=assertEnergyTraceWindow({evaluate:async()=>state},event,fault,0,0,
        {energyField:'playerEnergy',seqField:'lastSelfDamageSeq',requireDrop:true,requireAuthoritativeResult:true});
      if(fault==='none')await check;else await assert.rejects(check);
    }
    const a=findPrematureVisibleFullSamples(full,{metricsTrace:[]},event);
    const b=findPrematureVisibleFullSamples(projected,{metricsTrace:[]},event);
    assert.deepEqual(a.victimPremature.map(r=>r.at),b.victimPremature.map(r=>r.at));
  }
});

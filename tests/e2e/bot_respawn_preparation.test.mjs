import assert from 'node:assert/strict';
import test from 'node:test';
import {prepareBotSuccessorObservation,presentedLiveBotSuccessor} from './bot_combat_evidence.mjs';
const visible=()=>({playerAlive:true,serverPlayerAlive:true,playerLifeSeq:1,
 remotePlayers:[{id:768,lifeSeq:3,energy:1700}],authoritativeSnapshotActors:[{id:768,lifeSeq:3}]});
const missing=()=>({playerAlive:true,serverPlayerAlive:true,playerLifeSeq:1,
 remotePlayers:[],authoritativeSnapshotActors:[]});

test('successor observation leaves ordinary callers and already-visible actors untouched',async()=>{
 const actor={metricsState:async()=>({uiMetrics:visible()})};
 const never=()=>assert.fail('no navigation needed');
 assert.equal(await prepareBotSuccessorObservation({actor,playerId:768,predecessorLifeSeq:2,navigate:never,brake:never}),null);
 assert.deepEqual(await prepareBotSuccessorObservation({actor,playerId:768,predecessorLifeSeq:2,
  searchPoint:{x:8200,y:8200},navigate:never,brake:never}),{needed:false});
});
test('a distant pilot navigates until a real newer-life actor is admitted, then brakes',async()=>{
 let metrics=missing();const calls=[];const actor={metricsState:async()=>({uiMetrics:metrics})};
 const result=await prepareBotSuccessorObservation({actor,playerId:768,predecessorLifeSeq:2,
  searchPoint:{x:8200,y:8200},navigate:async(a,point,server,options)=>{
   assert.equal(a,actor);assert.equal(server,null);assert.deepEqual(point,{x:8200,y:8200});
   assert.equal(await options.complete(),false);metrics=visible();assert.equal(await options.complete(),true);
   calls.push('navigate');return {completedBy:'observed-scope'};
  },brake:async a=>{assert.equal(a,actor);calls.push('brake');return {braked:true};}});
 assert.deepEqual(calls,['navigate','brake']);assert.equal(result.keysOnly,true);
 assert.equal(presentedLiveBotSuccessor(metrics,768,2).lifeSeq,3);
});
test('reaching a search coordinate cannot manufacture missing snapshot membership',async()=>{
 const metrics=missing(),before=structuredClone(metrics);const actor={metricsState:async()=>({uiMetrics:metrics})};
 await prepareBotSuccessorObservation({actor,playerId:768,predecessorLifeSeq:2,searchPoint:{x:8200,y:8200},
  navigate:async(a,p,s,options)=>{assert.equal(await options.complete(),false);return {reached:true};},
  brake:async()=>({braked:true})});
 assert.deepEqual(metrics,before);assert.equal(presentedLiveBotSuccessor(metrics,768,2),null);
});
test('invalid bounds and real route failures remain failures',async()=>{
 const actor={metricsState:async()=>({uiMetrics:missing()})};
 for(const searchPoint of [{x:NaN,y:0},{x:-1,y:0},{x:0,y:16384}])
  await assert.rejects(prepareBotSuccessorObservation({actor,playerId:768,predecessorLifeSeq:2,searchPoint}),/invalid bounded/);
 await assert.rejects(prepareBotSuccessorObservation({actor,playerId:768,predecessorLifeSeq:2,searchPoint:{x:8200,y:8200},
  navigate:async()=>{throw new Error('no hull-clear route');}}),/no hull-clear route/);
});

test('only an explicitly configured origin recovery may retry a narrow successor route once',async()=>{
 for(const repeatFailure of [false,true]){
  let metrics=missing(),routes=0,recoveries=0;
  const actor={metricsState:async()=>({uiMetrics:metrics}),prepareBotObservationOrigin:async deadline=>{
   assert.ok(deadline>Date.now()&&deadline<=Date.now()+45_000);recoveries++;return {warps:[{outcome:1}]};
  }};
  const run=()=>prepareBotSuccessorObservation({actor,playerId:768,predecessorLifeSeq:2,
   searchPoint:{x:8200,y:8200},navigate:async()=>{
    routes++;if(routes===1||repeatFailure)throw new Error('authored approach origin lacks conservative hull clearance');
    metrics=visible();return {reached:true};
   },brake:async()=>({braked:true})});
  if(repeatFailure)await assert.rejects(run(),/origin lacks conservative/);
  else assert.equal((await run()).originRecoveries.length,1);
  assert.equal(routes,2);assert.equal(recoveries,1);
 }
});

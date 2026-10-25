import assert from 'node:assert/strict';
import test from 'node:test';
import {continuousDoorReceipt,continuousDoorDisplayJump,offscreenMotionFinding} from './bot_motion_boundaries.mjs';
const door={playerId:768,lifeSeq:1,serverTick:6720,destinationX:8024,destinationY:7848};
const sample=(changes={})=>({at:10,wallAt:1000,renderOrdinal:7,lifeSeq:1,initialCount:0,
  snapCount:1,lastSnapReason:3,positionTick:6725,positionSnapshotId:3619,
  positionX:8024,positionY:7848,displayX:8205.99,displayY:7574.5,
  combatViewTimeline:{generation:1,presentationRenderTick:6719},...changes});
const jump=()=>({dtMs:16,previous:sample(),current:sample({at:26,wallAt:1016,renderOrdinal:8,
  positionTick:6726,positionSnapshotId:3620,displayX:8024,displayY:7848,
  combatViewTimeline:{generation:1,presentationRenderTick:6721}})});
test('directed door display boundary requires the exact life, cursor crossing and destination',()=>{
  const j=jump();assert.ok(continuousDoorDisplayJump(j,768,[door]));
  for(const events of [[],[door,door],[{...door,playerId:769}],[{...door,lifeSeq:2}],
    [{...door,serverTick:6718}],[{...door,serverTick:6722}],[{...door,destinationX:7900}]]) {
    assert.equal(continuousDoorDisplayJump(j,768,events),null);
  }
  for(const mutate of [j=>{j.current.displayX+=100;},j=>{j.current.snapCount=2;},
    j=>{j.current.lastSnapReason=2;},j=>{j.current.initialCount=2;},
    j=>{j.current.combatViewTimeline.generation=2;},j=>{j.dtMs=80;}]){
    const bad=jump();mutate(bad);assert.equal(continuousDoorDisplayJump(bad,768,[door]),null);
  }
});
test('continuous Flash receipt is tied to a fresh exact server relocation',()=>{
  const snap={dtMs:17,snapDelta:1,snapReason:3,lifeSeq:1,
    previous:sample({positionTick:6718}),current:sample({positionTick:6720})};
  assert.ok(continuousDoorReceipt(snap,768,[door]));
  for(const event of [{...door,serverTick:6700},{...door,destinationX:8100},{...door,lifeSeq:2}])
    assert.equal(continuousDoorReceipt(snap,768,[event]),null);
  assert.equal(continuousDoorReceipt({...snap,snapDelta:2},768,[door]),null);
});
function sprite(s,changes={}){return {at:s.wallAt,renderOrdinal:s.renderOrdinal-1,entries:[{
  id:768,lifeSeq:s.lifeSeq,drawn:true,suppression:'none',lastSeenSnapshotId:s.positionSnapshotId,
  lastServerPositionTick:s.positionTick & 0xffff,renderedX:s.displayX,renderedY:s.displayY,
  screenX:-1700,screenY:-40,...changes}]};}
test('offscreen explanation needs actual draw/source identity at every observed hold frame',()=>{
  const a=sample(),b=sample({at:30,wallAt:1020,renderOrdinal:8});
  const hold={startedAt:10,endedAt:30,frameCount:2,frames:[a,b]};
  assert.ok(offscreenMotionFinding(hold,[a,b],768,[sprite(a),sprite(b)]));
  for(const changes of [{screenX:100,screenY:100},{lifeSeq:2},{drawn:false},
    {lastSeenSnapshotId:1},{renderedX:0},{screenX:NaN},{suppression:'cloak'}]) {
    assert.equal(offscreenMotionFinding(hold,[a,b],768,[sprite(a),sprite(b,changes)]),null);
  }
  assert.equal(offscreenMotionFinding(hold,[a,b],768,[sprite(a)]),null);
  assert.equal(offscreenMotionFinding({...hold,frameCount:3},[a,b],768,[sprite(a),sprite(b)]),null);
  const ambiguous=sprite(b,{screenX:-1600});
  assert.equal(offscreenMotionFinding(hold,[a,b],768,[sprite(a),sprite(b),ambiguous]),null);
});
test('offscreen classification never erases a visible endpoint or trusts a wrong ordinal',()=>{
  const j=jump(),a=sprite(j.previous),b=sprite(j.current);
  assert.ok(offscreenMotionFinding(j,[],768,[a,b]));
  b.entries[0].screenX=0;b.entries[0].screenY=0;
  assert.equal(offscreenMotionFinding(j,[],768,[a,b]),null);
  b.entries[0].screenX=-1700;b.renderOrdinal++;
  assert.equal(offscreenMotionFinding(j,[],768,[a,b]),null);
});
test('historical timing fallback still requires unique exact source and pose',()=>{
  const j=jump(),a=sprite(j.previous),b=sprite(j.current);
  delete a.renderOrdinal;delete b.renderOrdinal;a.at--;b.at--;
  assert.ok(offscreenMotionFinding(j,[],768,[a,b]));
  b.at-=3;assert.equal(offscreenMotionFinding(j,[],768,[a,b]),null);
});
test('right/bottom offscreen proof requires dimensions from each actual sprite frame',()=>{
 const j=jump(),a=sprite(j.previous,{screenX:900,screenY:1800}),b=sprite(j.current,{screenX:900,screenY:1800});
 assert.equal(offscreenMotionFinding(j,[],768,[a,b]),null);
 Object.assign(a,{screenWidth:1262,screenHeight:795});
 assert.equal(offscreenMotionFinding(j,[],768,[a,b]),null,'a later viewport cannot fill missing earlier dimensions');
 Object.assign(b,{screenWidth:1262,screenHeight:795});
 assert.ok(offscreenMotionFinding(j,[],768,[a,b]));
 b.entries[0].screenY=800;assert.equal(offscreenMotionFinding(j,[],768,[a,b]),null);
});

test('Distance fallback at a committed door destination requires the same exact receipt evidence',()=>{
 const event={playerId:769,lifeSeq:1,serverTick:45000,destinationX:7416,destinationY:7048};
 const snap={dtMs:16,snapDelta:1,snapReason:2,lifeSeq:1,
  previous:sample({initialCount:2,positionTick:44998,positionX:7991,positionY:8040}),
  current:sample({initialCount:2,positionTick:45001,positionX:7416,positionY:7048})};
 assert.ok(continuousDoorReceipt(snap,769,[event]));
 for(const events of [[],[event,event],[{...event,playerId:768}],[{...event,lifeSeq:2}],
  [{...event,serverTick:44998}],[{...event,serverTick:45002}],[{...event,destinationY:7148}]])
  assert.equal(continuousDoorReceipt(snap,769,events),null);
 for(const changes of [{snapReason:1},{snapReason:0},{snapDelta:2},{dtMs:100}])
  assert.equal(continuousDoorReceipt({...snap,...changes},769,[event]),null);
 assert.equal(continuousDoorReceipt({...snap,current:{...snap.current,initialCount:3}},769,[event]),null);
});

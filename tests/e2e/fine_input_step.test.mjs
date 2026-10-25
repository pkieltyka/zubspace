import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";

const source=fs.readFileSync(new URL("./combat_replay.mjs",import.meta.url),"utf8");
const begin=source.indexOf("    async pulseOnFreshInputStep(");
const end=source.indexOf("    async holdKeys(",begin);
assert(begin>=0&&end>begin);

async function replay({holdMs=4,requireAppliedStep=true,pauseDuringHold=false,ignoreInput=false}={}) {
  let now=0,nextFrame=1000/60,sequence=0,held=false,applied=0,lastBits=0;
  const edges=[],records=[];
  const sleep=async ms=>{
    const until=now+ms;
    while(nextFrame<=until){
      sequence++;lastBits=held&&!ignoreInput?1:0;
      if(held&&!pauseDuringHold&&!ignoreInput)applied++;
      nextFrame+=1000/60;
    }
    now=until;
  };
  const context={Date:{now:()=>now},Number,Math,sleep,send:()=>{},chrome:{label:"fine-native-step"},
    focusCanvas:async()=>{},keyEvent:key=>({key}),
    chromeMetricsState:async()=>({uiMetrics:{clientInputSeq:sequence,
      paused:held&&pauseDuringHold,localGameplayState:{inputBits:lastBits}}}),
    logicalInputLifecyclePaused:metrics=>metrics.paused===true,
    dispatchChromeKey:async(_send,kind)=>{held=kind==="keyDown";edges.push({kind,at:now});},
    recordInputTrace:async(_send,_label,kind,_event,at)=>records.push({kind,at}),
  };
  const method=vm.runInNewContext(`({${source.slice(begin,end)}}).pulseOnFreshInputStep`,context);
  let delivered, error;
  try {
    delivered=await method("w",holdMs,{requireAppliedStep});
  } catch (caught) {
    error=caught;
  }
  return {delivered,error,applied,held,edges,records};
}

test("a four-millisecond fine pulse reaches an actual logical movement step",async()=>{
  const result=await replay();
  assert.equal(result.error,undefined);
  assert.equal(result.delivered,true);
  assert(result.applied>=1,"a key edge alone is not an applied steering impulse");
  assert.equal(result.held,false);
  assert.deepEqual(result.edges.map(row=>row.kind),["keyDown","keyUp"]);
  assert.equal(result.records.length,2);
});

test("ordinary timed pulses retain their existing duration",async()=>{
  const result=await replay({holdMs:20,requireAppliedStep:false});
  assert.equal(result.error,undefined);
  assert.equal(result.delivered,true);
  assert.equal(result.edges[1].at-result.edges[0].at,20);
  assert(result.applied>=1);assert.equal(result.held,false);
});

test("a lifecycle pause releases a fine pulse without claiming applied input",async()=>{
  const result=await replay({pauseDuringHold:true});
  assert.equal(result.error,undefined);
  assert.equal(result.delivered,false);assert.equal(result.applied,0);
  assert.equal(result.held,false);
});

test("sequence progress without the requested movement bit is a hard failure and releases keys",async()=>{
  const result=await replay({ignoreInput:true});
  assert.match(result.error?.message,/reached no logical input step/);
  assert.equal(result.applied,0);
  assert.equal(result.held,false);
  assert.deepEqual(result.edges.map(row=>row.kind),["keyDown","keyUp"]);
  assert.equal(result.records.length,2);
});

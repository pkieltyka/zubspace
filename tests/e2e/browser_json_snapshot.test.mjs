import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { captureBrowserJsonSnapshot } from './browser_json_snapshot.mjs';

function browser(value, intercept = () => {}) {
  const context=vm.createContext({window:{value},performance,Date,structuredClone});
  let calls=0;
  return {context, actor:{evaluate:async expression=>{
    calls++; const result=vm.runInContext(expression,context);
    return intercept({expression,result,context,calls}) ?? result;
  }}};
}
test('bounded chunks preserve immutable rows, unicode and numeric precision',async()=>{
  const value={rows:Array.from({length:100},(_,i)=>({i,text:'🚀é',n:1173.5999999195337}))};
  const expected=structuredClone(value);
  const {context,actor}=browser(value,({result,calls})=>{
    if(calls===1) value.rows.length=0;
    if(result?.data) assert.ok(result.data.length<=17);
  });
  const capture=await captureBrowserJsonSnapshot(actor,'window.value',{chunkCharacters:17});
  assert.deepEqual(capture.value,expected);
  assert.ok(capture.timing.freezeMs>=0);
  assert.deepEqual(Object.keys(context.window),['value']);
});
test('oversize fails without retaining a browser snapshot',async()=>{
  const {context,actor}=browser('abcdef');
  await assert.rejects(captureBrowserJsonSnapshot(actor,'window.value',{maximumCharacters:2}),/limit/);
  assert.deepEqual(Object.keys(context.window),['value']);
});
test('read corruption and transport errors fail and release snapshot',async()=>{
  for(const mode of ['offset','data','transport']) {
    const {context,actor}=browser({rows:[1,2]},({result})=>{
      if(result?.data!==undefined) {
        if(mode==='transport') throw Error('transport failed');
        return mode==='offset'?{...result,ordinal:99}:{...result,data:'',done:false};
      }
    });
    await assert.rejects(captureBrowserJsonSnapshot(actor,'window.value'),/chunk|transport/);
    assert.deepEqual(Object.keys(context.window),['value']);
  }
});
test('concurrent captures cannot overwrite or release each other',async()=>{
  const {context,actor}=browser({rows:[1,2,3]});
  const captures=await Promise.all([1,2].map(()=>captureBrowserJsonSnapshot(actor,'window.value',{chunkCharacters:3})));
  assert.deepEqual(captures[0].value,captures[1].value);
  assert.deepEqual(Object.keys(context.window),['value']);
});
test('invalid limits never evaluate browser code',async()=>{
  for(const options of [{chunkCharacters:0},{chunkCharacters:65537},{maximumCharacters:Infinity}]) {
    await assert.rejects(captureBrowserJsonSnapshot({evaluate:()=>assert.fail('must not evaluate')},'null',options),/limits/);
  }
});

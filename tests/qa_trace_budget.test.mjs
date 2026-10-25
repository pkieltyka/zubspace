import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import assert from 'node:assert/strict';

const html=fs.readFileSync(new URL('../index.html',import.meta.url),'utf8');
const start=html.indexOf('          function fitQaTraceBundle(');
const end=html.indexOf('          function downloadQaTrace(',start);
assert(start>=0&&end>start);
function production(bundle, maximum, observed=JSON) {
  const context=vm.createContext({TextEncoder,JSON:observed,bundle,maximum});
  vm.runInContext(html.slice(start,end),context);
  return vm.runInContext('fitQaTraceBundle(bundle, maximum)',context);
}

// Independent, deliberately full-serialization reference for the pre-change
// deterministic selection policy. This is not used by production.
function reference(bundle, maximum) {
  const order=['metrics','prizes','remoteSprites','inputs','remoteLives'];
  const floors={metrics:8,prizes:8,remoteSprites:16,inputs:64,remoteLives:64};
  const size=()=>Buffer.byteLength(JSON.stringify(bundle));
  const remove=(key,n)=>{bundle.rings[key].splice(0,n);bundle.overwritten[key]+=n;bundle.truncated=true;};
  bundle.limits.retainedBytes=0;
  while(size()>maximum) {
    const key=order.find(key=>bundle.rings[key].length>floors[key]);
    if(key) remove(key,Math.min(bundle.rings[key].length-floors[key],Math.ceil(bundle.rings[key].length/4)));
    else {
      const fallback=['damagePresentation','damage','deaths',...order].find(key=>bundle.rings[key].length>1);
      if(!fallback) throw new Error('QA trace fixed metadata exceeds its byte budget');
      remove(fallback,1);
    }
  }
  bundle.limits.retainedBytes=size();
  while(size()>maximum) {
    const key=[...order,'damagePresentation','damage','deaths'].find(key=>bundle.rings[key].length>1);
    if(!key) throw new Error('QA trace fixed metadata exceeds its byte budget');
    remove(key,1);bundle.limits.retainedBytes=size();
  }
  bundle.limits.retainedBytes=size();return bundle;
}
function fixture(count) {
  const names=['metrics','prizes','remoteSprites','inputs','remoteLives','damagePresentation','damage','deaths'];
  return {schema:'test',rings:{...Object.fromEntries(names.map((key,k)=>[key,Array.from({length:count+k},(_,i)=>({at:i,text:'é🚀\\"'.repeat(i%11),nested:[null,i]}))])),gameKeyState:{bits:3}},
    overwritten:Object.fromEntries(names.map(key=>[key,9])),limits:{maxBytes:240*1024},truncated:false};
}
test('cached byte accounting preserves exact reference output across UTF-8 and budget boundaries',()=>{
  for(const count of [0,1,8,17,65,100]) for(const maximum of [100,1000,2000,9999,10000,240*1024]) {
    const input=fixture(count);let expected;
    try {expected=reference(structuredClone(input),maximum);} catch {
      assert.throws(()=>production(structuredClone(input),maximum),/byte budget/);continue;
    }
    const actual=production(structuredClone(input),maximum);
    assert.deepEqual(actual,expected);
    assert(Buffer.byteLength(JSON.stringify(actual))<=maximum);
  }
});
test('large actor frames are serialized once rather than on every trimming pass',()=>{
  const input=fixture(100);
  const actors=Array.from({length:200},(_,id)=>({id,x:id*4,y:id*3,life:1}));
  input.rings.remoteSprites=Array.from({length:600},(_,at)=>({at,actors}));
  let characters=0;
  const observed={stringify(value){const text=JSON.stringify(value);characters+=text.length;return text;}};
  const initialCharacters=JSON.stringify(input).length;
  const result=production(structuredClone(input),240*1024,observed);
  assert.deepEqual(result,reference(structuredClone(input),240*1024));
  assert(characters<initialCharacters*2,`repeated full serialization: ${characters}/${initialCharacters}`);
});

test('array holes and non-finite values use JSON null byte semantics',()=>{
  const input=fixture(1);
  input.rings.metrics=[undefined,NaN,Infinity,-Infinity,,null,'é🚀'];
  for(const maximum of [1000,2000]) {
    assert.deepEqual(production(structuredClone(input),maximum),reference(structuredClone(input),maximum));
  }
});

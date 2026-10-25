import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
import test from 'node:test';
import {createJsonChunkReader,captureCdpJson,prepareCdpJson} from './e2e/cdp_json_capture.mjs';

test('browser preparation exceptions retain the actual exception description and release handles',async()=>{
  const calls=[];
  const send=async(method)=>{
    calls.push(method);
    if(method==='Runtime.evaluate') return {exceptionDetails:{text:'Uncaught',
      exception:{description:'DataCloneError: fixture value could not be cloned'}}};
    if(method==='Runtime.releaseObjectGroup') return {};
    throw new Error('unexpected command');
  };
  await assert.rejects(prepareCdpJson(send,'value'), /DataCloneError: fixture value could not be cloned/);
  assert.deepEqual(calls,['Runtime.evaluate','Runtime.releaseObjectGroup']);
});

test('chunk boundaries preserve exact JSON text, Unicode and shared plain subobjects',()=>{
  const shared={x:'é🚀\\"', missing:undefined, nonfinite:NaN};
  const value={a:[shared,undefined,,null],b:shared,c:[Infinity,-Infinity,true,false,0],
    date:new Date(0),boxed:[new Number(3),new Boolean(false),new String('abc')]};
  for(const size of [1,2,3,7,64,65536]) {
    const reader=createJsonChunkReader(value,size);const parts=[];
    for(;;){const chunk=reader.read();assert(chunk.data.length<=size);parts.push(chunk.data);if(chunk.done)break;}
    assert.equal(parts.join(''),JSON.stringify(value));
  }
});
function fakeBrowser(value,{failure=null,mutate=null}={}) {
  const context=vm.createContext({structuredClone,value,ReadableStream,TextEncoderStream,
    CompressionStream,performance,setTimeout,btoa});
  let holder;const calls=[];
  return {calls,send:async(method,params)=>{
    calls.push({method,params});
    if(method==='Runtime.evaluate') {
      holder=vm.runInContext(params.expression,context);mutate?.(value);
      return {result:{objectId:'frozen-1'}};
    }
    if(method==='Runtime.callFunctionOn') {
      assert.equal(params.awaitPromise,true);
      if(params.functionDeclaration.includes('this.cancel()')) {
        await holder.cancel();return {result:{}};
      }
      if(failure)throw new Error(failure);
      assert.equal(params.objectId,'frozen-1');return {result:{value:await holder.read()}};
    }
    if(method==='Runtime.releaseObjectGroup'){holder=null;return {};}
    throw new Error('unexpected command');
  }};
}
test('cold transfer returns one frozen snapshot and releases its object group',async()=>{
  const value={rows:Array.from({length:1000},(_,i)=>({i,text:'🚀'.repeat(50)}))};
  const expected=structuredClone(value);
  const browser=fakeBrowser(value,{mutate:v=>{v.rows[0].i=-1;v.rows.push({late:true});}});
  assert.deepEqual(await captureCdpJson(browser.send,'value',{chunkCharacters:1024}),expected);
  const reads=browser.calls.filter(c=>c.params.functionDeclaration?.includes('this.read()'));
  assert(reads.length>1);
  assert(reads.length*1024<JSON.stringify(expected).length/4);
  assert.equal(browser.calls.at(-1).method,'Runtime.releaseObjectGroup');
  assert.equal(browser.calls[0].params.objectGroup,browser.calls.at(-1).params.objectGroup);
});
test('transfer and capacity failures release the snapshot without returning partial evidence',async()=>{
  for(const failure of ['connection lost',null]) {
    const browser=fakeBrowser({large:'x'.repeat(1000)},{failure});
    await assert.rejects(captureCdpJson(browser.send,'value',{chunkCharacters:16,maximumCharacters:100}),
      failure ? /connection lost/ : /capture limit/);
    assert.equal(browser.calls.at(-1).method,'Runtime.releaseObjectGroup');
  }
});
test('prepared snapshots freeze before read and can be released unread exactly once',async()=>{
  const value={rows:[{value:1}]};const browser=fakeBrowser(value);
  const snapshot=await prepareCdpJson(browser.send,'value');
  assert.equal(browser.calls.length,1);
  value.rows[0].value=2;
  assert.deepEqual(await snapshot.read(),{rows:[{value:1}]});
  await snapshot.release();await snapshot.release();
  await assert.rejects(snapshot.read(),/phase released/);
  assert.equal(browser.calls.filter(c=>c.method==='Runtime.releaseObjectGroup').length,1);
  const second=fakeBrowser(value);const unread=await prepareCdpJson(second.send,'value');
  await unread.release();await assert.rejects(unread.read(),/phase released/);
  assert.equal(second.calls.filter(c=>c.params.functionDeclaration?.includes('this.read()')).length,0);
  assert.equal(second.calls.filter(c=>c.params.functionDeclaration?.includes('this.cancel()')).length,1);
});
test('concurrent reads cannot consume the same snapshot twice',async()=>{
  const browser=fakeBrowser({value:1});const snapshot=await prepareCdpJson(browser.send,'value');
  const first=snapshot.read();
  await assert.rejects(snapshot.read(),/phase reading/);
  assert.deepEqual(await first,{value:1});
});
test('bounded parallel reads preserve JSON when browser execution order differs',async()=>{
  const value={rows:Array.from({length:200},(_,i)=>({i,text:'🚀'.repeat(10)}))};
  const browser=fakeBrowser(value);let pending=[],active=0,peak=0;
  const send=(method,params)=>{
    if(method!=='Runtime.callFunctionOn'||params.functionDeclaration.includes('this.cancel()')) {
      if(method==='Runtime.releaseObjectGroup')assert.equal(active,0);
      return browser.send(method,params);
    }
    active++;peak=Math.max(peak,active);
    return new Promise((resolve,reject)=>{
      pending.push({resolve,reject,method,params});
      if(pending.length===4) {
        const batch=pending.splice(0).reverse();
        for(const item of batch)browser.send(item.method,item.params).then(value=>{
          active--;item.resolve(value);
        },error=>{active--;item.reject(error);});
      }
    });
  };
  assert.deepEqual(await captureCdpJson(send,'value',{chunkCharacters:31}),value);
  assert.equal(peak,4);assert.equal(active,0);
  assert.deepEqual(await captureCdpJson(fakeBrowser(value).send,'value',{readConcurrency:1,chunkCharacters:31}),value);
});
test('a failed parallel read drains every outstanding request before release',async()=>{
  const browser=fakeBrowser({value:1});let reads=0,released=false;
  let ready;const allStarted=new Promise(resolve=>ready=resolve);const pending=[];
  const send=(method,params)=>{
    if(method==='Runtime.releaseObjectGroup'){assert.equal(pending.length,0);released=true;return browser.send(method,params);}
    if(method!=='Runtime.callFunctionOn'||params.functionDeclaration.includes('this.cancel()'))return browser.send(method,params);
    reads++;
    if(reads===1)return Promise.reject(new Error('first read failed'));
    return new Promise(resolve=>{pending.push(()=>browser.send(method,params).then(resolve));if(reads===4)ready();});
  };
  const capture=captureCdpJson(send,'value');
  const rejected=assert.rejects(capture,/first read failed/);
  await allStarted;assert.equal(released,false);
  const drain=pending.splice(0);for(const finish of drain)finish();
  await rejected;assert.equal(released,true);
});
test('malformed ordinals and nonempty chunks after the terminal are rejected',async()=>{
  for(const mode of ['missing','duplicate','after-terminal']) {
    const browser=fakeBrowser({value:1});let index=0;
    const send=async(method,params)=>{
      const result=await browser.send(method,params);
      if(method==='Runtime.callFunctionOn'&&params.functionDeclaration.includes('this.read()')) {
        if(mode==='missing')delete result.result.value.ordinal;
        if(mode==='duplicate')result.result.value.ordinal=0;
        if(mode==='after-terminal'&&result.result.value.done&&index++>0)result.result.value.data='x';
      }
      return result;
    };
    await assert.rejects(captureCdpJson(send,'value'),mode==='after-terminal'?/after terminal/:/ordinal/);
    assert.equal(browser.calls.at(-1).method,'Runtime.releaseObjectGroup');
  }
  for(const readConcurrency of [0,5,1.5,NaN])await assert.rejects(
    captureCdpJson(fakeBrowser({}).send,'value',{readConcurrency}),/invalid capture limits/);
});
test('cycles and depth overflow are explicit failures',()=>{
  const cyclic={};cyclic.self=cyclic;
  assert.throws(()=>createJsonChunkReader(cyclic,65536).read(),/cyclic/);
  let deep={};for(let i=0;i<130;i++)deep={deep};
  assert.throws(()=>createJsonChunkReader(deep,65536).read(),/depth limit/);
});
test('a cleanup failure cannot erase the original capture failure',async()=>{
  const browser=fakeBrowser({},{failure:'read failed'});
  const send=(method,params)=>method==='Runtime.releaseObjectGroup'
    ? Promise.reject(new Error('release failed')) : browser.send(method,params);
  await assert.rejects(captureCdpJson(send,'value'),error=>{
    assert(error instanceof AggregateError);
    assert.deepEqual(error.errors.map(e=>e.message),['read failed','release failed']);return true;
  });
});
test('explicit release during a batch drains reads before cancellation and handle release',async()=>{
  const browser=fakeBrowser({value:1});
  const pending=[];let started;const ready=new Promise(resolve=>started=resolve);
  const send=(method,params)=>{
    if(method==='Runtime.callFunctionOn'&&params.functionDeclaration.includes('this.read()')) {
      return new Promise((resolve,reject)=>{
        pending.push(()=>browser.send(method,params).then(resolve,reject));
        if(pending.length===4)started();
      });
    }
    if(method==='Runtime.releaseObjectGroup'||params.functionDeclaration?.includes('this.cancel()')) {
      assert.equal(pending.length,0);
    }
    return browser.send(method,params);
  };
  const snapshot=await prepareCdpJson(send,'value');
  const result=assert.rejects(snapshot.read(),/released during read/);
  await ready;
  const release=snapshot.release();
  assert.equal(browser.calls.length,1);
  for(const finish of pending.splice(0))finish();
  await Promise.all([result,release]);
  assert(browser.calls.at(-2).params.functionDeclaration.includes('this.cancel()'));
  assert.equal(browser.calls.at(-1).method,'Runtime.releaseObjectGroup');
});

test('failed compressor cancellation still releases the object group and preserves both errors',async()=>{
  const browser=fakeBrowser({value:1});
  const send=(method,params)=>{
    if(params.functionDeclaration?.includes('this.cancel()'))throw new Error('cancel failed');
    if(method==='Runtime.releaseObjectGroup') {
      browser.send(method,params);throw new Error('release failed');
    }
    return browser.send(method,params);
  };
  const snapshot=await prepareCdpJson(send,'value');
  await assert.rejects(snapshot.release(),error=>{
    assert(error instanceof AggregateError);
    assert.deepEqual(error.errors.map(error=>error.message),['cancel failed','release failed']);return true;
  });
  assert.equal(browser.calls.at(-1).method,'Runtime.releaseObjectGroup');
});
test('cold adapter is separate from ordinary gameplay feedback reads',()=>{
  const source=fs.readFileSync(new URL('./e2e/combat_replay.mjs',import.meta.url),'utf8');
  assert.match(source,/async pageState\(\) \{\s*return chromePageState\(send\);/);
  assert.match(source,/async artifactState\(\) \{\s*return chromePageState\(send, \{cold: true\}\);/);
});

test('abandoned Chrome snapshots finish their read window and preserve both cleanup errors',async()=>{
  const source=fs.readFileSync(new URL('./e2e/combat_replay.mjs',import.meta.url),'utf8');
  const start=source.indexOf('async function chromePageState(');
  const end=source.indexOf('async function chromeMetricsState(',start);
  assert(start>=0&&end>start);
  let evaluations=0;
  const windows=[];
  const context=vm.createContext({instrumentationReadWindows:windows,send:()=>{},
    evaluateChrome:async()=>{if(++evaluations>1)throw new Error('window end failed');return 123;},
    prepareCdpJson:async()=>({read:async()=>({}),release:async()=>{throw new Error('snapshot release failed');}})});
  vm.runInContext(source.slice(start,end),context);
  const prepared=await vm.runInContext('chromePageState(send,{cold:true,prepare:true})',context);
  assert.equal(windows[0].endedAt,null);
  await assert.rejects(prepared.release(),error=>{
    assert.equal(error.name,'AggregateError');
    assert.deepEqual(Array.from(error.errors,e=>e.message),['snapshot release failed','window end failed']);return true;
  });
  assert.notEqual(windows[0].endedAt,null);
});

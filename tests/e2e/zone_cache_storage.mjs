#!/usr/bin/env node
// Browser storage component only: no RTC/gameplay or timing acceptance claims.
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import crypto from "node:crypto";
import {freePort,stopProcess} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {startChrome,waitForChrome} from "./combat_replay.mjs";

const output=path.resolve(process.env.ZUBSPACE_ZONE_CACHE_EVIDENCE||`.debug/zone-cache-storage-${Date.now()}`);
fs.mkdirSync(output,{recursive:true});
const source=fs.readFileSync("js/zubspace_zone_cache.js");
const server=http.createServer((request,response)=>{
  if(request.url==="/cache.js"){response.setHeader("Content-Type","text/javascript");response.end(source);}
  else {response.setHeader("Content-Type","text/html");response.end('<!doctype html><title>Zone cache component</title><script src="/cache.js"></script>');}
});
await new Promise(resolve=>server.listen(0,"127.0.0.1",resolve));
const chrome=startChrome(await freePort(),"zone-cache-storage");
let ws;const pending=new Map();let next=0;
const result={kind:"browser-storage-component",ok:false,sourceSha256:crypto.createHash("sha256").update(source).digest("hex"),console:[],started:new Date().toISOString()};
try {
  await waitForChrome(chrome);
  const target=await(await fetch(`http://127.0.0.1:${chrome.port}/json/new?about:blank`,{method:"PUT"})).json();
  ws=new WebSocket(target.webSocketDebuggerUrl);await new Promise(resolve=>ws.addEventListener("open",resolve,{once:true}));
  ws.addEventListener("message",event=>{
    const message=JSON.parse(event.data);
    if(message.id&&pending.has(message.id)){const item=pending.get(message.id);pending.delete(message.id);clearTimeout(item.timer);message.error?item.reject(Error(JSON.stringify(message.error))):item.resolve(message.result);}
    if(message.method==="Runtime.exceptionThrown")result.console.push(message.params.exceptionDetails?.exception?.description||message.params.exceptionDetails?.text);
    if(message.method==="Runtime.consoleAPICalled"&&["error","warning"].includes(message.params.type))result.console.push(message.params.args.map(x=>x.value||x.description).join(" "));
  });
  const send=(method,params={})=>new Promise((resolve,reject)=>{const id=++next;const timer=setTimeout(()=>{pending.delete(id);reject(Error(`CDP timeout ${method}`));},30000);pending.set(id,{resolve,reject,timer});ws.send(JSON.stringify({id,method,params}));});
  const evaluate=async expression=>{const value=await send("Runtime.evaluate",{expression,awaitPromise:true,returnByValue:true});if(value.exceptionDetails)throw Error(value.exceptionDetails.exception?.description||value.exceptionDetails.text);return value.result.value;};
  await send("Runtime.enable");await send("Page.enable");
  await send("Page.navigate",{url:`http://127.0.0.1:${server.address().port}/`});
  for(let i=0;i<100;i++){if(await evaluate("!!globalThis.ZubspaceZoneCache"))break;await new Promise(r=>setTimeout(r,30));}
  result.cases=await evaluate(`(async()=>{
    const api=ZubspaceZoneCache;
    const check=(value,label)=>{if(!value)throw Error(label);};
    const sha="1".repeat(64),id=name=>"v2.chaos-zone."+sha+"."+name+".lvl";
    const name="zubspace-storage-check-"+Date.now();let clock=10000000000;
    const make=()=>api.create({databaseName:name,clock:()=>clock++});
    const list=async()=>{
      const db=await new Promise((resolve,reject)=>{const r=indexedDB.open(name);r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error);});
      try{return await new Promise((resolve,reject)=>{const tx=db.transaction("metadata");const r=tx.objectStore("metadata").getAll();r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error);});}finally{db.close();}
    };
    let cache=make();await cache.hydrate("server-a");
    check(cache.write(id("map"),new Uint8Array([1,2,3])),"queue map");
    await cache.flush();check(cache.stats.writes===1,"real IndexedDB commit");cache.close();
    cache=make();await cache.hydrate("server-b");check(cache.read(id("map"))===null,"endpoint isolation");cache.close();
    cache=make();await cache.hydrate("server-a");check(String(cache.read(id("map")))==="1,2,3","binary reopen");await cache.flush();
    check(cache.read(id("map").replace(sha,"2".repeat(64)))===null,"digest isolation");
    const originalPut=IDBObjectStore.prototype.put;let injected=0;
    IDBObjectStore.prototype.put=function(...args){if(this.name==="data"&&injected++===0)throw new DOMException("forced quota","QuotaExceededError");return originalPut.apply(this,args);};
    try{cache.write(id("afterquota"),new Uint8Array([4,5]));await cache.flush();}finally{IDBObjectStore.prototype.put=originalPut;}
    check(cache.stats.quotaRetries===1&&cache.stats.writes===1,"quota retry transaction and commit "+JSON.stringify({stats:cache.stats,injected,metadata:await list()}));
    check(cache.read(id("map"))===null,"quota evicts the actual oldest entry");check(String(cache.read(id("afterquota")))==="4,5","quota retry bytes");
    await cache.flush();cache.close();
    // Corrupt binary length in the actual store; hydration must remove both
    // metadata and payload without turning it into a held file.
    const db=await new Promise(resolve=>{const r=indexedDB.open(name);r.onsuccess=()=>resolve(r.result);});
    await new Promise((resolve,reject)=>{const tx=db.transaction("data","readwrite");tx.objectStore("data").put(new ArrayBuffer(1),api.keyFor("server-a",id("afterquota")));tx.oncomplete=resolve;tx.onabort=()=>reject(tx.error);});db.close();
    cache=make();await cache.hydrate("server-a");check(cache.read(id("afterquota"))===null&&cache.stats.invalid===1,"corrupt payload removed");
    // Actual object-store eviction, not just an arithmetic policy check.
    for(let i=0;i<65;i++)cache.write(id("file"+i),new Uint8Array(api.limits.fileBytes).fill(i));
    await cache.flush();check(cache.stats.writes===65,"all bounded writes commit");
    const records=await list(),bytes=records.reduce((sum,r)=>sum+r.size,0);
    check(bytes<=api.limits.bytes&&records.length===64,"global persistent byte budget");
    check(cache.read(id("file0"))===null&&cache.read(id("file64"))?.[0]===64,"real LRU order");
    await cache.flush();cache.close();
    cache=make();await cache.hydrate("server-a");check(cache.read(id("file64"))?.[0]===64,"bounded store survives reopen");await cache.flush();cache.close();
    return {binaryReopen:true,endpointIsolation:true,digestIsolation:true,quotaRetry:true,corruptPayloadCleanup:true,realLru:true,retainedFiles:records.length,retainedBytes:bytes};
  })()`);
  assert.equal(result.console.length,0,JSON.stringify(result.console));result.ok=true;
} catch(error){result.error=error.stack;process.exitCode=1;}
finally {
  for(const item of pending.values()){clearTimeout(item.timer);item.reject(Error("storage component cleanup"));}pending.clear();ws?.close();
  await stopProcess(chrome.child);await new Promise(resolve=>server.close(resolve));
  fs.writeFileSync(path.join(output,"chrome.log"),chrome.logs.join(""));
  fs.writeFileSync(path.join(output,"result.json"),JSON.stringify(result,null,2)+"\n");
}
console.log(JSON.stringify({ok:result.ok,evidence:output,cases:result.cases,error:result.error}));

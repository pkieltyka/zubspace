import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import crypto from "node:crypto";
import {zoneAssetManifest} from "../tools/zone_asset_manifest.mjs";
await import("../js/zubspace_zone_cache.js");
await import("../js/zubspace_zone_loading.js");
const api=globalThis.ZubspaceZoneCache;
const now=10_000_000_000;
const identity=(zone="chaos-zone",sha="1".repeat(64),name="map.lvl")=>`v2.${zone}.${sha}.${name}`;
const record=(endpoint,overrides={})=>{
  const id=identity();return {v:1,endpoint,identity:id,key:api.keyFor(endpoint,id),size:2,at:now-1,...overrides};
};

test("binary cache keys separate servers, zones, filenames and exact SHA-256",()=>{
  const id=identity();const original=api.keyFor("https://a/rtc/offer",id);
  for(const [endpoint,next] of [["https://b/rtc/offer",id],["https://a/rtc/offer",identity("extreme")],
    ["https://a/rtc/offer",identity("chaos-zone","2".repeat(64))],["https://a/rtc/offer",identity("chaos-zone","1".repeat(64),"art.lvz")]]) {
    assert.notEqual(api.keyFor(endpoint,next),original);
  }
  assert.equal(api.identityParts(identity()).filename,"map.lvl");
  assert.equal(api.identityParts(identity("chaos-zone","1".repeat(64),"art.lvz")).zone,"chaos-zone");
  for(const bad of ["v2.bad",identity("../bad"),identity("chaos-zone","g".repeat(64)),identity("chaos-zone","1".repeat(64),"../map.lvl")])assert.equal(api.identityParts(bad),null);
});

test("LRU admits exact byte and entry budgets and evicts only the oldest needed entries",()=>{
  const a=record("a",{at:now-3}),b=record("b",{at:now-2}),c=record("c");
  const exact=api.evictionPlan([a],b,now,{bytes:4,entries:2});
  assert.deepEqual(exact,{remove:[],bytes:4,count:2});
  const overflow=api.evictionPlan([a,b],c,now,{bytes:4,entries:2});
  assert.deepEqual(overflow,{remove:[a.key],bytes:4,count:2});
  const entryOverflow=api.evictionPlan([a,b],c,now,{bytes:100,entries:2});
  assert.deepEqual(entryOverflow.remove,[a.key]);
  const replace=api.evictionPlan([a,b],{...a,at:now},now,{bytes:4,entries:2});
  assert.deepEqual(replace,{remove:[],bytes:4,count:2});
  assert.throws(()=>api.evictionPlan([],c,now,{bytes:1,entries:2}),/budget/);
});

test("64 MiB persistent bound evicts the sixty-fifth maximal file",()=>{
  const records=Array.from({length:64},(_,i)=>record(String(i),{size:api.limits.fileBytes,at:now-100+i}));
  const add=record("new",{size:api.limits.fileBytes,at:now});
  const plan=api.evictionPlan(records,add,now);
  assert.deepEqual(plan.remove,[records[0].key]);assert.equal(plan.count,64);assert.ok(plan.bytes<=64*1024*1024);
});

test("corrupt, expired and future-dated metadata is removed before budget accounting",()=>{
  const inputs=[record("a",{v:0}),record("b",{size:0}),record("c",{size:api.limits.fileBytes+1}),
    record("d",{at:0}),record("e",{at:now+60001}),record("f",{key:"wrong"})];
  assert.deepEqual(api.evictionPlan(inputs,null,now).remove,inputs.map(x=>x.key));
  assert.equal(api.evictionPlan(inputs,null,now).bytes,0);
});

test("quota recovery evicts and retries once, preserving other errors and terminal failure",async()=>{
  let calls=0,evictions=0,retries=0;
  const quota=()=>Object.assign(Error("quota"),{name:"QuotaExceededError"});
  assert.equal(await api.withQuotaRetry(async()=>{if(++calls===1)throw quota();return 17;},async()=>{evictions++;},()=>{retries++;}),17);
  assert.deepEqual([calls,evictions,retries],[2,1,1]);
  calls=0;evictions=0;
  await assert.rejects(api.withQuotaRetry(async()=>{calls++;throw Error("disk");},async()=>{evictions++;}),/disk/);
  assert.deepEqual([calls,evictions],[1,0]);
  calls=0;await assert.rejects(api.withQuotaRetry(async()=>{calls++;throw quota();},async()=>{}),{name:"QuotaExceededError"});
  assert.equal(calls,2);
});

test("unavailable browser storage stays optional and never claims a persistent write",async()=>{
  const cache=api.create({indexedDB:null});await cache.hydrate("server");
  assert.equal(cache.read(identity()),null);
  assert.equal(cache.write(identity(),new Uint8Array([1,2])),true);
  assert.equal(cache.write("bad",new Uint8Array([1])),false);
  await cache.flush();assert.equal(cache.stats.writes,0);assert.equal(cache.stats.queuedWrites,1);assert.ok(cache.stats.storageErrors>=1);
  const evidence=await cache.policyEvidence();
  for(const key of ["identityIsolation","totalBudgetLru","quotaRetry","corruptStaleCleanup"])assert.equal(evidence[key],true,key);
});

test("generated manifest carries the current digest of every selected asset",()=>{
  const value=zoneAssetManifest(process.cwd());assert.equal(value.version,2);
  for(const paths of Object.values(value.zones))for(const path of paths) {
    assert.equal(value.sha256[path],crypto.createHash("sha256").update(fs.readFileSync(path)).digest("hex"));
  }
});

test("prefetch uses the exact manifest-digest cache entry and never downloads a held file",async()=>{
  const path="assets/zones/chaos-zone/map.lvl",bytes=new Uint8Array([1,2,3]),sha="1".repeat(64),seen=[];
  const cache={ready:Promise.resolve(),read:id=>{seen.push(id);return bytes;}};
  let network=0;
  const loader=globalThis.ZubspaceZoneLoading.create({cache,fetch:async url=>{
    assert.equal(url,"manifest");network++;
    return new Response(JSON.stringify({version:2,zones:{"chaos-zone":[path]},sha256:{[path]:sha}}));
  }});
  await loader.prepare("chaos-zone","manifest");
  assert.equal(network,1);assert.deepEqual(seen,[identity()]);assert.equal(loader.stats.cacheHits,1);
  assert.equal(loader.provisionalBytes(identity()),bytes);
  assert.equal(loader.provisionalBytes(identity("extreme")),null);
});

test("changed manifest digest misses the old cache and downloads new bytes",async()=>{
  const path="assets/zones/chaos-zone/map.lvl",sha="2".repeat(64),calls=[];
  const loader=globalThis.ZubspaceZoneLoading.create({cache:{ready:Promise.resolve(),read:id=>id===identity()?new Uint8Array([1]):null},fetch:async url=>{
    calls.push(url);return url==="manifest"?new Response(JSON.stringify({version:2,zones:{"chaos-zone":[path]},sha256:{[path]:sha}})):new Response(new Uint8Array([9]));
  }});
  await loader.prepare("chaos-zone","manifest");assert.deepEqual(calls,["manifest",path]);
  assert.deepEqual(Array.from(loader.provisionalBytes(identity("chaos-zone",sha))),[9]);
});

(function (root) {
  "use strict";
  const limits = Object.freeze({bytes:64*1024*1024, entries:512, fileBytes:1024*1024-17, ageMs:30*24*60*60*1000});
  const identityPattern = /^v2\.([a-z0-9](?:[a-z0-9-]{0,13}[a-z0-9])?)\.([a-f0-9]{64})\.([A-Za-z0-9_#@.-]+\.(?:lvl|lvz))$/i;
  function identityParts(identity) {
    const match = typeof identity === "string" && identityPattern.exec(identity);
    if (!match || match[3].includes("..") || match[3].length>=16) return null;
    return { zone:match[1], sha256:match[2], filename:match[3] };
  }
  const keyFor = (endpoint, identity) => JSON.stringify([endpoint, identity]);
  function validMetadata(record, now) {
    return record?.v===1 && typeof record.endpoint==="string" && record.endpoint.length<=4096 &&
      identityParts(record.identity) && record.key===keyFor(record.endpoint, record.identity) &&
      Number.isSafeInteger(record.size) && record.size>0 && record.size<=limits.fileBytes &&
      Number.isFinite(record.at) && record.at>=0 && record.at<=now+60000 && now-record.at<=limits.ageMs;
  }
  function evictionPlan(records, incoming, now, cap=limits) {
    const remove = records.filter(record => !validMetadata(record, now)).map(record=>record.key);
    const existing = records.filter(record => validMetadata(record, now) && record.key!==incoming?.key)
      .sort((a,b)=>a.at-b.at || a.key.localeCompare(b.key));
    let bytes=existing.reduce((sum,record)=>sum+record.size,0)+(incoming?.size||0);
    let count=existing.length+(incoming?1:0);
    while (existing.length && (bytes>cap.bytes || count>cap.entries)) {
      const record=existing.shift();remove.push(record.key);bytes-=record.size;count--;
    }
    if (bytes>cap.bytes || count>cap.entries) throw new Error("cache entry exceeds store budget");
    return { remove, bytes, count };
  }
  function open(factory, name) {
    return new Promise((resolve,reject)=>{
      if (!factory) { reject(new Error("IndexedDB unavailable"));return; }
      const request=factory.open(name,1);
      let settled=false;
      const fail=error=>{if(settled)return;settled=true;clearTimeout(deadline);reject(error);};
      const deadline=setTimeout(()=>fail(new Error("cache database open timed out")),1000);
      request.onupgradeneeded=()=>{
        const db=request.result;
        const meta=db.createObjectStore("metadata",{keyPath:"key"});meta.createIndex("endpoint","endpoint");
        db.createObjectStore("data");
      };
      request.onerror=()=>fail(request.error);
      request.onblocked=()=>fail(new Error("cache database upgrade blocked"));
      request.onsuccess=()=>{const db=request.result;if(settled){db.close();return;}settled=true;clearTimeout(deadline);db.onversionchange=()=>db.close();resolve(db);};
    });
  }
  async function withQuotaRetry(operation, evict, onRetry=()=>{}) {
    try { return await operation(); }
    catch(error) {
      if(error?.name!=="QuotaExceededError")throw error;
      onRetry();await evict();return operation();
    }
  }
  function create(options={}) {
    const clock=options.clock||Date.now;
    const stats={hits:0,httpHits:0,misses:0,writes:0,queuedWrites:0,invalid:0,evictions:0,quotaRetries:0,storageErrors:0};
    const memory=new Map();
    let endpoint="", db=null, sequence=Promise.resolve(), ready=Promise.resolve();
    function transaction(mode, run) {
      if (!db) return Promise.reject(new Error("cache unavailable"));
      return new Promise((resolve,reject)=>{
        const tx=db.transaction(["metadata","data"],mode);
        const meta=tx.objectStore("metadata"),data=tx.objectStore("data");
        let failure=null;
        const deadline=setTimeout(()=>{try{tx.abort();}catch{}reject(new Error("cache transaction timed out"));},2000);
        tx.oncomplete=()=>{clearTimeout(deadline);resolve();};
        tx.onabort=()=>{clearTimeout(deadline);reject(failure||tx.error||new Error("cache transaction aborted"));};
        tx.onerror=event=>{failure=failure||event.target?.error;};
        const abort=error=>{failure=error;try{tx.abort();}catch{};};
        try { run(meta,data,tx,abort); } catch(error) {abort(error);}
      });
    }
    async function hydrate(server) {
      endpoint=server;
      ready=(async()=>{
        try {
          db=await open(options.indexedDB===undefined?root.indexedDB:options.indexedDB,options.databaseName||"zubspace.zone-files.v1");
          const now=clock();let count=0,bytes=0;
          await transaction("readwrite",(meta,data)=>{
            const cursor=meta.index("endpoint").openCursor(server);
            cursor.onsuccess=()=>{
              const item=cursor.result;if(!item)return;
              const record=item.value;
              if (!validMetadata(record,now) || ++count>limits.entries || (bytes+=record.size)>limits.bytes) {
                meta.delete(item.primaryKey);data.delete(item.primaryKey);stats.invalid++;item.continue();return;
              }
              const get=data.get(record.key);
              get.onsuccess=()=>{
                const value=get.result;
                if (!(value instanceof ArrayBuffer) || value.byteLength!==record.size) {
                  meta.delete(record.key);data.delete(record.key);stats.invalid++;
                } else { memory.set(record.key,{...record,bytes:new Uint8Array(value)}); }
                item.continue();
              };
            };
          });
        } catch { stats.storageErrors++; db?.close();db=null;memory.clear(); }
      })();
      return ready;
    }
    function read(identity) {
      if (!identityParts(identity)) {stats.invalid++;return null;}
      const record=memory.get(keyFor(endpoint,identity));
      if (!record) {stats.misses++;return null;}
      if (!validMetadata(record,clock())) {remove(identity);return null;}
      stats.hits++;record.at=clock();
      // Recency writes are serialized with publication writes. Never hold an
      // IDB transaction across a Promise or a WASM frame boundary.
      sequence=sequence.then(()=>transaction("readwrite",(meta)=>{
        const get=meta.get(record.key);get.onsuccess=()=>{if(get.result)meta.put({...get.result,at:record.at});};
      })).catch(()=>{stats.storageErrors++;});
      return record.bytes;
    }
    async function put(record) {
      let removed=[];
      await transaction("readwrite",(meta,data,tx,abort)=>{
        const all=meta.getAll();all.onsuccess=()=>{
          try {
            const plan=evictionPlan(all.result,record,clock());
            removed=plan.remove;
            for(const key of removed){meta.delete(key);data.delete(key);}
            const {bytes,...metadata}=record;meta.put(metadata);data.put(bytes.buffer,record.key);
          } catch(error) {abort(error);}
        };
      });
      for(const key of removed){memory.delete(key);stats.evictions++;}
    }
    async function evictOldest() {
      await transaction("readwrite",(meta,data)=>{
        const all=meta.getAll();all.onsuccess=()=>{
          const record=all.result.sort((a,b)=>(Number(a.at)||0)-(Number(b.at)||0) || String(a.key).localeCompare(String(b.key)))[0];
          if(record){meta.delete(record.key);data.delete(record.key);memory.delete(record.key);stats.evictions++;}
        };
      });
    }
    function write(identity, value) {
      if(!identityParts(identity)||!(value instanceof Uint8Array)||value.byteLength<=0||value.byteLength>limits.fileBytes)return false;
      const bytes=new Uint8Array(value);const key=keyFor(endpoint,identity);
      const record={v:1,key,endpoint,identity,size:bytes.byteLength,at:clock(),bytes};
      stats.queuedWrites++;
      sequence=sequence.then(async()=>{
        await ready;
        if(!db)return;
        await withQuotaRetry(()=>put(record),evictOldest,()=>{stats.quotaRetries++;});
        memory.set(key,record);stats.writes++;
      }).catch(error=>{stats.storageErrors++;stats.lastStorageError=error?.name||"Error";});
      return true;
    }
    function remove(identity) {
      const key=keyFor(endpoint,identity);memory.delete(key);stats.invalid++;
      sequence=sequence.then(()=>transaction("readwrite",(meta,data)=>{meta.delete(key);data.delete(key);})).catch(()=>{stats.storageErrors++;});
    }
    async function policyEvidence() {
      const now=10000000000, sha="1".repeat(64), id=`v2.zone-a.${sha}.map.lvl`;
      const a={v:1,endpoint:"a",identity:id,key:keyFor("a",id),size:2,at:now-2};
      const b={...a,endpoint:"b",key:keyFor("b",id),at:now-1};
      const incoming={...a,endpoint:"c",key:keyFor("c",id),at:now};
      const plan=evictionPlan([a,b],incoming,now,{bytes:4,entries:2});
      let attempts=0,evicted=0;
      await withQuotaRetry(async()=>{if(++attempts===1)throw Object.assign(new Error("quota"),{name:"QuotaExceededError"});},async()=>{evicted++;});
      return {backend:"indexeddb-binary",identityIsolation:a.key!==b.key&&a.key!==keyFor("a",id.replace(sha,"2".repeat(64))),
        totalBudgetLru:plan.remove.length===1&&plan.remove[0]===a.key,
        corruptStaleCleanup:evictionPlan([{...a,at:0},{...b,size:-1}],null,now).remove.length===2,
        // Actual quota recovery is exercised by the browser storage test.
        quotaRetry:attempts===2&&evicted===1};
    }
    return {hydrate,read,write,remove,stats,policyEvidence,flush:()=>sequence, get ready(){return ready;},close:()=>db?.close()};
  }
  root.ZubspaceZoneCache={create,identityParts,keyFor,validMetadata,evictionPlan,withQuotaRetry,limits};
})(globalThis);

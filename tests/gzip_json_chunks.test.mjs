import assert from 'node:assert/strict';
import test from 'node:test';
import {gunzipSync} from 'node:zlib';
import {createHash} from 'node:crypto';
import {createJsonChunkReader} from './e2e/cdp_json_capture.mjs';
import {createGzipJsonReader} from './e2e/gzip_json_chunks.mjs';

async function drain(reader, limit) {
  const parts=[];
  let done=false, ordinal=0;
  while(!done) {
    const batch=await Promise.all(Array.from({length:4},()=>reader.read()));
    for(const chunk of batch) {
      assert.equal(chunk.ordinal,ordinal++);
      assert.equal(chunk.encoding,'gzip-base64');
      assert(chunk.data.length<=limit);
      assert(!done || (chunk.done && !chunk.data));
      parts.push(chunk.data); done ||= chunk.done;
    }
  }
  await reader.cancel();
  return Buffer.from(parts.join(''),'base64');
}

test('gzip diagnostic chunks reproduce exact JSON through split Unicode and parallel reads',async()=>{
  const shared={text:'é🚀\ud800\\"',missing:undefined,number:NaN};
  const value={rows:[shared,shared],nil:null,holes:[undefined,,Infinity],date:new Date(0)};
  const expected=JSON.stringify(value);
  for(const size of [1,2,3,7,31,65536]) {
    const reader=createGzipJsonReader(createJsonChunkReader(value,3),expected.length,size);
    const compressed=await drain(reader,size);
    assert.equal(gunzipSync(compressed).toString('utf8'),expected);
  }
});

test('compression cannot bypass the decoded character limit',async()=>{
  const value={text:'x'.repeat(50000)};
  const reader=createGzipJsonReader(createJsonChunkReader(value,128),100,64);
  await assert.rejects(drain(reader,64),/capture limit/);
  await reader.cancel().catch(()=>{});
});

test('multiple compressed output blocks form one lossless base64 stream',async()=>{
  // Deterministic, poorly compressible data forces many compressor blocks;
  // tiny fixtures alone cannot pin framing across those boundaries.
  const value={rows:Array.from({length:8192},(_,index)=>
    createHash('sha256').update(String(index)).digest('hex'))};
  const expected=JSON.stringify(value);
  for(const size of [4093,65536]) {
    const reader=createGzipJsonReader(createJsonChunkReader(value,8191),expected.length,size);
    const compressed=await drain(reader,size);
    assert(compressed.length>128*1024);
    assert.equal(gunzipSync(compressed).toString('utf8'),expected);
  }
});

test('unread cancellation performs no JSON work and prevents later reads',async()=>{
  const reader=createGzipJsonReader({read(){assert.fail('unread capture was evaluated');}},100,16);
  await reader.cancel();await reader.cancel();
  await assert.rejects(reader.read(),/cancelled/);
});

test('cancellation settles active reads and stops the source',async()=>{
  let reads=0;
  const reader=createGzipJsonReader({read(){reads++;return {data:' ',done:false};}},100000,16);
  const pending=Array.from({length:4},()=>reader.read());
  const settled=Promise.allSettled(pending);
  await new Promise(resolve=>setTimeout(resolve,10));
  await reader.cancel();
  await settled;
  const stopped=reads;
  await new Promise(resolve=>setTimeout(resolve,10));
  assert.equal(reads,stopped);
  await assert.rejects(reader.read(),/cancelled/);
});

test('source failures are explicit rather than a successful partial trace',async()=>{
  const reader=createGzipJsonReader({read(){throw new Error('source exploded');}},100,16);
  await assert.rejects(drain(reader,16),/source exploded/);
  await reader.cancel().catch(()=>{});
});

test('limits reject invalid values before starting streams',()=>{
  for(const size of [0,-1,1.5,NaN,65537])
    assert.throws(()=>createGzipJsonReader({},100,size),/limits/);
  for(const maximum of [0,-1,1.5,NaN,256*1024*1024+1])
    assert.throws(()=>createGzipJsonReader({},maximum,16),/limits/);
});

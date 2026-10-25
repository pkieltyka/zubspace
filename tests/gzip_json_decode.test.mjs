import assert from 'node:assert/strict';
import test from 'node:test';
import {gzipSync} from 'node:zlib';
import {createGzipJsonCollector} from './e2e/gzip_json_decode.mjs';
import {createGzipJsonReader} from './e2e/gzip_json_chunks.mjs';
import {createJsonChunkReader} from './e2e/cdp_json_capture.mjs';

function encoded(text, sourceCharacters = text.length) {
  return {encoding:'gzip-base64',data:gzipSync(text).toString('base64'),done:true,sourceCharacters};
}

test('collector round trips the actual encoder with split Unicode and tiny transport chunks',async()=>{
  const value={text:'é🚀\ud800'.repeat(1000),rows:[1,null,true]};
  const length=JSON.stringify(value).length;
  for(const size of [1,7,65536]) {
    const collector=createGzipJsonCollector(length,size);
    const reader=createGzipJsonReader(createJsonChunkReader(value,3),length,size);
    try {
      let done=false;
      while(!done)for(const chunk of await Promise.all(Array.from({length:4},()=>reader.read()))) {
        collector.append(chunk);done ||= chunk.done;
      }
      assert.deepEqual(await collector.decode(),value);
      await assert.rejects(collector.decode(),/already decoded/);
    } finally {await reader.cancel();}
  }
});

test('decompression enforces actual output bound despite a dishonest source count',async()=>{
  const collector=createGzipJsonCollector(100,65536);
  collector.append(encoded(JSON.stringify('x'.repeat(10000)),100));
  await assert.rejects(collector.decode(),/larger than|output length|buffer/i);
});

test('UTF-16 character count is checked independently of the UTF-8 byte bound',async()=>{
  const collector=createGzipJsonCollector(100,65536);
  collector.append(encoded(JSON.stringify('x'.repeat(150)),100));
  await assert.rejects(collector.decode(),/character count mismatch/);
});

test('invalid framing, truncated gzip, corrupt gzip and invalid JSON never return partial evidence',async()=>{
  const valid=encoded('{"ok":true}');
  const truncated=Buffer.from(valid.data,'base64').subarray(0,-4).toString('base64');
  const corrupt=Buffer.from(valid.data,'base64');corrupt[corrupt.length-8]^=1;
  for(const data of ['!',valid.data+'AAAA',valid.data.slice(0,-1),truncated,corrupt.toString('base64'),
    gzipSync('not-json').toString('base64')]) {
    const collector=createGzipJsonCollector(100,65536);
    collector.append({...valid,data});
    await assert.rejects(collector.decode());
  }
});

test('collector rejects wrong encoding, regressing counts and changed terminal records',async()=>{
  const good=encoded('{}');
  for(const change of [{encoding:'json'},{sourceCharacters:-1},{sourceCharacters:101},
    {sourceCharacters:NaN},{data:'' ,done:false},{data:12}]) {
    assert.throws(()=>createGzipJsonCollector(100,65536).append({...good,...change}),/invalid/);
  }
  const collector=createGzipJsonCollector(100,65536);
  collector.append(good);
  assert.throws(()=>collector.append({...good,data:'' ,sourceCharacters:1}),/invalid/);
  assert.throws(()=>collector.append({...good,data:'' ,sourceCharacters:3}),/after terminal/);
  assert.throws(()=>collector.append(good),/after terminal/);
  await assert.rejects(createGzipJsonCollector(100,65536).decode(),/incomplete/);
});

test('encoded transport is bounded even without an end marker',()=>{
  const collector=createGzipJsonCollector(1,65536);
  assert.throws(()=>collector.append({encoding:'gzip-base64',data:'A'.repeat(2048),
    done:false,sourceCharacters:1}),/encoded capture limit/);
});

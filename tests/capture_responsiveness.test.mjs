import test from 'node:test';
import assert from 'node:assert/strict';
import {measureCaptureResponsiveness} from './e2e/capture_responsiveness.mjs';

test('both connections are measured before and after the marker', async () => {
  const calls=[];
  const operation=name=>async()=>{calls.push(name);return name;};
  const result=await measureCaptureResponsiveness({existing:operation('existing'),fresh:operation('fresh'),freeze:operation('freeze')});
  assert.deepEqual(calls,['existing','fresh','freeze','existing','fresh']);
  assert.equal(result.marker.value,'freeze');
  assert.equal(result.after.fresh.ok,true);
});

test('failed original connection and marker remain failures despite fresh success', async () => {
  const result=await measureCaptureResponsiveness({existing:async()=>{throw new Error('original timeout');},fresh:async()=>42,freeze:async()=>{throw new Error('freeze timeout');}});
  assert.equal(result.before.existing.ok,false);
  assert.equal(result.marker.ok,false);
  assert.equal(result.after.existing.error,'original timeout');
  assert.equal(result.after.fresh.value,42);
  assert.equal(result.ok,undefined);
});

import assert from 'node:assert/strict';
import test from 'node:test';
import {summarizeWindow} from './client_debug_overhead_probe.mjs';

test('overhead measurement retains slow frames rather than excluding them',()=>{
  const report=summarizeWindow([10,20,30,40,100],30000,.25);
  assert.equal(report.frames,5);assert.equal(report.frameGapMaxMs,100);
  assert.equal(report.frameGapP50Ms,30);assert.equal(report.gapsOver50Ms,1);
  assert.equal(report.taskMsPerFrame,50);
});

test('incomplete, overflowed or non-finite measurements cannot produce a cost result',()=>{
  for(const [rows,duration,cpu] of [[[],30000,1],[[16],24000,1],[[NaN],30000,1],
    [[16],30000,NaN],[[16],30000,-1],[Array(8193).fill(16),30000,1]])
    assert.throws(()=>summarizeWindow(rows,duration,cpu),/complete bounded/);
});

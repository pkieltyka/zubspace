import assert from 'node:assert/strict';
import test from 'node:test';
import {chromeGraphicsArguments} from '../tools/chrome_graphics.mjs';
test('default graphics launch remains explicitly software rendered',()=>{
  assert.deepEqual(chromeGraphicsArguments(),['--enable-unsafe-swiftshader','--use-gl=angle','--use-angle=swiftshader']);
});
test('hardware diagnostic is explicit and rejects arbitrary flags',()=>{
  assert.deepEqual(chromeGraphicsArguments('vulkan'),['--use-gl=angle','--use-angle=vulkan']);
  for(const value of ['',null,'default','vulkan --disable-web-security'])assert.throws(()=>chromeGraphicsArguments(value));
});

test('native graphics uses the ordinary browser selection; runtime evidence must assert hardware',()=>{
  assert.deepEqual(chromeGraphicsArguments('native'),[]);
});

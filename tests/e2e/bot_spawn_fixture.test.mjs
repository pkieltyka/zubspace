import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {createBotSpawnFixture} from './bot_spawn_fixture.mjs';
const digest=s=>crypto.createHash('sha256').update(s).digest('hex');
const removeSpawns=s=>s.replace(/^Team[0-3]-(?:X|Y|Radius)=-?\d+/gm,'<declared scenario spawn>');
test('ordinary and Extreme bot runs keep their original spawn policy by default',()=>{
 assert.equal(createBotSpawnFixture('chaos-zone'),null);
 assert.equal(createBotSpawnFixture('extreme'),null);
 for(const [zone,option] of [['extreme','central'],['chaos-zone','random'],['chaos-zone','true']])
   assert.throws(()=>createBotSpawnFixture(zone,option),/explicitly/);
});
test('declared combat fixture changes only twelve spawn fields and retains exact replay material',t=>{
 const original=fs.readFileSync('assets/zones/chaos-zone/arena.conf','utf8');
 const originalMap=fs.readFileSync('assets/zones/chaos-zone/bdhadar.lvl');
 const fixture=createBotSpawnFixture('chaos-zone','central');
 t.after(()=>fs.rmSync(fixture.root,{recursive:true,force:true}));
 const actual=fs.readFileSync(path.join(fixture.root,'chaos-zone/arena.conf'),'utf8');
 assert.equal(removeSpawns(actual),removeSpawns(original));
 assert.equal((actual.match(/^Team[0-3]-(?:X|Y|Radius)=/gm)||[]).length,12);
 assert.deepEqual(fixture.spawnRegion,{x:512,y:512,radius:2});
 assert.equal(actual,fixture.evidence.settingsText);
 assert.equal(digest(actual),fixture.settingsSha256);
 assert.equal(digest(original),fixture.originalSettingsSha256);
 assert.match(fixture.evidence.manifestText,new RegExp(`^settings_sha256=${digest(actual)}$`,'m'));
 assert.equal(fs.readFileSync('assets/zones/chaos-zone/arena.conf','utf8'),original);
 assert.deepEqual(fs.readFileSync('assets/zones/chaos-zone/bdhadar.lvl'),originalMap);
 assert.equal(digest(originalMap),fixture.mapSha256);
 assert.equal(digest(fs.readFileSync(path.join(fixture.root,'chaos-zone/bdhadar.lvl'))),digest(originalMap));
 assert.match(fixture.evidence.scope,/not randomized-spawn acceptance/);
});

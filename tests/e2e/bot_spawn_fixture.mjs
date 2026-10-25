import fs from 'node:fs';
import path from 'node:path';
import {createSpawnRegionFixture} from './fixture_spawn_region.mjs';

// Explicit harness option only. Ordinary runs and shipped settings are untouched.
export function createBotSpawnFixture(arena, option='') {
  if (option === '') return null;
  if (option !== 'central' || arena !== 'chaos-zone') {
    throw new Error('bot spawn fixture must explicitly select central in Chaos');
  }
  const fixture=createSpawnRegionFixture({zoneId:arena,spawnRegion:{x:512,y:512,radius:2}});
  const {root,...identity}=fixture;
  return {...fixture,evidence:{...identity,contract:'declared-central-bot-combat-spawns',
    scope:'Scenario spawn fields only; not randomized-spawn acceptance',
    settingsText:fs.readFileSync(path.join(root,arena,'arena.conf'),'utf8'),
    manifestText:fs.readFileSync(path.join(root,arena,'zone.bundle'),'utf8')}};
}

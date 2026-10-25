import {logLines} from '../../tools/log_chunks.mjs';
import {serverLogRecord} from '../../tools/server_log_fields.mjs';
const tick = value => Number.isInteger(value) && value >= 0 && value <= 0x7fff_ffff;
const elapsed = (a,b) => (a-b) & 0x7fff_ffff;
// Bound to the committed server AOI constants in the native control below.
const NEAR_RADIUS=2048;
export function outsideAoiRespawnEvidence(logs, death, observed, observerId, slackTicks=150) {
  if (!tick(death?.serverTick) || !tick(observed?.latestSnapshotServerTick) ||
      observed.playerId !== death.victimId || observed.lifeSeq !== death.victimNextLifeSeq ||
      !(death.respawnDelayMs > 0) || !Number.isInteger(observerId) || observerId < 0) return null;
  const configuredTicks=Math.ceil(death.respawnDelayMs/10);
  const observedElapsed=elapsed(observed.latestSnapshotServerTick,death.serverTick);
  if (observedElapsed < configuredTicks || observedElapsed >= 0x4000_0000) return null;
  const rows=[];
  for (const line of logLines(logs)) {
    if (!line.includes('server bot movement trace')) continue;
    const {fields,message}=serverLogRecord(line);
    if (fields ? message !== 'server bot movement trace' :
        !/^(?:\S+\s+)?INFO\s+server bot movement trace /.test(line)) continue;
    const number=name=>{
      if (fields) return typeof fields[name] === 'number' ? fields[name] : NaN;
      const matches=[...line.matchAll(new RegExp(`(?:^|\\s)${name}=([^\\s]+)`,'g'))];
      return matches.length===1 ? Number(matches[0][1]) : NaN;
    };
    if (number('bot_id')!==death.victimId) continue;
    if (number('life_seq')!==observed.lifeSeq) {
      const at=number('server_tick');
      const sinceDeath=elapsed(at,death.serverTick);
      if (tick(at) && sinceDeath>=configuredTicks && sinceDeath<=observedElapsed) return null;
      continue;
    }
    const row={serverTick:number('server_tick'),x:number('x'),y:number('y'),
      velocityX:number('velocity_x'),velocityY:number('velocity_y'),
      observerId:number('target_player_id'),observerX:number('target_x'),observerY:number('target_y'),
      assignedHumanStatus:number('assigned_human_status')};
    row.elapsedTicks=elapsed(row.serverTick,death.serverTick);
    if (row.elapsedTicks >= 0x4000_0000 || row.elapsedTicks > observedElapsed+20) continue;
    if (!tick(row.serverTick) || !Object.values(row).every(Number.isFinite) ||
        row.observerId!==observerId || row.assignedHumanStatus!==0) return null;
    row.distance=Math.hypot(row.x-row.observerX,row.y-row.observerY);
    // Independent upper bound: the server projects only the candidate's
    // closing velocity. Its full speed bounds that projection in every
    // direction. Include the widest near hysteresis and rounding headroom.
    row.maximumNearEnvelope=NEAR_RADIUS+256+
      Math.min((Math.hypot(row.velocityX,row.velocityY)+1)*0.2,1024)+2;
    rows.push(row);
  }
  rows.sort((a,b)=>a.elapsedTicks-b.elapsedTicks);
  if (rows.length<2 || rows[0].elapsedTicks<configuredTicks ||
      rows[0].elapsedTicks>configuredTicks+slackTicks ||
      rows[0].distance<=rows[0].maximumNearEnvelope ||
      rows.at(-1).elapsedTicks<observedElapsed-10) return null;
  for(let i=1;i<rows.length;i++) {
    const gap=rows[i].elapsedTicks-rows[i-1].elapsedTicks;
    if(gap<=0 || gap>20) return null;
  }
  // Inside the plain near radius delivery is mandatory, even without any
  // hysteresis or predictive promotion. Retain the original150-tick budget
  // from that boundary. Earlier presentation during promotion is valid too.
  const mandatoryNear=rows.find(row=>row.distance<=NEAR_RADIUS) || null;
  if (mandatoryNear && observedElapsed>mandatoryNear.elapsedTicks+slackTicks) return null;
  return {kind:'timely-respawn-outside-observer-aoi',configuredTicks,slackTicks,
    firstAliveTrace:rows[0],firstMandatoryNearTrace:mandatoryNear,
    observedElapsedTicks:observedElapsed,traceSamples:rows.length,
    maximumTraceGapTicks:Math.max(...rows.slice(1).map((r,i)=>r.elapsedTicks-rows[i].elapsedTicks)),
    lastTrace:rows.at(-1),rows};
}

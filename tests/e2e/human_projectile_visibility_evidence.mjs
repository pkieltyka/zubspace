import assert from 'node:assert/strict';

const tickDiff = (a, b) => ((a - b) << 1) >> 1;

// Audit every ordinary bullet/bomb/Thor hit in the completely retained window.
// Preserve the minimum four BULLET contacts; extra splash hits cannot replace them.
// A bot-victim contact seen from this camera is not a local-victim witness.
// Draw submission does not establish the final GPU pixel or human perception.
export function humanProjectileVisibilityEvidence({playerId, startTick, endTick,
  frames, damageTrace, damage, collisionWitnesses, shotCommits = [], respawns = []}) {
  assert.ok(Number.isInteger(playerId) && playerId < 768);
  assert.ok(Number.isInteger(startTick) && Number.isInteger(endTick));
  const hits = damage.filter(d => d.victimId === playerId && d.shooterId >= 768 &&
    [1, 2, 3, 4, 8].includes(d.weapon & 31) && tickDiff(d.serverTick, startTick) >= 0 &&
    tickDiff(endTick, d.serverTick) >= 0);
  const cases = [], missing = [];
  for (const hit of hits) {
    const receipts = damageTrace.filter(d => d.forSelf === true && d.victimId === playerId &&
      d.damageSeq === hit.damageSeq && d.projectileId === hit.projectileId &&
      d.serverTick === hit.serverTick && d.victimLifeSeq === hit.victimLifeSeq &&
      d.attackerId === hit.shooterId && d.shooterLifeSeq === hit.shooterLifeSeq);
    if (!receipts.length) { missing.push({hit, reason:'missing-exact-local-damage-receipt'}); continue; }
    const receipt = receipts.reduce((a,b) => a.at < b.at ? a : b);
    assert.equal(receipt.damage, hit.damage, 'exact local damage amount');
    const witnesses = collisionWitnesses.filter(w => w.projectile_id === hit.projectileId &&
      w.owner_id === hit.shooterId && w.owner_life_seq === hit.shooterLifeSeq &&
      w.target_id === playerId && w.target_life_seq === hit.victimLifeSeq &&
      w.adjusted_damage === hit.damage);
    if (witnesses.length !== 1) { missing.push({hit, reason:'missing-unique-human-collision-witness'}); continue; }
    const witness = witnesses[0];
    const commits = shotCommits.filter(c => c.owner_id === hit.shooterId && c.owner_life_seq === hit.shooterLifeSeq &&
      String(c.shot_group_id) === String(witness.shot_group_id));
    if(commits.length !== 1) { missing.push({hit, reason:'missing-unique-source-session-commit'}); continue; }
    const sourceSession = String(commits[0].owner_session_id);
    // This witness is frozen at first collision, before a pending hit waits
    // for its presentation deadline. It is not a damage-commit readiness fact.
    if(typeof witness.harmful_eligibility?.harmful !== 'boolean') {
      missing.push({hit, reason:'missing-staged-readiness-witness'}); continue;
    }
    const pre = frames.filter(f => f.at >= receipt.at - 350 && f.at < receipt.at);
    if (pre.length < 2 || pre[0].at > receipt.at - 300 || pre.at(-1).at < receipt.at - 60 ||
      pre.some((f,i) => i && f.monotonicMs - pre[i-1].monotonicMs > 60)) {
      missing.push({hit, reason:'incomplete-pre-damage-frame-window'}); continue;
    }
    if (pre.some(f => f.projectiles.overflow !== 0)) {
      missing.push({hit, reason:'truncated-projectile-draw-set'}); continue;
    }
    const drawn = pre.flatMap(f => f.projectiles.entries.filter(p => p.drawn === true &&
      p.authorityProjectileId === hit.projectileId && p.sourcePlayerId === hit.shooterId &&
      p.sourceLifeSeq === hit.shooterLifeSeq && String(p.sourceSessionId) === sourceSession && String(p.shotGroupId) === String(witness.shot_group_id))
      .map(p => ({at:f.at, renderOrdinal:f.renderOrdinal, projectile:p})));
    if (!drawn.length) { missing.push({hit, reason:'no-drawn-exact-projectile-before-local-damage'}); continue; }
    const sessions = new Set(drawn.map(d => String(d.projectile.sourceSessionId)));
    assert.equal(sessions.size, 1, 'one exact projectile source session');
    assert.ok(!sessions.has('null') && !sessions.has('undefined'), 'authenticated source session');
    const spawn = respawns.find(r => r.player_id === playerId && r.life_seq === hit.victimLifeSeq);
    const sinceRespawnTicks = spawn ? tickDiff(hit.serverTick, spawn.server_tick) : null;
    cases.push({hit, firstDraw:drawn[0], lastDraw:drawn.at(-1), drawnFrames:drawn.length,
      receiptAt:receipt.at, sourceSession, shotGroup:String(witness.shot_group_id),
      sourceFireTick:Number.isInteger(commits[0].server_tick)?commits[0].server_tick:null,
      stagedReadiness:witness.harmful_eligibility, sinceRespawnTicks, earlyRespawn:sinceRespawnTicks !== null &&
        sinceRespawnTicks >= 0 && sinceRespawnTicks <= 300});
  }
  const allRapidPairs = [];
  const ordered = [...cases].sort((a,b) => tickDiff(a.hit.serverTick,b.hit.serverTick));
  const exactDraw=(entry,c)=>entry.drawn===true&&entry.authorityProjectileId===c.hit.projectileId&&
    entry.sourcePlayerId===c.hit.shooterId&&entry.sourceLifeSeq===c.hit.shooterLifeSeq&&
    String(entry.sourceSessionId)===c.sourceSession&&String(entry.shotGroupId)===c.shotGroup;
  for (let i=0;i<ordered.length;i++) for(let j=i+1;j<ordered.length;j++) {
    const ca=ordered[i],cb=ordered[j],a=ca.hit,b=cb.hit,gap=tickDiff(b.serverTick,a.serverTick);
    if (gap > 20) break;
    if (a.victimLifeSeq!==b.victimLifeSeq||a.projectileId===b.projectileId)continue;
    const sameSource=a.shooterId===b.shooterId&&a.shooterLifeSeq===b.shooterLifeSeq&&
      ca.sourceSession===cb.sourceSession;
    const bullet=hit=>[1,2].includes(hit.weapon&31);
    const shape=bullet(a)&&bullet(b)?'bullet-bullet':!bullet(a)&&!bullet(b)?'bomb-bomb':'bullet-bomb';
    // Both intents can be submitted yet be too close to read as two objects.
    // Report same-frame screen separation; never equate it with GPU pixels,
    // occlusion, or human perception. Non-overlapping flight windows are valid.
    const concurrent=[];
    for(const frame of frames){
      if(frame.at<Math.min(ca.receiptAt,cb.receiptAt)-350||
        frame.at>=Math.min(ca.receiptAt,cb.receiptAt))continue;
      const pa=frame.projectiles.entries.find(p=>exactDraw(p,ca));
      const pb=frame.projectiles.entries.find(p=>exactDraw(p,cb));
      if(pa&&pb&&[pa.screenX,pa.screenY,pb.screenX,pb.screenY].every(Number.isFinite))
        concurrent.push({at:frame.at,renderOrdinal:frame.renderOrdinal,
          centerSeparationPx:Math.hypot(pa.screenX-pb.screenX,pa.screenY-pb.screenY)});
    }
    allRapidPairs.push({first:a.projectileId,second:b.projectileId,gapTicks:gap,gapMs:gap*10,
      sameSource,twoShooters:a.shooterId!==b.shooterId,shape,
      firstSource:{playerId:a.shooterId,sessionId:ca.sourceSession,lifeSeq:a.shooterLifeSeq,shotGroup:ca.shotGroup},
      secondSource:{playerId:b.shooterId,sessionId:cb.sourceSession,lifeSeq:b.shooterLifeSeq,shotGroup:cb.shotGroup},
      sameActivation:sameSource&&ca.shotGroup===cb.shotGroup,
      signedFireGapTicks:ca.sourceFireTick!==null&&cb.sourceFireTick!==null?
        tickDiff(cb.sourceFireTick,ca.sourceFireTick):null,
      earlyRespawn:ca.earlyRespawn||cb.earlyRespawn,
      concurrentDrawFrames:concurrent.length,
      minimumCenterSeparationPx:concurrent.length?Math.min(...concurrent.map(p=>p.centerSeparationPx)):null,
      withinOnePixelFrames:concurrent.filter(p=>p.centerSeparationPx<=1).length,
      concurrent});
  }
  const rapidPairs=allRapidPairs.filter(p=>p.sameSource);
  const bulletCases=cases.filter(c=>[1,2].includes(c.hit.weapon&31));
  return {complete:bulletCases.length>=4&&missing.length===0&&bulletCases.some(c=>c.earlyRespawn),
    playerId,startTick,endTick,hits:hits.length,bulletHits:bulletCases.length,
    cases,missing,rapidPairs,allRapidPairs,
    pairShapes:{sameSourceWithin50ms:allRapidPairs.filter(p=>p.sameSource&&p.gapMs<=50).length,
      twoShooterWithin120ms:allRapidPairs.filter(p=>p.twoShooters&&p.gapMs<=120).length,
      bulletBombWithin200ms:allRapidPairs.filter(p=>p.shape==='bullet-bomb').length},
    earlyRespawnHits:cases.filter(c=>c.earlyRespawn).length,
    scope:'Every bot bullet/bomb/Thor hit on this local human in the declared retained window; exact source, damage, collision, submitted pre-damage draw and early-respawn classification. Requires four bullet contacts independently of bombs. Tick units are10ms; rapid means at most200ms. Same-frame separation describes draw intents, not GPU visibility, occlusion or perception; no unobserved pair-shape claim.'};
}

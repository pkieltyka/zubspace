import assert from "node:assert/strict";
import {explosiveContactOverlapsHull} from "./bot_combat_evidence.mjs";

// Join overlapping real captures before asking for a complete pre/post-hit
// proof. A single poll can straddle the next poll's half-second render tail.
// Keep a bounded recent candidate window; the audit's drawing, gap and
// retirement requirements below remain unchanged.
export function botProjectileObservationWindow(retained, now) {
  const terminalSince = now - 3600;
  return {
    packetLog: [...retained.packets.values()].filter(p => p.at >= terminalSince),
    remoteSpriteTrace: [...retained.frames.values()].filter(f => f.at >= terminalSince - 300)
      .sort((a, b) => a.at - b.at),
  };
}

function terminals(state) {
  const facts = new Map();
  for (const entry of [...(state.packetLogFirst || []), ...(state.packetLog || [])]) {
    const p = Buffer.from(entry.bytes || []);
    if (entry.direction !== "received" || p[0] !== 125 || p[1] !== 2 || p[2] !== 4) continue;
    assert.ok(p.length === 122 || p.length === 124, "current projectile-state packet length");
    assert.equal(p.length, 122 + (p[105]&8 ? 2 : 0));
    if (!(p[105]&4) || ![1,2].includes(p.readUInt16LE(83)&31)) continue;
    const fact = { at: entry.at, eventId:p.readUInt32LE(19), tick:p.readUInt32LE(23),
      server:p.readBigUInt64LE(3).toString(), arena:p.readBigUInt64LE(11).toString(),
      owner:p.readUInt16LE(27), session:p.readBigUInt64LE(29).toString(), life:p.readUInt32LE(37),
      group:p.readBigUInt64LE(41).toString(), id:p.readUInt32LE(74), x:p.readFloatLE(85), y:p.readFloatLE(89) };
    if (fact.owner < 768) continue;
    const key=`${fact.server}/${fact.arena}/${fact.eventId}`;
    const previous=facts.get(key);
    if (previous) assert.equal(previous.bytes,p.toString("hex"),"duplicate terminal copies must agree");
    if (!previous || fact.at<previous.fact.at) facts.set(key,{fact,bytes:p.toString("hex")});
  }
  return [...facts.values()].map(x=>x.fact);
}
const matching = (p,t) => p.sourcePlayerId===t.owner && String(p.sourceSessionId)===t.session
  && p.sourceLifeSeq===t.life && String(p.shotGroupId)===t.group;

export function botProjectilePresentationEvidence({ state, damage, collisionWitnesses = [], requireGeometry = true,
  minimumCases = 4, minimumShipTypes = 2, requireLinked = true, requireLethal = true, requireOutsideLegacyCircle = false }) {
  const frames = state.remoteSpriteTrace || [], cases=[], skipped={};
  const skip=reason=>{skipped[reason]=(skipped[reason]||0)+1;};
  for (const terminal of terminals(state)) {
    const hit=damage.find(d=>d.projectileId===terminal.id && d.shooterId===terminal.owner && d.shooterLifeSeq===terminal.life
      && d.victimId>=768 && d.victimId!==terminal.owner && [1,2].includes(d.weapon&31));
    if (!hit) continue;
    assert.equal(terminal.tick,hit.serverTick,"terminal and exact hit must share the authoritative collision tick");
    const post=frames.filter(f=>f.at>=terminal.at+50 && f.at<=terminal.at+600);
    for (const frame of post) assert.ok(!frame.projectiles.entries.some(p=>matching(p,terminal)),
      `consumed bot bullet group reappeared after its terminal: ${JSON.stringify({hit,terminal,frameAt:frame.at})}`);
    const pre=frames.filter(f=>f.at>=terminal.at-300 && f.at<=terminal.at
      && f.projectiles.entries.some(p=>matching(p,terminal)&&p.authorityProjectileId===terminal.id&&p.drawn)
      && f.entries.some(p=>p.id===hit.victimId&&p.lifeSeq===hit.victimLifeSeq&&p.drawn));
    if (!pre.length) {skip("no-drawn-projectile-and-victim-before-terminal");continue;}
    if (post.length<12 || post[0].at>terminal.at+100 || post.at(-1).at<terminal.at+550) {skip("incomplete-half-second-render-tail");continue;}
    if (post.some(f=>f.projectiles.overflow!==0)) {skip("truncated-projectile-tail");continue;}
    const gaps=post.slice(1).map((f,i)=>f.at-post[i].at), maxGap=Math.max(0,...gaps);
    if (maxGap>60) {skip("render-tail-observation-gap");continue;}
    const witnesses=collisionWitnesses.filter(w=>w.projectile_id===hit.projectileId&&w.owner_id===hit.shooterId
      &&w.owner_life_seq===hit.shooterLifeSeq&&w.target_id===hit.victimId&&w.target_life_seq===hit.victimLifeSeq
      &&w.collision_tick===hit.serverTick&&w.adjusted_damage===hit.damage);
    if (requireGeometry && witnesses.length!==1) {skip("missing-exact-collision-witness");continue;}
    const witness=witnesses[0]||null;
    if (witness) {
      assert.equal(String(witness.shot_group_id),terminal.group);
      assert.equal(witness.contact_reason,"direct");
      assert.equal(witness.history_source,"unified_exact_segment");
      assert.equal(witness.presentation_geometry.status,"server_native");
      assert.ok(witness.target_radius>0&&[witness.projectile_x,witness.projectile_y,witness.collision_anchor_x,witness.collision_anchor_y].every(Number.isFinite));
      assert.ok(explosiveContactOverlapsHull({weaponX:witness.projectile_x,weaponY:witness.projectile_y,
        shipX:witness.collision_anchor_x,shipY:witness.collision_anchor_y,shipRadius:witness.target_radius}),
        "native direct contact must satisfy the reference game's ship-first weapon boxes");
    }
    const last=pre.at(-1), source=last.entries.find(p=>p.id===terminal.owner), victim=last.entries.find(p=>p.id===hit.victimId);
    if (!Number.isInteger(source?.ship)) {skip("source-ship-not-observed");continue;}
    const maxLinkedDrawn=Math.max(...pre.map(f=>f.projectiles.entries.filter(p=>matching(p,terminal)&&p.drawn).length));
    cases.push({hit,terminal,sourceShip:source.ship,preDrawnFrames:pre.length,maxLinkedDrawn,
      lastRenderedVictim:{x:victim.renderedX,y:victim.renderedY,life:victim.lifeSeq,presentationTick:last.presentationRenderTick},
      postFrames:post.length,postSpanMs:post.at(-1).at-post[0].at,maxPostFrameGapMs:maxGap,collisionWitness:witness});
  }
  const shipTypes=[...new Set(cases.map(c=>c.sourceShip))];
  const linked=cases.filter(c=>c.sourceShip===4&&c.maxLinkedDrawn>=2),lethal=cases.filter(c=>c.hit.lethal);
  const outsideLegacyCircleCases=cases.filter(c=>c.collisionWitness && Math.hypot(
    c.collisionWitness.projectile_x-c.collisionWitness.collision_anchor_x,
    c.collisionWitness.projectile_y-c.collisionWitness.collision_anchor_y)>c.collisionWitness.target_radius).length;
  return {complete:cases.length>=minimumCases&&shipTypes.length>=minimumShipTypes&&(!requireLinked||linked.length>0)
    &&(!requireLethal||lethal.length>0)&&(!requireOutsideLegacyCircle||outsideLegacyCircleCases>0),
    geometryCorrelated:requireGeometry,requireOutsideLegacyCircle,outsideLegacyCircleCases,
    cases,shipTypes,linkedCases:linked.length,lethalCases:lethal.length,skipped};
}

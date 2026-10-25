import assert from "node:assert/strict";
import test from "node:test";
import dgram from "node:dgram";
import http from "node:http";
import { once } from "node:events";
import { setTimeout as sleep } from "node:timers/promises";
import { DatagramImpairment, createUdpAssociation, loopbackCandidate, rewriteLoopbackSdp,
  seededRandom, startRtcUdpImpairment } from "../tools/rtc_udp_impairment.mjs";
import { mixedUdpProfiles } from "./e2e/udp_path_profiles.mjs";

const sdp = ["v=0", "o=- 1 1 IN IP4 0.0.0.0", "s=-", "t=0 0", "a=group:BUNDLE 0",
  "m=application 9 UDP/DTLS/SCTP webrtc-datachannel", "c=IN IP4 0.0.0.0", "a=mid:0",
  "a=ice-ufrag:unchanged", "a=ice-pwd:keep-this-test-credential", "a=fingerprint:sha-256 11:22",
  "a=sctp-port:5000", "a=candidate:1 1 udp 123 192.0.2.1 3000 typ host",
  "a=candidate:2 1 udp 123 192.0.2.2 4000 typ srflx raddr 192.0.2.1 rport 3000",
  "a=remote-candidates:1 192.0.2.3 5000", "a=end-of-candidates", ""].join("\r\n");

test("both ICE rewrites remove every bypass route and preserve crypto/media identity", () => {
  const rewritten = rewriteLoopbackSdp(sdp, 12345);
  assert.deepEqual(loopbackCandidate(rewritten), { address: "127.0.0.1", port: 12345 });
  assert.equal(rewritten.match(/a=candidate:/g).length, 1);
  assert.doesNotMatch(rewritten, /192\.0\.2\.|remote-candidates/);
  for (const prefix of ["a=ice-ufrag:", "a=ice-pwd:", "a=fingerprint:", "a=sctp-port:", "a=mid:"]) {
    assert.equal(rewritten.split("\r\n").find(line => line.startsWith(prefix)),
      sdp.split("\r\n").find(line => line.startsWith(prefix)));
  }
  assert.throws(() => rewriteLoopbackSdp(sdp + "m=audio 9 UDP/TLS/RTP/SAVPF 0\r\n", 12345));
  assert.throws(() => rewriteLoopbackSdp(sdp, 0));
  assert.throws(() => loopbackCandidate(sdp));
  assert.throws(() => loopbackCandidate(rewritten.replace("udp", "tcp")));
  assert.throws(() => loopbackCandidate(rewritten + "a=candidate:2 1 udp 123 127.0.0.1 3000 typ host\r\n"));
  const withComponent2 = rewritten + "a=candidate:2 2 udp 123 127.0.0.1 3000 typ host\r\n";
  assert.equal(loopbackCandidate(withComponent2).port, 12345);
  assert.equal(rewriteLoopbackSdp(withComponent2, 54321).match(/a=candidate:/g).length, 1);
});

test("asymmetric bandwidth serialization, byte identity, jitter and cleanup are bounded", () => {
  let now = 100;
  const sent = [];
  const lane = new DatagramImpairment({ delayMs: 50, bytesPerSecond: 1000, maxPackets: 2, maxBytes: 200 },
    bytes => sent.push(bytes), { clock: () => now, random: () => 0.5 });
  const packet = Buffer.alloc(72, 0x37);
  lane.push(packet); packet.fill(0xff); lane.push(Buffer.alloc(72, 0x42));
  assert.equal(lane.push(Buffer.alloc(1)), false);
  now = 249; lane.drain(); assert.equal(sent.length, 0);
  now = 250; lane.drain(); assert.deepEqual(sent[0], Buffer.alloc(72, 0x37));
  now = 350; lane.drain(); assert.deepEqual(sent[1], Buffer.alloc(72, 0x42));
  assert.equal(lane.snapshot().overflowDrops, 1);
  lane.push(Buffer.alloc(10)); lane.close();
  assert.equal(lane.snapshot().queuedBytes, 0);
  assert.equal(lane.snapshot().queuedPackets, 0);
  assert.equal(lane.snapshot().timerActive, false);
  assert.equal(lane.snapshot().cancelled, 1);
  assert.equal(lane.push(Buffer.alloc(10)), false);
});

test("seeded burst loss is repeatable and counts encrypted application drops separately", () => {
  const packets = Array.from({ length: 200 }, (_, index) => {
    const bytes = Buffer.alloc(20); bytes[0] = index % 2 ? 23 : 0; bytes[1] = 254; bytes[2] = 253;
    bytes.writeUInt16BE(7, 11); return bytes;
  });
  function run() {
    const lane = new DatagramImpairment({ burstLoss: 0.2, burstLength: 3, loss: 0.02, delayMs: 10000 },
      () => {}, { random: seededRandom(42) });
    const decisions = packets.map(packet => lane.push(packet));
    const evidence = lane.snapshot(); lane.close(); return { decisions, evidence };
  }
  const first = run(); assert.deepEqual(first, run());
  assert.ok(first.evidence.authoredDrops > first.evidence.applicationDrops);
  assert.ok(first.evidence.applicationDrops > 0);
  assert.equal(first.evidence.overflowDrops, 0);
  const burst = new DatagramImpairment({ burstLoss: 1, burstLength: 3 }, () => {});
  assert.deepEqual(packets.slice(0, 8).map(packet => burst.push(packet)), Array(8).fill(false));
  burst.close();
});

test("jitter can reorder actual scheduled packets without changing bytes", () => {
  let now = 0;
  const randomValues = [0.5, 0.5, 1, 0.5, 0.5, 0];
  const sent = [];
  const lane = new DatagramImpairment({ delayMs: 100, jitterMs: 50 }, bytes => sent.push(bytes[0]),
    { clock: () => now, random: () => randomValues.shift() });
  lane.push(Buffer.from([1])); lane.push(Buffer.from([2]));
  now = 50; lane.drain(); assert.deepEqual(sent, [2]);
  now = 150; lane.drain(); assert.deepEqual(sent, [2, 1]);
  assert.equal(lane.snapshot().reorderedPackets, 1); lane.close();
});

test("invalid profiles fail before opening UDP sockets", async () => {
  for (const profile of [{ delayMs: -1 }, { loss: 1.1 }, { maxBytes: 0 }, { burstLength: 1.5 }, { preserveOrder: 1 }]) {
    assert.throws(() => new DatagramImpairment(profile, () => {}));
    await assert.rejects(createUdpAssociation({ s2c: profile }));
  }
});

test("FIFO WAN jitter preserves order while the separately named old stress profile still reorders", () => {
  assert.throws(() => mixedUdpProfiles("misspelled-clean-fallback"), /unknown UDP/);
  for (const kind of ["fifo-wan", "reorder-stress"]) {
    let now = 0; const sent = [];
    const values = [0.5, 0.5, 1, 0.5, 0.5, 0];
    const authored = mixedUdpProfiles(kind)[1].s2c;
    assert.equal(authored.delayMs, 115); assert.equal(authored.loss, 0.003);
    const lane = new DatagramImpairment({ ...authored, delayMs: 100, jitterMs: 50, bytesPerSecond: 0 },
      bytes => sent.push(bytes[0]), { clock: () => now, random: () => values.shift() });
    lane.push(Buffer.from([1])); lane.push(Buffer.from([2]));
    now = 50; lane.drain();
    assert.deepEqual(sent, kind === "fifo-wan" ? [] : [2]);
    now = 150; lane.drain();
    assert.deepEqual(sent, kind === "fifo-wan" ? [1, 2] : [2, 1]);
    assert.equal(lane.snapshot().reorderedPackets, kind === "fifo-wan" ? 0 : 1);
    lane.close();
  }
});

async function socket() {
  const value = dgram.createSocket("udp4"); value.bind(0, "127.0.0.1"); await once(value, "listening"); return value;
}

test("two real UDP associations isolate fast/slow paths, preserve bytes, reject foreign sources and close", async () => {
  const server = await socket(), client = await socket(), attacker = await socket();
  const fast = await createUdpAssociation(), slow = await createUdpAssociation({ c2s: { delayMs: 25 }, s2c: { delayMs: 40 } });
  const messages = [];
  server.on("message", (bytes, remote) => { server.send(bytes, remote.port, remote.address); });
  client.on("message", (bytes) => messages.push({ value: bytes.toString("hex"), at: Date.now() }));
  try {
    for (const path of [fast, slow]) path.setUpstream({ address: "127.0.0.1", port: server.address().port });
    const start = Date.now();
    client.send(Buffer.from([0, 1, 2, 255]), slow.frontPort, "127.0.0.1");
    client.send(Buffer.from([3, 0, 254]), fast.frontPort, "127.0.0.1");
    const deadline = Date.now() + 3000;
    while (messages.length < 2 && Date.now() < deadline) await sleep(5);
    assert.equal(messages.length, 2);
    assert.equal(messages[0].value, "0300fe"); assert.equal(messages[1].value, "000102ff");
    assert.ok(messages[1].at - start >= 60);
    attacker.send(Buffer.from([42]), slow.frontPort, "127.0.0.1"); await sleep(20);
    assert.equal(slow.snapshot().unexpectedSources, 1);
    assert.equal(slow.snapshot().c2s.forwarded, 1); assert.equal(slow.snapshot().s2c.forwarded, 1);
  } finally {
    await Promise.all([fast.close(), slow.close()]);
    for (const value of [server, client, attacker]) value.close();
  }
  for (const path of [fast, slow]) {
    const state = path.snapshot(); assert.equal(state.closed, true);
    assert.equal(state.pendingBytes, 0);
    for (const lane of [state.c2s, state.s2c]) {
      assert.equal(lane.queuedBytes, 0); assert.equal(lane.queuedPackets, 0); assert.equal(lane.timerActive, false);
      assert.equal(lane.sendErrors, 0);
    }
  }
});

test("signaling proxy is opt-in, loopback/origin scoped, capped and rewrites offer AND answer", async () => {
  const seen = [];
  const upstream = http.createServer(async (request, response) => {
    if (request.method === "GET") {
      response.setHeader("Content-Type", "application/json"); response.end(JSON.stringify({ path: request.url })); return;
    }
    const chunks = []; for await (const chunk of request) chunks.push(chunk);
    seen.push(JSON.parse(Buffer.concat(chunks).toString()));
    response.setHeader("Content-Type", "application/json");
    response.end(JSON.stringify({ type: "answer", sdp: rewriteLoopbackSdp(sdp, 12345) }));
  });
  upstream.listen(0, "127.0.0.1"); await once(upstream, "listening");
  const origin = "http://127.0.0.1:7000";
  const bridge = await startRtcUdpImpairment({ upstream: `http://127.0.0.1:${upstream.address().port}/v1/rtc/offer`,
    profiles: [{ seed: 17 }], origin });
  const post = (headers) => fetch(bridge.url, { method: "POST", headers,
    body: JSON.stringify({ type: "offer", sdp }) });
  try {
    assert.equal((await post({ Origin: "http://evil.invalid" })).status, 403);
    for (const endpoint of ["/v1/status", "/v1/zones", "/v1/players", "/v1/ping", "/v1/zones/chaos-zone", "/v1/zones/extreme/players", "/v1/zones/extreme/status"]) {
      const reply = await fetch(new URL(endpoint, bridge.url), { headers: { Origin: origin } });
      assert.equal(reply.headers.get("Access-Control-Allow-Origin"), origin);
      assert.deepEqual(await reply.json(), { path: endpoint });
    }
    const answer = await (await post({ Origin: origin })).json();
    const state = bridge.snapshot();
    assert.equal(loopbackCandidate(seen[0].sdp).port, state.sessions[0].backPort);
    assert.equal(loopbackCandidate(answer.sdp).port, state.sessions[0].frontPort);
    assert.equal((await post({ Origin: origin })).status, 429);
    assert.equal(state.errors.length, 0);
  } finally { await bridge.close(); upstream.closeAllConnections(); await new Promise(resolve => upstream.close(resolve)); }
  assert.equal(bridge.snapshot().sessions[0].transport.closed, true);
  await assert.rejects(startRtcUdpImpairment({ upstream: "http://192.0.2.1/v1/rtc/offer", profiles: [{}], origin }));
});

test("early server checks stay bounded until the signaled endpoint and browser source are known", async () => {
  const server = await socket(), client = await socket();
  const bridge = await createUdpAssociation();
  const messages = [];
  client.on("message", bytes => messages.push(bytes.toString()));
  try {
    server.send(Buffer.from("early-check"), bridge.backPort, "127.0.0.1");
    await sleep(10);
    assert.equal(bridge.snapshot().earlyServerPackets, 1);
    assert.equal(bridge.snapshot().pendingPackets, 1);
    bridge.setUpstream({ address: "127.0.0.1", port: server.address().port });
    client.send(Buffer.from("browser-check"), bridge.frontPort, "127.0.0.1");
    const deadline = Date.now() + 1000;
    while (!messages.length && Date.now() < deadline) await sleep(5);
    assert.deepEqual(messages, ["early-check"]);
    assert.equal(bridge.snapshot().unexpectedSources, 0);
    assert.equal(bridge.snapshot().pendingBytes, 0);
    bridge.setProfiles({ delayMs: 20 }, { delayMs: 30 });
    assert.equal(bridge.snapshot().c2s.profile.delayMs, 20);
    assert.equal(bridge.snapshot().s2c.profile.delayMs, 30);
  } finally { await bridge.close(); server.close(); client.close(); }
});

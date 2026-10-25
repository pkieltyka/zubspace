// Opt-in, loopback-only packet-path fixture. It forwards encrypted UDP bytes;
// it never parses or fabricates gameplay packets and changes no host networking.
import dgram from "node:dgram";
import http from "node:http";
import { performance } from "node:perf_hooks";

const LOOPBACK = "127.0.0.1";
const MAX_BODY = 128 * 1024;

function portNumber(value) {
  const port = Number(value);
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("invalid UDP port");
  return port;
}

// Only one bundled DataChannel media section is supported. Replace ALL routes
// in both directions: rewriting the answer alone permits a direct ICE bypass.
export function rewriteLoopbackSdp(sdp, port) {
  portNumber(port);
  if (typeof sdp !== "string") throw new Error("missing SDP");
  const lines = sdp.split(/\r?\n/).filter(Boolean);
  const media = lines.filter(line => line.startsWith("m="));
  if (media.length !== 1 || !/^m=application \d+ UDP\/DTLS\/SCTP webrtc-datachannel$/.test(media[0])) {
    throw new Error("fixture requires exactly one DataChannel media section");
  }
  const result = lines.filter(line => !/^(a=candidate:|a=end-of-candidates|a=remote-candidates:)/.test(line))
    .map(line => line.startsWith("c=") ? `c=IN IP4 ${LOOPBACK}` :
      line.startsWith("m=") ? `m=application ${port} UDP/DTLS/SCTP webrtc-datachannel` : line);
  result.push(`a=candidate:9001 1 udp 2130706431 ${LOOPBACK} ${port} typ host`, "a=end-of-candidates");
  return result.join("\r\n") + "\r\n";
}

export function loopbackCandidate(sdp) {
  const candidates = sdp.split(/\r?\n/).filter(line => line.startsWith("a=candidate:"));
  // Our owned server has one UDP listener. Reject ambiguous or non-local routes.
  const endpoints = candidates.flatMap(line => {
    const fields = line.slice(12).trim().split(/\s+/);
    if (!["1", "2"].includes(fields[1]) || fields[2].toLowerCase() !== "udp" || fields[4] !== LOOPBACK) {
      throw new Error("fixture upstream must advertise loopback UDP only: " +
        JSON.stringify({ component: fields[1], protocol: fields[2], address: fields[4] }));
    }
    const port = portNumber(fields[5]);
    // The RTC library also advertises component 2. SCTP DataChannels use
    // component 1 only; component 2 is removed, never offered as a bypass.
    return fields[1] === "1" ? [port] : [];
  });
  if (!endpoints.length || new Set(endpoints).size !== 1) throw new Error("ambiguous upstream ICE route");
  return { address: LOOPBACK, port: endpoints[0] };
}

export function seededRandom(seed) {
  let state = seed >>> 0 || 1;
  return () => {
    state ^= state << 13; state ^= state >>> 17; state ^= state << 5;
    return (state >>> 0) / 0x100000000;
  };
}

export function isDtlsApplicationDatagram(bytes) {
  // DTLS 1.0/1.2 record header: RFC 6347 section 4.1. Unified DTLS 1.3
  // headers are intentionally unclassified, rather than counted as data.
  for (let offset = 0; offset + 13 <= bytes.length;) {
    if (bytes[offset + 1] !== 254 || ![253, 255].includes(bytes[offset + 2])) return false;
    const length = bytes.readUInt16BE(offset + 11);
    if (offset + 13 + length > bytes.length) return false;
    if (bytes[offset] === 23) return true;
    offset += 13 + length;
  }
  return false;
}

export class DatagramImpairment {
  constructor(profile = {}, send, { clock = () => performance.now(), random = seededRandom(1) } = {}) {
    this.profile = { delayMs: 0, jitterMs: 0, loss: 0, burstLoss: 0, burstLength: 1,
      bytesPerSecond: 0, preserveOrder: false, maxPackets: 4096, maxBytes: 4 * 1024 * 1024, ...profile };
    const p = this.profile;
    if (typeof p.preserveOrder !== "boolean") throw new Error("invalid preserveOrder");
    for (const key of ["delayMs", "jitterMs", "bytesPerSecond"]) {
      if (!Number.isFinite(p[key]) || p[key] < 0) throw new Error(`invalid ${key}`);
    }
    for (const key of ["loss", "burstLoss"]) {
      if (!Number.isFinite(p[key]) || p[key] < 0 || p[key] > 1) throw new Error(`invalid ${key}`);
    }
    for (const key of ["burstLength", "maxPackets", "maxBytes"]) {
      if (!Number.isSafeInteger(p[key]) || p[key] < 1) throw new Error(`invalid ${key}`);
    }
    this.clock = clock; this.random = random; this.send = send;
    this.nextFreeAt = 0; this.burstRemaining = 0; this.queue = []; this.timer = null; this.closed = false;
    this.stats = { received: 0, receivedBytes: 0, forwarded: 0, forwardedBytes: 0,
      applicationReceived: 0, applicationForwarded: 0,
      reorderedPackets: 0,
      authoredDrops: 0, applicationDrops: 0, overflowDrops: 0, cancelled: 0,
      queuedBytes: 0, peakPackets: 0, peakBytes: 0, maximumQueueAgeMs: 0, sendErrors: 0 };
  }

  push(bytes) {
    if (this.closed) return false;
    const p = this.profile, s = this.stats, now = this.clock();
    s.received++; s.receivedBytes += bytes.length;
    const application = isDtlsApplicationDatagram(bytes);
    if (application) s.applicationReceived++;
    if (!this.burstRemaining && this.random() < p.burstLoss) this.burstRemaining = p.burstLength;
    if (this.burstRemaining || this.random() < p.loss) {
      if (this.burstRemaining) this.burstRemaining--;
      s.authoredDrops++;
      if (application) s.applicationDrops++;
      return false;
    }
    if (this.queue.length >= p.maxPackets || s.queuedBytes + bytes.length > p.maxBytes) {
      s.overflowDrops++; return false;
    }
    const serviceMs = p.bytesPerSecond ? (bytes.length + 28) * 1000 / p.bytesPerSecond : 0;
    this.nextFreeAt = Math.max(now, this.nextFreeAt) + serviceMs;
    const rawDue = Math.max(now, this.nextFreeAt + p.delayMs + (this.random() * 2 - 1) * p.jitterMs);
    const due = p.preserveOrder ? Math.max(rawDue, this.latestDue || 0) : rawDue;
    this.latestDue = Math.max(this.latestDue || 0, due);
    const entry = { bytes: Buffer.from(bytes), due, at: now, sequence: s.received };
    // Jitter can reorder datagrams. A single bounded queue/timer avoids a task
    // per packet; sort only the insertion tail (normal delay/bandwidth is FIFO).
    let index = this.queue.length;
    while (index && this.queue[index - 1].due > due) index--;
    this.queue.splice(index, 0, entry);
    s.queuedBytes += bytes.length;
    s.peakPackets = Math.max(s.peakPackets, this.queue.length);
    s.peakBytes = Math.max(s.peakBytes, s.queuedBytes);
    this.arm(); return true;
  }

  arm() {
    clearTimeout(this.timer); this.timer = null;
    if (this.closed || !this.queue.length) return;
    this.timer = setTimeout(() => { this.timer = null; this.drain(); },
      Math.max(0, this.queue[0].due - this.clock()));
  }

  drain() {
    const now = this.clock();
    while (!this.closed && this.queue.length && this.queue[0].due <= now) {
      const entry = this.queue.shift(); this.stats.queuedBytes -= entry.bytes.length;
      this.stats.maximumQueueAgeMs = Math.max(this.stats.maximumQueueAgeMs, now - entry.at);
      try {
        this.send(entry.bytes, error => { if (error) this.stats.sendErrors++; });
        this.stats.forwarded++; this.stats.forwardedBytes += entry.bytes.length;
        if (entry.sequence < (this.lastForwardedSequence || 0)) this.stats.reorderedPackets++;
        this.lastForwardedSequence = Math.max(this.lastForwardedSequence || 0, entry.sequence);
        if (isDtlsApplicationDatagram(entry.bytes)) this.stats.applicationForwarded++;
      } catch { this.stats.sendErrors++; }
    }
    this.arm();
  }

  snapshot() { return { ...this.stats, profile: { ...this.profile },
    queuedPackets: this.queue.length, timerActive: this.timer !== null }; }
  close() {
    this.closed = true; clearTimeout(this.timer); this.timer = null;
    this.stats.cancelled += this.queue.length; this.queue.length = 0; this.stats.queuedBytes = 0;
  }
}

async function bindUdp() {
  const socket = dgram.createSocket("udp4");
  await new Promise((resolve, reject) => {
    socket.once("error", reject); socket.bind(0, LOOPBACK, resolve);
  });
  return socket;
}

export async function createUdpAssociation({ c2s = {}, s2c = {}, seed = 1 } = {}) {
  // Validate before opening sockets, including the second direction.
  new DatagramImpairment(c2s, () => {}).close();
  new DatagramImpairment(s2c, () => {}).close();
  const front = await bindUdp();
  let back;
  try { back = await bindUdp(); } catch (error) { front.close(); throw error; }
  let upstream = null, browser = null, closed = false;
  const stats = { unexpectedSources: 0, earlyServerPackets: 0, socketErrors: 0 };
  front.on("error", () => stats.socketErrors++); back.on("error", () => stats.socketErrors++);
  const outbound = new DatagramImpairment(c2s, (bytes, done) => back.send(bytes, upstream.port, upstream.address, done),
    { random: seededRandom(seed) });
  const inbound = new DatagramImpairment(s2c, (bytes, done) => front.send(bytes, browser.port, browser.address, done),
    { random: seededRandom(seed ^ 0x9e3779b9) });
  // ICE may send server checks before the browser has checked our front port.
  // A small, independently bounded pre-route buffer is released on first check.
  let pending = [], pendingBytes = 0;
  back.on("message", (bytes, remote) => {
    if (remote.address !== LOOPBACK || (upstream && remote.port !== upstream.port)) {
      stats.unexpectedSources++; return;
    }
    if (!upstream) stats.earlyServerPackets++;
    if (browser) inbound.push(bytes);
    else if (pending.length < 32 && pendingBytes + bytes.length <= 65536) {
      pending.push({ bytes: Buffer.from(bytes), port: remote.port }); pendingBytes += bytes.length;
    } else stats.unexpectedSources++;
  });
  front.on("message", (bytes, remote) => {
    if (!upstream || remote.address !== LOOPBACK ||
        (browser && (browser.port !== remote.port || browser.address !== remote.address))) {
      stats.unexpectedSources++; return;
    }
    browser = { port: remote.port, address: remote.address };
    outbound.push(bytes);
    for (const packet of pending) {
      if (packet.port === upstream.port) inbound.push(packet.bytes);
      else stats.unexpectedSources++;
    }
    pending = []; pendingBytes = 0;
  });
  return {
    frontPort: front.address().port, backPort: back.address().port,
    setProfiles(c2sProfile, s2cProfile) {
      // Existing queued datagrams retain their original deadlines. Recovery
      // changes future traffic only, never flushes or fabricates delivery.
      const nextOut = new DatagramImpairment(c2sProfile, () => {});
      const nextIn = new DatagramImpairment(s2cProfile, () => {});
      outbound.profile = nextOut.profile; inbound.profile = nextIn.profile;
      outbound.burstRemaining = 0; inbound.burstRemaining = 0;
    },
    setUpstream(endpoint) {
      if (upstream || endpoint.address !== LOOPBACK) throw new Error("invalid/repeated upstream");
      upstream = { address: LOOPBACK, port: portNumber(endpoint.port) };
    },
    snapshot: () => ({ ...stats, browser, upstream, closed, pendingPackets: pending.length, pendingBytes,
      c2s: outbound.snapshot(), s2c: inbound.snapshot() }),
    async close() {
      if (closed) return;
      closed = true; outbound.close(); inbound.close(); pending = []; pendingBytes = 0;
      await Promise.all([front, back].map(socket => new Promise(resolve => socket.close(resolve))));
    },
  };
}

async function readBody(request) {
  let length = 0; const chunks = [];
  for await (const chunk of request) {
    length += chunk.length;
    if (length > MAX_BODY) throw new Error("signaling body exceeds fixture limit");
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

export async function startRtcUdpImpairment({ upstream, profiles, origin, maxSessions = 4 }) {
  const target = new URL(upstream);
  if (target.protocol !== "http:" || target.hostname !== LOOPBACK || target.pathname !== "/v1/rtc/offer" ||
      target.username || target.password || target.search || target.hash) throw new Error("loopback offer URL required");
  if (!Array.isArray(profiles) || !profiles.length || !Number.isSafeInteger(maxSessions) || maxSessions < 1) {
    throw new Error("bounded explicit fixture profiles required");
  }
  const allowedOrigin = new URL(origin).origin;
  if (new URL(allowedOrigin).hostname !== LOOPBACK) throw new Error("loopback browser origin required");
  const sessions = [], errors = [], active = new Set(); let closing = false;
  const server = http.createServer(async (request, response) => {
    const fail = (status, reason) => { response.writeHead(status, { "Content-Type": "application/json" });
      response.end(JSON.stringify({ error: reason })); };
    if (closing) return fail(503, "fixture closing");
    if (request.headers.origin !== allowedOrigin) return fail(403, "fixture origin rejected");
    response.setHeader("Access-Control-Allow-Origin", allowedOrigin);
    response.setHeader("Vary", "Origin");
    response.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
    response.setHeader("Access-Control-Allow-Headers", "Content-Type");
    if ((["/v1/status", "/v1/zones", "/v1/players", "/v1/ping"].includes(request.url) ||
        /^\/v1\/zones\/[a-z0-9-]+(?:\/(?:players|status))?$/.test(request.url)) && request.method === "GET") {
      const endpoint = new URL(request.url, target);
      try {
        const reply = await fetch(endpoint, { signal: AbortSignal.timeout(5000) });
        response.writeHead(reply.status, { "Content-Type": "application/json" });
        response.end(await reply.text());
      } catch { if (!response.destroyed) fail(502, "fixture status forwarding failed"); }
      return;
    }
    if (request.url !== "/v1/rtc/offer") return fail(404, "not found");
    if (request.method === "OPTIONS") { response.writeHead(204); return response.end(); }
    if (request.method !== "POST") return fail(405, "method rejected");
    // Reserve before awaiting body/socket creation so simultaneous offers cannot
    // defeat the session cap. Failed offers remain counted in retained evidence.
    if (sessions.length >= Math.min(maxSessions, profiles.length)) return fail(429, "fixture session limit");
    const record = { index: sessions.length, profile: profiles[sessions.length], association: null, error: null };
    sessions.push(record);
    const controller = new AbortController(); active.add(controller);
    const timeout = setTimeout(() => { controller.abort(); request.destroy(); }, 15000);
    try {
      const offer = await readBody(request);
      if (offer.type !== "offer") throw new Error("expected offer");
      record.association = await createUdpAssociation(record.profile);
      if (closing) throw new Error("fixture closing");
      const forwarded = { ...offer, sdp: rewriteLoopbackSdp(offer.sdp, record.association.backPort) };
      const answerResponse = await fetch(target, { method: "POST", signal: controller.signal,
        headers: { "Content-Type": "application/json", Origin: allowedOrigin }, body: JSON.stringify(forwarded) });
      if (!answerResponse.ok) throw new Error(`upstream signaling HTTP ${answerResponse.status}`);
      const answerText = await answerResponse.text();
      if (Buffer.byteLength(answerText) > MAX_BODY) throw new Error("answer exceeds fixture limit");
      const answer = JSON.parse(answerText);
      if (answer.type !== "answer") throw new Error("expected answer");
      record.association.setUpstream(loopbackCandidate(answer.sdp));
      response.setHeader("Content-Type", "application/json");
      response.end(JSON.stringify({ ...answer, sdp: rewriteLoopbackSdp(answer.sdp, record.association.frontPort) }));
    } catch (error) {
      record.error = error.message; errors.push({ index: record.index, message: error.message });
      await record.association?.close();
      if (!response.destroyed) fail(502, "fixture signaling failed");
    } finally { clearTimeout(timeout); active.delete(controller); }
  });
  server.requestTimeout = 15000; server.headersTimeout = 15000;
  server.maxConnections = 16;
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, LOOPBACK, resolve); });
  return {
    url: `http://${LOOPBACK}:${server.address().port}/v1/rtc/offer`,
    snapshot: () => ({ errors: [...errors], sessions: sessions.map(record => ({ index: record.index,
      profile: record.profile, error: record.error, frontPort: record.association?.frontPort,
      backPort: record.association?.backPort, transport: record.association?.snapshot() })) }),
    setProfiles(index, c2s, s2c) {
      if (closing || !sessions[index]?.association) throw new Error("no live fixture association");
      sessions[index].association.setProfiles(c2s, s2c);
    },
    async close() {
      closing = true; for (const controller of active) controller.abort();
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
      // Await in-flight request handlers before draining associations that could
      // have been created concurrently with close.
      while (active.size) await new Promise(resolve => setTimeout(resolve, 5));
      await Promise.all(sessions.map(record => record.association?.close()));
    },
  };
}

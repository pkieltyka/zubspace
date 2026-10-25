(function installPlayerTelemetry(global) {
  "use strict";
  // One small render callback per game frame; no extra animation loop, observers,
  // traces, geometry walks or per-frame allocation. Hidden pages are excluded.
  // Report versions by length: 1 carries frame and ICE counters; 2 adds the
  // game channel's peak bufferedAmount and its sends dropped at the page's limit.
  const REPORT_LENGTHS = { 1: 54, 2: 62 };
  const MAX_BUFFERED_PEAK = 16 * 1024 * 1024;
  const MAX_DROPPED_SENDS = 10_000;
  function selectedRttMs(stats) {
    let selectedId = null;
    stats.forEach(entry => { if (entry.type === "transport" && entry.selectedCandidatePairId) selectedId = entry.selectedCandidatePairId; });
    const candidates = [];
    stats.forEach(entry => {
      if (entry.type === "candidate-pair" && (selectedId ? entry.id === selectedId : entry.nominated && entry.state === "succeeded")) candidates.push(entry);
    });
    const seconds = candidates.length === 1 ? candidates[0].currentRoundTripTime : null;
    return Number.isFinite(seconds) && seconds >= 0 && seconds <= 10 ? Math.round(seconds * 1000) : null;
  }
  // Read by the HUD; one asynchronous stats request per second, never per frame.
  function createPing({ now, peer, visible }) {
    let active = null, pending = null, value = null, sampledAt = -Infinity, next = -Infinity;
    let samples = [];
    const read = () => {
      const current = peer(), time = now();
      if (current !== active) { samples = []; pending = null; }
      if (current !== active || !visible() || current?.connectionState !== "connected") {
        active = current; value = null; sampledAt = next = -Infinity;
      }
      if (!visible() || current?.connectionState !== "connected") return -1;
      if (!pending && time >= next) {
        next = time + 1000;
        // Promise.resolve also isolates synchronous getStats failures.
        const request = Promise.resolve().then(() => current.getStats()).then(stats => {
          if (current === peer() && current.connectionState === "connected" && visible() && now() - time <= 3000) {
            value = selectedRttMs(stats); sampledAt = time;
            if (value !== null) {
              samples.push(value);
              if (samples.length > 60) samples.shift();
            }
          }
        }).catch(() => { if (current === active) value = null; }).finally(() => {
          if (pending === request) pending = null;
        });
        pending = request;
      }
      return value !== null && time - sampledAt <= 3000 ? value : -1;
    };
    read.stats = () => {
      const current = read();
      if (current < 0 || samples.length === 0) return null;
      const average = Math.round(samples.reduce((sum, sample) => sum + sample, 0) / samples.length);
      const variation = samples.length < 2 ? 0 : Math.round(samples.slice(1).reduce(
        (sum, sample, i) => sum + Math.abs(sample - samples[i]), 0) / (samples.length - 1));
      return [current, average, Math.min(...samples), Math.max(...samples), variation];
    };
    return read;
  }
  function create({ now, visible, peer, send, transport = null }) {
    let previous = null;
    let lastDropped = null;
    let started = now();
    let frames = 0, slow = 0, sum = 0, maximum = 0, unready = 0;
    let lastCorrections = null, corrections = 0;
    let pending = false;
    function reset() {
      previous = null; started = now(); frames = slow = sum = maximum = unready = corrections = 0;
    }
    function frame(correctionCount, ready) {
      const time = now();
      if (!visible() || !peer() || peer().connectionState !== "connected") {
        reset(); lastCorrections = null; return;
      }
      if (lastCorrections !== null && correctionCount >= lastCorrections) corrections += correctionCount - lastCorrections;
      lastCorrections = correctionCount;
      if (previous !== null) {
        const interval = Math.max(0, Math.round((time - previous) * 1000));
        sum += interval; maximum = Math.max(maximum, interval); frames++;
        if (interval > 50_000) slow++;
        if (!ready) unready++;
      }
      previous = time;
    }
    async function report(template) {
      const active = peer();
      const elapsed = Math.round((now() - started) * 1000);
      if (pending || !visible() || !active || active.connectionState !== "connected" || frames === 0 || elapsed < 1_000_000 || elapsed > 30_000_000) { if (elapsed > 30_000_000) reset(); return; }
      const packet = new Uint8Array(template);
      if (packet[0] !== 0x80 || REPORT_LENGTHS[packet[1]] !== packet.length) return;
      const view = new DataView(packet.buffer);
      [frames, slow, sum, maximum, 0xffffffff, corrections, unready, elapsed].forEach((value, index) => view.setUint32(22 + index * 4, value, true));
      if (packet[1] === 2) {
        // The queue peak covers the time since the last report; dropped sends
        // are the page's cumulative counter turned into this window's increase.
        const sending = transport?.() ?? { bufferedPeak: 0, dropped: 0 };
        const dropped = lastDropped === null || sending.dropped < lastDropped ? 0 : sending.dropped - lastDropped;
        lastDropped = sending.dropped;
        view.setUint32(54, Math.min(Math.max(0, Math.round(sending.bufferedPeak)), MAX_BUFFERED_PEAK), true);
        view.setUint32(58, Math.min(dropped, MAX_DROPPED_SENDS), true);
      }
      reset(); pending = true;
      try {
        const stats = await active.getStats();
        let selectedId = null;
        stats.forEach(entry => { if (entry.type === "transport" && entry.selectedCandidatePairId) selectedId = entry.selectedCandidatePairId; });
        const candidates = [];
        stats.forEach(entry => {
          if (entry.type === "candidate-pair" && (selectedId ? entry.id === selectedId : entry.nominated && entry.state === "succeeded")) candidates.push(entry);
        });
        // Older browsers may omit selectedCandidatePairId. A unique nominated
        // pair is usable; multiple candidates are ambiguous and stay absent.
        const selected = candidates.length === 1 ? candidates[0] : null;
        if (selected && Number.isFinite(selected.currentRoundTripTime) && selected.currentRoundTripTime >= 0 && selected.currentRoundTripTime <= 10) {
          view.setUint32(38, Math.round(selected.currentRoundTripTime * 1e6), true);
        }
      } catch (_) { /* Missing RTT is explicitly absent, never a fabricated zero. */ }
      finally {
        pending = false;
        if (visible() && active === peer() && active.connectionState === "connected") send(packet);
      }
    }
    return { frame, report, reset };
  }
  global.ZubspacePlayerTelemetry = Object.freeze({ create, createPing });
})(globalThis);

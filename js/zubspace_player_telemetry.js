(function installPlayerTelemetry(global) {
  "use strict";
  // One small render callback per game frame; no extra animation loop, observers,
  // traces, geometry walks or per-frame allocation. Hidden pages are excluded.
  // Report versions by length: 1 carries frame and ICE counters; 2 adds the
  // game channel's peak bufferedAmount and its sends dropped at the page's limit.
  const REPORT_LENGTHS = { 1: 54, 2: 62 };
  const MAX_BUFFERED_PEAK = 16 * 1024 * 1024;
  const MAX_DROPPED_SENDS = 10_000;
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
  global.ZubspacePlayerTelemetry = Object.freeze({ create });
})(globalThis);

(function installClientDebug(global) {
  "use strict";
  const MAX_FRAMES = 1200, MAX_BYTES = 192 * 1024, WIDTH = 32, MAX_MUZZLES = 16;
  function create({ now, visible, active, fetch: fetcher, build, timers = global }) {
    let context = null, endpoint = null, enabled = false, generation = 0, sequence = 0;
    let interval = null, request = null, pending = false, previous = null;
    let ring = new Array(MAX_FRAMES), count = 0, head = 0, dropped = 0, events = [];
    let uploads = 0, failures = 0;
    let muzzleEnabled = false, muzzles = [], muzzleSeen = new Set(), muzzleDropped = 0, lastMuzzle = -Infinity;
    function reset() {
      ring = new Array(MAX_FRAMES); count = head = dropped = 0; events = []; previous = null;
      muzzles = []; muzzleSeen.clear(); muzzleDropped = 0; lastMuzzle = -Infinity;
    }
    function clear() {
      generation++; enabled = muzzleEnabled = false; context = endpoint = null; sequence = 0;
      if (interval !== null) timers.clearInterval(interval);
      interval = null; request?.abort(); request = null; pending = false; reset();
    }
    async function setContext(next, offerEndpoint) {
      if (context?.bundleId === next?.bundleId) return;
      clear();
      if (!next || !offerEndpoint) return;
      context = Object.freeze({ ...next });
      const url = new URL(offerEndpoint); url.pathname = "/v1/debug/client-trace"; url.search = ""; url.hash = "";
      endpoint = url.href;
      const serial = generation, controller = new AbortController(); request = controller;
      const deadline = timers.setTimeout(() => controller.abort(), 5000);
      try {
        // The disabled endpoint intentionally returns 404. Discover the flag
        // through the existing public status response without creating errors.
        const statusUrl = new URL(endpoint); statusUrl.pathname = "/v1/status";
        const status = await fetcher(statusUrl.href, { signal: controller.signal });
        if (!status.ok || generation !== serial) return;
        const advertised = await status.json();
        if (generation !== serial || advertised.client_debug_trace !== true) return;
        const response = await fetcher(endpoint, { signal: controller.signal, headers: headers() });
        if (!response.ok || generation !== serial) return;
        const policy = await response.json();
        if (generation !== serial || policy.enabled !== true || policy.schemaVersion !== 1 || policy.frameWidth !== WIDTH + 2 || policy.intervalMs !== 20000 || policy.maxFrames !== MAX_FRAMES || policy.maxBytes !== MAX_BYTES) return;
        enabled = true;
        muzzleEnabled = policy.muzzleWidth === WIDTH;
        interval = timers.setInterval(() => { void flush(); }, 20000);
      } catch { /* Disabled/unavailable diagnostics cannot delay or fail the join. */ }
      finally { timers.clearTimeout(deadline); if (serial === generation) request = null; }
    }
    function headers() { return { "Content-Type": "application/json", "X-Zubspace-QA-Bundle": context.bundleId, "X-Zubspace-QA-Token": context.uploadToken }; }
    function requested() { return enabled && visible() && active(); }
    function frame(values) {
      if (!requested()) { previous = null; return; }
      if (values.length !== WIDTH) return;
      for (let i = 0; i < WIDTH; i++) if (!Number.isFinite(values[i]) || Math.abs(values[i]) > 1e12) return;
      const at = now(), delta = previous === null ? 0 : Math.min(30000, Math.max(0, at - previous)); previous = at;
      const row = new Array(WIDTH + 2);
      row[0] = Math.round(at * 1000) / 1000; row[1] = Math.round(delta * 1000) / 1000;
      for (let i = 0; i < WIDTH; i++) row[i + 2] = Math.round(values[i] * 1000) / 1000;
      if (count === MAX_FRAMES) { ring[head] = row; head = (head + 1) % MAX_FRAMES; dropped++; }
      else { ring[(head + count) % MAX_FRAMES] = row; count++; }
    }
    function event(kind) {
      if (!requested() || !Number.isInteger(kind) || kind < 1 || kind > 4) return;
      if (events.length === 32) events.shift();
      events.push([kind, Math.round(now() * 1000) / 1000]);
    }
    function muzzleRequested() { return muzzleEnabled && requested() && now() - lastMuzzle >= 100; }
    function muzzle(values) {
      if (!muzzleRequested() || values.length !== WIDTH) return false;
      if (Array.from(values).some(v => !Number.isFinite(v) || Math.abs(v) > 1e12)) return false;
      // Exact owner/session/life/group/child identity; all u64s arrive as u32 halves.
      const key = Array.from(values).slice(0, 7).join(":");
      if (muzzleSeen.has(key)) return false;
      if (muzzleSeen.size === 64) muzzleSeen.delete(muzzleSeen.values().next().value);
      muzzleSeen.add(key);
      const at = now(); lastMuzzle = at;
      // Headings retain their f32 value: three-decimal frame rounding would
      // hide the one-bin muzzle discrepancy this diagnostic is meant to find.
      const row = [5, Math.round(at * 1000) / 1000, ...values];
      if (muzzles.length === MAX_MUZZLES) { muzzles.shift(); muzzleDropped++; }
      muzzles.push(row);
      return true;
    }
    async function flush() {
      if (!requested()) { reset(); return false; }
      if (pending || (count === 0 && events.length === 0 && muzzles.length === 0) || sequence >= 0xffffffff) return false;
      const serial = generation, target = endpoint, auth = headers();
      const report = { sequence: ++sequence, build: String(build()).slice(0, 128),
        scope: { serverSessionId: context.serverSessionId, arenaStreamId: context.arenaStreamId, playerId: context.playerId },
        frames: Array.from({ length: count }, (_, i) => ring[(head + i) % MAX_FRAMES]),
        events: [...events, ...muzzles, ...(muzzleDropped ? [[6, Math.round(now() * 1000) / 1000, muzzleDropped]] : [])], droppedFrames: dropped };
      reset();
      let body = JSON.stringify(report);
      while (body.length > MAX_BYTES && report.frames.length > 1) {
        const remove = Math.ceil(report.frames.length / 4);
        report.frames.splice(0, remove); report.droppedFrames += remove; body = JSON.stringify(report);
      }
      if (body.length > MAX_BYTES) { failures++; return false; }
      const controller = new AbortController(); request = controller; pending = true;
      const deadline = timers.setTimeout(() => controller.abort(), 5000);
      try {
        const response = await fetcher(target, { method: "POST", headers: auth, body, signal: controller.signal });
        await response.body?.cancel();
        if (serial !== generation) return false;
        if (!response.ok) {
          failures++;
          if (response.status === 403 || response.status === 404) { enabled = false; timers.clearInterval(interval); interval = null; reset(); }
          return false;
        }
        uploads++; return true;
      } catch { if (serial === generation) failures++; return false; }
      finally { timers.clearTimeout(deadline); if (serial === generation) { request = null; pending = false; } }
    }
    return { setContext, clear, reset, requested, frame, event, muzzleRequested, muzzle, flush,
      snapshot: () => ({ enabled, bufferedFrames: count, droppedFrames: dropped, bufferedMuzzles: muzzles.length, droppedMuzzles: muzzleDropped, uploads, failures, pending }) };
  }
  global.ZubspaceClientDebug = Object.freeze({ create });
})(globalThis);

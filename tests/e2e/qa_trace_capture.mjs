// Cold artifact collection only: never call this from a gameplay feedback loop.
// Keep failed capture explicit even if the page still holds an older marker.
export async function prepareBrowserArtifactState(client) {
  const startedAt = performance.now();
  const rendezvousTrace = structuredClone(client?.rendezvousTrace || []);
  let frozen = null;
  let error = null;
  try {
    if (typeof client?.freezeQaTrace !== "function") {
      throw new Error("missing QA trace capture adapter");
    }
    frozen = await client.freezeQaTrace();
  } catch (caught) {
    error = String(caught?.message || caught).slice(0, 512);
  }
  const frozenAt = performance.now();
  const deferred = typeof client?.prepareArtifactState === "function";
  let pending;
  try {
    if (deferred) pending = await client.prepareArtifactState();
    else {
      const state = typeof client?.artifactState === "function"
        ? await client.artifactState() : await client.pageState();
      pending = {read: async () => state, release: async () => {}};
    }
    if (typeof pending?.read !== "function" || typeof pending?.release !== "function") {
      throw new Error("invalid prepared artifact adapter");
    }
  } catch (caught) {
    // A preparation failure is retained, never replaced with a later snapshot.
    let failure = caught;
    if (typeof pending?.release === "function") {
      try { await pending.release(); }
      catch (cleanupError) { failure = new AggregateError([caught, cleanupError],
        `artifact preparation failed: ${caught.message}; release failed: ${cleanupError.message}`); }
    }
    pending = {read: async () => { throw failure; }, release: async () => {}};
  }
  const stateFrozenAt = Date.now();
  const stateFreezeElapsedMs = performance.now() - frozenAt;
  let released = false, consumed = false;
  const release = async () => {
    if (released) return;
    released = true;
    await pending.release();
  };
  return {release, async read() {
    if (consumed || released) throw new Error("artifact capture already consumed or released");
    consumed = true;
    const readStartedAt = performance.now();
    let state;
    try {
      let readFailure = null;
      try { state = await pending.read(); }
      catch (caught) { readFailure = caught; throw caught; }
      finally {
        try { await release(); }
        catch (cleanupError) {
          if (readFailure) throw new AggregateError([readFailure, cleanupError],
            `artifact read failed: ${readFailure.message}; release failed: ${cleanupError.message}`);
          throw cleanupError;
        }
      }
      if (!state || typeof state !== "object") throw new Error("browser returned no artifact state");
      if (state.qaTraceState?.firstStall?.phase === "captured") {
        // The first incident may have rolled out of every live ring before a
        // later test failure. Read its immutable copy only on this cold path.
        if (typeof client.evaluate !== "function") {
          throw new Error("missing first-stall artifact reader");
        }
        const incident = await client.evaluate("window.__zubspaceFirstStallCapture?.() || null");
        if (incident?.phase !== "captured" ||
            incident.bundle?.schema !== "zubspace-qa-trace-v1" ||
            Buffer.byteLength(JSON.stringify(incident.bundle)) > 240 * 1024) {
          throw new Error("missing or oversized first-stall artifact");
        }
        state.firstStallCapture = incident;
      }
    } catch (caught) {
      const message = String(caught?.message || caught).slice(0, 512);
      state = { ...state, error: message };
      error ||= message;
    }
    const trace = state.qaTraceState;
    let status = "failed";
    let bytes = 0;
    if (trace?.enabled === false && frozen == null && state.qaTraceBundle == null) {
      status = "disabled";
      error = null;
    } else if (!error && trace?.enabled === true && trace.ready === true) {
      bytes = Buffer.byteLength(JSON.stringify(frozen ?? null));
      if (frozen?.schema === "zubspace-qa-trace-v1" && frozen.version === 1 &&
          /^[0-9a-f]{32}$/.test(frozen.bundleId) && frozen.bundleId === trace.bundleId &&
          bytes <= 240 * 1024 && frozen.limits?.maxBytes === 240 * 1024) {
        status = "captured";
      } else {
        error = "QA marker returned a missing, mismatched, malformed, or oversized bundle";
      }
    } else {
      error ||= "QA trace state is unavailable or not ready";
    }
    return {
      ...state,
      ...(rendezvousTrace.length ? { rendezvousTrace } : {}),
      // Never substitute a previous successful marker for a failed fresh capture.
      qaTraceBundle: frozen || state.qaTraceBundle || null,
      qaTraceCapture: {
        status, enabled: trace?.enabled ?? null, ready: trace?.ready ?? null,
        bundleId: frozen?.bundleId ?? null, bytes, error,
        freezeElapsedMs: frozenAt - startedAt,
        stateFrozenAt, stateFreezeElapsedMs,
        stateReadElapsedMs: performance.now() - readStartedAt + (deferred ? 0 : stateFreezeElapsedMs),
      },
    };
  }};
}

export async function captureBrowserArtifactState(client) {
  return (await prepareBrowserArtifactState(client)).read();
}

export async function withPreparedBrowserArtifactStates(clients, callback) {
  const captures = await Promise.all(clients.map(({client}) => prepareBrowserArtifactState(client)));
  let failure = null;
  try { return await callback(captures); }
  catch (error) { failure = error; throw error; }
  finally {
    const results = await Promise.allSettled(captures.map(capture => capture.release()));
    const errors = results.filter(result => result.status === "rejected").map(result => result.reason);
    if (errors.length) throw new AggregateError(failure ? [failure, ...errors] : errors,
      `artifact writer/cleanup failed: ${[failure, ...errors].filter(Boolean).map(error => error.message).join("; ")}`);
  }
}

export function assertCompleteQaTraceCaptures(captures) {
  if (!Array.isArray(captures) || captures.length === 0) {
    throw new Error("retained browser acceptance is missing its QA capture audit");
  }
  for (const capture of captures) {
    const captured = capture?.status === "captured" && capture.enabled === true &&
      capture.ready === true && /^[0-9a-f]{32}$/.test(capture.bundleId) &&
      Number.isInteger(capture.bytes) && capture.bytes > 0 && capture.bytes <= 240 * 1024;
    const disabled = capture?.status === "disabled" && capture.enabled === false &&
      capture.bundleId === null && capture.bytes === 0;
    if ((!captured && !disabled) || capture.error != null) {
      throw new Error(`incomplete QA trace capture: ${JSON.stringify(capture)}`);
    }
  }
  return { ok: true, captured: captures.filter((entry) => entry.status === "captured").length,
    disabled: captures.filter((entry) => entry.status === "disabled").length };
}

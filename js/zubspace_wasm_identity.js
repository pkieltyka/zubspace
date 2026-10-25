(function installWasmIdentity(global) {
  "use strict";

  // Diagnostic-only, single-pass binding to the response consumed by the WASM
  // compiler. No second fetch, unbounded Response.clone tee, or frame-loop hash.
  function createCapture(workerUrl, options = {}) {
    const maxBytes = options.maxBytes ?? 128 * 1024 * 1024;
    const maxChunkBytes = 256 * 1024;
    const timeoutMs = options.timeoutMs ?? 30000;
    let started = false;
    let worker = null;
    let pending = null;
    let timer = null;
    let startedAt = 0;
    let processedBytes = 0;
    let maximumRelayMs = 0;
    let state = { kind: "loaded-wasm-sha256", status: "pending", sha256: null,
      bytes: 0, processedBytes: 0, elapsedMs: 0, maximumRelayMs: 0,
      workerCpuMs: 0, maxBytes, maxChunkBytes, workerTerminated: true, reason: null };
    const now = () => global.performance.now();
    const cleanup = () => {
      global.clearTimeout(timer);
      worker?.terminate();
      worker = null;
      state.workerTerminated = true;
    };
    const fail = (reason) => {
      if (state.status !== "pending") return;
      state = { ...state, status: "unavailable", reason, processedBytes,
        elapsedMs: now() - startedAt, maximumRelayMs };
      cleanup();
      const waiter = pending;
      pending = null;
      waiter?.reject(new Error(reason));
    };
    const exchange = (message, transfer = []) => new Promise((resolve, reject) => {
      if (!worker || pending) return reject(new Error("worker_unavailable"));
      pending = { resolve, reject };
      try { worker.postMessage(message, transfer); }
      catch { fail("worker_send_failed"); }
    });

    function wrapResponse(response) {
      if (started) return response;
      started = true;
      startedAt = now();
      try {
        const length = response.headers.get("content-length");
        if (!response.ok || !response.body ||
            (length != null && Number(length) > maxBytes)) {
          fail("response_unavailable_or_oversized");
          return response;
        }
        worker = new global.Worker(workerUrl);
        state.workerTerminated = false;
        worker.onmessage = ({ data }) => {
          const waiter = pending;
          pending = null;
          if (!waiter) return fail("unexpected_worker_reply");
          if (data?.type === "error") {
            waiter.reject(new Error("worker_failed"));
            fail("worker_failed");
          } else waiter.resolve(data);
        };
        worker.onerror = (event) => {
          event.preventDefault?.();
          fail("worker_failed");
        };
        worker.onmessageerror = () => fail("worker_reply_failed");
        timer = global.setTimeout(() => fail("capture_timeout"), timeoutMs);
        const stream = new global.TransformStream({
          async transform(chunk, controller) {
            if (state.status === "pending") {
              try {
                if (!(chunk instanceof Uint8Array) || processedBytes + chunk.byteLength > maxBytes) {
                  throw new Error("response_oversized");
                }
                for (let offset = 0; offset < chunk.byteLength; offset += maxChunkBytes) {
                  if (state.status !== "pending") break;
                  const relayStartedAt = now();
                  // Transfer a copy only: the original bytes are still owned by
                  // the compiler stream. At most one bounded chunk is in flight.
                  const copy = new Uint8Array(chunk.subarray(offset, offset + maxChunkBytes));
                  const size = copy.byteLength;
                  const replyPromise = exchange({ type: "chunk", buffer: copy.buffer }, [copy.buffer]);
                  maximumRelayMs = Math.max(maximumRelayMs, now() - relayStartedAt);
                  const reply = await replyPromise;
                  processedBytes += size;
                  if (reply?.type !== "ack" || reply.bytes !== processedBytes) {
                    throw new Error("worker_byte_mismatch");
                  }
                }
              } catch { fail("capture_failed"); }
            }
            // Diagnostic failure releases backpressure and preserves the exact
            // original compiler bytes. Hash work never blocks the UI thread.
            controller.enqueue(chunk);
          },
          async flush() {
            if (state.status !== "pending") return;
            try {
              const reply = await exchange({ type: "finish" });
              if (reply?.type !== "digest" || reply.bytes !== processedBytes ||
                  processedBytes === 0 || !/^[0-9a-f]{64}$/.test(reply.sha256) ||
                  !Number.isFinite(reply.cpuMs) || reply.cpuMs < 0) {
                throw new Error("invalid_worker_digest");
              }
              state = { ...state, status: "ready", sha256: reply.sha256,
                bytes: processedBytes, processedBytes, elapsedMs: now() - startedAt,
                maximumRelayMs, workerCpuMs: reply.cpuMs };
              cleanup();
            } catch { fail("capture_failed"); }
          },
        });
        return new global.Response(response.body.pipeThrough(stream), {
          status: response.status, statusText: response.statusText, headers: response.headers,
        });
      } catch {
        fail("capture_unavailable");
        return response;
      }
    }
    return Object.freeze({ wrapResponse, snapshot: () => ({ ...state }) });
  }

  global.ZubspaceWasmIdentity = Object.freeze({ createCapture });
})(globalThis);

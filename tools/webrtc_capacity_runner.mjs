import { spawn as nodeSpawn } from "node:child_process";

export async function runWithCleanup(operation, cleanup) {
  try {
    return await operation();
  } finally {
    await cleanup();
  }
}

// Keep this self-contained: the capacity runner serializes it into Chrome and
// supplies the production zubspaceCreateRtcSignalingDeadline implementation.
export async function capacitySignalingExchange(options) {
  const peer = options?.peer;
  const createDeadline = options?.createDeadline;
  const fetchImpl = options?.fetchImpl;
  if (!peer || typeof createDeadline !== "function" || typeof fetchImpl !== "function") {
    throw new Error("capacity signaling requires a peer, deadline factory, and fetch implementation");
  }
  let deadline = null;
  try {
    deadline = createDeadline({ timeoutMs: options.timeoutMs });
    const response = await deadline.wait(
      fetchImpl(options.offerUrl, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(options.signalingToken
            ? { Authorization: `Bearer ${options.signalingToken}` }
            : {}),
        },
        body: JSON.stringify(peer.localDescription),
        signal: deadline.signal,
      }),
    );
    const answer = await deadline.wait(response.json());
    if (!response.ok) throw new Error(`offer failed: ${JSON.stringify(answer)}`);
    await deadline.wait(peer.setRemoteDescription(answer));
    return answer;
  } catch (error) {
    deadline?.cancel(true);
    try {
      peer.close();
    } catch {
      // The browser may already have closed a failed peer.
    }
    throw error;
  } finally {
    deadline?.cancel(false);
  }
}

export async function fetchJsonWithDeadline(
  url,
  {
    timeoutMs,
    createDeadline = globalThis.zubspaceCreateRtcSignalingDeadline,
    fetchImpl = globalThis.fetch,
    label = "HTTP request",
    request = {},
  } = {},
) {
  if (typeof createDeadline !== "function" || typeof fetchImpl !== "function") {
    throw new Error(`${label} requires a deadline factory and fetch implementation`);
  }
  const deadline = createDeadline({ timeoutMs });
  try {
    const response = await deadline.wait(
      fetchImpl(url, { ...request, signal: deadline.signal }),
    );
    const value = await deadline.wait(response.json());
    if (!response.ok) throw new Error(`${label} returned HTTP ${response.status}`);
    return value;
  } catch (error) {
    deadline.cancel(true);
    throw error;
  } finally {
    deadline.cancel(false);
  }
}

export async function fetchTextWithDeadline(
  url,
  {
    timeoutMs,
    createDeadline = globalThis.zubspaceCreateRtcSignalingDeadline,
    fetchImpl = globalThis.fetch,
    label = "HTTP request",
    request = {},
  } = {},
) {
  if (typeof createDeadline !== "function" || typeof fetchImpl !== "function") {
    throw new Error(`${label} requires a deadline factory and fetch implementation`);
  }
  const deadline = createDeadline({ timeoutMs });
  try {
    const response = await deadline.wait(
      fetchImpl(url, { ...request, signal: deadline.signal }),
    );
    const text = await deadline.wait(response.text());
    return { ok: response.ok, status: response.status, text };
  } catch (error) {
    deadline.cancel(true);
    throw error;
  } finally {
    deadline.cancel(false);
  }
}

function checkedTimeoutMs(timeoutMs, label) {
  const value = Number(timeoutMs);
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new Error(`${label} timeout must be a positive integer, got ${JSON.stringify(timeoutMs)}`);
  }
  return value;
}

function rejectPending(pending, error) {
  for (const entry of pending.values()) {
    entry.timers.clearTimeout(entry.timer);
    entry.reject(error);
  }
  pending.clear();
}

export function createCdpSession(
  socket,
  { timeoutMs, timers = globalThis, onEvent = () => {} } = {},
) {
  const value = checkedTimeoutMs(timeoutMs, "CDP RPC");
  const pending = new Map();
  let nextId = 0;
  let closed = false;

  function close(reason = new Error("CDP session closed")) {
    if (closed) return;
    closed = true;
    rejectPending(pending, reason);
    try {
      socket.close();
    } catch {
      // The browser may already have closed the debugger socket.
    }
  }

  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    const entry = pending.get(message.id);
    if (!entry) {
      onEvent(message);
      return;
    }
    pending.delete(message.id);
    timers.clearTimeout(entry.timer);
    if (message.error) entry.reject(new Error(message.error.message));
    else entry.resolve(message.result);
  };
  socket.onerror = () => close(new Error("CDP WebSocket failed"));
  socket.onclose = () => close(new Error("CDP WebSocket closed before its RPC completed"));

  return {
    send(method, params = {}) {
      if (closed) return Promise.reject(new Error("CDP session is closed"));
      const id = ++nextId;
      return new Promise((resolve, reject) => {
        const timer = timers.setTimeout(() => {
          if (!pending.delete(id)) return;
          const error = new Error(`CDP ${method} did not complete within ${value}ms`);
          reject(error);
          close(error);
        }, value);
        pending.set(id, { reject, resolve, timer, timers });
        try {
          socket.send(JSON.stringify({ id, method, params }));
        } catch (error) {
          pending.delete(id);
          timers.clearTimeout(timer);
          reject(error);
          close(error);
        }
      });
    },
    close,
    pendingCount() {
      return pending.size;
    },
  };
}

export async function openCdpSession(
  webSocketDebuggerUrl,
  {
    timeoutMs,
    WebSocketImpl = globalThis.WebSocket,
    timers = globalThis,
    onEvent = () => {},
  } = {},
) {
  const value = checkedTimeoutMs(timeoutMs, "CDP WebSocket open");
  if (typeof WebSocketImpl !== "function") {
    throw new Error("CDP requires a WebSocket implementation");
  }
  const socket = new WebSocketImpl(webSocketDebuggerUrl);
  try {
    await new Promise((resolve, reject) => {
      const timer = timers.setTimeout(() => {
        reject(new Error(`CDP WebSocket did not open within ${value}ms`));
      }, value);
      socket.onopen = () => {
        timers.clearTimeout(timer);
        resolve();
      };
      socket.onerror = () => {
        timers.clearTimeout(timer);
        reject(new Error("CDP WebSocket failed before opening"));
      };
    });
    return createCdpSession(socket, { timeoutMs: value, timers, onEvent });
  } catch (error) {
    try {
      socket.close();
    } catch {
      // Continue surfacing the bounded open failure.
    }
    throw error;
  }
}

function appendBoundedOutput(current, chunk, maxOutputBytes) {
  const addition = Buffer.isBuffer(chunk) ? chunk : Buffer.from(String(chunk));
  const combined = current.length === 0 ? addition : Buffer.concat([current, addition]);
  return combined.length <= maxOutputBytes
    ? combined
    : combined.subarray(combined.length - maxOutputBytes);
}

async function settleWithin(promise, timeoutMs, timers) {
  let timer;
  try {
    return await Promise.race([
      promise.then(() => true),
      new Promise((resolve) => {
        timer = timers.setTimeout(() => resolve(false), timeoutMs);
      }),
    ]);
  } finally {
    if (timer !== undefined) timers.clearTimeout(timer);
  }
}

function commandSnapshot(child, state) {
  return {
    pid: child.pid,
    status: state.code,
    signal: state.signal,
    error: state.error,
    stdout: state.stdout.toString("utf8"),
    stderr: state.stderr.toString("utf8"),
    timedOut: state.timedOut,
    killSignals: [...state.killSignals],
  };
}

export async function terminateChildWithDeadline(
  child,
  {
    graceMs = 1_500,
    killGraceMs = 1_500,
    timers = globalThis,
  } = {},
) {
  const termGrace = checkedTimeoutMs(graceMs, "child SIGTERM grace");
  const killGrace = checkedTimeoutMs(killGraceMs, "child SIGKILL grace");
  if (!child || child.exitCode !== null || child.signalCode !== null) return true;

  const closed = new Promise((resolve) => child.once("close", resolve));
  child.kill("SIGTERM");
  if (await settleWithin(closed, termGrace, timers)) return true;
  child.kill("SIGKILL");
  if (await settleWithin(closed, killGrace, timers)) return true;

  child.stdout?.destroy();
  child.stderr?.destroy();
  child.stdin?.destroy();
  child.unref?.();
  return false;
}

export async function runCommandWithDeadline(
  command,
  args,
  {
    cwd,
    env,
    timeoutMs,
    termGraceMs = 1_500,
    killGraceMs = 1_500,
    maxOutputBytes = 64 * 1024 * 1024,
    spawnImpl = nodeSpawn,
    timers = globalThis,
    label = [command, ...args].join(" "),
  } = {},
) {
  const timeout = checkedTimeoutMs(timeoutMs, label);
  const termGrace = checkedTimeoutMs(termGraceMs, `${label} SIGTERM grace`);
  const killGrace = checkedTimeoutMs(killGraceMs, `${label} SIGKILL grace`);
  if (!Number.isSafeInteger(maxOutputBytes) || maxOutputBytes < 1) {
    throw new Error(`${label} max output must be a positive integer`);
  }

  const child = spawnImpl(command, args, {
    cwd,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const state = {
    code: null,
    signal: null,
    error: null,
    stdout: Buffer.alloc(0),
    stderr: Buffer.alloc(0),
    timedOut: false,
    killSignals: [],
  };
  child.stdout?.on("data", (chunk) => {
    state.stdout = appendBoundedOutput(state.stdout, chunk, maxOutputBytes);
  });
  child.stderr?.on("data", (chunk) => {
    state.stderr = appendBoundedOutput(state.stderr, chunk, maxOutputBytes);
  });

  let settle;
  const settled = new Promise((resolve) => {
    settle = resolve;
  });
  child.once("error", (error) => {
    state.error = error;
  });
  child.once("close", (code, signal) => {
    state.code = code;
    state.signal = signal;
    settle();
  });

  const first = await settleWithin(settled, timeout, timers);
  if (first) return commandSnapshot(child, state);

  state.timedOut = true;
  state.killSignals.push("SIGTERM");
  child.kill("SIGTERM");
  if (!(await settleWithin(settled, termGrace, timers))) {
    state.killSignals.push("SIGKILL");
    child.kill("SIGKILL");
    if (!(await settleWithin(settled, killGrace, timers))) {
      child.stdout?.destroy();
      child.stderr?.destroy();
      child.stdin?.destroy();
      child.unref?.();
    }
  }

  const result = commandSnapshot(child, state);
  const error = new Error(
    `${label} did not exit within ${timeout}ms; sent ${result.killSignals.join(" then ")}`,
  );
  error.commandResult = result;
  throw error;
}

export async function evaluateCdpExpression(
  webSocketDebuggerUrl,
  expression,
  options = {},
) {
  const session = await openCdpSession(webSocketDebuggerUrl, options);
  try {
    await session.send("Runtime.enable");
    const evaluation = await session.send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (evaluation.exceptionDetails) {
      throw new Error(
        evaluation.exceptionDetails.exception?.description || evaluation.exceptionDetails.text,
      );
    }
    return evaluation.result.value;
  } finally {
    session.close();
  }
}

export function capacityCdpRpcTimeoutMs({
  raw,
  signalingTimeoutMs,
  holdMs,
  clientCount,
  batchSize,
}) {
  const batches = Math.ceil(clientCount / batchSize);
  const fallback = signalingTimeoutMs * batches + holdMs + 60_000;
  const value = raw === undefined || raw === null || raw === "" ? fallback : Number(raw);
  if (!Number.isSafeInteger(value) || value < 1_000 || value > 86_400_000) {
    throw new Error(
      `ZUBSPACE_RTC_LOAD_CDP_RPC_TIMEOUT_MS must be an integer in 1000..86400000, got ${JSON.stringify(raw)}`,
    );
  }
  return value;
}

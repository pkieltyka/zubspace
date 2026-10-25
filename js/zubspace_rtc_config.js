(function installZubspaceRtcConfig(global) {
  "use strict";

  const defaultIceGatheringTimeoutMs = 15000;
  const minimumIceGatheringTimeoutMs = 1000;
  const maximumIceGatheringTimeoutMs = 60000;
  const defaultSignalingTimeoutMs = 20000;
  const minimumSignalingTimeoutMs = 1000;
  const maximumSignalingTimeoutMs = 60000;

  // A 0x84 message carries several 0x7d realtime events for this player and
  // states their shared session and arena-stream ids once
  // (src/protocol/realtime_event.rs). Returns the exact 0x7d packets, an empty
  // list for a malformed batch, or null for any other message.
  function expandRealtimeEventBatch(packet) {
    if (packet?.[0] !== 0x84) return null;
    const header = 19, ids = packet.subarray(3, header);
    if (packet.length < header || packet[1] !== 1 || packet[2] < 2) return [];
    const events = [];
    let offset = header;
    for (let index = 0; index < packet[2]; index += 1) {
      if (offset + 2 > packet.length) return [];
      const length = packet[offset] | (packet[offset + 1] << 8);
      offset += 2;
      if (length < 3 || offset + length > packet.length || packet[offset] !== 0x7d) return [];
      const event = new Uint8Array(length + ids.length);
      event.set(packet.subarray(offset, offset + 3), 0);
      event.set(ids, 3);
      event.set(packet.subarray(offset + 3, offset + length), 3 + ids.length);
      events.push(event);
      offset += length;
    }
    return offset === packet.length ? events : [];
  }

  // Retain every accepted fact in order; overflow is a terminal transport
  // condition, not permission to discard ordered control or a delta base.
  function createRtcReceiveQueue(options = {}) {
    const maxPackets = options.maxPackets ?? 4096;
    const maxBytes = options.maxBytes ?? 8 * 1024 * 1024;
    if (![maxPackets, maxBytes].every(value => Number.isSafeInteger(value) && value > 0)) {
      throw new RangeError("RTC receive queue limits must be positive safe integers");
    }
    const now = options.now || (() => global.performance.now());
    let slots = [], head = 0, retainedBytes = 0;
    return {
      get length() { return slots.length - head; },
      get bytes() { return retainedBytes; },
      get oldestAgeMs() { return head < slots.length ? Math.max(0, now() - slots[head].at) : 0; },
      front() { return slots[head]?.value; },
      push(value) {
        const bytes = value?.packet?.byteLength;
        if (!Number.isSafeInteger(bytes) || bytes <= 0) {
          throw new TypeError("RTC receive queue requires a binary packet");
        }
        if (slots.length - head >= maxPackets || bytes > maxBytes - retainedBytes) {
          throw new RangeError("RTC receive backlog exceeds capacity; reconnect required");
        }
        slots.push({ value, bytes, at: now() });
        retainedBytes += bytes;
      },
      shift() {
        if (head === slots.length) return undefined;
        const slot = slots[head];
        slots[head++] = undefined;
        retainedBytes -= slot.bytes;
        if (head === slots.length) {
          slots = []; head = 0;
        } else if (head >= 1024 && head * 2 >= slots.length) {
          slots = slots.slice(head); head = 0;
        }
        return slot.value;
      },
      clear() { slots = []; head = 0; retainedBytes = 0; },
    };
  }
  const networkProfileSeeds = Object.freeze({
    mild: Object.freeze({
      oneWayDelayMs: 30,
      jitterMs: 10,
      s2cSnapshotOmissionMillionths: 10_000,
      c2sInputOmissionMillionths: 5_000,
      realtimeDuplicateMillionths: 5_000,
      realtimeReorderMillionths: 5_000,
    }),
    slow: Object.freeze({
      oneWayDelayMs: 75,
      jitterMs: 25,
      s2cSnapshotOmissionMillionths: 30_000,
      c2sInputOmissionMillionths: 10_000,
      realtimeDuplicateMillionths: 10_000,
      realtimeReorderMillionths: 10_000,
    }),
    "degraded-latency": Object.freeze({
      oneWayDelayMs: 350,
      jitterMs: 20,
      s2cSnapshotOmissionMillionths: 0,
      c2sInputOmissionMillionths: 0,
      realtimeDuplicateMillionths: 0,
      realtimeReorderMillionths: 0,
    }),
    "degraded-loss": Object.freeze({
      oneWayDelayMs: 30,
      jitterMs: 10,
      s2cSnapshotOmissionMillionths: 200_000,
      c2sInputOmissionMillionths: 200_000,
      qualityProbeOmissionMillionths: 200_000,
      realtimeDuplicateMillionths: 0,
      realtimeReorderMillionths: 0,
    }),
  });

  function networkProfile(search) {
    const params = new URLSearchParams(search || "");
    const name = params.get("network_profile") || "";
    if (!name) return null;
    const policy = networkProfileSeeds[name];
    if (!policy) {
      throw new Error(`unsupported deterministic network profile ${JSON.stringify(name)}`);
    }
    const seedText = params.get("network_profile_seed") || "271828";
    if (!/^[1-9][0-9]{0,9}$/.test(seedText)) {
      throw new Error("network_profile_seed must be a nonzero u32 decimal integer");
    }
    const seed = Number(seedText);
    if (!Number.isSafeInteger(seed) || seed > 0xffff_ffff) {
      throw new Error("network_profile_seed must fit u32");
    }
    return Object.freeze({ name, seed, ...policy });
  }

  function mixNetworkProfileSample(seed, ordinal, salt) {
    let value = (seed ^ Math.imul(ordinal >>> 0, 0x9e3779b1) ^ salt) >>> 0;
    value ^= value >>> 16;
    value = Math.imul(value, 0x7feb352d) >>> 0;
    value ^= value >>> 15;
    value = Math.imul(value, 0x846ca68b) >>> 0;
    value ^= value >>> 16;
    return value >>> 0;
  }

  function networkProfileDecision(profile, direction, ordinal, packetClass = {}) {
    if (!profile || !new Set(["s2c", "c2s"]).has(direction)) {
      throw new Error("network profile decision requires a profile and s2c/c2s direction");
    }
    if (!Number.isSafeInteger(ordinal) || ordinal < 1 || ordinal > 0xffff_ffff) {
      throw new Error("network profile ordinal must be a nonzero u32 integer");
    }
    const directionSalt = direction === "s2c" ? 0x51c0a11c : 0xc2511e17;
    const jitterSample = mixNetworkProfileSample(profile.seed, ordinal, directionSalt);
    const jitterRange = profile.jitterMs * 2 + 1;
    const jitter = jitterRange > 1 ? jitterSample % jitterRange - profile.jitterMs : 0;
    const dropLimit = packetClass.quality === true && packetClass.reliable !== true
      ? (profile.qualityProbeOmissionMillionths || 0)
      : direction === "s2c"
      ? packetClass.snapshot === true
        ? profile.s2cSnapshotOmissionMillionths
        : 0
      : packetClass.input === true
        ? profile.c2sInputOmissionMillionths
        : 0;
    const realtime = packetClass.realtime === true && packetClass.reliable !== true;
    const sampleMillionth = (salt) =>
      mixNetworkProfileSample(profile.seed, ordinal, directionSalt ^ salt) % 1_000_000;
    const dropped = sampleMillionth(0xd04f00d) < dropLimit;
    const duplicated =
      !dropped && realtime &&
      sampleMillionth(0xd0b1ca7e) < profile.realtimeDuplicateMillionths;
    const reordered =
      !dropped && realtime &&
      sampleMillionth(0x0e0ede12) < profile.realtimeReorderMillionths;
    const baseDelayMs = Math.max(0, profile.oneWayDelayMs + jitter);
    return Object.freeze({
      dropped,
      duplicated,
      reordered,
      delayMs: baseDelayMs + (reordered ? profile.oneWayDelayMs + profile.jitterMs : 0),
      duplicateDelayMs: duplicated ? baseDelayMs + 1 + (sampleMillionth(0xd0011ca7) % 7) : null,
    });
  }

  function orderedNetworkProfileSchedule(scheduledAt, requestedDelayMs, previousDueAt) {
    if (
      !Number.isFinite(scheduledAt) ||
      !Number.isFinite(requestedDelayMs) ||
      !Number.isFinite(previousDueAt)
    ) {
      throw new Error("ordered network profile schedule requires finite clock values");
    }
    const dueAt = Math.max(
      scheduledAt + Math.max(0, requestedDelayMs),
      previousDueAt + 1,
    );
    return Object.freeze({
      dueAt,
      delayMs: Math.max(0, dueAt - scheduledAt),
    });
  }

  function splitIceAuthority(authority) {
    if (authority.startsWith("[")) {
      const close = authority.indexOf("]");
      if (close < 0 || authority[close + 1] !== ":") {
        throw new Error("bracketed IPv6 ICE host must be followed by :port");
      }
      return [authority.slice(0, close + 1), authority.slice(close + 2)];
    }
    const colon = authority.lastIndexOf(":");
    if (colon <= 0 || authority.slice(0, colon).includes(":")) {
      throw new Error("ICE host must be followed by :port; IPv6 requires brackets");
    }
    return [authority.slice(0, colon), authority.slice(colon + 1)];
  }

  function validateCanonicalIceHost(host) {
    if (host.startsWith("[") || host.endsWith("]")) {
      throw new Error(
        "IPv6 ICE literals are unsupported while server peer sockets bind only IPv4",
      );
    }

    const pieces = host.split(".");
    const isCanonicalIpv4 =
      pieces.length === 4 &&
      pieces.every(
        (piece) =>
          /^(?:0|[1-9][0-9]{0,2})$/.test(piece) && Number(piece) <= 255,
      );
    if (isCanonicalIpv4) {
      return;
    }
    if (
      pieces.every((piece) => /^(?:[0-9]+|0x[0-9a-f]+)$/i.test(piece))
    ) {
      throw new Error("numeric IPv4 ICE host must use canonical dotted decimal");
    }

    if (!host || host.length > 253) {
      throw new Error("canonical DNS ICE host must contain 1 through 253 characters");
    }
    for (const label of pieces) {
      if (
        label.length < 1 ||
        label.length > 63 ||
        !/^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label)
      ) {
        throw new Error(
          "canonical DNS ICE hosts use lowercase ASCII labels without edge hyphens",
        );
      }
    }
  }

  function iceUrlKind(value) {
    const scheme = value.startsWith("stun:")
      ? "stun"
      : value.startsWith("turn:")
        ? "turn"
        : null;
    const turnQuery = "?transport=udp";
    const authority =
      scheme === "stun"
        ? value.slice("stun:".length)
        : scheme === "turn" && value.endsWith(turnQuery)
          ? value.slice("turn:".length, -turnQuery.length)
          : "";
    let host;
    let portText;
    try {
      [host, portText] = splitIceAuthority(authority);
      validateCanonicalIceHost(host);
    } catch {
      host = "";
    }
    if (
      !scheme ||
      !authority ||
      !host ||
      !/^[1-9][0-9]{0,4}$/.test(portText || "") ||
      Number(portText) > 65535
    ) {
      throw new Error(
        `ICE URL must be canonical stun:host:port or turn:host:port?transport=udp: ${value}`,
      );
    }
    return scheme;
  }

  function rtcConfiguration(search) {
    const params = new URLSearchParams(search || "");
    const urls = params
      .getAll("ice")
      .flatMap((value) => value.split(","))
      .map((value) => value.trim())
      .filter(Boolean);
    for (const value of urls) {
      iceUrlKind(value);
    }

    const username = params.get("ice_username") || "";
    const credential = params.get("ice_credential") || "";
    if (
      urls.some((value) => iceUrlKind(value) === "turn") &&
      (!username || !credential)
    ) {
      throw new Error("TURN ICE URLs require ice_username and ice_credential");
    }
    const iceTransportPolicy = params.get("ice_policy") || "all";
    if (!new Set(["all", "relay"]).has(iceTransportPolicy)) {
      throw new Error(`unsupported ice_policy: ${iceTransportPolicy}`);
    }
    if (
      iceTransportPolicy === "relay" &&
      !urls.some((value) => iceUrlKind(value) === "turn")
    ) {
      throw new Error("ice_policy=relay requires a TURN ICE URL");
    }

    const iceServers = urls.length
      ? [
          {
            urls,
            ...(username ? { username } : {}),
            ...(credential ? { credential } : {}),
          },
        ]
      : [];
    return { iceServers, iceTransportPolicy };
  }

  function signalingHeaders(search, tokenOverride, offerUrl, pageOrigin) {
    const token =
      tokenOverride || new URLSearchParams(search || "").get("rtc_token") || "";
    if (token) {
      let resolvedOffer;
      let resolvedPageOrigin;
      try {
        resolvedPageOrigin = new URL(pageOrigin).origin;
        resolvedOffer = new URL(offerUrl, resolvedPageOrigin);
      } catch (error) {
        throw new Error(
          `cannot authorize RTC signaling without valid page and offer URLs: ${error.message}`,
        );
      }
      if (resolvedOffer.origin !== resolvedPageOrigin) {
        throw new Error(
          `refusing to send a signaling Bearer token to cross-origin RTC endpoint ${resolvedOffer.origin}; authenticated signaling must use the page origin ${resolvedPageOrigin}, or omit the token for an explicitly unauthenticated development endpoint`,
        );
      }
    }
    return {
      "Content-Type": "application/json",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    };
  }

  function rtcIceGatheringTimeoutMs(raw) {
    const value = raw === undefined || raw === null || raw === ""
      ? defaultIceGatheringTimeoutMs
      : Number(raw);
    if (
      !Number.isSafeInteger(value) ||
      value < minimumIceGatheringTimeoutMs ||
      value > maximumIceGatheringTimeoutMs
    ) {
      throw new Error(
        `ice_gather_timeout_ms must be an integer in ${minimumIceGatheringTimeoutMs}..${maximumIceGatheringTimeoutMs}, got ${JSON.stringify(raw)}`,
      );
    }
    return value;
  }

  function rtcSignalingTimeoutMs(raw) {
    const value = raw === undefined || raw === null || raw === ""
      ? defaultSignalingTimeoutMs
      : Number(raw);
    if (
      !Number.isSafeInteger(value) ||
      value < minimumSignalingTimeoutMs ||
      value > maximumSignalingTimeoutMs
    ) {
      throw new Error(
        `signaling_timeout_ms must be an integer in ${minimumSignalingTimeoutMs}..${maximumSignalingTimeoutMs}, got ${JSON.stringify(raw)}`,
      );
    }
    return value;
  }

  // Keep this function self-contained: test and load harnesses serialize it
  // into blank browser pages so every non-trickle signaling path has exactly
  // the same completion, timeout, and cleanup behavior.
  function waitForIceGatheringComplete(
    peer,
    timeoutMs,
    timerOverride,
    abortSignal,
  ) {
    const value = Number(timeoutMs);
    if (!Number.isSafeInteger(value) || value < 1000 || value > 60000) {
      throw new Error(
        `ICE gathering timeout must be an integer in 1000..60000ms, got ${JSON.stringify(timeoutMs)}`,
      );
    }
    if (peer.iceGatheringState === "complete") {
      return Promise.resolve();
    }
    const timers = timerOverride || globalThis;
    return new Promise((resolve, reject) => {
      let settled = false;
      let timeout = null;
      let abortListener = null;
      function finish(error) {
        if (settled) return;
        settled = true;
        if (timeout !== null) timers.clearTimeout(timeout);
        peer.removeEventListener("icegatheringstatechange", check);
        if (abortListener && abortSignal?.removeEventListener) {
          abortSignal.removeEventListener("abort", abortListener);
        }
        if (error) {
          reject(error);
        } else {
          resolve();
        }
      }
      function check() {
        if (peer.iceGatheringState === "complete") finish();
      }
      peer.addEventListener("icegatheringstatechange", check);
      check();
      if (settled) return;
      if (abortSignal) {
        abortListener = () => {
          const reason = abortSignal.reason;
          finish(
            reason instanceof Error
              ? reason
              : new Error("RTC signaling attempt was cancelled"),
          );
        };
        if (abortSignal.aborted) {
          abortListener();
          return;
        }
        if (typeof abortSignal.addEventListener !== "function") {
          finish(new Error("RTC signaling cancellation requires AbortSignal support"));
          return;
        }
        abortSignal.addEventListener("abort", abortListener, { once: true });
      }
      timeout = timers.setTimeout(() => {
        finish(
          new Error(
            `non-trickle ICE gathering did not complete within ${value}ms; refusing to signal incomplete SDP (check STUN/TURN reachability or increase ice_gather_timeout_ms, maximum 60000)`,
          ),
        );
      }, value);
    });
  }

  function createRtcSignalingDeadline(options) {
    // Keep the deadline factory self-contained so browser/load harnesses can
    // serialize this exact production implementation into an isolated page.
    const timeoutMs = options?.timeoutMs === undefined || options?.timeoutMs === null || options?.timeoutMs === ""
      ? 20000
      : Number(options.timeoutMs);
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1000 || timeoutMs > 60000) {
      throw new Error(
        `signaling_timeout_ms must be an integer in 1000..60000, got ${JSON.stringify(options?.timeoutMs)}`,
      );
    }
    const timers = options?.timers || globalThis;
    const AbortControllerImpl = options?.AbortController || globalThis.AbortController;
    const isCurrent = options?.isCurrent || (() => true);
    const onTimeout = options?.onTimeout || (() => {});
    if (typeof AbortControllerImpl !== "function") {
      throw new Error("RTC signaling deadline requires AbortController support");
    }
    const abortController = new AbortControllerImpl();
    let active = true;
    let timer = null;
    let rejectDeadline;
    const deadlinePromise = new Promise((_, reject) => {
      rejectDeadline = reject;
    });
    // A deadline can be cancelled just before its first wait is installed.
    // Keep that owner-driven cleanup from becoming an unhandled rejection;
    // every active wait still receives the same rejection through Promise.race.
    void deadlinePromise.catch(() => {});
    const timeoutError = new Error(
      `WebRTC signaling did not complete within ${timeoutMs}ms; check proxy health and retry`,
    );
    function settle(error, abort) {
      if (!active) return false;
      active = false;
      if (timer !== null) timers.clearTimeout(timer);
      rejectDeadline(error);
      if (abort) abortController.abort(error);
      return true;
    }
    function start() {
      if (!active || timer !== null) return false;
      timer = timers.setTimeout(() => {
        if (!active) return;
        if (!isCurrent()) {
          settle(new Error("RTC signaling attempt was superseded"), true);
          return;
        }
        // Settle every raced operation with the explicit timeout before abort()
        // can surface a generic AbortError from fetch or response body parsing.
        settle(timeoutError, false);
        onTimeout(timeoutError);
        abortController.abort(timeoutError);
      }, timeoutMs);
      return true;
    }
    if (options?.startImmediately !== false) start();

    return {
      signal: abortController.signal,
      wait(operation) {
        return Promise.race([Promise.resolve(operation), deadlinePromise]);
      },
      start,
      cancel(abort = false) {
        if (!abort) {
          if (!active) return false;
          active = false;
          if (timer !== null) timers.clearTimeout(timer);
          return true;
        }
        return settle(new Error("RTC signaling attempt was superseded"), true);
      },
      isActive() {
        return active;
      },
      isStarted() {
        return timer !== null;
      },
    };
  }

  function createRtcSignalingAttemptRegistry() {
    const attempts = new Map();
    return {
      attach(peer, deadline) {
        if (!peer || !deadline || typeof deadline.cancel !== "function") {
          throw new Error("RTC signaling attempt registry requires a peer and deadline");
        }
        if (attempts.has(peer)) {
          throw new Error("RTC signaling peer already owns a deadline");
        }
        attempts.set(peer, deadline);
        return deadline;
      },
      cancel(peer, abort = true) {
        const deadline = attempts.get(peer);
        if (!deadline) return false;
        attempts.delete(peer);
        deadline.cancel(abort);
        return true;
      },
      has(peer) {
        return attempts.has(peer);
      },
      size() {
        return attempts.size;
      },
    };
  }

  function createRtcLifecycleController(options) {
    const state = options?.state;
    if (!state) {
      throw new Error("RTC lifecycle controller requires shared transport state");
    }
    const disconnectGraceMs = Number(options.disconnectGraceMs ?? 5000);
    if (
      !Number.isSafeInteger(disconnectGraceMs) ||
      disconnectGraceMs < 1000 ||
      disconnectGraceMs > 30000
    ) {
      throw new Error(
        `RTC disconnect grace must be an integer in 1000..30000ms, got ${JSON.stringify(options.disconnectGraceMs)}`,
      );
    }
    const timers = options.timers || globalThis;
    const log = options.log || (() => {});
    const onMessage = options.onMessage || (() => {});
    const onClose = options.onClose || (() => {});
    let active = null;

    function isRecordActive(record) {
      return active === record && state.peerConnection === record.peer;
    }

    function removeListeners(record) {
      for (const [target, name, listener] of record.listeners) {
        target.removeEventListener(name, listener);
      }
      record.listeners.length = 0;
    }

    function cancelDisconnectGrace(record) {
      if (record.disconnectTimer !== null) {
        timers.clearTimeout(record.disconnectTimer);
        record.disconnectTimer = null;
      }
    }

    function closeRecord(record, status, reason, level = "log") {
      if (!isRecordActive(record)) return false;

      // Make every callback stale before close() can synchronously dispatch
      // another event. A replacement peer can therefore never be cleared by
      // teardown work belonging to this record.
      active = null;
      cancelDisconnectGrace(record);
      removeListeners(record);
      onClose(record.peer);
      if (state.peerConnection === record.peer) state.peerConnection = null;
      for (const [name, channel] of Object.entries(record.channels)) {
        if (state[name] === channel) state[name] = null;
        try {
          if (channel.readyState !== "closed") channel.close();
        } catch {
          // Continue closing the rest of the transport.
        }
      }
      try {
        if (record.peer.connectionState !== "closed") record.peer.close();
      } catch {
        // The peer may already have reached a browser-managed terminal state.
      }
      state.lastClosedPeerConnectionState = record.peer.connectionState;
      state.status = status;
      state.lastErrorMessage = reason || null;
      if (reason) {
        log(level, "rtc transport closed", {
          reason,
          peerState: record.peer.connectionState,
        });
      }
      return true;
    }

    function updateOpenStatus(record) {
      if (!isRecordActive(record)) return;
      if (["disconnected", "failed", "closed"].includes(record.peer.connectionState)) {
        return;
      }
      if (
        Object.values(record.channels).every(
          (channel) => channel.readyState === "open",
        )
      ) {
        state.status = 2;
        state.lastErrorMessage = null;
        // A transport that reached open at least once is a working session that may later die, which is a different
        // state from one that never connected. Recovery must fire only for the former, and the raw status code cannot
        // express the difference (3 is both a disconnect grace window and a final close), so the open instant is
        // recorded here. Kept as a timestamp rather than a boolean so a retained artifact can also date the transition.
        if (state.openNotifiedAt == null) {
          state.openNotifiedAt = Date.now();
        }
      }
    }

    function listen(record, target, name, listener) {
      target.addEventListener(name, listener);
      record.listeners.push([target, name, listener]);
    }

    function activate(peer, channels) {
      if (!peer || !channels?.gameChannel || !channels?.eventChannel || !channels?.reliableChannel) {
        throw new Error("RTC lifecycle activation requires a peer and all data channels");
      }
      if (active) {
        closeRecord(active, 3, "RTC transport superseded by a new connection");
      }
      const record = {
        peer,
        channels: {
          gameChannel: channels.gameChannel,
          eventChannel: channels.eventChannel,
          reliableChannel: channels.reliableChannel,
        },
        listeners: [],
        disconnectTimer: null,
      };
      active = record;
      state.peerConnection = peer;
      state.gameChannel = channels.gameChannel;
      state.eventChannel = channels.eventChannel;
      state.reliableChannel = channels.reliableChannel;

      listen(record, peer, "connectionstatechange", () => {
        if (!isRecordActive(record)) return;
        const peerState = peer.connectionState;
        log("log", "rtc connection state", { state: peerState });
        if (peerState === "connected") {
          cancelDisconnectGrace(record);
          state.lastErrorMessage = null;
          updateOpenStatus(record);
        } else if (peerState === "disconnected") {
          state.status = 3;
          state.lastErrorMessage =
            `RTC peer disconnected; waiting up to ${disconnectGraceMs}ms for recovery`;
          if (record.disconnectTimer === null) {
            record.disconnectTimer = timers.setTimeout(() => {
              record.disconnectTimer = null;
              closeRecord(
                record,
                4,
                `RTC peer did not recover within ${disconnectGraceMs}ms`,
                "error",
              );
            }, disconnectGraceMs);
          }
        } else if (peerState === "failed") {
          closeRecord(record, state.disconnectRequested ? 3 : 4,
            state.disconnectRequested ? "RTC disconnect requested" : "RTC peer connection failed",
            state.disconnectRequested ? "log" : "error");
        } else if (peerState === "closed") {
          closeRecord(record, 3, "RTC peer connection closed");
        }
      });
      listen(record, peer, "iceconnectionstatechange", () => {
        if (!isRecordActive(record)) return;
        log("log", "rtc ice state", { state: peer.iceConnectionState });
      });

      for (const [name, channel] of Object.entries(record.channels)) {
        const label = channel.label || name;
        channel.binaryType = "arraybuffer";
        listen(record, channel, "open", () => {
          if (!isRecordActive(record)) return;
          // Per-lane ingress evidence. The server's application counters and the client's reliable sequencer no longer
          // overlap in the stall class being investigated, so the only way to localize the loss is to count, per lane,
          // what the browser is actually handed. Exposed on window for the smoke's failure report.
          const lanes = laneIngress();
          lanes.opens[label] = (lanes.opens[label] || 0) + 1;
          log("log", "rtc data channel open", {
            label,
            ordered: channel.ordered,
            maxPacketLifeTime: channel.maxPacketLifeTime,
            maxRetransmits: channel.maxRetransmits,
          });
          updateOpenStatus(record);
        });
        listen(record, channel, "close", () => {
          if (!isRecordActive(record)) return;
          closeRecord(record, 3, `RTC data channel ${label} closed`);
        });
        listen(record, channel, "error", (event) => {
          if (!isRecordActive(record)) return;
          // Browser RTCErrorEvent objects cannot be structured-cloned into QA
          // snapshots. Retain the useful WebIDL fields as plain data instead.
          const error = event?.error;
          const disconnectRequested = state.disconnectRequested === true;
          log(disconnectRequested ? "log" : "error", "rtc data channel error", {
            disconnectRequested,
            label,
            eventType: String(event?.type || "error"),
            error: error ? {
              name: String(error.name || "RTCError"),
              message: String(error.message || ""),
              errorDetail: error.errorDetail ?? null,
              sctpCauseCode: error.sctpCauseCode ?? null,
              sdpLineNumber: error.sdpLineNumber ?? null,
              httpRequestStatusCode: error.httpRequestStatusCode ?? null,
              receivedAlert: error.receivedAlert ?? null,
              sentAlert: error.sentAlert ?? null,
            } : null,
          });
          closeRecord(record, disconnectRequested ? 3 : 4,
            disconnectRequested ? "RTC disconnect requested" : `RTC data channel ${label} failed`,
            disconnectRequested ? "log" : "error");
        });
        listen(record, channel, "message", (event) => {
          if (!isRecordActive(record)) return;
          const lanes = laneIngress();
          const size = event && event.data ? event.data.byteLength || 0 : 0;
          lanes.messages[label] = (lanes.messages[label] || 0) + 1;
          lanes.bytes[label] = (lanes.bytes[label] || 0) + size;
          try {
            onMessage(channel, event);
          } catch (error) {
            closeRecord(
              record,
              4,
              `RTC data channel ${label} message handling failed: ${error?.message || String(error)}`,
              "error",
            );
          }
        });
      }
      updateOpenStatus(record);
      return peer;
    }

    function laneIngress() {
      const target = typeof window === "undefined" ? null : window;
      if (!target) return { opens: {}, messages: {}, bytes: {} };
      if (!target.__zubspaceLaneIngress) {
        target.__zubspaceLaneIngress = { opens: {}, messages: {}, bytes: {} };
      }
      return target.__zubspaceLaneIngress;
    }

    return {
      activate,
      closeActive(status = 3, reason = null) {
        return active ? closeRecord(active, status, reason) : false;
      },
      terminatePeer(peer, status, reason) {
        return active?.peer === peer
          ? closeRecord(active, status, reason, status === 4 ? "error" : "log")
          : false;
      },
      isActive(peer) {
        return active?.peer === peer && state.peerConnection === peer;
      },
    };
  }

  // The recovery precondition, in one place so the page, its tests and the wasm import cannot disagree about it. The raw
  // status code cannot answer this: 3 covers both a disconnect grace window - where a rejoin would abort a recoverable
  // session - and a final close, and 4 also covers a signaling failure that never reached open. What matters is the
  // conjunction: the transport reached open at least once and is gone now.
  function rtcTransportIsTerminal(state) {
    return (
      state?.openNotifiedAt != null &&
      (state.status === 3 || state.status === 4)
    );
  }

  global.zubspaceRtcConfiguration = rtcConfiguration;
  global.zubspaceCreateRtcReceiveQueue = createRtcReceiveQueue;
  global.zubspaceExpandRealtimeEventBatch = expandRealtimeEventBatch;
  global.zubspaceSignalingHeaders = signalingHeaders;
  global.zubspaceRtcIceGatheringTimeoutMs = rtcIceGatheringTimeoutMs;
  global.zubspaceRtcSignalingTimeoutMs = rtcSignalingTimeoutMs;
  global.zubspaceWaitForIceGatheringComplete = waitForIceGatheringComplete;
  global.zubspaceCreateRtcSignalingDeadline = createRtcSignalingDeadline;
  global.zubspaceCreateRtcSignalingAttemptRegistry =
    createRtcSignalingAttemptRegistry;
  global.zubspaceCreateRtcLifecycleController = createRtcLifecycleController;
  global.zubspaceRtcTransportIsTerminal = rtcTransportIsTerminal;
  global.zubspaceNetworkProfile = networkProfile;
  global.zubspaceNetworkProfileDecision = networkProfileDecision;
  global.zubspaceOrderedNetworkProfileSchedule = orderedNetworkProfileSchedule;
})(typeof window === "undefined" ? globalThis : window);

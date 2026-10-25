// Passive diagnostic, serialized into a page before application startup.
// Never sends, changes or suppresses a packet, nor drives gameplay state.
export function observeProbeTiming() {
  const NativePeer = window.RTCPeerConnection;
  const rows = new Map();
  const capture = { rows: [], omittedRows: 0, maxFramesGapMs: 0, frames: 0, startedAt: performance.now() };
  window.__udpProbeTiming = capture;
  const identity = bytes => {
    if (bytes?.byteLength !== 30) return null;
    const data = bytes instanceof ArrayBuffer ? new Uint8Array(bytes) :
      ArrayBuffer.isView(bytes) ? new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength) : null;
    if (!data || ![0x81, 0x82].includes(data[0]) || data[1] !== 1) return null;
    const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
    return { type: data[0], id: `${view.getBigUint64(2, true)}:${view.getBigUint64(10, true)}:${view.getUint32(18, true)}` };
  };
  window.RTCPeerConnection = class extends NativePeer {
    createDataChannel(...args) {
      const channel = super.createDataChannel(...args);
      if (channel.label !== "zubspace-game") return channel;
      channel.addEventListener("message", event => {
        const probe = identity(event.data);
        if (probe?.type !== 0x81) return;
        const prior = rows.get(probe.id);
        if (prior) { prior.receiveCount++; return; }
        const row = { id: probe.id, receivedAt: performance.now(), receiveCount: 1,
          echoedAt: null, echoCount: 0, bufferedBytes: null, bufferedBytesAfter: null, sendError: null };
        rows.set(probe.id, row); capture.rows.push(row);
        if (capture.rows.length > 512) {
          rows.delete(capture.rows.shift().id); capture.omittedRows++;
        }
      });
      const nativeSend = channel.send.bind(channel);
      channel.send = bytes => {
        const probe = identity(bytes);
        const row = probe?.type === 0x82 ? rows.get(probe.id) : null;
        if (row) row.bufferedBytes = channel.bufferedAmount;
        try {
          const result = nativeSend(bytes);
          if (row) {
            row.echoedAt = performance.now(); row.echoCount++;
            row.bufferedBytesAfter = channel.bufferedAmount;
          }
          return result;
        } catch (error) {
          if (row) row.sendError = String(error?.message || error).slice(0, 256);
          throw error;
        }
      };
      return channel;
    }
  };
  let previous = performance.now();
  function frame(now) {
    capture.frames++; capture.maxFramesGapMs = Math.max(capture.maxFramesGapMs, now - previous); previous = now;
    if (now - capture.startedAt < 180000) requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

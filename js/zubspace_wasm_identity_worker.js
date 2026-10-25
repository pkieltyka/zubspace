"use strict";

// Incremental SHA-256 in a dedicated worker, including insecure HTTP LAN QA
// where WebCrypto is unavailable. Fixed SHA-256 constants; Node crypto and
// standard known-answer vectors independently verify padding and chunk splits.
const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);
const h = new Uint32Array([
  0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
  0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
]);
const tail = new Uint8Array(64);
const words = new Uint32Array(64);
let tailBytes = 0;
let bytes = 0;
let cpuMs = 0;
let finished = false;
const rotate = (value, count) => (value >>> count) | (value << (32 - count));

function block(input, offset) {
  for (let i = 0; i < 16; i += 1) {
    const j = offset + i * 4;
    words[i] = (input[j] << 24) | (input[j + 1] << 16) | (input[j + 2] << 8) | input[j + 3];
  }
  for (let i = 16; i < 64; i += 1) {
    const x = words[i - 15], y = words[i - 2];
    const s0 = rotate(x, 7) ^ rotate(x, 18) ^ (x >>> 3);
    const s1 = rotate(y, 17) ^ rotate(y, 19) ^ (y >>> 10);
    words[i] = words[i - 16] + s0 + words[i - 7] + s1;
  }
  let [a, b, c, d, e, f, g, last] = h;
  for (let i = 0; i < 64; i += 1) {
    const s1 = rotate(e, 6) ^ rotate(e, 11) ^ rotate(e, 25);
    const choice = (e & f) ^ (~e & g);
    const t1 = (last + s1 + choice + K[i] + words[i]) >>> 0;
    const s0 = rotate(a, 2) ^ rotate(a, 13) ^ rotate(a, 22);
    const majority = (a & b) ^ (a & c) ^ (b & c);
    last = g; g = f; f = e; e = (d + t1) >>> 0;
    d = c; c = b; b = a; a = (t1 + s0 + majority) >>> 0;
  }
  h[0] += a; h[1] += b; h[2] += c; h[3] += d;
  h[4] += e; h[5] += f; h[6] += g; h[7] += last;
}

function update(input) {
  let offset = 0;
  bytes += input.byteLength;
  if (tailBytes > 0) {
    const size = Math.min(64 - tailBytes, input.byteLength);
    tail.set(input.subarray(0, size), tailBytes);
    tailBytes += size;
    offset = size;
    if (tailBytes === 64) { block(tail, 0); tailBytes = 0; }
  }
  for (; offset + 64 <= input.byteLength; offset += 64) block(input, offset);
  if (offset < input.byteLength) {
    tail.set(input.subarray(offset), 0);
    tailBytes = input.byteLength - offset;
  }
}

function digest() {
  tail[tailBytes++] = 0x80;
  tail.fill(0, tailBytes);
  if (tailBytes > 56) { block(tail, 0); tail.fill(0); }
  const view = new DataView(tail.buffer);
  view.setUint32(56, Math.floor(bytes / 0x20000000));
  view.setUint32(60, (bytes * 8) >>> 0);
  block(tail, 0);
  return Array.from(h, (word) => word.toString(16).padStart(8, "0")).join("");
}

self.onmessage = ({ data }) => {
  const startedAt = performance.now();
  try {
    if (finished) throw new Error("already_finished");
    if (data?.type === "chunk") {
      if (!(data.buffer instanceof ArrayBuffer) || data.buffer.byteLength === 0 ||
          data.buffer.byteLength > 256 * 1024 || bytes + data.buffer.byteLength > 128 * 1024 * 1024) {
        throw new Error("invalid_chunk");
      }
      update(new Uint8Array(data.buffer));
      cpuMs += performance.now() - startedAt;
      self.postMessage({ type: "ack", bytes });
    } else if (data?.type === "finish") {
      finished = true;
      const sha256 = digest();
      cpuMs += performance.now() - startedAt;
      self.postMessage({ type: "digest", bytes, sha256, cpuMs });
    } else throw new Error("invalid_message");
  } catch {
    finished = true;
    self.postMessage({ type: "error" });
  }
};

import fs from "node:fs";

// Preserve character offsets without assembling the whole retained log into
// one V8 string. A line remains the parsing unit, including its original CR.
export function* logLines(logs) {
  const chunks = Array.isArray(logs) ? logs : [String(logs || "")];
  let pending = "";
  for (const chunk of chunks) {
    const text = pending + String(chunk ?? "");
    let start = 0, newline;
    while ((newline = text.indexOf("\n", start)) !== -1) {
      yield text.slice(start, newline);
      start = newline + 1;
    }
    pending = text.slice(start);
  }
  yield pending;
}

export function writeLogChunks(filename, logs) {
  const fd = fs.openSync(filename, "w");
  let pending = "";
  const write = (text) => {
    const bytes = Buffer.from(text);
    for (let offset = 0; offset < bytes.length;) {
      const written = fs.writeSync(fd, bytes, offset, bytes.length - offset);
      if (!written) throw new Error("retained log write made no progress");
      offset += written;
    }
  };
  try {
    for (const chunk of Array.isArray(logs) ? logs : [String(logs || "")]) {
      let text = pending + String(chunk ?? "");
      pending = "";
      if (/[\uD800-\uDBFF]$/.test(text)) {
        pending = text.slice(-1);
        text = text.slice(0, -1);
      }
      write(text);
    }
    write(pending);
  } finally {
    fs.closeSync(fd);
  }
}

// Live evidence export must keep draining the child's stdout. One synchronous
// write per tiny tracing chunk blocked the runner for ~1s on a retained27MiB
// log. Batch bounded text, preserve split surrogate pairs, and yield on I/O.
export async function writeLogChunksAsync(filename, logs) {
  // Snapshot before the first await: logs can grow while the file is written.
  const chunks = Array.isArray(logs) ? logs.slice() : [String(logs || "")];
  const maximumCharacters = 256 * 1024;
  const file = await fs.promises.open(filename, "w");
  let parts = [], characters = 0;
  const flush = async (final = false) => {
    let text = parts.join("");
    const tail = !final && /[\uD800-\uDBFF]$/.test(text) ? text.slice(-1) : "";
    if (tail) text = text.slice(0, -1);
    parts = tail ? [tail] : [];
    characters = tail.length;
    const bytes = Buffer.from(text);
    for (let offset = 0; offset < bytes.length;) {
      const { bytesWritten } = await file.write(bytes, offset, bytes.length - offset);
      if (!bytesWritten) throw new Error("retained log write made no progress");
      offset += bytesWritten;
    }
  };
  try {
    for (const chunk of chunks) {
      const text = String(chunk ?? "");
      for (let offset = 0; offset < text.length;) {
        const count = Math.min(maximumCharacters - characters, text.length - offset);
        parts.push(text.slice(offset, offset + count));
        offset += count;
        characters += count;
        if (characters === maximumCharacters) await flush();
      }
    }
    await flush(true);
  } finally {
    await file.close();
  }
}

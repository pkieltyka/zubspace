// Keyboard feedback retains the original field parser and fresh result objects.
// Only consumption changes: each append-only log chunk is scanned once per
// marker, rather than rebuilding the complete server history on every poll.
const caches = new WeakMap();

function parseLine(line) {
  const record = {};
  for (const match of line.matchAll(/(\w+)=("[^"]*"|\S+)/g)) {
    const raw = match[2];
    if (raw === "true" || raw === "false") record[match[1]] = raw === "true";
    else if (raw !== "" && Number.isFinite(Number(raw))) record[match[1]] = Number(raw);
    else record[match[1]] = raw.replace(/^"|"$/g, "");
  }
  return record;
}

export function createKeyboardTraceReader(marker, {
  maximumRecords = 200000, maximumLineCharacters = 1024 * 1024,
} = {}) {
  if (typeof marker !== "string" || !marker) throw new Error("keyboard trace marker is required");
  if (![maximumRecords, maximumLineCharacters].every(n => Number.isSafeInteger(n) && n > 0)) {
    throw new Error("keyboard trace bounds must be positive integers");
  }
  let cursor = 0, pending = "", lastChunk = null, chunksRef = null, failure = null;
  const records = [], stats = { calls: 0, chunks: 0, characters: 0 };
  return {
    get stats() { return { ...stats, records: records.length, pendingCharacters: pending.length, failure }; },
    read(chunks) {
      if (failure) throw new Error(failure);
      try {
        if (!Array.isArray(chunks)) throw new Error("keyboard server log must be an append-only chunk array");
        if ((chunksRef && chunksRef !== chunks) || chunks.length < cursor ||
            (cursor && chunks[cursor - 1] !== lastChunk)) {
          throw new Error("keyboard server log changed behind its append-only cursor");
        }
        chunksRef = chunks;
        stats.calls++;
        while (cursor < chunks.length) {
          const chunk = chunks[cursor];
          if (typeof chunk !== "string") throw new Error("keyboard log chunk must be a string");
          const text = pending + chunk;
          let start = 0, newline;
          while ((newline = text.indexOf("\n", start)) !== -1) {
            if (newline - start > maximumLineCharacters) throw new Error("keyboard log line bound exceeded");
            const line = text.slice(start, newline);
            if (line.includes(marker)) {
              if (records.length >= maximumRecords) throw new Error("keyboard trace record bound exceeded");
              records.push(parseLine(line));
            }
            start = newline + 1;
          }
          pending = text.slice(start);
          if (pending.length > maximumLineCharacters) throw new Error("keyboard partial log line bound exceeded");
          cursor++; lastChunk = chunk;
          stats.chunks++; stats.characters += chunk.length;
        }
        const provisional = pending.includes(marker);
        if (provisional && records.length >= maximumRecords) throw new Error("keyboard trace record bound exceeded");
        const rows = records.map(record => ({ ...record }));
        // The former joined-string parser exposed the final unterminated line.
        // Keep that row provisional so later bytes replace it, never duplicate it.
        if (provisional) rows.push(parseLine(pending));
        return rows;
      } catch (error) {
        failure = error.message;
        throw error;
      }
    },
  };
}

function readerFor(server, marker) {
  const chunks = server.logs;
  let entry = caches.get(server);
  if (entry?.chunks !== chunks) {
    entry = { chunks, markers: new Map() };
    caches.set(server, entry);
  }
  let reader = entry.markers.get(marker);
  if (!reader) {
    reader = createKeyboardTraceReader(marker);
    entry.markers.set(marker, reader);
  }
  return { chunks, reader };
}

export function keyboardTraceRecords(server, marker) {
  const { chunks, reader } = readerFor(server, marker);
  return reader.read(chunks);
}

export function keyboardTraceReaderStats(server, marker) {
  return readerFor(server, marker).reader.stats;
}

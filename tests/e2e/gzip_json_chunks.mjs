// Cold diagnostic transport only. Self-contained for browser evaluation.
// The supplied reader owns an immutable snapshot and bounds each JSON slice.
export function createGzipJsonReader(jsonReader, maximumCharacters, chunkCharacters) {
  if (!Number.isSafeInteger(maximumCharacters) || maximumCharacters < 1 ||
      maximumCharacters > 256 * 1024 * 1024 || !Number.isSafeInteger(chunkCharacters) ||
      chunkCharacters < 1 || chunkCharacters > 64 * 1024) throw new Error('invalid gzip capture limits');
  let consumed = 0, reader = null, pending = '', ended = false, ordinal = 0;
  let binaryTail = '';
  let streamFailure = null;
  let cancelled = false, chain = Promise.resolve(), budgetStart = performance.now();
  function start() {
    const text = new ReadableStream({
      async pull(controller) {
        // Yield cold serialization work to the browser task queue. This does
        // not alter a gameplay clock, timeout or acceptance window.
        if (performance.now() - budgetStart >= 4) {
          await new Promise(resolve => setTimeout(resolve, 0));
          budgetStart = performance.now();
        }
        if (cancelled) return;
        const chunk = jsonReader.read();
        if (typeof chunk?.data !== 'string' || typeof chunk.done !== 'boolean') {
          throw new Error('invalid source JSON chunk');
        }
        consumed += chunk.data.length;
        if (consumed > maximumCharacters) throw new Error('diagnostic JSON exceeds capture limit');
        if (chunk.data) controller.enqueue(chunk.data);
        if (chunk.done) controller.close();
      },
      cancel() { cancelled = true; },
    });
    // TextEncoderStream preserves surrogate pairs split across source chunks.
    reader = text.pipeThrough(new TextEncoderStream())
      .pipeThrough(new CompressionStream('gzip')).getReader();
  }
  async function next() {
    if (cancelled) throw new Error('diagnostic compression cancelled');
    if (!reader) start();
    while (!pending.length && !ended) {
      const item = await reader.read();
      ended = item.done;
      if (item.value) {
        let binary = binaryTail;
        for (let offset = 0; offset < item.value.length; offset += 4096) {
          binary += String.fromCharCode(...item.value.subarray(offset, offset + 4096));
        }
        // Base64 padding is legal only at the end of the entire stream,
        // not at each CompressionStream output boundary.
        const complete = binary.length - binary.length % 3;
        pending = btoa(binary.slice(0, complete));
        binaryTail = binary.slice(complete);
      }
      if (ended && binaryTail) {
        pending += btoa(binaryTail);
        binaryTail = '';
      }
    }
    const data = pending.slice(0, chunkCharacters);
    pending = pending.slice(data.length);
    return { data, done: ended && !pending.length, ordinal: ordinal++,
      encoding: 'gzip-base64', sourceCharacters: consumed };
  }
  return {
    read() {
      // Concurrent RPCs share one ordered stream; failures never strand later
      // requests behind a rejected chain.
      const result = chain.then(next);
      chain = result.catch(error => { streamFailure = error; });
      return result;
    },
    async cancel() {
      cancelled = true;
      if (reader) {
        try { await reader.cancel(); }
        catch (error) { if (error !== streamFailure) throw error; }
      }
      await chain;
    },
  };
}

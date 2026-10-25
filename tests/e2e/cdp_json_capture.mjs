import {createGzipJsonReader} from './gzip_json_chunks.mjs';
import {createGzipJsonCollector} from './gzip_json_decode.mjs';

// Cold diagnostics only. Runs inside the browser via its source text; keep
// this function self-contained. The caller supplies a detached snapshot.
export function createJsonChunkReader(value, chunkCharacters) {
  if (!Number.isSafeInteger(chunkCharacters) || chunkCharacters < 1) throw new Error('invalid JSON chunk size');
  const ancestors = new Set();
  function* encode(value, key = '', depth = 0) {
    if (depth > 128) throw new Error('diagnostic JSON exceeds depth limit');
    if (value && typeof value.toJSON === 'function') value = value.toJSON(key);
    if (value && typeof value === 'object' && ['[object Number]', '[object String]', '[object Boolean]'].includes(Object.prototype.toString.call(value))) {
      value = value.valueOf();
    }
    if (value === null || typeof value !== 'object') {
      const text = JSON.stringify(value);
      if (text === undefined) throw new Error('unserializable diagnostic JSON root');
      yield text; return;
    }
    if (ancestors.has(value)) throw new Error('cyclic diagnostic JSON');
    ancestors.add(value);
    if (Array.isArray(value)) {
      yield '[';
      for (let i = 0; i < value.length; i++) {
        if (i) yield ',';
        const item = value[i];
        if (item === undefined || typeof item === 'function' || typeof item === 'symbol') yield 'null';
        else yield* encode(item, String(i), depth + 1);
      }
      yield ']';
    } else {
      yield '{'; let first = true;
      for (const key of Object.keys(value)) {
        const item = value[key];
        if (item === undefined || typeof item === 'function' || typeof item === 'symbol') continue;
        if (!first) yield ',';
        first = false;
        yield JSON.stringify(key); yield ':';
        yield* encode(item, key, depth + 1);
      }
      yield '}';
    }
    ancestors.delete(value);
  }
  const iterator = encode(value);
  let pending = '', done = false, ordinal = 0;
  return {read() {
    let data = '';
    while (data.length < chunkCharacters && !done) {
      if (!pending) {
        const next = iterator.next();
        if (next.done) { done = true; break; }
        pending = next.value;
      }
      const count = Math.min(pending.length, chunkCharacters - data.length);
      data += pending.slice(0, count);
      pending = pending.slice(count);
    }
    return {data, done, ordinal: ordinal++};
  }};
}

let captureSerial = 0;
export async function prepareCdpJson(send, expression, {
  chunkCharacters = 64 * 1024, maximumCharacters = 256 * 1024 * 1024,
  readConcurrency = 4,
} = {}) {
  if (!Number.isSafeInteger(chunkCharacters) || chunkCharacters < 1 || chunkCharacters > 64 * 1024 ||
      !Number.isSafeInteger(maximumCharacters) || maximumCharacters < 1 ||
      maximumCharacters > 256 * 1024 * 1024 ||
      !Number.isInteger(readConcurrency) || readConcurrency < 1 || readConcurrency > 4) throw new Error('invalid capture limits');
  const objectGroup = `zubspace-cold-capture-${++captureSerial}`;
  const checked = (response) => {
    if (response?.exceptionDetails) {
      const details = response.exceptionDetails;
      throw new Error(String(details.exception?.description ?? details.exception?.value ??
        details.text ?? 'diagnostic browser exception').slice(0, 2048));
    }
    return response?.result;
  };
  let phase = 'preparing';
  let releasePromise = null;
  let handle, batch = Promise.resolve();
  const release = () => {
    phase = 'released';
    releasePromise ||= (async () => {
      await batch;
      const errors = [];
      if (handle?.objectId) {
        try {
          checked(await send('Runtime.callFunctionOn', {
            objectId: handle.objectId, functionDeclaration: 'function() { return this.cancel(); }',
            returnByValue: true, awaitPromise: true,
          }));
        } catch (error) { errors.push(error); }
      }
      try { await send('Runtime.releaseObjectGroup', {objectGroup}); }
      catch (error) { errors.push(error); }
      if (errors.length === 1) throw errors[0];
      if (errors.length) throw new AggregateError(errors, 'diagnostic cancellation and handle release failed');
    })();
    return releasePromise;
  };
  const releaseAfter = async (failure) => {
    try { await release(); }
    catch (cleanupError) {
      if (failure) throw new AggregateError([failure, cleanupError],
        `diagnostic capture failed: ${failure.message}; cleanup failed: ${cleanupError.message}`);
      throw cleanupError;
    }
  };
  try {
    handle = checked(await send('Runtime.evaluate', {
      expression: `(${createGzipJsonReader.toString()})(( ${createJsonChunkReader.toString()})(structuredClone(${expression}), ${chunkCharacters}), ${maximumCharacters}, ${chunkCharacters})`,
      returnByValue: false, objectGroup,
    }));
    if (!handle?.objectId) throw new Error('diagnostic snapshot handle missing');
  } catch (error) {
    await releaseAfter(error);
    throw error;
  }
  phase = 'ready';
  return {release, async read() {
    if (phase !== 'ready') throw new Error(`diagnostic snapshot cannot be read in phase ${phase}`);
    phase = 'reading';
    let failure = null;
    try {
      const collector = createGzipJsonCollector(maximumCharacters, chunkCharacters);
      let ordinal = 0, done = false;
      for (;;) {
        // Keep each RPC bounded. Drain the entire batch before cleanup on a
        // failure, so no late request can read a released object group.
        batch = Promise.allSettled(Array.from({length: readConcurrency}, () =>
          Promise.resolve().then(() => send('Runtime.callFunctionOn', {
            objectId: handle.objectId, functionDeclaration: 'function() { return this.read(); }', returnByValue: true,
            awaitPromise: true,
          })).then(response => checked(response)?.value)));
        const settled = await batch;
        const rejected = settled.find(result => result.status === 'rejected');
        if (rejected) throw rejected.reason;
        if (phase !== 'reading') throw new Error('diagnostic snapshot released during read');
        const chunks = settled.map(result => result.value);
        if (chunks.some(chunk => !Number.isSafeInteger(chunk?.ordinal))) throw new Error('invalid diagnostic chunk ordinal');
        chunks.sort((a, b) => a.ordinal - b.ordinal);
        for (const chunk of chunks) {
          if (chunk.ordinal !== ordinal++) throw new Error('diagnostic chunk ordinal discontinuity');
          collector.append(chunk);
          done ||= chunk.done;
        }
        if (done) break;
      }
      return await collector.decode();
    } catch (error) {
      failure = error;
      throw error;
    } finally {
      await releaseAfter(failure);
    }
  }};
}

export async function captureCdpJson(send, expression, options) {
  return (await prepareCdpJson(send, expression, options)).read();
}

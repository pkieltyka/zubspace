import {gunzip} from 'node:zlib';
import {promisify} from 'node:util';

const decompress = promisify(gunzip);

// Cold diagnostic decoding only. The bound is on the original UTF-16 JSON
// characters, not compressed size: compression must never bypass that limit.
export function createGzipJsonCollector(maximumCharacters, chunkCharacters) {
  if (!Number.isSafeInteger(maximumCharacters) || maximumCharacters < 1 ||
      maximumCharacters > 256 * 1024 * 1024 ||
      !Number.isSafeInteger(chunkCharacters) || chunkCharacters < 1 ||
      chunkCharacters > 64 * 1024) throw new Error('invalid gzip capture limits');
  const maximumBytes = maximumCharacters * 3;
  // UTF-8 uses at most three bytes per UTF-16 code unit. Allow conservative
  // DEFLATE expansion and gzip framing, even for tiny incompressible inputs.
  const maximumEncoded = Math.ceil((maximumBytes + Math.ceil(maximumBytes / 8) +
    Math.ceil(maximumBytes / 64) + 1024) / 3) * 4;
  const parts = [];
  let encoded = 0, sourceCharacters = 0, terminal = false, decoded = false;
  return {
    append(chunk) {
      if (decoded) throw new Error('diagnostic gzip collector already decoded');
      if (chunk?.encoding !== 'gzip-base64' || typeof chunk.data !== 'string' ||
          typeof chunk.done !== 'boolean' || chunk.data.length > chunkCharacters ||
          (!chunk.done && !chunk.data.length) ||
          !Number.isSafeInteger(chunk.sourceCharacters) ||
          chunk.sourceCharacters < sourceCharacters || chunk.sourceCharacters > maximumCharacters) {
        throw new Error('invalid diagnostic gzip chunk');
      }
      if (terminal && (!chunk.done || chunk.data.length || chunk.sourceCharacters !== sourceCharacters)) {
        throw new Error('diagnostic data after terminal chunk');
      }
      encoded += chunk.data.length;
      if (encoded > maximumEncoded) throw new Error('diagnostic gzip exceeds encoded capture limit');
      parts.push(chunk.data);
      sourceCharacters = chunk.sourceCharacters;
      terminal ||= chunk.done;
    },
    async decode() {
      if (!terminal || decoded) throw new Error('diagnostic gzip is incomplete or already decoded');
      decoded = true;
      const base64 = parts.join('');
      parts.length = 0;
      // Buffer.from is permissive; reject malformed/truncated framing before
      // decoding instead of silently accepting ignored trailing characters.
      if (!base64.length || base64.length % 4 || !/^[A-Za-z0-9+/]*={0,2}$/.test(base64)) {
        throw new Error('invalid diagnostic gzip base64');
      }
      const compressed = Buffer.from(base64, 'base64');
      if (compressed.toString('base64') !== base64) throw new Error('noncanonical diagnostic gzip base64');
      const bytes = await decompress(compressed, {maxOutputLength: maximumBytes});
      const text = new TextDecoder('utf-8', {fatal: true}).decode(bytes);
      if (text.length > maximumCharacters || text.length !== sourceCharacters) {
        throw new Error('diagnostic JSON decoded character count mismatch');
      }
      return JSON.parse(text);
    },
  };
}

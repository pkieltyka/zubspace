import { randomUUID } from 'node:crypto';
import { createJsonChunkReader } from './cdp_json_capture.mjs';

// Immutable diagnostic data only. Freeze once, then bound every browser RPC;
// no gameplay hooks, dropped rows, timers or stall-observer exclusions.
export function captureBrowserJsonSnapshot(actor, expression, options) {
  return captureBrowserJsonReader(actor, size =>
    `(${createJsonChunkReader.toString()})(structuredClone(${expression}),${size})`, options);
}

// Reader producers own detachment. A live snapshot clones once above; an
// already sealed collector can expose its immutable records without a copy.
export async function captureBrowserJsonReader(actor, readerExpression, {
  chunkCharacters=16*1024, maximumCharacters=64*1024*1024,
}={}) {
  if(!Number.isSafeInteger(chunkCharacters)||chunkCharacters<1||chunkCharacters>64*1024||
     !Number.isSafeInteger(maximumCharacters)||maximumCharacters<1||maximumCharacters>64*1024*1024) {
    throw new Error('invalid snapshot limits');
  }
  const token=randomUUID(),key=`__zubspaceJsonSnapshot_${token}`;
  const k=JSON.stringify(key),t=JSON.stringify(token);
  const nodeStartedAt=Date.now();
  let failure;
  try {
    const metadata=await actor.evaluate(`(() => {
      if(Object.hasOwn(window,${k})) throw new Error('snapshot key occupied');
      const start=performance.now(),wallStart=Date.now();
      const reader=${readerExpression(chunkCharacters)};
      window[${k}]={token:${t},reader};
      return {start,wallStart,frozenAt:performance.now()};
    })()`);
    if(!Number.isFinite(metadata?.start)||!Number.isFinite(metadata?.frozenAt)||metadata.frozenAt<metadata.start) {
      throw new Error('invalid snapshot timing');
    }
    const parts=[];
    let characters=0;
    for(let ordinal=0;;ordinal++) {
      const chunk=await actor.evaluate(`(() => {
        const saved=window[${k}];
        if(saved?.token!==${t}) throw new Error('snapshot ownership lost');
        return saved.reader.read();
      })()`);
      if(chunk?.ordinal!==ordinal||typeof chunk.data!=='string'||typeof chunk.done!=='boolean'||
         chunk.data.length>chunkCharacters||(!chunk.done&&chunk.data.length!==chunkCharacters)) {
        throw new Error('invalid snapshot chunk');
      }
      characters+=chunk.data.length;
      if(characters>maximumCharacters) throw new Error('snapshot exceeds JSON limit');
      parts.push(chunk.data);
      if(chunk.done) break;
    }
    return {value:JSON.parse(parts.join('')),timing:{nodeStartedAt,nodeReceivedAt:Date.now(),
      browserStartedAt:metadata.start,browserWallStart:metadata.wallStart,
      browserFrozenAt:metadata.frozenAt,freezeMs:metadata.frozenAt-metadata.start,
      characters,chunkCharacters,chunks:parts.length}};
  } catch(error) {failure=error;throw error;}
  finally {
    try {
      await actor.evaluate(`(() => {
        if(window[${k}]?.token===${t}) delete window[${k}];
        return true;
      })()`);
    } catch(cleanupError) {
      if(failure) throw new AggregateError([failure,cleanupError],'snapshot read and cleanup failed');
      throw cleanupError;
    }
  }
}

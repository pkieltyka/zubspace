// Live cutover assertions need only cancellation and roster-removal frames.
// Full packet/metrics/sprite histories are retained at artifact boundaries;
// transferring them inside a predicate can itself pause the browser clock.
export function cutoverWireEvidence(source) {
  const select = entries => (Array.isArray(entries) ? entries : []).filter(entry => {
    if (entry?.direction !== "received") return false;
    const b = entry.bytes;
    return b?.length === 52 && b[0] === 0x7d && b[1] === 2 && b[2] === 5 ||
      b?.length === 58 && b[0] === 0 && b[1] === 3 && b[6] === 0x7d && b[7] === 2 && b[8] === 5 ||
      b?.length === 9 && b[0] === 0 && b[1] === 3 && b[6] === 4;
  });
  return { packetLogFirst: select(source.__zubspacePacketLogFirst),
    packetLog: select(source.__zubspacePacketLog) };
}

export function readCutoverWireEvidence(client) {
  return client.evaluate(`(${cutoverWireEvidence.toString()})(window)`);
}

// Different authored network conditions, never interchangeable acceptance
// claims. The original R9 stress profile remains available without changes.
export function mixedUdpProfiles(kind) {
  if (!["fifo-wan", "reorder-stress"].includes(kind)) throw new Error(`unknown UDP fixture profile: ${kind}`);
  const preserveOrder = kind === "fifo-wan";
  return [{ seed: 41 }, { seed: 42,
    c2s: { delayMs: 65, jitterMs: 8, loss: 0.003, burstLoss: 0.001, burstLength: 2,
      bytesPerSecond: 192000, preserveOrder },
    s2c: { delayMs: 115, jitterMs: 12, loss: 0.003, burstLoss: 0.001, burstLength: 2,
      bytesPerSecond: 384000, preserveOrder } }];
}

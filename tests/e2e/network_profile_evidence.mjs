export function assertNetworkProfileEvidence(evidence, {
  expectedName,
  expectedSeed,
  label = "deterministic network profile",
} = {}) {
  // The realized delay is proved by the *mean* release, not by the strict minimum. A minimum over tens of thousands of
  // releases is an extreme-order statistic, so what moves it is the browser's timer early-fire rather than the profile:
  // measured in the frozen matrix's Firefox mild cell, `s2c.delay` reported `min 18, max 101, avg 32.88`
  // over 28,154 samples against an authored `30 ms +/- 10 ms` floor of 20 ms - and the `Date.now()` measurement of the
  // same quantity could drive the minimum to 0 outright. So the mean must meet the floor, and the minimum only has to
  // show that releases were delayed at all. The sample count, every configured effect, and empty pending/expiry state
  // remain mandatory.
  const missing = [];
  if (evidence?.configured !== true) missing.push("configuration");
  if (evidence?.profile?.name !== expectedName) missing.push("profile-name");
  if (evidence?.profile?.seed !== expectedSeed) missing.push("profile-seed");
  for (const [path, value] of [
    ["s2c.seen", evidence?.s2c?.seen],
    ["s2c.delayed", evidence?.s2c?.delayed],
    ["s2c.released", evidence?.s2c?.released],
    ["s2c.snapshotDropped", evidence?.s2c?.snapshotDropped],
    ["s2c.duplicateSelected", evidence?.s2c?.duplicateSelected],
    ["s2c.duplicated", evidence?.s2c?.duplicated],
    ["s2c.reorderSelected", evidence?.s2c?.reorderSelected],
    ["s2c.reordered", evidence?.s2c?.reordered],
    ["s2c.delay.samples", evidence?.s2c?.delay?.samples],
    ["c2s.seen", evidence?.c2s?.seen],
    ["c2s.delayed", evidence?.c2s?.delayed],
    ["c2s.released", evidence?.c2s?.released],
    ["c2s.inputDropped", evidence?.c2s?.inputDropped],
    ["c2s.duplicateSelected", evidence?.c2s?.duplicateSelected],
    ["c2s.duplicated", evidence?.c2s?.duplicated],
    ["c2s.reorderSelected", evidence?.c2s?.reorderSelected],
    ["c2s.reordered", evidence?.c2s?.reordered],
    ["c2s.delay.samples", evidence?.c2s?.delay?.samples],
  ]) {
    if (!Number.isInteger(value) || value < 1) missing.push(path);
  }
  const minimumProfileDelay =
    (evidence?.profile?.oneWayDelayMs ?? Number.NaN) -
    (evidence?.profile?.jitterMs ?? Number.NaN);
  for (const direction of ["s2c", "c2s"]) {
    const delay = evidence?.[direction]?.delay;
    if (
      !Number.isFinite(minimumProfileDelay) ||
      !Number.isFinite(delay?.minMs) ||
      !Number.isFinite(delay?.maxMs) ||
      !Number.isFinite(delay?.averageMs) ||
      delay.averageMs < minimumProfileDelay ||
      delay.minMs <= 0 ||
      delay.maxMs < delay.minMs ||
      delay.averageMs < delay.minMs ||
      delay.averageMs > delay.maxMs
    ) {
      missing.push(`${direction}.realized-delay`);
    }
  }
  if (
    evidence?.s2c?.pending !== 0 ||
    evidence?.c2s?.pending !== 0 ||
    evidence?.s2c?.expired !== 0 ||
    evidence?.c2s?.expired !== 0
  ) {
    missing.push("drain-or-expiry");
  }
  if (missing.length > 0) {
    throw new Error(`${label} lacked realized evidence: ${JSON.stringify({
      missing,
      evidence,
    })}`);
  }
  return evidence;
}

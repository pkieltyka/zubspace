// Keep distributions as bounds: the server's bounded histogram does not claim
// exact percentiles. These measurements are evidence, not final performance SLOs.
export function serverTimingEvidence(status) {
  const timing = status?.tick_timing;
  if (timing?.scope !== "server-lifetime") throw new Error("missing server timing scope");
  for (const kind of ["interval", "absolute_jitter", "work"]) {
    const histogram = timing[kind];
    if (!histogram || !Number.isSafeInteger(histogram.samples) || histogram.samples <= 0 ||
        histogram.saturated !== false || !Number.isSafeInteger(histogram.min_us) ||
        !Number.isSafeInteger(histogram.max_us) || histogram.min_us < 0 || histogram.max_us < histogram.min_us) {
      throw new Error(`invalid server ${kind} timing population`);
    }
    let lower = histogram.min_us;
    let upper = histogram.min_us;
    for (const percentile of ["p50", "p95", "p99"]) {
      const bound = histogram[percentile];
      if (!bound || !Number.isSafeInteger(bound.lower_us) || !Number.isSafeInteger(bound.upper_us) ||
          bound.lower_us < lower || bound.upper_us < upper || bound.upper_us < bound.lower_us ||
          bound.upper_us > histogram.max_us) throw new Error(`invalid server ${kind}.${percentile} bounds`);
      lower = bound.lower_us;
      upper = bound.upper_us;
    }
  }
  // The server records a tick's boundary and work samples together at the end of the tick, so a
  // status snapshot - even one served while a tick is in flight - must see three populations that
  // count the same ticks. Any drift is a recording path that ran without its partner.
  if (timing.interval.samples !== timing.absolute_jitter.samples ||
      timing.work.samples !== timing.interval.samples) {
    throw new Error(
      `server timing populations do not match tick boundaries: ${JSON.stringify({
        interval: timing.interval.samples,
        absoluteJitter: timing.absolute_jitter.samples,
        work: timing.work.samples,
      })}`,
    );
  }
  return timing;
}

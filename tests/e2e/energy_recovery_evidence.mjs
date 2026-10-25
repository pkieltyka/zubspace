function predictionTracksAuthoritativeSnapshot(sample, rechargePerSecond) {
  if (
    !Number.isFinite(sample?.predicted) ||
    !Number.isFinite(sample?.server) ||
    !Number.isFinite(sample?.predictionTolerance) ||
    !Number.isFinite(rechargePerSecond) ||
    rechargePerSecond < 0
  ) return false;

  // A metrics frame can combine either ordering of two adjacent 60 Hz facts:
  // local prediction followed by a newer snapshot, or snapshot application
  // followed by newer local prediction. Preserve the product's published
  // prediction tolerance only on the predictive (lead) side, while allowing
  // the same single recharge step plus integer/f32 presentation slack in both
  // directions.
  const frameQuantization = rechargePerSecond / 60 + 2;
  return sample.predicted >= sample.server - frameQuantization &&
    sample.predicted <= sample.server + sample.predictionTolerance + frameQuantization;
}

export { predictionTracksAuthoritativeSnapshot };

// Independent planning oracle from the reference game's Random and WeaponManager.
// This only chooses a real pilot's staging point; it never injects game state.
export function referenceFragmentDirections({ x, y, count, level, frequency }) {
  let seed = (count + level - 1 + Math.round(x)*1000 + Math.round(y)*1000 + frequency) | 0;
  return Array.from({ length: count }, (_, index) => {
    seed = ((seed % 0x1f31d)*0x41a7 - Math.trunc(seed/0x1f31d)*0xb14 + 0x7b) | 0;
    if (seed < 1) seed += 0x7fffffff;
    const angle = (seed % 40000) * 9 / 1000 * Math.PI / 180;
    return { index, x: Math.sin(angle), y: -Math.cos(angle) };
  });
}

export function referenceDirectionsForSeed(seed, count) {
  // Cancel the other seed terms so the same independently implemented RNG
  // can be checked against the retained reference-game direction corpus.
  return referenceFragmentDirections({ x: 0, y: 0, count, level: 1, frequency: (seed - count) | 0 });
}

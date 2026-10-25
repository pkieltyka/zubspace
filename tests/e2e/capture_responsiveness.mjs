// Cold diagnostic only. A fresh CDP connection is an observation, never a
// fallback which turns a failed acceptance capture into a passing one.
export async function measureCaptureResponsiveness({ existing, fresh, freeze }) {
  const measure = async (operation) => {
    const startedAt = Date.now(), started = performance.now();
    try {
      const value = await operation();
      return {ok: true, startedAt, elapsedMs: performance.now() - started, value};
    } catch (error) {
      return {ok: false, startedAt, elapsedMs: performance.now() - started,
        error: String(error?.message || error).slice(0, 512)};
    }
  };
  const pair = async () => {
    const [ordinary, independent] = await Promise.all([measure(existing), measure(fresh)]);
    return {existing: ordinary, fresh: independent};
  };
  const before = await pair();
  const marker = await measure(freeze);
  const after = await pair();
  return {kind: "diagnostic-only capture responsiveness", before, marker, after};
}

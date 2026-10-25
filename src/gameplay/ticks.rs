// Canonical lives in `crate::continuum::continuum_tick_diff` — re-export so
// existing `gameplay::ticks::continuum_tick_diff` callers keep compiling.
#[allow(dead_code)]
pub(crate) use crate::continuum::continuum_tick_diff;

//! Lifetime, allocation-free timing distributions. These are observations only;
//! gameplay pacing/load shedding continues to consume its existing EWMA.
//!
//! Values 0..15 us are exact. Larger values use eight bins per power of two,
//! giving a <12.5% bucket width. Report bounds, never an invented exact percentile.

const BUCKETS: usize = 16 + 60 * 8;

pub(crate) struct TimingHistogram {
    counts: [u64; BUCKETS],
    samples: u64,
    min: u64,
    max: u64,
    saturated: bool,
    sum_us: u128,
}

impl Default for TimingHistogram {
    fn default() -> Self {
        Self {
            counts: [0; BUCKETS],
            samples: 0,
            min: 0,
            max: 0,
            saturated: false,
            sum_us: 0,
        }
    }
}

fn bucket(value: u64) -> usize {
    if value < 16 {
        return value as usize;
    }
    let exponent = 63 - value.leading_zeros() as usize;
    16 + (exponent - 4) * 8 + ((value >> (exponent - 3)) as usize - 8)
}

fn bounds(index: usize) -> (u64, u64) {
    if index < 16 {
        return (index as u64, index as u64);
    }
    let exponent = 4 + (index - 16) / 8;
    let sub = (index - 16) % 8;
    let lower = ((8 + sub) as u128) << (exponent - 3);
    let upper = (((9 + sub) as u128) << (exponent - 3)) - 1;
    (lower as u64, upper as u64)
}

impl TimingHistogram {
    pub(crate) fn record(&mut self, value_us: u64) {
        if self.samples == u64::MAX {
            self.saturated = true;
            return;
        }
        if self.samples == 0 {
            self.min = value_us;
        }
        self.min = self.min.min(value_us);
        self.max = self.max.max(value_us);
        self.samples += 1;
        self.sum_us += u128::from(value_us);
        self.counts[bucket(value_us)] += 1;
    }

    /// Exact cumulative bins at existing power-of-two bucket boundaries.
    /// No percentile approximation is converted into a Prometheus histogram.
    pub(crate) fn prometheus(&self, name: &str, help: &str, output: &mut String) {
        self.prometheus_scaled(name, help, output, 1_000_000.0);
    }

    /// The same exact bins for a value that is not microseconds, such as
    /// bytes (`divisor` 1): bucket bounds and the sum are divided by `divisor`.
    pub(crate) fn prometheus_scaled(
        &self,
        name: &str,
        help: &str,
        output: &mut String,
        divisor: f64,
    ) {
        use std::fmt::Write;
        let _ = writeln!(output, "# HELP {name} {help}\n# TYPE {name} histogram");
        for exponent in 4..=24 {
            let upper = (1_u64 << exponent) - 1;
            let count: u64 = self
                .counts
                .iter()
                .enumerate()
                .filter(|(index, _)| bounds(*index).1 <= upper)
                .map(|(_, count)| *count)
                .sum();
            let _ = writeln!(
                output,
                "{name}_bucket{{le=\"{}\"}} {count}",
                upper as f64 / divisor
            );
        }
        let _ = writeln!(
            output,
            "{name}_bucket{{le=\"+Inf\"}} {}\n{name}_count {}\n{name}_sum {}",
            self.samples,
            self.samples,
            self.sum_us as f64 / divisor
        );
    }

    fn percentile(&self, percent: u8) -> Option<(u64, u64)> {
        if self.samples == 0 {
            return None;
        }
        let rank = (u128::from(self.samples) * u128::from(percent)).div_ceil(100) as u64;
        let mut cumulative = 0_u64;
        for (index, count) in self.counts.iter().enumerate() {
            cumulative += count;
            if cumulative >= rank {
                let (lower, upper) = bounds(index);
                return Some((lower.max(self.min), upper.min(self.max)));
            }
        }
        unreachable!("recorded sample counts must match histogram membership")
    }

    pub(crate) fn summary(&self) -> serde_json::Value {
        let percentile = |percent| {
            self.percentile(percent)
                .map(|(lower, upper)| serde_json::json!({ "lower_us": lower, "upper_us": upper }))
        };
        serde_json::json!({
            "samples": self.samples, "saturated": self.saturated,
            "min_us": (self.samples != 0).then_some(self.min),
            "max_us": (self.samples != 0).then_some(self.max),
            "p50": percentile(50), "p95": percentile(95), "p99": percentile(99),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_bucket_edges_are_contiguous_and_round_trip() {
        let mut next = 0_u128;
        for index in 0..BUCKETS {
            let (lower, upper) = bounds(index);
            assert_eq!(u128::from(lower), next);
            assert_eq!(bucket(lower), index);
            assert_eq!(bucket(upper), index);
            next = u128::from(upper) + 1;
        }
        assert_eq!(next, u128::from(u64::MAX) + 1);
        assert!(std::mem::size_of::<TimingHistogram>() <= 4_096);
    }

    #[test]
    fn bounded_percentiles_contain_independently_sorted_exact_ranks() {
        let mut histogram = TimingHistogram::default();
        assert_eq!(histogram.summary()["p95"], serde_json::Value::Null);
        let mut values = vec![0, 1, 15, 16, 16_667, u64::MAX];
        values.extend((0_u64..10_000).map(|i| i.wrapping_mul(6364136223846793005)));
        for value in &values {
            histogram.record(*value);
        }
        values.sort_unstable();
        for percent in [50, 95, 99] {
            let exact = values[(values.len() * percent as usize).div_ceil(100) - 1];
            let (lower, upper) = histogram.percentile(percent).unwrap();
            assert!((lower..=upper).contains(&exact));
        }
        assert_eq!(histogram.summary()["samples"], values.len());
        assert_eq!(histogram.min, 0);
        assert_eq!(histogram.max, u64::MAX);
    }

    #[test]
    fn constant_samples_have_exact_bounds_and_saturation_is_explicit() {
        let mut histogram = TimingHistogram::default();
        for _ in 0..100 {
            histogram.record(16_667);
        }
        assert_eq!(histogram.percentile(99), Some((16_667, 16_667)));
        histogram.samples = u64::MAX;
        histogram.record(0);
        assert!(histogram.saturated);
        assert_eq!(histogram.min, 16_667);
        assert_eq!(histogram.max, 16_667);
    }
}

//! Phase 5 verification: synthetic per-packet outbound drop for the
//! two-client combat playtest under simulated WebRTC packet loss.
//!
//! `SimulatedTxLossSettings` is immutable configuration data. Each
//! `ServerState` builds its own `SimulatedTxLoss` runtime controller from those
//! settings, then shares that one controller with the state's client handles.
//! Cloned configurations therefore never share decision sequences or counters.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

struct SimulatedTxLossInner {
    settings: SimulatedTxLossSettings,
    resolved_seed: AtomicU64,
    sequence: AtomicU64,
    packets_dropped: AtomicU64,
    bytes_dropped: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SimulatedTxLossSettings {
    probability_millionths: u32,
    configured_seed: Option<u64>,
}

impl SimulatedTxLossSettings {
    pub(crate) fn new(probability_millionths: u32, configured_seed: Option<u64>) -> Self {
        Self {
            probability_millionths: probability_millionths.min(1_000_000),
            configured_seed,
        }
    }

    pub(crate) fn with_probability_millionths(self, probability_millionths: u32) -> Self {
        Self::new(probability_millionths, self.configured_seed)
    }

    pub(crate) fn with_seed(self, seed: Option<u64>) -> Self {
        Self::new(self.probability_millionths, seed)
    }

    pub(crate) fn probability_millionths(self) -> u32 {
        self.probability_millionths
    }

    pub(crate) fn configured_seed(self) -> Option<u64> {
        self.configured_seed
    }

    pub(crate) fn controller(self) -> SimulatedTxLoss {
        SimulatedTxLoss::new(self)
    }
}

#[derive(Clone)]
pub(crate) struct SimulatedTxLoss(Arc<SimulatedTxLossInner>);

impl SimulatedTxLoss {
    pub(crate) fn disabled() -> Self {
        SimulatedTxLossSettings::default().controller()
    }

    fn new(settings: SimulatedTxLossSettings) -> Self {
        Self(Arc::new(SimulatedTxLossInner {
            settings,
            resolved_seed: AtomicU64::new(settings.configured_seed().unwrap_or(0)),
            sequence: AtomicU64::new(0),
            packets_dropped: AtomicU64::new(0),
            bytes_dropped: AtomicU64::new(0),
        }))
    }

    fn probability_millionths(&self) -> u32 {
        self.0.settings.probability_millionths()
    }

    fn configured_seed(&self) -> Option<u64> {
        self.0.settings.configured_seed()
    }

    pub(crate) fn should_drop(&self, packet_len: usize) -> bool {
        let probability = self.probability_millionths() as f64 / 1_000_000.0;
        if probability <= 0.0 {
            return false;
        }
        let sequence = self.0.sequence.fetch_add(1, Ordering::Relaxed);
        if deterministic_loss_roll(self.resolved_seed(), sequence) < probability {
            self.0.packets_dropped.fetch_add(1, Ordering::Relaxed);
            self.0
                .bytes_dropped
                .fetch_add(packet_len as u64, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub(crate) fn drain_counts(&self) -> (u64, u64) {
        let packets = self.0.packets_dropped.swap(0, Ordering::Relaxed);
        let bytes = self.0.bytes_dropped.swap(0, Ordering::Relaxed);
        (packets, bytes)
    }

    fn resolved_seed(&self) -> u64 {
        if let Some(seed) = self.configured_seed() {
            return seed;
        }
        let current = self.0.resolved_seed.load(Ordering::Relaxed);
        if current != 0 {
            return current;
        }
        let generated = fresh_seed().max(1);
        match self.0.resolved_seed.compare_exchange(
            0,
            generated,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => generated,
            Err(seed) => seed,
        }
    }
}

impl Default for SimulatedTxLoss {
    fn default() -> Self {
        Self::disabled()
    }
}

impl PartialEq for SimulatedTxLoss {
    fn eq(&self, other: &Self) -> bool {
        self.probability_millionths() == other.probability_millionths()
            && self.configured_seed() == other.configured_seed()
    }
}

impl Eq for SimulatedTxLoss {}

impl fmt::Debug for SimulatedTxLoss {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SimulatedTxLoss")
            .field("probability_millionths", &self.probability_millionths())
            .field("configured_seed", &self.configured_seed())
            .finish_non_exhaustive()
    }
}

fn fresh_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    nanos ^ u64::from(std::process::id())
}

fn deterministic_loss_roll(seed: u64, sequence: u64) -> f64 {
    // SplitMix64 gives a stable, well-distributed value for each monotonically
    // increasing send decision without shared mutable PRNG state.
    let mut value = seed.wrapping_add(sequence.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    (value >> 11) as f64 / (1_u64 << 53) as f64
}

/// Decide whether to drop one outbound packet of the given size.
/// Returns `true` to drop; the caller skips the `tx.send` and the
/// counters in this module bump for the periodic summary. A seeded sequence
/// keeps browser loss gates reproducible while an automatically generated
/// process seed preserves varied manual QA when no seed is configured.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_returns_and_resets_counters() {
        let loss = SimulatedTxLossSettings::new(1_000_000, Some(7)).controller();
        assert!(loss.should_drop(1_024));
        let (packets, bytes) = loss.drain_counts();
        assert_eq!(packets, 1);
        assert_eq!(bytes, 1_024);
        let (packets2, bytes2) = loss.drain_counts();
        assert_eq!(packets2, 0, "drain must reset");
        assert_eq!(bytes2, 0);
    }

    #[test]
    fn disabled_and_enabled_instances_are_independent_in_one_process() {
        let disabled = SimulatedTxLoss::disabled();
        let enabled = SimulatedTxLossSettings::new(1_000_000, Some(99)).controller();
        assert!(!disabled.should_drop(100));
        assert!(enabled.should_drop(100));
        assert_eq!(disabled.drain_counts(), (0, 0));
        assert_eq!(enabled.drain_counts(), (1, 100));
    }

    #[test]
    fn seeded_loss_roll_is_reproducible_and_well_distributed() {
        let first: Vec<_> = (0..10_000)
            .map(|sequence| deterministic_loss_roll(141_421, sequence))
            .collect();
        let second: Vec<_> = (0..10_000)
            .map(|sequence| deterministic_loss_roll(141_421, sequence))
            .collect();
        assert_eq!(first, second);
        let drops = first.iter().filter(|&&roll| roll < 0.20).count();
        assert!(
            (1_800..=2_200).contains(&drops),
            "20% roll yielded {drops} drops"
        );
    }

    #[test]
    fn configured_zero_seed_is_preserved() {
        let loss = SimulatedTxLossSettings::new(500_000, Some(0)).controller();
        assert_eq!(loss.resolved_seed(), 0);
    }
}

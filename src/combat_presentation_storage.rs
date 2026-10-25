//! Lossless internal retention format, not a gameplay wire format.
//! Byte storage avoids Rust identity/struct padding without unaligned references,
//! truncating identifiers, or introducing a separately mutable identity registry.
use super::*;
use crate::authority_identity::{LifeIdentity, PlayerSessionId, PlayerSessionIdentity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StoredSample([u8; 52]);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredFrame {
    pub(super) observer: PlayerRuntimeIdentity,
    pub(super) arena_stream_id: u64,
    pub(crate) snapshot_id: u16,
    pub(super) server_tick: u32,
    pub(super) actors: std::sync::Arc<[StoredSample]>,
}

impl StoredFrame {
    pub(crate) fn samples(&self) -> impl Iterator<Item = ObserverPresentationSampleV1> + '_ {
        self.actors.iter().map(|sample| sample.sample())
    }

    pub(super) fn from_validated(frame: ObserverPresentationFrameV1) -> Self {
        Self {
            observer: frame.observer,
            arena_stream_id: frame.arena_stream_id,
            snapshot_id: frame.snapshot_id,
            server_tick: frame.server_tick,
            // Samples never mutate after admission. Transactional ClientHandle
            // clones share this allocation, while frame queues and scope remain
            // independently owned. Retain exactly len samples, not spare capacity.
            actors: frame
                .actors
                .into_iter()
                .map(|sample| {
                    StoredSample::from_sample(sample).expect("validated presentation sample")
                })
                .collect(),
        }
    }

    pub(super) fn matches(&self, frame: &ObserverPresentationFrameV1) -> bool {
        self.observer == frame.observer
            && self.arena_stream_id == frame.arena_stream_id
            && self.snapshot_id == frame.snapshot_id
            && self.server_tick == frame.server_tick
            && self.actors.len() == frame.actors.len()
            && self
                .actors
                .iter()
                .zip(&frame.actors)
                .all(|(stored, sample)| stored.sample() == *sample)
    }

    pub(super) fn sample_for_segment(
        &self,
        segment: MotionSegmentIdentity,
    ) -> Option<ObserverPresentationSampleV1> {
        self.stored_sample_for_segment(segment)
            .map(|sample| sample.sample())
    }

    pub(super) fn stored_sample_for_segment(
        &self,
        segment: MotionSegmentIdentity,
    ) -> Option<&StoredSample> {
        // Admission already sorts and rejects duplicate exact runtimes. Search
        // that immutable order; never select by reusable numeric id alone.
        let index = self
            .actors
            .binary_search_by_key(&segment.actor, |sample| sample.actor())
            .ok()?;
        let sample = &self.actors[index];
        (u64::from_le_bytes(sample.0[16..24].try_into().unwrap()) == segment.arena_stream_id
            && u32::from_le_bytes(sample.0[24..28].try_into().unwrap())
                == segment.discontinuity_generation)
            .then_some(sample)
    }
}

impl StoredSample {
    pub(super) fn sample_time(&self) -> PresentationSampleTime {
        PresentationSampleTime {
            tick: u32::from_le_bytes(self.0[28..32].try_into().unwrap()),
            third: self.0[32],
        }
    }
    pub(super) fn from_sample(sample: ObserverPresentationSampleV1) -> Option<Self> {
        if sample.actor != sample.motion_segment.actor || sample.motion_segment.arena_stream_id == 0
        {
            return None;
        }
        let mut bytes = [0; 52];
        bytes[0..8].copy_from_slice(&sample.actor.session.session_id.get().to_le_bytes());
        bytes[8..10].copy_from_slice(&sample.actor.session.player_id.to_le_bytes());
        bytes[10..12].copy_from_slice(&sample.actor.life.player_id.to_le_bytes());
        bytes[12..16].copy_from_slice(&sample.actor.life.life_seq.to_le_bytes());
        bytes[16..24].copy_from_slice(&sample.motion_segment.arena_stream_id.to_le_bytes());
        bytes[24..28]
            .copy_from_slice(&sample.motion_segment.discontinuity_generation.to_le_bytes());
        bytes[28..32].copy_from_slice(&sample.sample_time.tick.to_le_bytes());
        bytes[32] = sample.sample_time.third;
        bytes[33..37].copy_from_slice(&sample.x_q8.to_le_bytes());
        bytes[37..41].copy_from_slice(&sample.y_q8.to_le_bytes());
        bytes[41..45].copy_from_slice(&sample.velocity_x_q8.to_le_bytes());
        bytes[45..49].copy_from_slice(&sample.velocity_y_q8.to_le_bytes());
        bytes[49..51].copy_from_slice(&sample.orientation_u16.to_le_bytes());
        bytes[51] = u8::from(sample.snap_discontinuity);
        Some(Self(bytes))
    }

    pub(super) fn actor(self) -> PlayerRuntimeIdentity {
        PlayerRuntimeIdentity::new(
            PlayerSessionIdentity::new(
                u16::from_le_bytes(self.0[8..10].try_into().unwrap()),
                PlayerSessionId::new(u64::from_le_bytes(self.0[0..8].try_into().unwrap())),
            ),
            LifeIdentity::new(
                u16::from_le_bytes(self.0[10..12].try_into().unwrap()),
                u32::from_le_bytes(self.0[12..16].try_into().unwrap()),
            ),
        )
    }

    pub(super) fn sample(self) -> ObserverPresentationSampleV1 {
        let actor = self.actor();
        ObserverPresentationSampleV1 {
            actor,
            motion_segment: MotionSegmentIdentity {
                actor,
                arena_stream_id: u64::from_le_bytes(self.0[16..24].try_into().unwrap()),
                discontinuity_generation: u32::from_le_bytes(self.0[24..28].try_into().unwrap()),
            },
            sample_time: PresentationSampleTime {
                tick: u32::from_le_bytes(self.0[28..32].try_into().unwrap()),
                third: self.0[32],
            },
            x_q8: i32::from_le_bytes(self.0[33..37].try_into().unwrap()),
            y_q8: i32::from_le_bytes(self.0[37..41].try_into().unwrap()),
            velocity_x_q8: i32::from_le_bytes(self.0[41..45].try_into().unwrap()),
            velocity_y_q8: i32::from_le_bytes(self.0[45..49].try_into().unwrap()),
            orientation_u16: u16::from_le_bytes(self.0[49..51].try_into().unwrap()),
            snap_discontinuity: self.0[51] != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_retention_matches_linear_exact_identity_selection() {
        let actors: Vec<_> = (0..128_u32)
            .map(|index| {
                let actor = PlayerRuntimeIdentity::new(
                    PlayerSessionIdentity::new(
                        (index % 4) as u16,
                        PlayerSessionId::new(u64::MAX - index as u64),
                    ),
                    LifeIdentity::new((index % 4) as u16, u32::MAX - index),
                );
                ObserverPresentationSampleV1 {
                    actor,
                    motion_segment: MotionSegmentIdentity {
                        actor,
                        arena_stream_id: u64::MAX - index as u64,
                        discontinuity_generation: index + 1,
                    },
                    sample_time: PresentationSampleTime {
                        tick: 0x7fff_ffff,
                        third: (index % 3) as u8,
                    },
                    x_q8: index as i32,
                    y_q8: -(index as i32),
                    velocity_x_q8: i32::MAX,
                    velocity_y_q8: i32::MIN,
                    orientation_u16: index as u16,
                    snap_discontinuity: index % 2 == 0,
                }
            })
            .collect();
        let mut frame = ObserverPresentationFrameV1 {
            observer: actors[0].actor,
            arena_stream_id: 1,
            snapshot_id: u16::MAX,
            server_tick: 0x7fff_ffff,
            actors,
        };
        frame.actors.sort_unstable_by_key(|sample| sample.actor);
        let stored = StoredFrame::from_validated(frame.clone());
        assert!(stored.matches(&frame));
        assert_eq!(stored.samples().collect::<Vec<_>>(), frame.actors);
        for sample in &frame.actors {
            let mut queries = [sample.motion_segment; 4];
            queries[1].discontinuity_generation ^= 1;
            queries[2].arena_stream_id ^= 1;
            queries[3].actor.session.session_id = PlayerSessionId::new(0);
            for query in queries {
                let linear = frame
                    .actors
                    .iter()
                    .copied()
                    .find(|sample| sample.motion_segment == query);
                assert_eq!(stored.sample_for_segment(query), linear);
                let selected = stored.stored_sample_for_segment(query);
                assert_eq!(selected.map(|sample| sample.sample()), linear);
                assert_eq!(
                    selected.map(StoredSample::sample_time),
                    linear.map(|sample| sample.sample_time)
                );
            }
        }
        frame.actors[0].orientation_u16 ^= 1;
        assert!(
            !stored.matches(&frame),
            "duplicate comparison retains non-collision fields too"
        );
    }

    #[test]
    fn compact_retention_preserves_every_identity_time_and_geometry_bit() {
        assert_eq!(std::mem::size_of::<StoredSample>(), 52);
        let mut seed = 0x789a_bcef_1234_5678_u64;
        for index in 0..10_000_u32 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let actor = PlayerRuntimeIdentity::new(
                PlayerSessionIdentity::new(seed as u16, PlayerSessionId::new(seed)),
                // Preserve even differing player ids; storage is not a new
                // validator or a place to normalize an existing typed value.
                LifeIdentity::new((seed >> 16) as u16, (seed >> 32) as u32),
            );
            let sample = ObserverPresentationSampleV1 {
                actor,
                motion_segment: MotionSegmentIdentity {
                    actor,
                    arena_stream_id: seed.max(1),
                    discontinuity_generation: !index,
                },
                sample_time: PresentationSampleTime {
                    tick: seed as u32 & 0x7fff_ffff,
                    third: (index % 3) as u8,
                },
                x_q8: seed as i32,
                y_q8: !(seed as i32),
                velocity_x_q8: (seed >> 32) as i32,
                velocity_y_q8: !((seed >> 32) as i32),
                orientation_u16: (seed >> 48) as u16,
                snap_discontinuity: index % 2 == 0,
            };
            let stored = StoredSample::from_sample(sample).unwrap();
            assert_eq!(stored.actor(), actor);
            assert_eq!(stored.sample(), sample);
            let mut invalid = sample;
            invalid.motion_segment.actor.life.life_seq ^= 1;
            assert!(StoredSample::from_sample(invalid).is_none());
            invalid = sample;
            invalid.motion_segment.arena_stream_id = 0;
            assert!(StoredSample::from_sample(invalid).is_none());
        }
    }
}

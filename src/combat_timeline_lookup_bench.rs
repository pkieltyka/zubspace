//! Explicit capacity measurement, separate from real RTC session capacity.
//! No timing threshold is invented here: retain the release baseline first.
use super::*;
use std::{hint::black_box, mem::size_of, time::Instant};

struct Fixture {
    motion: Vec<ActorMotionHistory>,
    hull_samples: Vec<Vec<ActorMotionSampleV1>>,
    observers: Vec<ObserverPresentationHistory>,
}

fn fixture(count: usize) -> Fixture {
    use crate::authority_identity::{LifeIdentity, PlayerSessionId, PlayerSessionIdentity};
    let actors: Vec<_> = (0..count)
        .map(|id| {
            PlayerRuntimeIdentity::new(
                PlayerSessionIdentity::new(id as u16, PlayerSessionId::new(id as u64 + 1)),
                LifeIdentity::new(id as u16, 1),
            )
        })
        .collect();
    let mut motion = Vec::new();
    for (id, actor) in actors.iter().enumerate() {
        let mut history = ActorMotionHistory::new();
        for tick in 1000..=1064 {
            assert_eq!(
                history.append_state(
                    *actor,
                    1,
                    tick,
                    id as f32 * 16.0 + (tick - 1000) as f32 * 0.125,
                    200.0,
                    12.5,
                    0.0,
                    0.0,
                    14.0,
                    0,
                    if id % 2 == 0 {
                        ActorMotionSourceKind::AcceptedHumanWitness
                    } else {
                        ActorMotionSourceKind::ServerBotSimulation
                    }
                ),
                HistoryAppendDisposition::Applied
            );
        }
        motion.push(history);
    }
    let hull_samples = motion
        .iter()
        .map(|history| history.current_samples().collect())
        .collect();
    let observers = actors
        .iter()
        .map(|observer| {
            let mut history = ObserverPresentationHistory::new();
            history.begin_scope(*observer, 1);
            for index in 0..OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS {
                let tick = 1002 + index as u32 * 2;
                let samples = motion
                    .iter()
                    .map(|motion| {
                        let sample = motion
                            .at_or_before(motion.current_segment().unwrap(), tick)
                            .unwrap();
                        ObserverPresentationSampleV1 {
                            sample_time: PresentationSampleTime::from_tick(tick),
                            actor: sample.segment.actor,
                            motion_segment: sample.segment,
                            x_q8: sample.x_q8,
                            y_q8: sample.y_q8,
                            velocity_x_q8: sample.velocity_x_q8,
                            velocity_y_q8: sample.velocity_y_q8,
                            orientation_u16: 0,
                            snap_discontinuity: false,
                        }
                    })
                    .collect();
                assert_eq!(
                    history.observe_emitted(ObserverPresentationFrameV1 {
                        observer: *observer,
                        arena_stream_id: 1,
                        snapshot_id: index as u16 + 1,
                        server_tick: tick,
                        actors: samples,
                    }),
                    ObserverPresentationAppendDisposition::Applied
                );
            }
            history
        })
        .collect();
    Fixture {
        motion,
        hull_samples,
        observers,
    }
}

fn query(
    f: &Fixture,
    observer: usize,
    actor: usize,
    tick: u32,
) -> Result<PresentationGeometryAnchorV1, PresentationProjectionUnavailable> {
    PresentationGeometryResolver::resolve_from_samples(
        &f.observers[observer],
        f.motion[actor].current_segment().unwrap(),
        tick,
        OBSERVER_PRESENTATION_HISTORY_SNAPSHOTS as u16,
        &f.hull_samples[actor],
    )
}

#[test]
fn history_lookup_workload_checks_exact_interpolated_held_and_missing_results() {
    let f = fixture(3);
    for observer in 0..3 {
        for actor in 0..3 {
            for (tick, disposition, source_tick) in [
                (1060, PresentationProjectionDisposition::Exact, 1060),
                (1059, PresentationProjectionDisposition::Interpolated, 1059),
                (
                    1065,
                    PresentationProjectionDisposition::HeldNoUpperSample,
                    1064,
                ),
            ] {
                let result = query(&f, observer, actor, tick).unwrap();
                assert_eq!(result.disposition, disposition);
                assert_eq!(
                    result.x_q8,
                    actor as i32 * 16 * 256 + (source_tick - 1000) * 32
                );
            }
            assert_eq!(
                query(&f, observer, actor, 999),
                Err(PresentationProjectionUnavailable::BeforeFirstSample)
            );
        }
    }
}

#[test]
#[ignore = "explicit release capacity measurement: make bench-history-lookup"]
fn history_lookup_capacity_benchmark() {
    assert!(
        !cfg!(debug_assertions),
        "measure optimized production code with --release"
    );
    const ACTORS: usize = 200;
    const REPEATS: usize = 7;
    let f = fixture(ACTORS);
    // Allocated backing storage, including the production hull-sample copies.
    // This excludes allocator metadata, unrelated client state and RTC buffers.
    let motion_bytes: usize = f
        .motion
        .iter()
        .map(|h| {
            size_of::<ActorMotionHistory>()
                + h.samples.capacity() * size_of::<ActorMotionSampleV1>()
                + h.retired_segments.capacity() * size_of::<MotionSegmentIdentity>()
        })
        .sum();
    let hull_bytes: usize = f
        .hull_samples
        .iter()
        .map(|h| {
            size_of::<Vec<ActorMotionSampleV1>>() + h.capacity() * size_of::<ActorMotionSampleV1>()
        })
        .sum();
    let presentation_bytes: usize = f
        .observers
        .iter()
        .map(|h| {
            size_of::<ObserverPresentationHistory>()
                + h.frames.capacity() * size_of::<presentation_storage::StoredFrame>()
                + h.frames
                    .iter()
                    .map(|frame| {
                        2 * size_of::<usize>() // Arc strong/weak counts
                            + frame.actors.len() * size_of::<presentation_storage::StoredSample>()
                    })
                    .sum::<usize>()
        })
        .sum();
    println!(
        "history_lookup_memory actors={ACTORS} observers={ACTORS} motion_bytes={motion_bytes} hull_copy_bytes={hull_bytes} presentation_bytes={presentation_bytes}"
    );
    assert_eq!(
        presentation_bytes,
        ObserverPresentationHistory::dense_retention_bytes(ACTORS, ACTORS)
    );
    assert!(
        presentation_bytes <= 64 * 1024 * 1024,
        "retain the approved presentation budget"
    );
    for (name, tick) in [
        ("exact", 1060),
        ("interpolated", 1059),
        ("held", 1065),
        ("missing", 999),
    ] {
        black_box(query(&f, 0, 0, tick)).ok();
        let mut batches = Vec::new();
        let mut hits = 0;
        let mut misses = 0;
        for _ in 0..REPEATS {
            let start = Instant::now();
            for observer in 0..ACTORS {
                for actor in 0..ACTORS {
                    match black_box(query(black_box(&f), observer, actor, black_box(tick))) {
                        Ok(_) => hits += 1,
                        Err(PresentationProjectionUnavailable::BeforeFirstSample)
                            if tick == 999 =>
                        {
                            misses += 1
                        }
                        error => panic!("unexpected capacity-query outcome: {error:?}"),
                    }
                }
            }
            batches.push(start.elapsed().as_nanos());
        }
        batches.sort_unstable();
        let queries = ACTORS * ACTORS * REPEATS;
        assert_eq!(hits + misses, queries);
        assert_eq!(misses, if tick == 999 { queries } else { 0 });
        let mean_ns_per_query = batches.iter().sum::<u128>() / queries as u128;
        println!(
            "history_lookup_bench workload={name} actors={ACTORS} observers={ACTORS} queries={queries} hits={hits} misses={misses} mean_ns_per_query={mean_ns_per_query} batch_median_ns={} batch_max_ns={}",
            batches[REPEATS / 2],
            batches[REPEATS - 1]
        );
    }
}

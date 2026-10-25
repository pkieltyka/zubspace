//! Native-only diagnostic over the production resolver. Inactive in ordinary
//! tests and absent from shipped builds; never changes a resolved result.
use crate::authority_identity::PlayerRuntimeIdentity;
use crate::combat_timeline::{MotionSegmentIdentity, ProjectilePresentationQueryV1};
use std::cell::RefCell;
use std::collections::HashSet;
use std::time::Instant;

const MAX_KEYS_PER_STEP: usize = 8_192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    observer: PlayerRuntimeIdentity,
    arena_stream_id: u64,
    target: MotionSegmentIdentity,
    accepted_generation: u32,
    effective_generation: u32,
    render_tick: u32,
    applied_snapshot_id: u16,
    advanced_watermark: bool,
}

#[derive(Default)]
struct Measurement {
    keys: HashSet<Key>,
    steps: u64,
    calls: u64,
    repeated: u64,
    unique: u64,
    unclassified_at_capacity: u64,
    resolve_ns: u128,
}

thread_local! {
    static ACTIVE: RefCell<Option<Measurement>> = const { RefCell::new(None) };
}

pub(crate) fn begin_step() {
    ACTIVE.with(|active| {
        if let Some(measurement) = active.borrow_mut().as_mut() {
            measurement.keys.clear();
            measurement.steps += 1;
        }
    });
}

pub(crate) fn resolve<T>(
    observer: PlayerRuntimeIdentity,
    arena_stream_id: u64,
    target: MotionSegmentIdentity,
    query: ProjectilePresentationQueryV1,
    resolve: impl FnOnce() -> T,
) -> T {
    let enabled = ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let Some(measurement) = active.as_mut() else {
            return false;
        };
        measurement.calls += 1;
        let key = Key {
            observer,
            arena_stream_id,
            target,
            accepted_generation: query.accepted_timeline_generation,
            effective_generation: query.effective_timeline_generation,
            render_tick: query.render_tick,
            applied_snapshot_id: query.applied_snapshot_id,
            advanced_watermark: query.server_advanced_watermark,
        };
        if measurement.keys.contains(&key) {
            measurement.repeated += 1;
        } else if measurement.keys.len() < MAX_KEYS_PER_STEP {
            measurement.keys.insert(key);
            measurement.unique += 1;
        } else {
            measurement.unclassified_at_capacity += 1;
        }
        true
    });
    if !enabled {
        return resolve();
    }
    let started = Instant::now();
    let result = resolve();
    let elapsed = started.elapsed().as_nanos();
    ACTIVE.with(|active| active.borrow_mut().as_mut().unwrap().resolve_ns += elapsed);
    result
}

pub(crate) fn measure(run: impl FnOnce()) -> serde_json::Value {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            ACTIVE.with(|active| {
                active.borrow_mut().take();
            });
        }
    }
    let _clear = Clear;
    ACTIVE.with(|active| {
        assert!(active.borrow().is_none(), "nested projection measurement");
        *active.borrow_mut() = Some(Measurement::default());
    });
    run();
    let m = ACTIVE.with(|active| active.borrow_mut().take().unwrap());
    serde_json::json!({
        "measurement": "production_projection_reuse_before_cache",
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "steps": m.steps, "resolverCalls": m.calls, "repeatedKeys": m.repeated,
        "uniqueKeys": m.unique, "unclassifiedAtCapacity": m.unclassified_at_capacity,
        "keyCapacityPerStep": MAX_KEYS_PER_STEP, "resolverNs": m.resolve_ns,
        "scope": "admitted native combat, not live WebRTC capacity or browser acceptance"
    })
}

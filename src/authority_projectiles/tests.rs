use std::time::Duration;

use super::*;

fn projectile(owner_id: u16, weapon_data: u16, age: Duration) -> ProjectileRecord {
    let now_tick = 20_000;
    let age_ticks = (age.as_millis() / 10).min(u128::from(u32::MAX)) as u32;
    ProjectileRecord {
        id: 1,
        shot_group_id: 1,
        shot_index: 0,
        owner_id,
        owner_life_seq: 1,
        combat_timeline: None,
        presentation_source: None,
        weapon_data,
        x: 500,
        y: 700,
        timestamp: 0x1122_3344,
        created_tick: crate::continuum::continuum_tick_sub(now_tick, age_ticks),
        velocity_x: 0,
        velocity_y: 0,
        expiry_tick: None,
        pos_x: 500.0,
        source_server_tick: None,
        precise_velocity: None,
        pos_y: 700.0,
        bounces_remaining: 0,
        behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
        proximity_target: None,
        proximity_sensor_end_tick: None,
        proximity_highest_offset: 0.0,
    }
}

fn shrapnel_parent(count: u8, bouncing: bool) -> (ProjectileRecord, ExplosionFragmentTuning) {
    let weapon_data = crate::gameplay_rules::encode_weapon_data(3, 2, bouncing, 3, count, false);
    let mut parent = projectile(1, weapon_data, Duration::ZERO);
    parent.presentation_source = Some(crate::realtime_event::RealtimeWeaponEvent {
        source_player_id: parent.owner_id,
        source_session_id: 100,
        source_life_seq: parent.owner_life_seq,
        shot_group_id: parent.shot_group_id,
        x: 500.0,
        y: 700.0,
        velocity_x: 3.4,
        velocity_y: -8.25,
        orientation: 0.25,
        frequency: 1,
        ship: 0,
        weapon_data,
    });
    let tuning = ExplosionFragmentTuning {
        initial_projectile: crate::gameplay_rules::ProjectileInitializationTuning {
            bullet_speed: 300.0,
            bomb_speed: 200.0,
            burst_speed: 250.0,
            ship_radius: 14.0,
            multifire_angle: 500,
            double_barrel: false,
            burst_count: 8,
            bomb_bounce_count: 0,
        },
        speed: 100.0,
        alive_ticks: 100,
        random: false,
    };
    (parent, tuning)
}

#[test]
fn hostile_brick_solidity_preserves_each_weapon_trait_and_captured_frequency() {
    use crate::gameplay_rules::ProjectileBehavior;
    let cases = [
        (1, ProjectileBehavior::Standard, 1, 0, false),
        (2, ProjectileBehavior::BouncingBullet, 1, 1, false),
        (3, ProjectileBehavior::Standard, 1, 0, true),
        (4, ProjectileBehavior::Standard, 1, 0, true),
        (7, ProjectileBehavior::Burst { active: false }, 1, 1, false),
        (8, ProjectileBehavior::Standard, 1, 1, false),
        (15, ProjectileBehavior::BouncingBullet, 1, 0, false),
        (15, ProjectileBehavior::BouncingBullet, 26, 1, false),
    ];
    for (kind, behavior, age, hostile_remaining, wall_explosion) in cases {
        for wall_team in [1, 2] {
            let (mut record, _) = shrapnel_parent(4, true);
            record.weapon_data =
                crate::gameplay_rules::encode_weapon_data(kind, 2, true, 3, 4, false);
            if kind != 15 {
                record.presentation_source.as_mut().unwrap().weapon_data = record.weapon_data;
            }
            record.pos_x = 100.0;
            record.pos_y = 100.0;
            record.velocity_x = 1000;
            record.velocity_y = 0;
            record.behavior = behavior;
            record.created_tick = 101 - age;
            record.expiry_tick = Some(1000);
            let mut registry = ProjectileRegistry::default();
            registry.record(record, 128, 1024);
            // The source changed frequency after firing. The weapon must
            // still use captured team 1 for brick and player collision.
            let players = [shooter_hull(1, 99, 500.0, 500.0)];
            let mut step = |tick| {
                registry.step_with_frequency_aware_solidity_transactional(
                    tick,
                    |frequency, x, _| {
                        assert_eq!(frequency, 1);
                        frequency != wall_team && x >= 100.5
                    },
                    &players,
                    3.0,
                    0,
                    64.0,
                    |_, _, _| {
                        crate::combat_timeline::ProjectilePresentationWitnessV1::server_native()
                    },
                    |_| true,
                )
            };
            assert!(step(100).is_empty());
            assert!(step(101).is_empty());
            let remaining = if wall_team == 1 { 1 } else { hostile_remaining };
            assert_eq!(
                registry.len(),
                remaining,
                "kind {kind}, age {age}, wall team {wall_team}"
            );
            assert_eq!(
                registry.pending_wall_explosions().count(),
                usize::from(wall_team == 2 && wall_explosion)
            );
            if let Some(record) = registry.last() {
                if wall_team == 1 || kind == 8 {
                    assert_eq!(record.pos_x, 101.0);
                    assert_eq!(record.velocity_x, 1000);
                } else {
                    assert_eq!(record.pos_x, 100.0);
                    assert_eq!(record.velocity_x, -1000);
                    if kind == 7 {
                        assert_eq!(record.behavior, ProjectileBehavior::Burst { active: true });
                    }
                }
            }
        }
    }
}

#[test]
fn retired_source_fragments_use_captured_identity_without_a_live_owner_hull() {
    for trusted_source in [true, false] {
        let (parent, tuning) = shrapnel_parent(4, false);
        let mut next_id = 100;
        let mut children = build_shrapnel_records(
            &parent,
            tuning,
            crate::gameplay_rules::RulesVector::new(100.0, 100.0),
            100,
            |_, _, _| false,
            || {
                next_id += 1;
                next_id
            },
        );
        if !trusted_source {
            for child in &mut children {
                child.presentation_source = None;
            }
        }
        let mut registry = ProjectileRegistry::default();
        registry.record_group(children, 128, 1024);
        let mut successor = shooter_hull(1, 99, 500.0, 500.0);
        successor.life_seq = 2;
        let mut victim = shooter_hull(2, 2, 100.0, 50.0);
        victim.lag_compensation_ticks = 0;
        add_exact_motion_sample(
            &mut victim,
            150,
            100.0,
            50.0,
            crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
        );
        let players = [successor, victim];
        registry.step(100, no_collision, &players);
        let hits = registry.step(150, no_collision, &players);
        if trusted_source {
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].owner_identity(), parent.owner_identity());
            assert_eq!(hits[0].victim_id, 2);
            assert_eq!(registry.len(), 3);
        } else {
            assert!(
                hits.is_empty(),
                "a reused numeric owner id cannot authenticate a carrier-less old flight"
            );
            assert_eq!(registry.len(), 0);
        }
    }
}

#[test]
fn shrapnel_records_use_immutable_launch_and_own_wrapping_lifetime() {
    let (mut parent, mut tuning) = shrapnel_parent(8, true);
    tuning.random = true;
    let position = crate::gameplay_rules::RulesVector::new(100.5, 200.25);
    let tick = 0x7fff_ffce; // +100 wraps to 50.
    parent.expiry_tick = Some(tick + 1);
    parent.proximity_target = Some(crate::authority_identity::LifeIdentity::new(9, 1));
    parent.proximity_sensor_end_tick = Some(tick);
    let mut next_id = 100;
    let children = build_shrapnel_records(
        &parent,
        tuning,
        position,
        tick,
        |_, _, frequency| {
            assert_eq!(frequency, 1);
            false
        },
        || {
            next_id += 1;
            next_id
        },
    );
    assert_eq!(children.len(), 8);
    for (index, child) in children.iter().enumerate() {
        assert_eq!(child.id, 101 + index as u32);
        assert_eq!(child.shot_index, index as u8);
        assert_eq!(child.owner_identity(), parent.owner_identity());
        assert_eq!(child.shot_group_id, parent.shot_group_id);
        assert_eq!(child.presentation_source, parent.presentation_source);
        assert_eq!(child.combat_timeline, parent.combat_timeline);
        assert_eq!(child.timestamp, tick);
        assert_eq!(child.created_tick, tick);
        assert_eq!(child.expiry_tick, Some(50));
        assert_eq!(child.weapon_data, 15 | (2 << 5));
        assert_eq!(
            child.behavior,
            crate::gameplay_rules::ProjectileBehavior::BouncingBullet
        );
        assert_eq!(child.proximity_target, None);
        assert_eq!(child.proximity_sensor_end_tick, None);
        assert_eq!((child.pos_x, child.pos_y), (100.5, 200.25));
    }
    // Neither a wall bounce nor Repel may reseed an already-fired explosive.
    parent.velocity_x = -12345;
    parent.velocity_y = 23456;
    let mut next_id = 100;
    let after_bounce = build_shrapnel_records(
        &parent,
        tuning,
        position,
        tick,
        |_, _, _| false,
        || {
            next_id += 1;
            next_id
        },
    );
    assert_eq!(after_bounce, children);

    let mut registry = ProjectileRegistry::default();
    registry.record_group(children, 128, 1024);
    registry.step(
        crate::continuum::continuum_tick_sub(tick, 1),
        no_collision,
        &[],
    );
    assert_eq!(registry.len(), 8);
    registry.step(50, no_collision, &[]);
    assert_eq!(
        registry.len(),
        0,
        "fragment lifetime ends at its own wrapped expiry"
    );
}

#[test]
fn shrapnel_solid_spawn_and_missing_source_do_not_allocate_ids() {
    let (mut parent, tuning) = shrapnel_parent(8, true);
    let position = crate::gameplay_rules::RulesVector::new(100.0, 100.0);
    assert!(
        build_shrapnel_records(
            &parent,
            tuning,
            position,
            0,
            |_, _, _| true,
            || panic!("blocked spawn allocated an id")
        )
        .is_empty()
    );
    parent.presentation_source = None;
    assert!(
        build_shrapnel_records(
            &parent,
            tuning,
            position,
            0,
            |_, _, _| false,
            || panic!("missing source allocated an id")
        )
        .is_empty()
    );
}

#[test]
fn shrapnel_children_hit_independently_in_the_authoritative_registry() {
    let (parent, tuning) = shrapnel_parent(4, false);
    let mut next_id = 100;
    let children = build_shrapnel_records(
        &parent,
        tuning,
        crate::gameplay_rules::RulesVector::new(100.0, 100.0),
        0,
        |_, _, _| false,
        || {
            next_id += 1;
            next_id
        },
    );
    let mut registry = ProjectileRegistry::default();
    registry.record_group(children, 128, 1024);
    let mut players = [
        shooter_hull(1, 1, 500.0, 500.0),
        shooter_hull(2, 2, 100.0, 50.0),
        shooter_hull(3, 3, 150.0, 100.0),
    ];
    for player in &mut players {
        player.lag_compensation_ticks = 0;
        add_exact_motion_sample(
            player,
            50,
            player.x,
            player.y,
            crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
        );
    }
    assert!(
        registry
            .step_with_proximity_distance_transactional(0, no_collision, &players, 3.0, 0)
            .is_empty()
    );
    let hits =
        registry.step_with_proximity_distance_transactional(50, no_collision, &players, 3.0, 0);
    assert_eq!(hits.len(), 2);
    assert_eq!((hits[0].victim_id, hits[1].victim_id), (2, 3));
    assert!(hits.iter().all(|hit| hit.weapon_data == 15 | (2 << 5)));
    assert_eq!(
        registry.len(),
        4,
        "uncommitted contacts retain all authority"
    );
    assert!(registry.resolve_hit(hits[0].projectile_id));
    assert_eq!(registry.len(), 3, "one child must not consume its siblings");
    assert!(registry.contains_pending_hit(hits[1].projectile_id));
    assert!(registry.resolve_hit(hits[1].projectile_id));
    assert_eq!(registry.len(), 2);
}

#[test]
fn shrapnel_bouncing_wall_contact_times_out_through_tick_25() {
    for (age, remaining) in [(25, 0), (26, 1)] {
        let (parent, tuning) = shrapnel_parent(1, true);
        let children = build_shrapnel_records(
            &parent,
            tuning,
            crate::gameplay_rules::RulesVector::new(100.0, 100.0),
            0,
            |_, _, _| false,
            || 101,
        );
        let mut registry = ProjectileRegistry::default();
        registry.record_group(children, 128, 1024);
        let players = [shooter_hull(1, 1, 500.0, 500.0)];
        // Advance the earlier free flight first: this assertion isolates the
        // contact at the specified age, rather than a wall hit at tick one.
        registry.step(age - 1, no_collision, &players);
        assert!(registry.step(age, |_, y| y < 100.0, &players).is_empty());
        assert_eq!(registry.len(), remaining, "fragment age {age}");
        if remaining > 0 {
            assert!(registry.last().unwrap().velocity_y > 0);
        }
    }
}

#[test]
fn shrapnel_repel_restarts_inactive_age_without_rewriting_source_or_life_gate() {
    let (parent, tuning) = shrapnel_parent(1, true);
    let children = build_shrapnel_records(
        &parent,
        tuning,
        crate::gameplay_rules::RulesVector::new(503.0, 704.0),
        100,
        |_, _, _| false,
        || 101,
    );
    let mut registry = ProjectileRegistry::default();
    registry.record_group(children, 128, 1024);
    let owners = std::collections::HashMap::from([(parent.owner_identity(), (1, 0))]);
    let summary = registry.apply_repel(
        crate::gameplay_rules::RulesVector::new(500.0, 700.0),
        2,
        500.0,
        50.0,
        150,
        &owners,
        |_| Some(100),
    );
    assert_eq!(summary.projectiles_repelled, 1);
    assert_eq!(summary.states.len(), 1);
    assert_eq!(summary.states[0].created_tick, 150);
    assert_eq!(summary.states[0].expires_at_tick, 250);
    let child = registry.last().unwrap();
    assert_eq!(child.created_tick, 150);
    assert_eq!(child.timestamp, 100);
    assert_eq!(child.presentation_source, parent.presentation_source);
    assert_eq!(child.owner_identity(), parent.owner_identity());
    assert_eq!((child.velocity_x, child.velocity_y), (3000, 4000));
    registry.step(150, no_collision, &[]);
    registry.step(151, |_, y| y > 704.0, &[shooter_hull(1, 1, 1000.0, 1000.0)]);
    assert_eq!(
        registry.len(),
        0,
        "a newly repelled fragment is inactive on a wall again"
    );
}

#[test]
fn repelled_fragment_keeps_immutable_carrier_tick_across_repeated_extensions() {
    let (mut parent, tuning) = shrapnel_parent(1, true);
    parent.source_server_tick = Some(80);
    let children = build_shrapnel_records(
        &parent,
        tuning,
        crate::gameplay_rules::RulesVector::new(503.0, 704.0),
        100,
        |_, _, _| false,
        || 101,
    );
    let mut registry = ProjectileRegistry::default();
    registry.record_group(children, 128, 1024);
    let owners = std::collections::HashMap::from([(parent.owner_identity(), (1, 0))]);
    for tick in [150, 200] {
        let mutation = registry.apply_repel(
            crate::gameplay_rules::RulesVector::new(500.0, 700.0),
            2,
            500.0,
            50.0,
            tick,
            &owners,
            |_| Some(100),
        );
        assert_eq!(
            mutation.states[0].created_tick, 80,
            "Repel must still identify the original carrier"
        );
        assert_eq!(
            registry.last().unwrap().created_tick,
            tick,
            "fragment inactivity restarts separately"
        );
        assert_eq!(mutation.states[0].expires_at_tick, tick + 100);
    }
}

#[test]
fn projectile_record_matches_damage_weapon_types_and_shrapnel() {
    assert!(projectile(0, 2, Duration::ZERO).matches_damage_weapon(2));
    assert!(!projectile(0, 2, Duration::ZERO).matches_damage_weapon(15));
    assert!(
        projectile(0, 2 | (2 << 5) | (5 << 10), Duration::ZERO)
            .matches_damage_weapon(15 | (2 << 5))
    );
    assert!(
        !projectile(0, 2 | (1 << 5) | (5 << 10), Duration::ZERO)
            .matches_damage_weapon(15 | (2 << 5))
    );
    assert!(
        projectile(0, 3 | (1 << 8) | (5 << 10), Duration::ZERO)
            .matches_damage_weapon(15 | (1 << 5))
    );
    assert!(projectile(0, 3 | (5 << 10), Duration::ZERO).matches_damage_weapon(15));
    assert!(!projectile(0, 3 | (5 << 10), Duration::ZERO).matches_damage_weapon(15 | (1 << 5)));
    assert!(
        !projectile(0, 3 | (1 << 8) | (5 << 10), Duration::ZERO)
            .matches_damage_weapon(15 | (2 << 5))
    );
    assert!(!projectile(0, 3, Duration::ZERO).matches_damage_weapon(15));
    assert!(!projectile(0, 2, Duration::ZERO).matches_damage_weapon(3));
}

#[test]
fn projectile_registry_expires_and_matches_recent_weapons() {
    let mut registry = ProjectileRegistry::default();
    let now_tick = 20_000;
    registry.record(projectile(7, 2, Duration::from_secs(1)), 8, 16);
    registry.record(projectile(7, 3, Duration::from_secs(20)), 8, 16);

    assert!(registry.has_recent_weapon(7, 2, now_tick, 1_200));
    assert!(!registry.has_recent_weapon(7, 3, now_tick, 1_200));

    registry.expire_older_than(now_tick, 1_200);
    assert_eq!(registry.len(), 1);
    assert!(registry.has_recent_weapon(7, 2, now_tick, 1_200));
}

#[test]
fn projectile_registry_caps_per_owner_and_total() {
    let mut registry = ProjectileRegistry::default();
    for index in 0..5 {
        let mut record = projectile(1, 2, Duration::ZERO);
        record.timestamp = index;
        registry.record(record, 3, 10);
    }
    assert_eq!(registry.len(), 3);
    assert_eq!(
        registry
            .iter()
            .map(|projectile| projectile.timestamp)
            .collect::<Vec<_>>(),
        vec![2, 3, 4]
    );

    for index in 0..5 {
        let mut record = projectile(2, 2, Duration::ZERO);
        record.timestamp = 10 + index;
        registry.record(record, 5, 4);
    }
    assert_eq!(registry.len(), 4);
    assert_eq!(
        registry
            .iter()
            .map(|projectile| projectile.timestamp)
            .collect::<Vec<_>>(),
        vec![3, 4, 13, 14],
        "global cap pressure is charged to the busier owner's own oldest records, \
         so the second owner's flood cannot evict the first owner's retained share"
    );
}

#[test]
fn pending_collision_caps_are_exact_fair_and_release_owned_memory() {
    let mut registry = ProjectileRegistry::default();
    let mut players = Vec::new();
    for owner_id in 1..=24_u16 {
        players.push(shooter_hull(owner_id, 1, 0.0, 0.0));
        for ordinal in 0..MAX_PENDING_HITS_PER_OWNER {
            let mut record = projectile(owner_id, 2, Duration::ZERO);
            // These collision steps run at ticks 100+, not the age factory's
            // separate tick-20,000 expiry clock.
            record.created_tick = 100;
            record.timestamp = 100;
            record.id = u32::from(owner_id) * 1_000 + ordinal as u32;
            record.shot_group_id = u64::from(record.id);
            registry.record(record, usize::MAX, usize::MAX);
        }
    }
    players.push(shooter_hull(700, 99, 500.0, 700.0));
    registry.step_with_proximity_distance_transactional(100, no_collision, &players, 3.0, 0);
    let first_batch =
        registry.step_with_proximity_distance_transactional(101, no_collision, &players, 3.0, 0);
    assert_eq!(first_batch.len(), MAX_PENDING_HIT_RETRIES_PER_TICK);
    assert_eq!(
        registry.pending_hit_count(),
        MAX_PENDING_HIT_RETRIES_PER_TICK
    );
    let fifo_ids = first_batch
        .iter()
        .map(|hit| hit.projectile_id)
        .collect::<Vec<_>>();
    for tick in 102..=123 {
        let batch = registry.step_with_proximity_distance_transactional(
            tick,
            no_collision,
            &players,
            3.0,
            0,
        );
        assert_eq!(batch.len(), MAX_PENDING_HIT_RETRIES_PER_TICK);
        assert_eq!(
            batch
                .iter()
                .map(|hit| hit.projectile_id)
                .collect::<Vec<_>>(),
            fifo_ids,
            "a full old FIFO prefix owns the complete bounded work batch"
        );
    }
    assert_eq!(
        registry.pending_hit_count(),
        MAX_PENDING_HIT_RETRIES_PER_TICK,
        "sustained pressure cannot grow authority behind an undrained prefix"
    );
    let retry_batch =
        registry.step_with_proximity_distance_transactional(124, no_collision, &players, 3.0, 0);
    assert_eq!(retry_batch.len(), MAX_PENDING_HIT_RETRIES_PER_TICK);
    assert_eq!(
        retry_batch
            .iter()
            .map(|hit| hit.projectile_id)
            .collect::<Vec<_>>(),
        fifo_ids
    );
    for owner_id in 1..=2_u16 {
        assert_eq!(
            registry.pending_hit_count_for_owner(owner_id),
            MAX_PENDING_HITS_PER_OWNER
        );
    }
    for owner_id in 3..=24_u16 {
        assert_eq!(registry.pending_hit_count_for_owner(owner_id), 0);
    }
    assert!(registry.pending_hit_retained_bytes() > 0);
    assert!(
        registry.pending_hit_retained_bytes()
            <= MAX_PENDING_HIT_RETRIES_PER_TICK.saturating_mul(MAX_PENDING_HIT_RECORD_BYTES),
        "the structured collision witness stays inside its explicit per-record memory budget"
    );

    let protected_projectile_id = retry_batch[0].projectile_id;
    let mut trim_pressure = projectile(1, 2, Duration::ZERO);
    trim_pressure.id = 88_888;
    trim_pressure.shot_group_id = 88_888;
    registry.record_group(
        vec![trim_pressure],
        MAX_PENDING_HITS_PER_OWNER,
        MAX_PENDING_HITS_TOTAL,
    );
    assert!(
        registry
            .iter()
            .any(|record| record.id == protected_projectile_id),
        "ordinary projectile trimming must never remove a pending authority transaction"
    );
    assert!(registry.contains_pending_hit(protected_projectile_id));
    assert!(
        !registry.iter().any(|record| record.id == 88_888),
        "the uncommitted newest group, not an immutable pending collision, absorbs cap pressure"
    );
    for tick in 125..129 {
        let sustained = registry.step_with_proximity_distance_transactional(
            tick,
            no_collision,
            &players,
            3.0,
            0,
        );
        assert_eq!(sustained.len(), MAX_PENDING_HIT_RETRIES_PER_TICK);
        assert_eq!(
            sustained
                .iter()
                .map(|hit| hit.projectile_id)
                .collect::<Vec<_>>(),
            fifo_ids,
            "sustained pressure retries one bounded FIFO prefix without growth or reordering"
        );
        assert_eq!(
            registry.pending_hit_count(),
            MAX_PENDING_HIT_RETRIES_PER_TICK
        );
    }

    let mut overflow = projectile(25, 2, Duration::ZERO);
    overflow.id = 99_999;
    overflow.shot_group_id = 99_999;
    registry.record(overflow, usize::MAX, usize::MAX);
    players.push(shooter_hull(25, 1, 0.0, 0.0));
    let before = registry
        .iter()
        .find(|record| record.id == 99_999)
        .copied()
        .expect("overflow projectile retained");
    registry.step_with_proximity_distance_transactional(129, no_collision, &players, 3.0, 0);
    assert_eq!(
        registry.pending_hit_count(),
        MAX_PENDING_HIT_RETRIES_PER_TICK
    );
    assert_eq!(
        registry.iter().find(|record| record.id == 99_999).copied(),
        Some(before),
        "max+1 collision freezes instead of losing or mutating its projectile"
    );

    let oldest = retry_batch[0];
    assert!(registry.resolve_hit(oldest.projectile_id));
    let after_one_drain =
        registry.step_with_proximity_distance_transactional(130, no_collision, &players, 3.0, 0);
    assert_eq!(after_one_drain.len(), MAX_PENDING_HIT_RETRIES_PER_TICK);
    assert_eq!(
        &after_one_drain[..MAX_PENDING_HIT_RETRIES_PER_TICK - 1],
        &retry_batch[1..],
        "every remaining old transaction precedes the one newly admitted collision"
    );
    assert_eq!(
        registry.pending_hit_count(),
        MAX_PENDING_HIT_RETRIES_PER_TICK
    );
    assert!(registry.contains_pending_hit(3_000));
    assert!(!registry.contains_pending_hit(99_999));
    registry.remove_owner(25);
    assert!(!registry.contains_pending_hit(99_999));
    registry.clear();
    assert_eq!(registry.pending_hit_count(), 0);
    assert_eq!(registry.pending_hit_retained_bytes(), 0);
}

fn moving_projectile(velocity_x: i16, velocity_y: i16, expiry_tick: u32) -> ProjectileRecord {
    ProjectileRecord {
        id: 2,
        shot_group_id: 2,
        shot_index: 0,
        owner_id: 1,
        owner_life_seq: 1,
        combat_timeline: None,
        presentation_source: None,
        weapon_data: 1,
        x: 1000,
        y: 1000,
        timestamp: 100,
        created_tick: 100,
        velocity_x,
        velocity_y,
        expiry_tick: Some(expiry_tick),
        pos_x: 1000.0,
        source_server_tick: None,
        precise_velocity: None,
        pos_y: 1000.0,
        bounces_remaining: 0,
        behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
        proximity_target: None,
        proximity_sensor_end_tick: None,
        proximity_highest_offset: 0.0,
    }
}

/// A predicate that says "no tile is solid" — for tests that focus
/// on the lifetime/velocity portions of the step without exercising
/// collision.
fn no_collision(_x: f32, _y: f32) -> bool {
    false
}

fn active_owner_hull() -> PlayerHullSnapshot {
    shooter_hull(1, 1, 0.0, 0.0)
}

#[test]
fn near_hull_diagnostics_capture_native_bot_decisions_without_changing_authority() {
    use super::ContactDiagnosticReason as Reason;
    for (offset, frequency, missing_history, harmful, expected) in [
        (0.0, 2, false, true, Some(Reason::ContactFrozenReady)),
        (0.0, 2, false, false, Some(Reason::ContactFrozenNotHarmful)),
        (20.0, 2, false, true, Some(Reason::OutsideContactEnvelope)),
        (0.0, 2, true, true, Some(Reason::GeometryUnavailable)),
        (0.0, 1, false, true, Some(Reason::SameFrequency)),
        (100.0, 2, false, true, None),
    ] {
        let mut plain = ProjectileRegistry::default();
        let mut shot = moving_projectile(0, 0, 1000);
        shot.presentation_source = Some(crate::realtime_event::RealtimeWeaponEvent {
            source_player_id: shot.owner_id,
            source_session_id: 987,
            source_life_seq: shot.owner_life_seq,
            shot_group_id: shot.shot_group_id,
            x: shot.pos_x,
            y: shot.pos_y,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.0,
            frequency: 1,
            ship: 0,
            weapon_data: shot.weapon_data,
        });
        plain.record(shot, 128, 1024);
        plain.step(100, no_collision, &[]);
        let mut traced = plain.clone();
        traced.set_contact_diagnostics_enabled(true);
        let mut target = shooter_hull(2, frequency, 1000.0 + offset, 1000.0);
        add_exact_motion_sample(
            &mut target,
            101,
            1000.0 + offset,
            1000.0,
            crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
        );
        if missing_history {
            target.motion_samples.clear();
        }
        let hulls = [active_owner_hull(), target];
        let run = |registry: &mut ProjectileRegistry| {
            registry.step_with_combat_timeline_geometry_and_harmful_eligibility_transactional(
                101,
                no_collision,
                &hulls,
                0.0,
                0,
                0.0,
                |_, _, _| panic!("server-native bot shots must bypass human projection"),
                |_| harmful,
            )
        };
        let plain_hits = run(&mut plain);
        let traced_hits = run(&mut traced);
        assert_eq!(traced_hits, plain_hits);
        assert_eq!(
            traced.iter().collect::<Vec<_>>(),
            plain.iter().collect::<Vec<_>>()
        );
        assert_eq!(
            traced.frozen_projectile_ids(),
            plain.frozen_projectile_ids()
        );
        assert_eq!(
            traced.pending_hits_for_preparation(),
            plain.pending_hits_for_preparation()
        );
        assert!(plain.drain_contact_diagnostics().is_none());
        let batch = traced.drain_contact_diagnostics();
        if let Some(expected) = expected {
            let (samples, suppressed) = batch.expect("near human candidate must be diagnosed");
            assert_eq!(suppressed, 0);
            assert_eq!(samples.len(), 1);
            assert_eq!(samples[0].reason, expected);
            assert_eq!(
                samples[0]
                    .projectile
                    .captured_source()
                    .unwrap()
                    .source_session_id,
                987
            );
            assert_eq!(samples[0].target_session_id, 10_002);
            if expected != Reason::SameFrequency {
                assert_eq!(
                    samples[0].presentation.unwrap().status,
                    crate::combat_timeline::ProjectilePresentationStatusV1::ServerNative
                );
            }
        } else {
            assert!(batch.is_none(), "far-away candidates are not near misses");
        }
    }
}

#[test]
fn near_hull_diagnostics_remain_bounded_across_drains_and_tick_wrap() {
    let start = 0x7fff_fff0;
    let mut registry = ProjectileRegistry::default();
    registry.set_contact_diagnostics_enabled(true);
    for id in 1..=80 {
        let mut shot = moving_projectile(0, 0, 1000);
        shot.id = id;
        shot.shot_group_id = u64::from(id);
        shot.created_tick = start;
        shot.timestamp = start;
        shot.expiry_tick = None;
        registry.record(shot, 128, 1024);
    }
    registry.step(start, no_collision, &[]);
    let mut target = shooter_hull(2, 2, 1020.0, 1000.0);
    target.life_started_tick = None;
    target.motion_samples.clear();
    add_exact_motion_sample(
        &mut target,
        crate::continuum::continuum_tick_sub(start, 10),
        1020.0,
        1000.0,
        crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
    );
    let hulls = [active_owner_hull(), target];
    registry.step(
        crate::continuum::continuum_tick_add(start, 1),
        no_collision,
        &hulls,
    );
    let (samples, suppressed) = registry
        .drain_contact_diagnostics()
        .expect("first bounded batch");
    assert_eq!(samples.len(), MAX_CONTACT_DIAGNOSTIC_SAMPLES);
    assert_eq!(suppressed, 16);
    assert!(
        samples
            .iter()
            .all(|sample| sample.reason == ContactDiagnosticReason::OutsideContactEnvelope)
    );
    registry.step(
        crate::continuum::continuum_tick_add(start, 2),
        no_collision,
        &hulls,
    );
    assert!(
        registry.drain_contact_diagnostics().is_none(),
        "draining does not reset the rate budget"
    );
    registry.step(
        crate::continuum::continuum_tick_add(start, 100),
        no_collision,
        &hulls,
    );
    let (samples, suppressed) = registry
        .drain_contact_diagnostics()
        .expect("window renews across u31 wrap");
    assert_eq!(samples.len(), MAX_CONTACT_DIAGNOSTIC_SAMPLES);
    assert!(suppressed >= 16);
    registry.set_contact_diagnostics_enabled(false);
    registry.step(
        crate::continuum::continuum_tick_add(start, 101),
        no_collision,
        &hulls,
    );
    assert!(registry.drain_contact_diagnostics().is_none());
    registry.set_contact_diagnostics_enabled(true);
    registry.clear();
    assert!(
        registry.drain_contact_diagnostics().is_none(),
        "arena reset clears diagnostic identities"
    );
}

#[test]
fn step_advances_position_by_velocity_times_dt() {
    let mut registry = ProjectileRegistry::default();
    // Velocity 1000 px/s east (10000 wire-format = ×10), dt = 1 tick
    // = 0.01s ⇒ +10 px per step.
    registry.record(moving_projectile(10_000, 0, 1_000), 100, 1_000);

    // First step: anchor only, no movement.
    registry.step(100, no_collision, &[]);
    assert_eq!(registry.last().unwrap().x, 1000);

    // Second step at +1 tick.
    registry.step(101, no_collision, &[active_owner_hull()]);
    let p = registry.last().unwrap();
    assert!(
        (p.pos_x - 1010.0).abs() < 0.1,
        "expected pos_x ≈ 1010 after one tick, got {}",
        p.pos_x
    );
    assert_eq!(p.x, 1010);

    // Step 50 more ticks (0.5s). Total displacement = 60 ticks × 10 px = ?
    // Actually: from anchor at tick 100 to tick 151 = 51 ticks ⇒ 510 px.
    registry.step(151, no_collision, &[active_owner_hull()]);
    let p = registry.last().unwrap();
    assert!(
        (p.pos_x - 1510.0).abs() < 0.5,
        "expected pos_x ≈ 1510 after 51 ticks, got {}",
        p.pos_x
    );
}

#[test]
fn step_despawns_projectile_past_expiry_tick() {
    let mut registry = ProjectileRegistry::default();
    // Lifetime 200 ticks: expiry_tick = spawn (100) + 200 = 300.
    registry.record(moving_projectile(10_000, 0, 300), 100, 1_000);
    assert_eq!(registry.len(), 1);

    registry.step(100, no_collision, &[]); // anchor
    registry.step(250, no_collision, &[active_owner_hull()]); // before expiry
    assert_eq!(registry.len(), 1);

    registry.step(310, no_collision, &[active_owner_hull()]); // past expiry
    assert_eq!(registry.len(), 0);
}

#[test]
fn step_with_zero_velocity_leaves_position_unchanged() {
    let mut registry = ProjectileRegistry::default();
    let mut mine = moving_projectile(0, 0, 0);
    mine.expiry_tick = None;
    mine.weapon_data = 3 | (1 << 15); // mine bit
    registry.record(mine, 100, 1_000);
    registry.step(100, no_collision, &[]);
    registry.step(200, no_collision, &[active_owner_hull()]); // 1 second later
    let p = registry.last().unwrap();
    assert_eq!(p.pos_x, 1000.0);
    assert_eq!(p.pos_y, 1000.0);
    assert_eq!(p.x, 1000);
    assert_eq!(p.y, 1000);
}

#[test]
fn step_handles_31_bit_tick_wraparound() {
    let mut registry = ProjectileRegistry::default();
    // Spawn near the 31-bit tick boundary; expiry is past it.
    let near_max = 0x7fff_ffff - 50;
    let expiry_after_wrap = crate::continuum::continuum_tick_add(near_max, 100);
    let mut p = moving_projectile(10_000, 0, expiry_after_wrap);
    p.timestamp = near_max;
    registry.record(p, 100, 1_000);

    registry.step(near_max, no_collision, &[]); // anchor
    // 99 ticks after spawn — wraps past 0x7fff_ffff into low ticks.
    // Lifetime is 100 (expiry exclusive) so this is the last alive
    // tick. Should still be alive.
    let just_before_expiry = crate::continuum::continuum_tick_add(near_max, 99);
    registry.step(just_before_expiry, no_collision, &[active_owner_hull()]);
    assert_eq!(
        registry.len(),
        1,
        "projectile alive across wrap before expiry"
    );

    let well_after_expiry = crate::continuum::continuum_tick_add(near_max, 251);
    registry.step(well_after_expiry, no_collision, &[active_owner_hull()]);
    assert_eq!(registry.len(), 0, "projectile despawns after expiry");
}

#[test]
fn exact_zero_expiry_is_not_a_sentinel_and_cannot_deal_damage() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(0, 0, 0);
    bullet.timestamp = 0x7fff_ffff;
    bullet.created_tick = 0x7fff_ffff;
    bullet.expiry_tick = Some(0);
    registry.record(bullet, 100, 1_000);

    let owner = shooter_hull(1, 1, 0.0, 0.0);
    let enemy = shooter_hull(2, 99, 1000.0, 1000.0);
    assert!(
        registry
            .step(0x7fff_ffff, no_collision, &[owner.clone(), enemy.clone()])
            .is_empty()
    );
    assert_eq!(registry.len(), 1, "deadline zero is still in the future");

    let hits = registry.step(0, no_collision, &[owner, enemy]);
    assert!(
        hits.is_empty(),
        "expired projectile must not reach collision"
    );
    assert_eq!(registry.len(), 0);
}

#[test]
fn step_despawns_non_bouncing_projectile_on_wall_hit() {
    let mut registry = ProjectileRegistry::default();
    // Bullet moving east at 1000 px/s, lifetime far in the future.
    let mut bullet = moving_projectile(10_000, 0, 1_000);
    bullet.bounces_remaining = 0; // explicit: regular bullet, no bounces
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]); // anchor

    // Wall directly east of the projectile's path.
    let solid = |x: f32, _y: f32| x >= 1005.0;
    registry.step(101, solid, &[active_owner_hull()]);
    assert_eq!(
        registry.len(),
        0,
        "regular bullet despawns on first wall hit"
    );
}

#[test]
fn thor_crosses_solid_tiles_without_bouncing_or_despawning() {
    let mut registry = ProjectileRegistry::default();
    let mut thor = moving_projectile(10_000, 0, 1_000);
    thor.weapon_data = 8;
    thor.bounces_remaining = 0;
    registry.record(thor, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let solid = |x: f32, _y: f32| x >= 1005.0;
    registry.step(101, solid, &[active_owner_hull()]);
    let thor = registry.last().expect("Thor passes through the wall");
    assert_eq!(thor.velocity_x, 10_000);
    assert!((thor.pos_x - 1010.0).abs() < 0.01);
}

#[test]
fn repel_preserves_captured_frequency_session_and_rejects_invalid_carriers() {
    // The live slot now has the opposite frequency. Production's owner map
    // supplies the captured firing ship's bounce count, not mutable roster data.
    for captured_frequency in [10, 20] {
        for invalid in 0..6 {
            let mut shot = projectile(2, 2, Duration::ZERO);
            let mut source = crate::realtime_event::RealtimeWeaponEvent {
                source_player_id: shot.owner_id,
                source_session_id: 123,
                source_life_seq: shot.owner_life_seq,
                shot_group_id: shot.shot_group_id,
                x: shot.pos_x,
                y: shot.pos_y,
                velocity_x: 0.0,
                velocity_y: 0.0,
                orientation: 0.0,
                frequency: captured_frequency,
                ship: 6,
                weapon_data: shot.weapon_data,
            };
            match invalid {
                0 => {}
                1 => source.source_player_id += 1,
                2 => source.source_session_id = 0,
                3 => source.source_life_seq += 1,
                4 => source.shot_group_id += 1,
                5 => source.ship = 8,
                _ => unreachable!(),
            }
            shot.pos_x += 10.0;
            shot.presentation_source = Some(source);
            let owners = std::collections::HashMap::from([(
                shot.owner_identity(),
                (if captured_frequency == 10 { 20 } else { 10 }, 4),
            )]);
            let mut registry = ProjectileRegistry::default();
            registry.record(shot, 100, 1000);
            let expected_count = usize::from(invalid == 0 && captured_frequency != 10);
            assert_eq!(
                registry.preview_repel_mutation_count(
                    crate::gameplay_rules::RulesVector::new(500.0, 700.0),
                    10,
                    50.0,
                    &owners
                ),
                expected_count,
                "preview frequency={captured_frequency} invalid={invalid}"
            );
            let summary = registry.apply_repel(
                crate::gameplay_rules::RulesVector::new(500.0, 700.0),
                10,
                500.0,
                50.0,
                1000,
                &owners,
                |_| Some(700),
            );
            assert_eq!(summary.states.len(), expected_count);
            if expected_count == 1 {
                assert_eq!(summary.states[0].source_session_id, 123);
                assert_eq!(registry.iter().next().unwrap().motion_velocity().x, 500.0);
            } else {
                assert_eq!(
                    *registry.iter().next().unwrap(),
                    shot,
                    "friendly or invalid source must remain byte-for-byte unchanged"
                );
            }
        }
    }
}

#[test]
fn repel_mutates_only_hostile_square_range_and_converts_mine_authority() {
    let mut registry = ProjectileRegistry::default();
    let mut hostile_mine = projectile(2, 3 | 0x8000, Duration::ZERO);
    hostile_mine.id = 2;
    hostile_mine.shot_group_id = 2;
    hostile_mine.pos_x = 503.0;
    hostile_mine.pos_y = 704.0;
    hostile_mine.expiry_tick = Some(1_001);
    let mut friendly = projectile(1, 2, Duration::ZERO);
    friendly.id = 3;
    friendly.shot_group_id = 3;
    friendly.pos_x = 510.0;
    friendly.pos_y = 700.0;
    let mut boundary = projectile(2, 2, Duration::ZERO);
    boundary.id = 4;
    boundary.shot_group_id = 4;
    boundary.pos_x = 550.0;
    boundary.pos_y = 700.0;
    registry.record(hostile_mine, 100, 1_000);
    registry.record(friendly, 100, 1_000);
    registry.record(boundary, 100, 1_000);

    let owners = std::collections::HashMap::from([
        (crate::authority_identity::LifeIdentity::new(1, 1), (10, 2)),
        (crate::authority_identity::LifeIdentity::new(2, 1), (20, 4)),
    ]);
    let summary = registry.apply_repel(
        crate::gameplay_rules::RulesVector::new(500.0, 700.0),
        10,
        500.0,
        50.0,
        1_000,
        &owners,
        |_| Some(700),
    );
    assert_eq!(summary.projectiles_repelled, 1);
    assert_eq!(summary.mines_converted, 1);
    assert_eq!(summary.states.len(), 1);
    assert_eq!(
        summary.states[0],
        RepelledProjectileState {
            motion_frozen: false,
            projectile_id: 2,
            source_session_id: 0,
            owner_id: 2,
            owner_life_seq: 1,
            created_tick: hostile_mine.created_tick,
            source_weapon_data: 3 | 0x8000,
            source_projectile_index: 0,
            result_weapon_data: 3,
            position_x: 503.0,
            position_y: 704.0,
            velocity_x: 300.0,
            velocity_y: 400.0,
            expires_at_tick: 1_700,
            bounces_remaining: 4,
        }
    );
    assert_eq!(
        registry.preview_repel_mutation_count(
            crate::gameplay_rules::RulesVector::new(500.0, 700.0),
            10,
            50.0,
            &owners,
        ),
        1,
        "preview and committed public mutation reservation stay exact"
    );
    let mine = registry
        .iter()
        .find(|projectile| projectile.id == 2)
        .unwrap();
    assert!(!crate::continuum_weapon::continuum_weapon_is_mine(
        mine.weapon_data
    ));
    assert_eq!(mine.velocity_x, 3_000);
    assert_eq!(mine.velocity_y, 4_000);
    assert_eq!(mine.bounces_remaining, 4);
    assert_eq!(mine.expiry_tick, Some(1_700));
    assert_eq!(
        registry
            .iter()
            .find(|projectile| projectile.id == 3)
            .unwrap()
            .velocity_x,
        0
    );
    assert_eq!(
        registry
            .iter()
            .find(|projectile| projectile.id == 4)
            .unwrap()
            .velocity_x,
        0
    );
}

#[test]
fn step_bounces_projectile_when_bounces_remaining() {
    let mut registry = ProjectileRegistry::default();
    let mut bouncing = moving_projectile(10_000, 0, 1_000);
    bouncing.bounces_remaining = 2;
    registry.record(bouncing, 100, 1_000);
    registry.step(100, no_collision, &[]); // anchor

    // First bounce: hit east wall, reflect velocity, decrement counter.
    let solid_east = |x: f32, _y: f32| x >= 1005.0;
    registry.step(101, solid_east, &[active_owner_hull()]);
    let p = registry.last().expect("still alive after first bounce");
    assert_eq!(p.bounces_remaining, 1);
    assert_eq!(p.velocity_x, -10_000, "velocity reflects on bounce");
    // Position stayed put on the X axis (the candidate move was rejected).
    assert!((p.pos_x - 1000.0).abs() < 0.5);

    // Second bounce: now moving west, hit west wall.
    let solid_west = |x: f32, _y: f32| x <= 995.0;
    registry.step(102, solid_west, &[active_owner_hull()]);
    let p = registry.last().expect("still alive after second bounce");
    assert_eq!(p.bounces_remaining, 0);
    assert_eq!(p.velocity_x, 10_000, "velocity reflects again");

    // Third hit: counter exhausted ⇒ despawn.
    let solid_east_again = |x: f32, _y: f32| x >= 1005.0;
    registry.step(103, solid_east_again, &[active_owner_hull()]);
    assert_eq!(registry.len(), 0, "projectile despawns after final bounce");
}

#[test]
fn step_bounces_only_on_colliding_axis() {
    // A diagonal-moving projectile that hits an east-west wall (only
    // x is blocked) should reflect vx but keep vy intact.
    let mut registry = ProjectileRegistry::default();
    let mut diag = moving_projectile(10_000, 5_000, 1_000);
    diag.bounces_remaining = 5;
    registry.record(diag, 100, 1_000);
    registry.step(100, no_collision, &[]); // anchor

    let solid_east_only = |x: f32, _y: f32| x >= 1005.0;
    registry.step(101, solid_east_only, &[active_owner_hull()]);
    let p = registry.last().expect("still alive");
    assert_eq!(p.velocity_x, -10_000, "x reflected");
    assert_eq!(p.velocity_y, 5_000, "y unchanged");
    // y advanced normally (5000 / 10 = 500 px/s × 0.01s = 5 px).
    assert!((p.pos_y - 1005.0).abs() < 0.5);
}

fn shooter_hull(player_id: u16, freq: u16, x: f32, y: f32) -> PlayerHullSnapshot {
    let mut hull = PlayerHullSnapshot {
        player_id,
        life_seq: 1,
        x,
        y,
        frequency: freq,
        radius_px: 14.0,
        motion_segment: None,
        motion_samples: Vec::new(),
        motion_source_kind: Some(
            crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
        ),
        lag_compensation_ticks: 5,
        life_started_tick: Some(0),
    };
    add_exact_motion_sample(
        &mut hull,
        0,
        x,
        y,
        crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
    );
    hull
}

fn add_exact_motion_sample(
    hull: &mut PlayerHullSnapshot,
    tick: u32,
    x: f32,
    y: f32,
    source_kind: crate::combat_timeline::ActorMotionSourceKind,
) {
    let actor = crate::authority_identity::PlayerRuntimeIdentity::new(
        crate::authority_identity::PlayerSessionIdentity::new(
            hull.player_id,
            crate::authority_identity::PlayerSessionId::new(10_000 + u64::from(hull.player_id)),
        ),
        hull.identity(),
    );
    let segment = crate::combat_timeline::MotionSegmentIdentity {
        actor,
        arena_stream_id: 1,
        discontinuity_generation: 1,
    };
    hull.motion_segment = Some(segment);
    hull.motion_source_kind = Some(source_kind);
    hull.motion_samples
        .push(crate::combat_timeline::ActorMotionSampleV1 {
            segment,
            server_tick: tick,
            sample_time: crate::combat_timeline::PresentationSampleTime::from_tick(tick),
            x_q8: (x * 256.0).round() as i32,
            y_q8: (y * 256.0).round() as i32,
            velocity_x_q8: 0,
            velocity_y_q8: 0,
            orientation_u16: 0,
            protocol_direction: 0,
            hull_radius_q8: (hull.radius_px * 256.0).round() as u16,
            safe_or_attached: 0,
            source_kind,
        });
}

#[test]
fn surviving_projectile_hits_successor_life_only_after_current_life_sample_exists() {
    for weapon in [1, 2, 3, 4, 8, 15, 3 | (1 << 15), 4 | (1 << 15)] {
        let mut registry = ProjectileRegistry::default();
        let mut bullet = moving_projectile(0, 0, 1_000);
        bullet.owner_id = 1;
        bullet.weapon_data = weapon;
        bullet.timestamp = 100;
        registry.record(bullet, 100, 1_000);
        registry.step(100, no_collision, &[]); // anchor

        let mut respawned_victim = shooter_hull(2, 99, 1000.0, 1000.0);
        respawned_victim.life_seq = 2;
        respawned_victim.motion_samples.clear();
        add_exact_motion_sample(
            &mut respawned_victim,
            0,
            1000.0,
            1000.0,
            crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
        );
        respawned_victim.life_started_tick = Some(101);
        let hits = registry.step(
            101,
            no_collision,
            &[active_owner_hull(), respawned_victim.clone()],
        );
        assert!(
            hits.is_empty(),
            "pre-birth held geometry cannot hit the successor life"
        );
        assert_eq!(
            registry.len(),
            1,
            "stale projectile remains for other valid targets"
        );

        add_exact_motion_sample(
            &mut respawned_victim,
            101,
            1000.0,
            1000.0,
            crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
        );
        assert!(
            registry
                .step(
                    102,
                    no_collision,
                    &[active_owner_hull(), respawned_victim.clone()]
                )
                .is_empty(),
            "the unchanged five-tick rewind has not reached the new life yet"
        );
        let hits = registry.step(106, no_collision, &[active_owner_hull(), respawned_victim]);
        assert_eq!(
            hits.len(),
            1,
            "the still-live old shot can hit current-life geometry"
        );
        assert_eq!(hits[0].victim_id, 2);
        assert_eq!(hits[0].victim_life_seq, 2);
    }
}

#[test]
fn selected_collision_anchor_rejects_prebirth_native_and_observer_samples_across_tick_wrap() {
    use crate::combat_timeline::{
        PresentationProjectionDisposition, ProjectilePresentationStatusV1,
        ProjectilePresentationWitnessV1,
    };
    let mut target = shooter_hull(2, 99, 1000.0, 1000.0);
    let started = crate::continuum::CONTINUUM_TICK_MASK - 2;
    target.life_started_tick = Some(started);
    for (tick, available) in [(started - 1, false), (started, true), (3, true)] {
        let exact = CollisionTargetWitness {
            x: 1000.0,
            y: 1000.0,
            requested_tick: tick,
            selected_tick: Some(tick),
            source: MotionHistorySource::UnifiedExactSegment,
            collision_available: true,
        };
        let native = select_collision_target_witness(
            &target,
            exact,
            ProjectilePresentationWitnessV1::server_native(),
        );
        assert_eq!(native.collision_available, available);
        // A valid current exact witness cannot authorize an older observer anchor.
        let current = CollisionTargetWitness {
            selected_tick: Some(3),
            ..exact
        };
        let presented = ProjectilePresentationWitnessV1 {
            status: ProjectilePresentationStatusV1::Resolved(
                PresentationProjectionDisposition::Exact,
            ),
            render_tick: tick,
            target_x_q8: 1000 * 256,
            target_y_q8: 1000 * 256,
            ..ProjectilePresentationWitnessV1::server_native()
        };
        let projected = select_collision_target_witness(&target, current, presented);
        assert_eq!(projected.collision_available, available);
    }
}

#[test]
fn step_emits_hit_when_projectile_overlaps_enemy_hull() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(10_000, 0, 1_000);
    bullet.id = 42;
    bullet.owner_id = 1;
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let enemy = shooter_hull(2, 99, 1010.0, 1000.0);
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let hits = registry.step(101, no_collision, &[shooter, enemy]);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].projectile_id, 42);
    assert_eq!(hits[0].owner_id, 1);
    assert_eq!(hits[0].victim_id, 2);
    assert_eq!(registry.len(), 0, "projectile despawns on hit");
}

#[test]
fn step_uses_reference_game_explosive_box_for_diagonal_direct_contact() {
    let mut registry = ProjectileRegistry::default();
    let mut bomb = moving_projectile(0, 0, 1_000);
    bomb.owner_id = 1;
    bomb.weapon_data = 3 | (2 << 5);
    registry.record(bomb, 100, 1_000);
    registry.step(100, no_collision, &[]);

    // A 4px weapon box overlaps the 14px ship box on both axes. The retired
    // 14px circular authority check rejected this source-valid corner hit.
    let enemy = shooter_hull(2, 99, 1017.0, 1017.0);
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let hits = registry.step(101, no_collision, &[shooter, enemy]);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].victim_id, 2);
    assert_eq!(
        registry.len(),
        0,
        "direct bomb contact consumes the projectile"
    );
}

#[test]
fn thor_authority_uses_reference_proximity_box_at_every_raw_level() {
    // The reference game's WeaponManager::SimulateHit: (ProximityDistance + raw + 3)
    // * 18px, subtract 14px for the weapon box, then overlap the ship box.
    // With a 14px hull the positive-axis reach is exactly that first value.
    for raw_level in 0..=3 {
        let reach = (3 + raw_level + 3) * 18;
        for (dx, dy, expected_hits) in [
            (reach - 1, 0, 1),
            (reach - 1, reach - 1, 1),
            (reach + 1, 0, 0),
            (0, reach + 1, 0),
        ] {
            let mut registry = ProjectileRegistry::default();
            let mut thor = moving_projectile(0, 0, 1_000);
            thor.owner_id = 1;
            thor.weapon_data = 8 | ((raw_level as u16) << 5);
            registry.record(thor, 100, 1_000);
            registry.step(100, no_collision, &[]);
            let shooter = shooter_hull(1, 1, 0.0, 0.0);
            let enemy = shooter_hull(2, 99, (1000 + dx) as f32, (1000 + dy) as f32);
            let hits = registry.step(101, no_collision, &[shooter, enemy]);
            assert_eq!(
                hits.len(),
                expected_hits,
                "Thor raw level {raw_level}, offset ({dx}, {dy})"
            );
            assert_eq!(registry.len(), 1 - expected_hits);
        }
    }
}

#[test]
fn thor_proximity_arms_outside_contact_and_obeys_explode_delay() {
    let mut registry = ProjectileRegistry::default();
    let mut thor = moving_projectile(0, 0, 1_000);
    thor.weapon_data = 8;
    registry.record(thor, 100, 1_000);
    registry.step(100, no_collision, &[]);
    let players = [
        shooter_hull(1, 1, 0.0, 0.0),
        // Beyond a stock proximity bomb's 54px sensor, inside Thor's 108px.
        shooter_hull(2, 99, 1100.0, 1000.0),
    ];
    for tick in 101..105 {
        let hits = registry.step_with_proximity_distance(tick, no_collision, &players, 3.0, 4);
        assert!(hits.is_empty(), "Thor must wait for its sensor deadline");
        assert_eq!(
            registry.last().unwrap().proximity_target,
            Some(players[1].identity()),
            "Thor must arm without direct hull contact"
        );
    }
    let hits = registry.step_with_proximity_distance(105, no_collision, &players, 3.0, 4);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].victim_id, 2);
    assert_eq!(registry.len(), 0);
}

#[test]
fn step_emits_proximity_bomb_hit_before_direct_hull_overlap() {
    let mut registry = ProjectileRegistry::default();
    let mut bomb = moving_projectile(0, 0, 1_000);
    bomb.owner_id = 1;
    bomb.weapon_data = 4 | (2 << 5); // level-3 proximity bomb: (3 + 2) * 18 = 90px
    registry.record(bomb, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let enemy = shooter_hull(2, 99, 1088.0, 1000.0);
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let hits = registry.step(101, no_collision, &[shooter, enemy]);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].owner_id, 1);
    assert_eq!(hits[0].victim_id, 2);
    assert_eq!(registry.len(), 0, "proximity bomb despawns on hit");
}

#[test]
fn step_uses_reference_game_box_for_diagonal_proximity_contact() {
    let mut registry = ProjectileRegistry::default();
    let mut bomb = moving_projectile(0, 0, 1_000);
    bomb.owner_id = 1;
    bomb.weapon_data = 4 | (2 << 5); // level-3 proximity bomb: 90px box reach
    registry.record(bomb, 100, 1_000);
    registry.step(100, no_collision, &[]);

    // Both axes are within the 90px the reference game's sensor overlap, while the
    // Euclidean distance is greater than 90px. Authority must follow the
    // authored AABB rule rather than a circular approximation.
    let enemy = shooter_hull(2, 99, 1088.0, 1088.0);
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let hits = registry.step(101, no_collision, &[shooter, enemy]);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].victim_id, 2);
    assert_eq!(
        registry.len(),
        0,
        "initial proximity contact detonates immediately"
    );
}

#[test]
fn step_armed_proximity_bomb_continues_until_close_pass_or_delay() {
    let mut registry = ProjectileRegistry::default();
    let mut bomb = moving_projectile(10_000, 0, 1_000);
    bomb.owner_id = 1;
    bomb.weapon_data = 4 | (2 << 5); // level-3 proximity bomb: (3 + 2) * 18 = 90px
    registry.record(bomb, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let enemy = shooter_hull(2, 99, 1088.0, 1000.0);
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let hits = registry.step_with_proximity_distance(
        101,
        no_collision,
        &[shooter.clone(), enemy.clone()],
        3.0,
        150,
    );
    assert!(
        hits.is_empty(),
        "proximity sensor should arm instead of detonating immediately"
    );
    assert_eq!(registry.len(), 1);
    assert_eq!(
        registry.last().unwrap().proximity_target,
        Some(crate::authority_identity::LifeIdentity::new(2, 1))
    );

    for tick in 102..=109 {
        let hits = registry.step_with_proximity_distance(
            tick,
            no_collision,
            &[shooter.clone(), enemy.clone()],
            3.0,
            150,
        );
        assert!(
            hits.is_empty(),
            "still approaching the target at tick {tick}, so no proximity explosion yet"
        );
    }

    let hits =
        registry.step_with_proximity_distance(110, no_collision, &[shooter, enemy], 3.0, 150);
    assert_eq!(
        hits.len(),
        1,
        "proximity bomb should explode after passing closest approach"
    );
    assert_eq!(hits[0].victim_id, 2);
    assert!(
        (hits[0].x as i32 - 1100).abs() <= 1,
        "detonation should happen near the close pass, not at the outer proximity radius"
    );
    assert_eq!(registry.len(), 0);
}

#[test]
fn step_discards_projectile_when_the_owner_life_is_no_longer_active() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(0, 0, 1_000);
    bullet.owner_id = 1;
    bullet.owner_life_seq = 7;
    bullet.weapon_data = 2;
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let mut replacement_owner = shooter_hull(1, 1, 0.0, 0.0);
    replacement_owner.life_seq = 8;
    let enemy = shooter_hull(2, 99, 1000.0, 1000.0);
    let hits = registry.step(101, no_collision, &[replacement_owner, enemy]);

    assert!(hits.is_empty());
    assert!(registry.is_empty());
}

#[test]
fn step_stamps_detected_hit_with_captured_owner_and_victim_lives() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(0, 0, 1_000);
    bullet.owner_life_seq = 17;
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let mut owner = shooter_hull(1, 1, 0.0, 0.0);
    owner.life_seq = 17;
    let mut victim = shooter_hull(2, 99, 1000.0, 1000.0);
    victim.life_seq = 23;
    let hits = registry.step(101, no_collision, &[owner, victim]);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].owner_life_seq, 17);
    assert_eq!(hits[0].victim_life_seq, 23);
}

#[test]
fn armed_proximity_target_must_remain_the_same_hostile_life() {
    fn armed_registry() -> ProjectileRegistry {
        let mut registry = ProjectileRegistry::default();
        let mut bomb = moving_projectile(10_000, 0, 1_000);
        bomb.weapon_data = 4 | (2 << 5);
        registry.record(bomb, 100, 1_000);
        registry.step(100, no_collision, &[]);
        let owner = shooter_hull(1, 1, 0.0, 0.0);
        let target = shooter_hull(2, 99, 1088.0, 1000.0);
        assert!(
            registry
                .step_with_proximity_distance(101, no_collision, &[owner, target], 3.0, 150)
                .is_empty()
        );
        assert!(registry.last().unwrap().proximity_target.is_some());
        registry
    }

    let owner = shooter_hull(1, 1, 0.0, 0.0);
    let mut respawned = shooter_hull(2, 99, 1088.0, 1000.0);
    respawned.life_seq = 2;
    let mut registry = armed_registry();
    assert!(
        registry
            .step_with_proximity_distance(102, no_collision, &[owner.clone(), respawned], 3.0, 150)
            .is_empty()
    );
    assert!(
        registry.is_empty(),
        "a stale target life invalidates the latch"
    );

    let now_friendly = shooter_hull(2, 1, 1088.0, 1000.0);
    let mut registry = armed_registry();
    assert!(
        registry
            .step_with_proximity_distance(
                102,
                no_collision,
                &[owner.clone(), now_friendly],
                3.0,
                150
            )
            .is_empty()
    );
    assert!(registry.is_empty(), "a now-friendly target cannot detonate");

    let mut registry = armed_registry();
    assert!(
        registry
            .step_with_proximity_distance(102, no_collision, &[owner], 3.0, 150)
            .is_empty()
    );
    assert!(
        registry.is_empty(),
        "a dead or absent target cannot detonate"
    );
}

#[test]
fn step_consumes_direct_gun_projectile_after_first_hit() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(10_000, 0, 1_000);
    bullet.owner_id = 1;
    bullet.weapon_data = 2;
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let enemy = shooter_hull(2, 99, 1010.0, 1000.0);
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let first_hits = registry.step(101, no_collision, &[shooter.clone(), enemy.clone()]);
    assert_eq!(first_hits.len(), 1);
    assert_eq!(registry.len(), 0, "direct gun projectile is consumed");

    let second_hits = registry.step(102, no_collision, &[shooter, enemy]);
    assert!(
        second_hits.is_empty(),
        "consumed direct gun projectile must not hit again on later ticks"
    );
}

#[test]
fn step_skips_teammate_hit() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(10_000, 0, 1_000);
    bullet.owner_id = 1;
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let teammate = shooter_hull(2, 5, 1010.0, 1000.0);
    let shooter = shooter_hull(1, 5, 0.0, 0.0);
    let hits = registry.step(101, no_collision, &[shooter, teammate]);
    assert!(hits.is_empty(), "no friendly fire");
    assert_eq!(registry.len(), 1, "projectile continues past teammate");
}

#[test]
fn step_skips_direct_self_hit_for_every_projectile_family() {
    // The reference game excludes the owner through the same-frequency filter before
    // projectile/hull contact. Bombs can still damage their shooter through a
    // later explosion transaction, but they cannot detonate merely because
    // inherited ship velocity leaves them overlapping their launch hull.
    for weapon_data in [1, 2, 66, 3, 4 | (1 << 5), 7, 8, 15 | (2 << 5)] {
        let mut registry = ProjectileRegistry::default();
        let mut projectile = moving_projectile(10_000, 0, 1_000);
        projectile.owner_id = 1;
        projectile.weapon_data = weapon_data;
        registry.record(projectile, 100, 1_000);
        registry.step(100, no_collision, &[]);
        let shooter = shooter_hull(1, 1, 1010.0, 1000.0);
        let hits = registry.step(101, no_collision, &[shooter]);
        assert!(
            hits.is_empty(),
            "weapon {weapon_data:#06x} must not hit its own shooter"
        );
        assert_eq!(
            registry.len(),
            1,
            "owner contact must not consume the projectile"
        );
    }
}

#[test]
fn step_uses_rewound_victim_position_for_hit_check() {
    // Lag compensation: a victim who's *currently* far from the
    // projectile but *was* in its path 5 ticks ago should still
    // register a hit. The rewind constant in registry.rs is 5
    // ticks; the test seeds a sample at exactly that tick offset
    // so the rewound position lands on the projectile.
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(0, 0, 1_000); // stationary projectile
    bullet.owner_id = 1;
    bullet.pos_x = 1000.0;
    bullet.pos_y = 1000.0;
    bullet.x = 1000;
    bullet.y = 1000;
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]); // anchor

    // Victim's CURRENT position is far from the projectile, but
    // their position 5 ticks ago was right on it.
    let mut victim = shooter_hull(2, 99, 5000.0, 5000.0); // current = far away
    // Step is at tick 105; rewind target = 105 - 5 = 100. Seed a
    // sample at tick 100 with the projectile's coordinates.
    add_exact_motion_sample(
        &mut victim,
        100,
        1000.0,
        1000.0,
        crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
    );
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let hits = registry.step_with_proximity_distance_transactional(
        105,
        no_collision,
        &[shooter, victim],
        3.0,
        0,
    );
    assert_eq!(hits.len(), 1, "rewound position resolves to a hit");
    assert_eq!(hits[0].victim_id, 2);
    let witness = registry
        .collision_witness(hits[0].projectile_id)
        .expect("transactional contact retains its witness");
    assert_eq!(witness.selected_target_tick, Some(100));
    assert_eq!(
        witness.history_source,
        MotionHistorySource::UnifiedExactSegment
    );
    assert_eq!(
        (witness.target_current_x, witness.target_current_y),
        (5000.0, 5000.0)
    );
    assert_eq!(
        (witness.target_anchor_x, witness.target_anchor_y),
        (1000.0, 1000.0)
    );
    assert_eq!(witness.contact_reason, ProjectileContactReason::Direct);
}

#[test]
fn combat_timeline_geometry_is_the_human_projectile_collision_owner() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(0, 0, 1_000);
    let owner = crate::authority_identity::PlayerRuntimeIdentity::new(
        crate::authority_identity::PlayerSessionIdentity::new(
            1,
            crate::authority_identity::PlayerSessionId::new(10_001),
        ),
        crate::authority_identity::LifeIdentity::new(1, 1),
    );
    let binding = crate::combat_timeline::ProjectileCombatTimelineV1 {
        owner,
        owner_input_sequence: 50,
        accepted_timeline_generation: 7,
        accepted_game_applied_snapshot_id: 20,
        accepted_server_tick: 100,
        maximum_rewind_ticks: 7,
    };
    bullet.combat_timeline = Some(binding);
    registry.record(bullet, 100, 1_000);
    registry.step_with_combat_timeline_geometry_and_harmful_eligibility_transactional(
        100,
        no_collision,
        &[],
        3.0,
        0,
        80.0,
        |_, _, _| crate::combat_timeline::ProjectilePresentationWitnessV1::not_evaluated(),
        |_| true,
    );

    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let mut victim = shooter_hull(2, 99, 1_026.0, 1_000.0);
    add_exact_motion_sample(
        &mut victim,
        100,
        1_026.0,
        1_000.0,
        crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
    );
    let hits = registry.step_with_combat_timeline_geometry_and_harmful_eligibility_transactional(
        105,
        no_collision,
        &[shooter, victim],
        3.0,
        0,
        80.0,
        |_, target, collision_tick| {
            if target.player_id != 2 {
                return crate::combat_timeline::ProjectilePresentationWitnessV1::local_self(
                    binding,
                    collision_tick,
                    (target.x * 256.0) as i32,
                    (target.y * 256.0) as i32,
                );
            }
            crate::combat_timeline::ProjectilePresentationWitnessV1 {
                status: crate::combat_timeline::ProjectilePresentationStatusV1::Resolved(
                    crate::combat_timeline::PresentationProjectionDisposition::Exact,
                ),
                accepted_timeline_generation: 7,
                effective_timeline_generation: 7,
                render_tick: 98,
                applied_snapshot_id: 20,
                lower_snapshot_id: 19,
                upper_snapshot_id: 20,
                target_x_q8: 1_014 * 256,
                target_y_q8: 1_000 * 256,
                authority_x_q8: 1_026 * 256,
                authority_y_q8: 1_000 * 256,
                authority_error_q8: 12 * 256,
                server_advanced_watermark: false,
            }
        },
        |_| true,
    );
    assert_eq!(hits.len(), 1, "verified visible geometry selects contact");
    let witness = registry
        .collision_witness(hits[0].projectile_id)
        .expect("collision freezes the selected presentation anchor");
    assert_eq!(witness.target_anchor_x, 1_014.0);
    assert_eq!(
        witness.history_source,
        MotionHistorySource::ObserverPresentation
    );
    assert_eq!(
        witness.presentation_geometry.target_anchor_q8(),
        Some((1_014 * 256, 1_000 * 256))
    );
    let metrics = registry.presentation_geometry_metrics();
    assert_eq!(metrics.lookups_total, 1);
    assert_eq!(metrics.resolved_total, 1);
    assert_eq!(metrics.unavailable_total, 0);
    assert_eq!(metrics.server_native_total, 0);
    assert_eq!(metrics.local_self_total, 0);
}

#[test]
fn combat_timeline_geometry_accounts_every_admitted_target_without_fallback() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(0, 0, 2_000);
    let owner = crate::authority_identity::PlayerRuntimeIdentity::new(
        crate::authority_identity::PlayerSessionIdentity::new(
            1,
            crate::authority_identity::PlayerSessionId::new(20_001),
        ),
        crate::authority_identity::LifeIdentity::new(1, 1),
    );
    let binding = crate::combat_timeline::ProjectileCombatTimelineV1 {
        owner,
        owner_input_sequence: 50,
        accepted_timeline_generation: 7,
        accepted_game_applied_snapshot_id: 20,
        accepted_server_tick: 100,
        maximum_rewind_ticks: 7,
    };
    bullet.combat_timeline = Some(binding);
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let mut players = vec![shooter_hull(1, 1, 0.0, 0.0)];
    for player_id in 2..=302_u16 {
        let mut target = shooter_hull(player_id, 99, 5_000.0, 5_000.0);
        add_exact_motion_sample(
            &mut target,
            100,
            5_000.0,
            5_000.0,
            crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
        );
        players.push(target);
    }
    let mut evaluations = 0_usize;
    let hits = registry.step_with_combat_timeline_geometry_and_harmful_eligibility_transactional(
        101,
        no_collision,
        &players,
        3.0,
        0,
        80.0,
        |_, _, _| {
            evaluations += 1;
            crate::combat_timeline::ProjectilePresentationWitnessV1::query_unavailable(
                binding,
                crate::combat_timeline::ProjectilePresentationQueryRejection::WrongRuntime,
            )
        },
        |_| true,
    );
    assert!(hits.is_empty());
    assert_eq!(evaluations, players.len() - 1);
    let metrics = registry.presentation_geometry_metrics();
    assert_eq!(metrics.lookups_total as usize, evaluations);
    assert_eq!(metrics.unavailable_total as usize, evaluations);
    assert!(metrics.eval_nanos_max <= metrics.eval_nanos_total);
}

#[test]
fn human_projectile_without_presentation_resolver_fails_closed() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(0, 0, 2_000);
    let owner = crate::authority_identity::PlayerRuntimeIdentity::new(
        crate::authority_identity::PlayerSessionIdentity::new(
            1,
            crate::authority_identity::PlayerSessionId::new(20_002),
        ),
        crate::authority_identity::LifeIdentity::new(1, 1),
    );
    bullet.combat_timeline = Some(crate::combat_timeline::ProjectileCombatTimelineV1 {
        owner,
        owner_input_sequence: 50,
        accepted_timeline_generation: 7,
        accepted_game_applied_snapshot_id: 20,
        accepted_server_tick: 100,
        maximum_rewind_ticks: 7,
    });
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let mut victim = shooter_hull(2, 99, 1_014.0, 1_000.0);
    add_exact_motion_sample(
        &mut victim,
        100,
        1_014.0,
        1_000.0,
        crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
    );
    let hits = registry.step_with_damage_geometry_and_harmful_eligibility_transactional(
        105,
        no_collision,
        &[shooter, victim],
        3.0,
        0,
        80.0,
        |_| true,
    );
    assert!(hits.is_empty());
    let metrics = registry.presentation_geometry_metrics();
    assert_eq!(metrics.lookups_total, 1);
    assert_eq!(metrics.resolved_total, 0);
    assert_eq!(metrics.unavailable_total, 1);
}

#[test]
fn server_native_fixture_uses_exact_motion_instead_of_current_position() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(0, 0, 1_000);
    bullet.owner_id = 1;
    bullet.pos_x = 1000.0;
    bullet.pos_y = 1000.0;
    bullet.x = 1000;
    bullet.y = 1000;
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let mut victim = shooter_hull(2, 99, 5000.0, 5000.0);
    add_exact_motion_sample(
        &mut victim,
        100,
        1200.0,
        1000.0,
        crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
    );
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let hits = registry.step_with_proximity_distance_transactional(
        105,
        no_collision,
        &[shooter, victim],
        3.0,
        0,
    );

    assert!(
        hits.is_empty(),
        "server-native exact motion history, not current position, owns collision"
    );
}

#[test]
fn bot_collision_uses_unified_exact_segment_and_never_current_fallback() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(0, 0, 1_000);
    bullet.owner_id = 1;
    bullet.pos_x = 1000.0;
    bullet.pos_y = 1000.0;
    bullet.x = 1000;
    bullet.y = 1000;
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let mut bot = shooter_hull(2, 99, 1000.0, 1000.0);
    add_exact_motion_sample(
        &mut bot,
        100,
        1200.0,
        1000.0,
        crate::combat_timeline::ActorMotionSourceKind::ServerBotSimulation,
    );
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let hits = registry.step_with_proximity_distance_transactional(
        105,
        no_collision,
        &[shooter, bot],
        3.0,
        0,
    );
    assert!(
        hits.is_empty(),
        "bot contact must use the exact segment, not current fallback"
    );

    let mut unavailable_registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(0, 0, 1_000);
    bullet.owner_id = 1;
    bullet.pos_x = 1000.0;
    bullet.pos_y = 1000.0;
    bullet.x = 1000;
    bullet.y = 1000;
    unavailable_registry.record(bullet, 100, 1_000);
    unavailable_registry.step(100, no_collision, &[]);
    let mut bot_without_sample = shooter_hull(2, 99, 1000.0, 1000.0);
    bot_without_sample.motion_segment = None;
    bot_without_sample.motion_samples.clear();
    bot_without_sample.motion_source_kind =
        Some(crate::combat_timeline::ActorMotionSourceKind::ServerBotSimulation);
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let hits = unavailable_registry.step_with_proximity_distance_transactional(
        105,
        no_collision,
        &[shooter, bot_without_sample.clone()],
        3.0,
        0,
    );
    assert!(hits.is_empty(), "unavailable bot history fails closed");
    let unavailable = resolve_collision_target_witness(&bot_without_sample, 105);
    assert_eq!(
        unavailable.source,
        MotionHistorySource::UnifiedHistoryUnavailable
    );
    assert!(!unavailable.collision_available);
}

#[test]
fn missing_exact_history_fails_closed_without_current_position_fallback() {
    // Missing exact history must not resurrect the removed pre-cutover path.
    // Hulls always name an exact motion source and fail closed when its
    // segment is unavailable.
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(10_000, 0, 1_000);
    bullet.owner_id = 1;
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let mut enemy = shooter_hull(2, 99, 1010.0, 1000.0);
    enemy.motion_segment = None;
    enemy.motion_samples.clear();
    enemy.motion_source_kind = None;
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let hits = registry.step_with_proximity_distance_transactional(
        101,
        no_collision,
        &[shooter, enemy.clone()],
        3.0,
        0,
    );
    assert!(hits.is_empty(), "missing exact history must fail closed");
    let unavailable = resolve_collision_target_witness(&enemy, 101);
    assert_eq!(
        unavailable.source,
        MotionHistorySource::UnifiedHistoryUnavailable
    );
    assert!(!unavailable.collision_available);
}

#[test]
fn step_skips_self_hit_inside_bomb_safety_zone() {
    let mut registry = ProjectileRegistry::default();
    let mut bomb = moving_projectile(100, 0, 1_000); // 10 px/s
    bomb.owner_id = 1;
    bomb.weapon_data = 3;
    registry.record(bomb, 100, 1_000);
    registry.step(100, no_collision, &[]);
    let shooter = shooter_hull(1, 1, 1000.0, 1000.0);
    let hits = registry.step(101, no_collision, &[shooter]);
    assert!(hits.is_empty(), "no self-hit while inside safety zone");
    assert_eq!(registry.len(), 1, "projectile still alive");
}

#[test]
fn direct_bomb_ignores_its_owner_at_an_exact_motion_anchor() {
    let mut registry = ProjectileRegistry::default();
    let mut bomb = moving_projectile(0, 0, 1_000);
    bomb.owner_id = 1;
    bomb.weapon_data = 3;
    registry.record(bomb, 100, 1_000);
    registry.step(100, no_collision, &[]);

    // This is the exact one-pixel current/history disagreement retained from
    // the Firefox Extreme failure. Neither anchor can turn launch overlap into
    // direct self-collision; only a later explosion may damage the shooter.
    let mut owner = shooter_hull(1, 1, 1019.0, 1000.0);
    add_exact_motion_sample(
        &mut owner,
        96,
        1018.0,
        1000.0,
        crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
    );
    let hits = registry.step(101, no_collision, &[owner]);
    assert!(hits.is_empty());
    assert_eq!(registry.len(), 1);
}

#[test]
fn diagonal_bomb_and_thor_leave_the_owner_without_direct_self_collision() {
    for weapon_type in [3, 8] {
        let mut registry = ProjectileRegistry::default();
        // One 10 ms step lands at (+16,+15). For the 14px ship and 4px
        // explosive boxes this is still a direct overlap on both axes, even
        // though the old 1.5-radius circle considered sqrt(16²+15²) clear.
        let mut projectile = moving_projectile(16_000, 15_000, 1_000);
        projectile.owner_id = 1;
        projectile.weapon_data = weapon_type;
        registry.record(projectile, 100, 1_000);
        registry.step(100, no_collision, &[]);
        let owner = shooter_hull(1, 1, 1000.0, 1000.0);

        let hits = registry.step(101, no_collision, &[owner.clone()]);
        assert!(
            hits.is_empty(),
            "weapon {weapon_type} self-hit while leaving"
        );
        let hits = registry.step(102, no_collision, &[owner]);
        assert!(hits.is_empty());
        assert_eq!(registry.len(), 1);
    }
}

#[test]
fn step_never_arms_proximity_contact_from_its_owner() {
    let mut registry = ProjectileRegistry::default();
    let mut bomb = moving_projectile(10_000, 0, 1_000); // 1000 px/s
    bomb.owner_id = 1;
    bomb.weapon_data = 4 | (1 << 5); // level-2 proximity bomb: (3 + 1) * 18 = 72px
    registry.record(bomb, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let shooter = shooter_hull(1, 1, 1000.0, 1000.0);
    let hits = registry.step(101, no_collision, &[shooter.clone()]);
    assert!(
        hits.is_empty(),
        "proximity bomb must not self-hit while still inside its own trigger radius"
    );
    assert_eq!(registry.len(), 1, "projectile still alive");
    let hits = registry.step(108, no_collision, &[shooter]);
    assert!(
        hits.is_empty(),
        "crossing the proximity radius should not create a direct self-hit"
    );
    assert_eq!(
        registry.len(),
        1,
        "projectile still alive after crossing the owner radius"
    );
}

#[test]
fn step_proximity_bomb_can_hit_enemy_while_owner_is_still_inside_safety_radius() {
    let mut registry = ProjectileRegistry::default();
    let mut bomb = moving_projectile(10_000, 0, 1_000); // 1000 px/s
    bomb.owner_id = 1;
    bomb.weapon_data = 4 | (1 << 5); // level-2 proximity bomb: 72px trigger radius
    registry.record(bomb, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let shooter = shooter_hull(1, 1, 1000.0, 1000.0);
    let enemy = shooter_hull(2, 99, 1080.0, 1000.0);
    let hits = registry.step(101, no_collision, &[shooter, enemy]);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].victim_id, 2);
    assert_eq!(registry.len(), 0, "proximity bomb despawns on enemy hit");
}

#[test]
fn step_emits_hit_when_projectile_overlaps_player_and_wall_same_substep() {
    // B5 regression: a projectile that's about to hit a wall AND is
    // overlapping a player should fire the hit event before
    // despawning. Earlier behavior (`continue` on wall-despawn)
    // would skip the hit-scan and let the player escape unscathed.
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(10_000, 0, 1_000);
    bullet.owner_id = 1;
    bullet.bounces_remaining = 0; // non-bouncing, will despawn on wall
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]); // anchor

    // Wall directly east — projectile is at (1000, 1000), candidate
    // (1010, 1000) overlaps the wall.
    let solid_east = |x: f32, _y: f32| x >= 1005.0;
    // Victim co-located with the projectile's CURRENT position
    // (1000, 1000).
    let victim = shooter_hull(2, 99, 1000.0, 1000.0);
    let shooter = shooter_hull(1, 1, 0.0, 0.0);
    let hits = registry.step(101, solid_east, &[shooter, victim]);
    assert_eq!(hits.len(), 1, "hit fires before wall despawn");
    assert_eq!(hits[0].victim_id, 2);
    assert_eq!(registry.len(), 0, "projectile still despawns from wall");
}

#[test]
fn wall_explosion_freezes_nearby_victim_outside_direct_and_proximity_hulls() {
    let mut registry = ProjectileRegistry::default();
    let mut bomb = moving_projectile(10_000, 0, 1_000);
    bomb.id = 504;
    bomb.shot_group_id = 504;
    bomb.owner_id = 1;
    bomb.weapon_data = 4 | (1 << 5); // level-2 prox: trigger 72px, blast 160px
    bomb.bounces_remaining = 0;
    registry.record(bomb, 100, 1_000);
    registry.step_with_damage_geometry_transactional(100, no_collision, &[], 3.0, 150, 80.0);

    let owner = shooter_hull(1, 1, 700.0, 1_000.0);
    let enemy = shooter_hull(2, 99, 1_120.0, 1_000.0);
    let solid_east = |x: f32, _y: f32| x >= 1_005.0;
    let hits = registry.step_with_damage_geometry_transactional(
        101,
        solid_east,
        &[owner, enemy],
        3.0,
        150,
        80.0,
    );

    assert_eq!(
        hits.len(),
        1,
        "a wall explosion inside the authored 160px radius must own damage"
    );
    assert_eq!(hits[0].projectile_id, 504);
    assert_eq!(hits[0].victim_id, 2);
    assert_eq!((hits[0].x, hits[0].y), (1_000, 1_000));
    assert!(registry.contains_pending_hit(504));
}

#[test]
fn harmful_readiness_freezes_one_global_collision_and_preserves_fifo_order() {
    let mut registry = ProjectileRegistry::default();
    let mut first = moving_projectile(0, 0, 1_000);
    first.id = 601;
    first.shot_group_id = 601;
    first.owner_id = 1;
    first.weapon_data = 2;
    first.pos_x = 1_000.0;
    first.pos_y = 1_000.0;
    first.x = 1_000;
    first.y = 1_000;
    let mut second = first;
    second.id = 602;
    second.shot_group_id = 602;
    registry.record(first, 100, 1_000);
    registry.record(second, 100, 1_000);

    let players = [
        shooter_hull(1, 1, 700.0, 1_000.0),
        shooter_hull(2, 99, 1_000.0, 1_000.0),
        shooter_hull(3, 98, 1_000.0, 1_000.0),
    ];
    registry.step_with_damage_geometry_and_harmful_eligibility_transactional(
        100,
        no_collision,
        &players,
        3.0,
        150,
        80.0,
        |_| true,
    );

    let early = registry.step_with_damage_geometry_and_harmful_eligibility_transactional(
        101,
        no_collision,
        &players,
        3.0,
        150,
        80.0,
        |_| false,
    );
    assert!(early.is_empty());
    assert_eq!(registry.pending_hit_count(), 2);
    assert_eq!(registry.harmful_readiness_deferrals_total(), 2);
    assert_eq!(registry.len(), 2, "both projectile groups remain frozen");

    let still_early = registry.step_with_damage_geometry_and_harmful_eligibility_transactional(
        102,
        no_collision,
        &players,
        3.0,
        150,
        80.0,
        |_| false,
    );
    assert!(still_early.is_empty());
    assert_eq!(
        registry.harmful_readiness_deferrals_total(),
        2,
        "retries do not inflate the first-deferral counter"
    );

    let first_ready = registry.step_with_damage_geometry_and_harmful_eligibility_transactional(
        103,
        no_collision,
        &players,
        3.0,
        150,
        80.0,
        |hit| hit.projectile_id == 601,
    );
    assert_eq!(
        first_ready
            .iter()
            .map(|hit| hit.projectile_id)
            .collect::<Vec<_>>(),
        vec![601],
        "an unready second collision cannot overtake the ready FIFO head"
    );
    assert!(registry.resolve_hit(601));

    let second_ready = registry.step_with_damage_geometry_and_harmful_eligibility_transactional(
        104,
        no_collision,
        &players,
        3.0,
        150,
        80.0,
        |_| true,
    );
    assert_eq!(
        second_ready
            .iter()
            .map(|hit| hit.projectile_id)
            .collect::<Vec<_>>(),
        vec![602]
    );
}

#[test]
fn bouncing_bullet_uses_kind_and_survives_repeated_walls() {
    let mut registry = ProjectileRegistry::default();
    let mut bullet = moving_projectile(10_000, 0, 1_000);
    bullet.weapon_data = 2;
    bullet.behavior = crate::gameplay_rules::ProjectileBehavior::BouncingBullet;
    registry.record(bullet, 100, 1_000);
    registry.step(100, no_collision, &[]);

    registry.step(101, |x, _| x >= 1005.0, &[active_owner_hull()]);
    assert_eq!(registry.last().unwrap().velocity_x, -10_000);
    assert_eq!(registry.last().unwrap().bounces_remaining, 0);

    registry.step(102, |x, _| x <= 995.0, &[active_owner_hull()]);
    assert_eq!(registry.last().unwrap().velocity_x, 10_000);
    assert_eq!(registry.len(), 1, "kind-based bounce has no finite counter");
}

#[test]
fn burst_is_harmless_until_first_wall_then_bounces_and_hits() {
    let mut registry = ProjectileRegistry::default();
    let mut burst = moving_projectile(10_000, 0, 1_000);
    burst.weapon_data = 7;
    burst.behavior = crate::gameplay_rules::ProjectileBehavior::Burst { active: false };
    registry.record(burst, 100, 1_000);
    registry.step(100, no_collision, &[]);

    let owner = active_owner_hull();
    let enemy_before_wall = shooter_hull(2, 99, 1010.0, 1000.0);
    assert!(
        registry
            .step(101, no_collision, &[owner.clone(), enemy_before_wall])
            .is_empty(),
        "inactive burst cannot damage before touching a wall"
    );
    assert_eq!(registry.len(), 1);

    assert!(
        registry
            .step(102, |x, _| x >= 1015.0, &[owner.clone()])
            .is_empty()
    );
    let active = registry.last().unwrap();
    assert_eq!(
        active.behavior,
        crate::gameplay_rules::ProjectileBehavior::Burst { active: true }
    );
    assert_eq!(active.velocity_x, -10_000);

    let enemy_after_wall = shooter_hull(2, 99, 1000.0, 1000.0);
    let hits = registry.step(103, no_collision, &[owner, enemy_after_wall]);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].victim_id, 2);
}

#[test]
fn delayed_burst_step_cannot_tunnel_through_one_tile_wall() {
    let mut registry = ProjectileRegistry::default();
    let mut burst = moving_projectile(0, -3_000, 1_000);
    burst.weapon_data = 7;
    burst.behavior = crate::gameplay_rules::ProjectileBehavior::Burst { active: false };
    registry.record(burst, 100, 1_000);
    registry.step(100, no_collision, &[]);

    // Ten elapsed Continuum ticks move this Burst from y=1000 to y=970.
    // Endpoint-only collision skips the complete 16 px wall at 980..996,
    // exactly the failure observed when browser startup briefly delayed the
    // real server loop.
    assert!(
        registry
            .step(
                110,
                |_x, y| (980.0..996.0).contains(&y),
                &[active_owner_hull()],
            )
            .is_empty()
    );
    let burst = registry.last().expect("Burst survives its first wall");
    assert_eq!(
        burst.behavior,
        crate::gameplay_rules::ProjectileBehavior::Burst { active: true }
    );
    assert_eq!(burst.velocity_y, 3_000);
    // One step reaches 997, the next reflects there, and the remaining
    // eight 10 ms steps travel 24 px away from the wall.
    assert_eq!(burst.pos_y, 1_021.0);
}

fn grouped_bullet(id: u32, group: u64, x: f32, velocity_x: i16) -> ProjectileRecord {
    let mut bullet = moving_projectile(velocity_x, 0, 10_000);
    bullet.id = id;
    bullet.shot_group_id = group;
    bullet.weapon_data = 1;
    bullet.x = x.round() as u16;
    bullet.pos_x = x;
    bullet
}

#[test]
fn overlapping_double_barrel_children_produce_only_one_hit() {
    let mut registry = ProjectileRegistry::default();
    registry.record_group(
        vec![
            grouped_bullet(10, 50, 1000.0, 0),
            grouped_bullet(11, 50, 1000.0, 0),
        ],
        100,
        1_000,
    );
    registry.step(100, no_collision, &[]);
    let hits = registry.step(
        101,
        no_collision,
        &[active_owner_hull(), shooter_hull(2, 99, 1000.0, 1000.0)],
    );
    assert_eq!(hits.len(), 1);
    assert!(registry.is_empty(), "all linked siblings are removed");
}

#[test]
fn separated_and_multifire_children_keep_independent_contact_geometry() {
    let mut registry = ProjectileRegistry::default();
    registry.record_group(
        vec![
            grouped_bullet(20, 60, 800.0, 0),
            grouped_bullet(21, 60, 900.0, 0),
            grouped_bullet(22, 60, 1000.0, 0),
            grouped_bullet(23, 60, 1100.0, 0),
        ],
        100,
        1_000,
    );
    registry.step(100, no_collision, &[]);
    let hits = registry.step(
        101,
        no_collision,
        &[active_owner_hull(), shooter_hull(2, 99, 1000.0, 1000.0)],
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].projectile_id, 22,
        "the actual contacting child wins"
    );
    assert!(registry.is_empty());
}

#[test]
fn wall_cleanup_does_not_suppress_linked_siblings() {
    let mut registry = ProjectileRegistry::default();
    registry.record_group(
        vec![
            grouped_bullet(30, 70, 1000.0, 10_000),
            grouped_bullet(31, 70, 900.0, 0),
        ],
        100,
        1_000,
    );
    registry.step(100, no_collision, &[]);
    assert!(
        registry
            .step(101, |x, _| x >= 1005.0, &[active_owner_hull()])
            .is_empty()
    );
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.last().unwrap().id, 31);
}

#[test]
fn group_identity_survives_projectile_id_wrap_reuse_and_delayed_steps() {
    let mut registry = ProjectileRegistry::default();
    registry.record_group(vec![grouped_bullet(u32::MAX, 80, 950.0, 1_000)], 100, 1_000);
    registry.record_group(vec![grouped_bullet(1, 81, 950.0, 1_000)], 100, 1_000);
    registry.step(100, no_collision, &[]);
    let hits = registry.step(
        150,
        no_collision,
        &[active_owner_hull(), shooter_hull(2, 99, 1000.0, 1000.0)],
    );
    assert_eq!(hits.len(), 2, "distinct accepted shots remain independent");
    assert_eq!(hits[0].projectile_id, u32::MAX);
    assert_eq!(hits[1].projectile_id, 1);
}

#[test]
fn grouped_capacity_cleanup_never_retains_partial_salvos() {
    let mut registry = ProjectileRegistry::default();
    registry.record_group(
        vec![
            grouped_bullet(40, 90, 800.0, 0),
            grouped_bullet(41, 90, 900.0, 0),
        ],
        100,
        3,
    );
    registry.record_group(
        vec![
            grouped_bullet(42, 91, 1000.0, 0),
            grouped_bullet(43, 91, 1100.0, 0),
        ],
        100,
        3,
    );
    assert_eq!(registry.len(), 2);
    assert!(registry.iter().all(|shot| shot.shot_group_id == 91));

    registry.record_group(
        vec![
            grouped_bullet(44, 92, 1200.0, 0),
            grouped_bullet(45, 92, 1300.0, 0),
        ],
        3,
        100,
    );
    assert_eq!(registry.len(), 2);
    assert!(registry.iter().all(|shot| shot.shot_group_id == 92));
    registry.remove_owner(1);
    assert!(registry.is_empty());
}

fn owned_bullet(id: u32, owner_id: u16, group: u64) -> ProjectileRecord {
    let mut bullet = grouped_bullet(id, group, 1_000.0, 0);
    bullet.owner_id = owner_id;
    bullet
}

#[test]
fn global_capacity_pressure_is_charged_to_the_busiest_owner() {
    let mut registry = ProjectileRegistry::default();
    // The quiet owner fires first, so its lone record is the globally oldest.
    registry.record_group(vec![owned_bullet(10, 2, 200)], 100, 5);
    // A continuously firing owner then pushes the registry over its cap, one
    // accepted salvo per packet.
    for index in 0..5_u32 {
        registry.record_group(
            vec![owned_bullet(20 + index, 1, 300 + u64::from(index))],
            100,
            5,
        );
    }
    assert_eq!(
        [1_u16, 2].map(|owner| registry
            .iter()
            .filter(|record| record.owner_id == owner)
            .count()),
        [4, 1],
        "the heaviest publisher must shed its own volume, not the oldest foreign record"
    );
    assert_eq!(registry.len(), 5, "cap is met exactly");
}

#[test]
fn equal_owner_shares_charge_global_capacity_oldest_first() {
    let mut registry = ProjectileRegistry::default();
    for (owner_id, group) in [(1_u16, 400_u64), (2, 401)] {
        registry.record_group(
            vec![owned_bullet(30 + u32::from(owner_id), owner_id, group)],
            100,
            2,
        );
    }
    // Equal retained shares must keep the previous FIFO behaviour.
    registry.record_group(vec![owned_bullet(33, 3, 402)], 100, 2);

    assert!(
        !registry.iter().any(|record| record.owner_id == 1),
        "with equal shares the globally oldest record still pays"
    );
    assert!(registry.iter().any(|record| record.owner_id == 2));
    assert!(registry.iter().any(|record| record.owner_id == 3));
    assert_eq!(registry.len(), 2);
}

#[test]
fn newborn_weapons_do_not_travel_before_their_server_creation_tick() {
    for kind in [1, 2, 3, 4, 7, 8, 15] {
        for (last, now, created, elapsed_ticks) in [
            (100, 102, 102, 0),
            (100, 102, 101, 1),
            (100, 102, 100, 2),
            (100, 102, 103, 0), // Not born yet: hold without movement/contact.
            (0x7fff_fffe, 1, 0, 1),
        ] {
            let mut registry = ProjectileRegistry::default();
            registry.step(last, no_collision, &[]);
            let mut record = moving_projectile(1000, 0, 1000);
            record.weapon_data = kind;
            record.created_tick = created;
            record.timestamp = created;
            registry.record(record, 128, 1024);
            assert!(
                registry
                    .step(now, no_collision, &[active_owner_hull()])
                    .is_empty()
            );
            let expected_x = 1000.0 + elapsed_ticks as f32;
            assert!(
                (registry.last().unwrap().pos_x - expected_x).abs() < 0.0001,
                "kind={kind}, previous={last}, current={now}, created={created}: x={} expected={expected_x}",
                registry.last().unwrap().pos_x
            );
        }
    }
}

#[test]
fn newborn_weapon_cannot_hit_a_target_using_time_before_it_existed() {
    let mut registry = ProjectileRegistry::default();
    registry.step(100, no_collision, &[]);
    let mut bullet = moving_projectile(7600, 0, 650);
    bullet.created_tick = 102;
    bullet.timestamp = 102;
    registry.record(bullet, 128, 1024);
    let mut owner = active_owner_hull();
    owner.lag_compensation_ticks = 0;
    let mut target = shooter_hull(2, 99, 1026.0, 1000.0);
    for tick in [102, 104] {
        add_exact_motion_sample(
            &mut target,
            tick,
            1026.0,
            1000.0,
            crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
        );
    }
    assert!(
        registry
            .step(102, no_collision, &[owner.clone(), target.clone()])
            .is_empty(),
        "a birth-time projectile cannot travel 15.2px from the preceding interval and hit early"
    );
    assert_eq!(registry.last().unwrap().pos_x, 1000.0);
    let hits = registry.step(104, no_collision, &[owner, target]);
    assert_eq!(
        hits.len(),
        1,
        "contact remains valid after genuine elapsed flight"
    );
    assert_eq!(hits[0].victim_id, 2);
}

#[test]
fn direct_gun_contacts_use_reference_game_boxes_including_corners_and_pixel_edges() {
    // Independent transcription of the reference game's WeaponManager's call order:
    // BoxBoxOverlap(player min/max, weapon min/max), in pixel units.
    // Every non-proximity weapon gets the same four-pixel half extent.
    for kind in [1_u16, 2, 7, 15] {
        for radius in [14.0_f32, 20.0] {
            for (dx, dy) in [
                (0.0, 0.0),
                (radius + 3.9, radius + 3.9),
                (radius + 4.0, 0.0),
                (0.0, radius + 4.0),
                (radius + 5.0, 0.0),
                (0.0, radius + 5.0),
                (-radius - 3.0, -radius - 3.0),
                (-radius - 4.0, 0.0),
                (0.0, -radius - 4.0),
            ] {
                let (x, y) = (1000.25 + dx, 1000.25 + dy);
                let (px, py) = ((1000.25_f32 as i32) as f32, (1000.25_f32 as i32) as f32);
                let (wx, wy) = ((x as i32) as f32, (y as i32) as f32);
                let expected = px + radius >= wx - 4.0
                    && px - radius < wx + 4.0
                    && py + radius >= wy - 4.0
                    && py - radius < wy + 4.0;
                let mut record = moving_projectile(0, 0, 1000);
                record.weapon_data = kind;
                record.pos_x = x;
                record.pos_y = y;
                if kind == 7 {
                    record.behavior =
                        crate::gameplay_rules::ProjectileBehavior::Burst { active: true };
                }
                let mut target = shooter_hull(2, 2, 1000.25, 1000.25);
                target.radius_px = radius;
                let mut registry = ProjectileRegistry::default();
                registry.record(record, 128, 1024);
                registry.step(100, no_collision, &[]);
                let hits = registry.step_with_proximity_distance_transactional(
                    101,
                    no_collision,
                    &[active_owner_hull(), target],
                    3.0,
                    0,
                );
                assert_eq!(
                    !hits.is_empty(),
                    expected,
                    "kind={kind} radius={radius} offset=({dx},{dy})"
                );
                if expected {
                    let witness = registry.collision_witness(hits[0].projectile_id).unwrap();
                    assert_eq!(
                        witness.history_source,
                        MotionHistorySource::UnifiedExactSegment
                    );
                    assert_eq!(witness.target_anchor_x, 1000.25);
                    assert_eq!(witness.target_anchor_y, 1000.25);
                }
            }
        }
    }
}

#[test]
fn moving_bullet_corner_hit_uses_verified_observer_geometry_and_readiness_barrier() {
    use crate::combat_timeline::{
        PresentationProjectionDisposition, ProjectileCombatTimelineV1,
        ProjectilePresentationStatusV1, ProjectilePresentationWitnessV1,
    };
    for weapon in [1_u16, 2, 7, 15] {
        let owner = active_owner_hull();
        let binding = ProjectileCombatTimelineV1 {
            owner: owner.motion_segment.unwrap().actor,
            owner_input_sequence: 50,
            accepted_timeline_generation: 7,
            accepted_game_applied_snapshot_id: 20,
            accepted_server_tick: 100,
            maximum_rewind_ticks: 7,
        };
        let mut record = moving_projectile(6000, 0, 1000);
        record.weapon_data = weapon;
        record.pos_x = 976.0;
        record.pos_y = 1017.0;
        record.combat_timeline = Some(binding);
        if weapon == 7 {
            record.behavior = crate::gameplay_rules::ProjectileBehavior::Burst { active: true };
        }
        let mut registry = ProjectileRegistry::default();
        registry.record(record, 128, 1024);
        registry.step(100, no_collision, &[]);
        let target = shooter_hull(2, 2, 1012.0, 1000.0);
        let step = |registry: &mut ProjectileRegistry, tick, harmful| {
            registry.step_with_combat_timeline_geometry_and_harmful_eligibility_transactional(
                tick,
                no_collision,
                &[owner.clone(), target.clone()],
                3.0,
                0,
                80.0,
                |_, _, _| ProjectilePresentationWitnessV1 {
                    status: ProjectilePresentationStatusV1::Resolved(
                        PresentationProjectionDisposition::Exact,
                    ),
                    accepted_timeline_generation: 7,
                    effective_timeline_generation: 7,
                    render_tick: tick - 7,
                    applied_snapshot_id: 20,
                    lower_snapshot_id: 19,
                    upper_snapshot_id: 20,
                    target_x_q8: 1000 * 256,
                    target_y_q8: 1000 * 256,
                    authority_x_q8: 1012 * 256,
                    authority_y_q8: 1000 * 256,
                    authority_error_q8: 12 * 256,
                    server_advanced_watermark: false,
                },
                |_| harmful,
            )
        };
        // At x982 the reference's lower edge is open. At x988,y1017,
        // the boxes overlap, but both the old circle and current hull miss.
        assert!(step(&mut registry, 101, false).is_empty());
        assert_eq!(registry.pending_hit_count(), 0);
        assert!(step(&mut registry, 102, false).is_empty());
        assert_eq!(
            registry.pending_hit_count(),
            1,
            "contact freezes before harm opens"
        );
        let frozen = registry.collision_witness(2).unwrap();
        assert_eq!(frozen.projectile_x, 988.0);
        assert_eq!(frozen.projectile_y, 1017.0);
        assert_eq!(
            frozen.history_source,
            MotionHistorySource::ObserverPresentation
        );
        assert_eq!(frozen.selected_target_tick, Some(95));
        assert_eq!(frozen.target_anchor_x, 1000.0);
        assert_eq!(frozen.target_current_x, 1012.0);
        let hits = step(&mut registry, 103, true);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].tick, 102,
            "opening readiness cannot rewrite collision time"
        );
        assert_eq!(registry.collision_witness(2), Some(frozen));
    }
}

#[test]
fn capacity_paused_free_flight_has_a_frozen_presentation_flag() {
    let mut registry = ProjectileRegistry::default();
    let players = [
        shooter_hull(1, 1, 0.0, 0.0),
        shooter_hull(2, 1, 0.0, 0.0),
        shooter_hull(3, 1, 1000.0, 1000.0),
        shooter_hull(700, 99, 500.0, 700.0),
    ];
    for owner in 1..=2_u16 {
        for ordinal in 0..MAX_PENDING_HITS_PER_OWNER {
            let mut p = projectile(owner, 2, Duration::ZERO);
            p.id = u32::from(owner) * 1000 + ordinal as u32;
            p.shot_group_id = u64::from(p.id);
            p.created_tick = 100;
            registry.record(p, usize::MAX, usize::MAX);
        }
    }
    registry.step_with_proximity_distance_transactional(100, no_collision, &players, 3.0, 0);
    let hits =
        registry.step_with_proximity_distance_transactional(101, no_collision, &players, 3.0, 0);
    assert_eq!(hits.len(), MAX_PENDING_HIT_RETRIES_PER_TICK);
    let mut free = projectile(3, 2, Duration::ZERO);
    free.id = 900001;
    free.shot_group_id = 900001;
    free.created_tick = 101;
    free.pos_x = 1000.0;
    free.pos_y = 1000.0;
    free.x = 1000;
    free.y = 1000;
    free.precise_velocity = Some(crate::gameplay_rules::RulesVector::new(100.0, 0.0));
    registry.record(free, usize::MAX, usize::MAX);
    registry.step_with_proximity_distance_transactional(102, no_collision, &players, 3.0, 0);
    assert_eq!(
        registry.iter().find(|p| p.id == free.id).unwrap().pos_x,
        1000.0,
        "authority really pauses moving free flight at exhausted collision work capacity"
    );
    assert!(
        registry.frozen_projectile_ids().contains(&free.id),
        "a capacity-paused moving projectile must tell clients its flight is frozen"
    );
    registry.step_with_proximity_distance_transactional(102, no_collision, &players, 3.0, 0);
    assert!(
        registry.frozen_projectile_ids().contains(&free.id),
        "zero-time queries cannot clear a pause"
    );
    let mut reset = registry.clone();
    reset.clear();
    reset.record(free, usize::MAX, usize::MAX);
    assert!(
        reset.frozen_projectile_ids().is_empty(),
        "arena cleanup cannot leak a pause into a new record"
    );
    let owners = [(free.owner_identity(), (1, 0))].into_iter().collect();
    let repel = registry.apply_repel(
        crate::gameplay_rules::RulesVector::new(900.0, 1000.0),
        99,
        300.0,
        200.0,
        102,
        &owners,
        |_| Some(500),
    );
    assert_eq!(repel.states.len(), 1);
    assert!(
        repel.states[0].motion_frozen,
        "velocity-changing Repel does not free capacity-paused flight"
    );
    assert_eq!(repel.states[0].velocity_x, 300.0);
    for hit in hits {
        assert!(registry.resolve_hit(hit.projectile_id));
    }
    registry.step_with_proximity_distance_transactional(103, no_collision, &players, 3.0, 0);
    assert!(!registry.frozen_projectile_ids().contains(&free.id));
    assert!(
        (registry.iter().find(|p| p.id == free.id).unwrap().pos_x - 1003.0).abs() < 0.001,
        "resumption advances only the current physics step with the new Repel velocity"
    );
    registry.step_with_proximity_distance_transactional(602, no_collision, &players, 3.0, 0);
    assert!(
        registry.is_empty(),
        "the authoritative Repel deadline still owns expiry"
    );
    assert!(registry.frozen_projectile_ids().is_empty());
}

#[test]
fn capacity_pause_owner_limit_resumes_or_expires_without_extending_flight() {
    for expires in [false, true] {
        let mut registry = ProjectileRegistry::default();
        let players = [
            shooter_hull(1, 1, 0.0, 0.0),
            shooter_hull(700, 99, 500.0, 700.0),
        ];
        for id in 1..=MAX_PENDING_HITS_PER_OWNER as u32 {
            let mut p = projectile(1, 2, Duration::ZERO);
            p.id = id;
            p.shot_group_id = u64::from(id);
            p.created_tick = 100;
            registry.record(p, usize::MAX, usize::MAX);
        }
        registry.step_with_proximity_distance_transactional(100, no_collision, &players, 3.0, 0);
        let hits = registry.step_with_proximity_distance_transactional(
            101,
            no_collision,
            &players,
            3.0,
            0,
        );
        assert_eq!(hits.len(), MAX_PENDING_HITS_PER_OWNER);
        let mut free = projectile(1, 2, Duration::ZERO);
        free.id = 900001;
        free.shot_group_id = 900001;
        free.created_tick = 101;
        free.pos_x = 1000.0;
        free.pos_y = 1000.0;
        free.x = 1000;
        free.y = 1000;
        free.precise_velocity = Some(crate::gameplay_rules::RulesVector::new(100.0, 0.0));
        free.expiry_tick = expires.then_some(103);
        registry.record(free, usize::MAX, usize::MAX);
        registry.step_with_proximity_distance_transactional(102, no_collision, &players, 3.0, 0);
        assert!(registry.frozen_projectile_ids().contains(&free.id));
        assert_eq!(
            registry.iter().find(|p| p.id == free.id).unwrap().pos_x,
            1000.0
        );
        if !expires {
            assert!(registry.resolve_hit(hits[0].projectile_id));
        }
        registry.step_with_proximity_distance_transactional(103, no_collision, &players, 3.0, 0);
        assert!(!registry.frozen_projectile_ids().contains(&free.id));
        if expires {
            assert!(!registry.iter().any(|p| p.id == free.id));
        } else {
            assert!(
                (registry.iter().find(|p| p.id == free.id).unwrap().pos_x - 1001.0).abs() < 0.001
            );
        }
    }
}

#[test]
fn capacity_pause_wall_explosion_budget_preserves_then_releases_the_wall_contact() {
    let mut registry = ProjectileRegistry::default();
    for ordinal in 0..=MAX_PENDING_HIT_RETRIES_PER_TICK {
        let (mut p, _) = shrapnel_parent(0, false);
        p.owner_id = (ordinal / MAX_PENDING_HITS_PER_OWNER) as u16 + 1;
        p.id = ordinal as u32 + 1;
        p.shot_group_id = u64::from(p.id);
        p.created_tick = 100;
        p.precise_velocity = Some(crate::gameplay_rules::RulesVector::new(100.0, 0.0));
        let source = p.presentation_source.as_mut().unwrap();
        source.source_player_id = p.owner_id;
        source.source_session_id = u64::from(p.owner_id) + 100;
        source.shot_group_id = p.shot_group_id;
        registry.record(p, usize::MAX, usize::MAX);
    }
    let solid = |x: f32, _: f32| x >= 500.5;
    registry.step_with_proximity_distance_transactional(100, solid, &[], 3.0, 0);
    registry.step_with_proximity_distance_transactional(101, solid, &[], 3.0, 0);
    assert_eq!(
        registry.pending_wall_explosions().count(),
        MAX_PENDING_HIT_RETRIES_PER_TICK
    );
    let paused_id = MAX_PENDING_HIT_RETRIES_PER_TICK as u32 + 1;
    assert!(registry.frozen_projectile_ids().contains(&paused_id));
    assert_eq!(
        registry.iter().find(|p| p.id == paused_id).unwrap().pos_x,
        500.0
    );
    let oldest = registry.pending_wall_explosions().next().unwrap().parent.id;
    registry.resolve_wall_explosion(oldest);
    registry.step_with_proximity_distance_transactional(102, solid, &[], 3.0, 0);
    assert!(!registry.frozen_projectile_ids().contains(&paused_id));
    assert!(!registry.iter().any(|p| p.id == paused_id));
    assert!(
        registry
            .pending_wall_explosions()
            .any(|p| p.parent.id == paused_id),
        "resuming must preserve the real wall explosion rather than fly through it"
    );
}

use crate::vie::vie_rng_next;

pub(crate) const SECURITY_PRIZE_SEED: i32 = 1;
const PRIZE_REPLAY_ATTEMPT_LIMIT: usize = 9999;
#[allow(dead_code)] // server-owned path; shared module also builds in the browser binary
const CONTINUUM_PRIZE_ID_COUNT_INCLUDING_NONE: u32 = 29;

pub(crate) fn generate_weighted_prize_id<const N: usize>(
    weights: &[u8; N],
    weight_total: u16,
    negative_factor: u16,
    negative_allowed: bool,
    rng_seed: &mut i32,
) -> Option<(i16, bool)> {
    if weight_total == 0 {
        return None;
    }

    let roll = vie_rng_next(rng_seed) % weight_total as u32;
    let mut cumulative = 0_u32;
    for (index, weight) in weights.iter().enumerate() {
        cumulative += *weight as u32;
        if roll < cumulative {
            let negative_roll = vie_rng_next(rng_seed);
            let negative = negative_allowed
                && negative_factor > 0
                && negative_roll.is_multiple_of(negative_factor as u32);
            return Some(((index + 1) as i16, negative));
        }
    }

    None
}

pub(crate) fn initial_bounty_prize_allowed_id(prize_id: i16) -> bool {
    crate::gameplay_rules::prize_eligible(
        crate::gameplay_rules::PrizeEligibilityContext::InitialBounty,
        prize_id,
    )
}

pub(crate) fn multiprize_sub_prize_allowed_id(prize_id: i16) -> bool {
    crate::gameplay_rules::prize_eligible(
        crate::gameplay_rules::PrizeEligibilityContext::Multiprize,
        prize_id,
    )
}

pub(crate) fn initial_bounty_prize_ids<const N: usize>(
    weights: &[u8; N],
    weight_total: u16,
    initial_bounty: u32,
    prize_seed: i32,
) -> Vec<i16> {
    replay_prize_ids(
        weights,
        weight_total,
        initial_bounty as usize,
        prize_seed,
        initial_bounty_prize_allowed_id,
    )
}

pub(crate) fn multiprize_sub_prize_ids<const N: usize>(
    weights: &[u8; N],
    weight_total: u16,
    count: u16,
    prize_seed: i32,
) -> Vec<i16> {
    replay_prize_ids(
        weights,
        weight_total,
        count as usize,
        prize_seed,
        multiprize_sub_prize_allowed_id,
    )
}

/// Exact reference-game non-lethal damage-loss roll. The chance draw occurs first;
/// on success the uniform id draw (0=None through 28=Portal) rerolls only the
/// DamageLoss exclusions. The caller owns the stream so production damage
/// order, not browser timing, determines all results.
#[allow(dead_code)] // server-owned path; shared module also builds in the browser binary
pub(crate) fn damage_prize_loss_id(
    rng_seed: &mut i32,
    damage_factor: u16,
    damage: u16,
) -> Option<i16> {
    if damage_factor == 0 || damage == 0 {
        return None;
    }
    // Both inputs are integers on the Continuum wire. Preserve the reference game's
    // truncating division without an f32 round-trip at custom u16 edges.
    let numerator = u64::from(damage_factor) * 200_000;
    let denominator = u64::from(damage) * 1_000;
    let chance = u32::try_from(numerator / denominator)
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    if vie_rng_next(rng_seed) % chance != 0 {
        return None;
    }
    for _ in 0..PRIZE_REPLAY_ATTEMPT_LIMIT {
        let prize_id = (vie_rng_next(rng_seed) % CONTINUUM_PRIZE_ID_COUNT_INCLUDING_NONE) as i16;
        if crate::gameplay_rules::prize_eligible(
            crate::gameplay_rules::PrizeEligibilityContext::DamageLoss,
            prize_id,
        ) {
            return Some(prize_id);
        }
    }
    None
}

#[allow(dead_code)] // server-owned path; shared module also builds in the browser binary
pub(crate) fn none_replacement_prize_id<const N: usize>(
    weights: &[u8; N],
    weight_total: u16,
    rng_seed: &mut i32,
) -> Option<i16> {
    // The reference game saves the damage stream, consumes exactly one `rand()` from
    // it, and uses that value as a temporary prize stream for the complete
    // GeneratePrize reroll loop. Weighted/negative draws must never leak back
    // into the damage stream.
    let mut replacement_seed = vie_rng_next(rng_seed) as i32;
    for _ in 0..PRIZE_REPLAY_ATTEMPT_LIMIT {
        let (prize_id, _) =
            generate_weighted_prize_id(weights, weight_total, 0, false, &mut replacement_seed)?;
        if !matches!(prize_id, 7 | 14 | 17 | 18 | 25) {
            return Some(prize_id);
        }
    }
    None
}

fn replay_prize_ids<const N: usize>(
    weights: &[u8; N],
    weight_total: u16,
    count: usize,
    prize_seed: i32,
    allowed: fn(i16) -> bool,
) -> Vec<i16> {
    if weight_total == 0 || count == 0 {
        return Vec::new();
    }

    let mut seed = prize_seed;
    let mut prizes = Vec::with_capacity(count.min(PRIZE_REPLAY_ATTEMPT_LIMIT));
    let mut attempts = 0_usize;
    while prizes.len() < count && attempts < PRIZE_REPLAY_ATTEMPT_LIMIT {
        attempts = attempts.saturating_add(1);
        let Some((prize_id, _)) =
            generate_weighted_prize_id(weights, weight_total, 0, false, &mut seed)
        else {
            break;
        };

        if allowed(prize_id) {
            prizes.push(prize_id);
        }
    }

    prizes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_loss_roll_is_deterministic_and_never_returns_excluded_ids() {
        let mut successes = Vec::new();
        for initial_seed in 1..=10_000 {
            let mut seed = initial_seed;
            if let Some(prize_id) = damage_prize_loss_id(&mut seed, 1, u16::MAX) {
                assert!((0..=28).contains(&prize_id));
                assert!(!matches!(prize_id, 7 | 13 | 14 | 17 | 18 | 25));
                successes.push((initial_seed, prize_id, seed));
                if successes.len() == 16 {
                    break;
                }
            }
        }
        assert_eq!(successes.len(), 16);
        for (initial_seed, prize_id, final_seed) in successes {
            let mut replay_seed = initial_seed;
            assert_eq!(
                damage_prize_loss_id(&mut replay_seed, 1, u16::MAX),
                Some(prize_id)
            );
            assert_eq!(replay_seed, final_seed);
        }
    }

    #[test]
    fn phase_one_browser_seed_preserves_emp_splash_then_hits_primary() {
        for emp_self_splash_damage in 1..=730 {
            let mut seed = 50;
            assert_eq!(
                damage_prize_loss_id(&mut seed, 30, emp_self_splash_damage),
                None,
                "first draw must miss natural Chaos EMP self-splash damage {emp_self_splash_damage}",
            );
            assert_eq!(
                damage_prize_loss_id(&mut seed, 30, 400),
                Some(22),
                "second draw must select the following ordinary Chaos primary",
            );
        }
    }

    #[test]
    fn disabled_damage_loss_consumes_no_rng() {
        let mut seed = 12345;
        assert_eq!(damage_prize_loss_id(&mut seed, 0, 100), None);
        assert_eq!(damage_prize_loss_id(&mut seed, 100, 0), None);
        assert_eq!(seed, 12345);
    }

    #[test]
    fn damage_loss_denominator_uses_exact_integer_u16_boundaries() {
        for (factor, damage) in [
            (1_u16, 1_u16),
            (u16::MAX, 1),
            (u16::MAX, u16::MAX),
            (32_769, 127),
        ] {
            let expected = ((u64::from(factor) * 200_000) / (u64::from(damage) * 1_000)) as u32 + 1;
            let mut actual_seed = 7;
            let mut expected_seed = actual_seed;
            let draw = vie_rng_next(&mut expected_seed);
            let actual = damage_prize_loss_id(&mut actual_seed, factor, damage);
            if draw % expected != 0 {
                assert_eq!(actual, None);
                assert_eq!(actual_seed, expected_seed);
            }
        }
    }

    #[test]
    fn none_replacement_has_its_own_safe_allowlist_and_zero_weight_edge() {
        let mut zero_seed = 99;
        assert_eq!(none_replacement_prize_id(&[0; 28], 0, &mut zero_seed), None);
        let mut one_draw = 99;
        vie_rng_next(&mut one_draw);
        assert_eq!(zero_seed, one_draw);

        let mut weights = [0_u8; 28];
        weights[13] = 1; // EngineShutdown is unsafe for None replacement.
        weights[0] = 1; // Recharge is safe.
        let mut seed = 1;
        assert_eq!(none_replacement_prize_id(&weights, 2, &mut seed), Some(1));
    }

    #[test]
    fn none_replacement_consumes_exactly_one_damage_stream_draw() {
        let mut weights = [0_u8; 28];
        weights[13] = 255; // rejected EngineShutdown forces temporary rerolls
        weights[0] = 1;
        let mut seed = 17;
        let mut expected = seed;
        vie_rng_next(&mut expected);

        assert_eq!(none_replacement_prize_id(&weights, 256, &mut seed), Some(1));
        assert_eq!(seed, expected);
    }
}

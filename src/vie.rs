pub(crate) const VIE_RNG_MODULUS: u32 = 0x7fff_ffff;

pub(crate) fn vie_rng_next(seed: &mut i32) -> u32 {
    let next = ((*seed % 0x1f31d) as i64 * 0x41a7) - ((*seed / 0x1f31d) as i64 * 0x0b14) + 0x7b;
    *seed = next as i32;
    if *seed < 1 {
        *seed += VIE_RNG_MODULUS as i32;
    }
    *seed as u32
}

/// Advance the VIE affine RNG without replaying every intermediate value.
///
/// The Schrage-form recurrence above is exactly
/// `seed = (16807 * seed + 123) mod (2^31 - 1)` for its positive state
/// domain. Binary affine composition makes an arbitrarily old door anchor a
/// fixed-width operation. The returned work count is useful to server tests
/// which assert that door maintenance is independent of elapsed uptime.
#[allow(dead_code)] // server-bin only
pub(crate) fn vie_rng_advance(seed: &mut i32, steps: u64) -> (u32, usize) {
    const MODULUS: u64 = VIE_RNG_MODULUS as u64;
    const MULTIPLIER: u64 = 0x41a7;
    const INCREMENT: u64 = 0x7b;
    const DIRECT_STEPS: u64 = 32;

    if steps == 0 {
        return (*seed as u32, 0);
    }
    if steps <= DIRECT_STEPS {
        let mut value = *seed as u32;
        for _ in 0..steps {
            value = vie_rng_next(seed);
        }
        return (value, steps as usize);
    }

    // Compute the state immediately before the requested final step, then
    // use the canonical routine once so its exact i32 representation remains
    // the single source of truth.
    let mut exponent = steps - 1;
    let mut result_multiplier = 1_u64;
    let mut result_increment = 0_u64;
    let mut base_multiplier = MULTIPLIER;
    let mut base_increment = INCREMENT;
    let mut work = 0;
    // Always execute one full u64 width. This makes the large-jump work
    // literally independent of elapsed update count, not merely logarithmic
    // in it, while still remaining tiny compared with replaying door ticks.
    for _ in 0..u64::BITS {
        if exponent & 1 != 0 {
            result_multiplier = (base_multiplier * result_multiplier) % MODULUS;
            result_increment = (base_multiplier * result_increment + base_increment) % MODULUS;
        }
        base_increment = (base_multiplier * base_increment + base_increment) % MODULUS;
        base_multiplier = (base_multiplier * base_multiplier) % MODULUS;
        exponent >>= 1;
        work += 1;
    }
    let initial = i64::from(*seed).rem_euclid(MODULUS as i64) as u64;
    let previous = (result_multiplier * initial + result_increment) % MODULUS;
    *seed = if previous == 0 {
        MODULUS as i32
    } else {
        previous as i32
    };
    (vie_rng_next(seed), work + 1)
}

pub(crate) fn randomized_door_mode_seed(rng_seed: &mut i32) -> u8 {
    let mut random = [0_u32; 7];
    for value in &mut random {
        *value = vie_rng_next(rng_seed);
    }

    let mut seed = 0;
    if random[0] & 1 != 0 {
        seed |= 0x11;
    }
    if random[1] & 3 != 0 {
        seed |= 0x02;
    }
    if random[2] & 7 != 0 {
        seed |= 0x04;
    }
    if random[3] & 15 != 0 {
        seed |= 0x08;
    }
    if random[4] & 3 != 0 {
        seed |= 0x20;
    }
    if random[5] & 7 != 0 {
        seed |= 0x40;
    }
    if random[6] & 15 != 0 {
        seed |= 0x80;
    }
    seed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_jump_matches_iterative_recurrence() {
        for initial in [1, 2, 12_345, 0x7fff_ffff] {
            for steps in [0_u64, 1, 2, 31, 32, 33, 999, 100_000] {
                let mut expected = initial;
                let mut expected_value = expected as u32;
                for _ in 0..steps {
                    expected_value = vie_rng_next(&mut expected);
                }

                let mut actual = initial;
                let (actual_value, work) = vie_rng_advance(&mut actual, steps);
                assert_eq!((actual_value, actual), (expected_value, expected));
                assert!(work <= 65, "jump work was not fixed-width: {work}");
            }
        }
    }
}

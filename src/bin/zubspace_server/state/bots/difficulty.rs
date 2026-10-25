//! AI skill only: never changes ship tuning, damage, energy or authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BotDifficulty {
    Hard,
    Medium,
    Easy,
}

impl BotDifficulty {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Hard => "hard",
            Self::Medium => "medium",
            Self::Easy => "easy",
        }
    }

    pub(crate) fn cooldown_extra(self) -> u32 {
        match self {
            Self::Hard => 0,
            Self::Medium => 1,
            Self::Easy => 2,
        }
    }

    pub(crate) fn reaction_ticks(self) -> u32 {
        match self {
            Self::Hard => 0,
            Self::Medium => 20,
            Self::Easy => 40,
        }
    }

    pub(crate) fn aim_error_degrees(self) -> f32 {
        match self {
            Self::Hard => 0.0,
            Self::Medium => 3.0,
            Self::Easy => 7.0,
        }
    }

    pub(crate) fn lead_fraction(self) -> f32 {
        match self {
            Self::Hard => 1.0,
            Self::Medium => 0.85,
            Self::Easy => 0.65,
        }
    }
}

pub(super) fn shuffled_difficulties(count: usize, entropy: &[u8]) -> Vec<BotDifficulty> {
    // At fifteen: 3 hard, 5 medium, 7 easy. Round small rosters toward
    // retaining a hard opponent; a single-bot diagnostic keeps its old AI.
    let hard = count.div_ceil(5);
    let medium = ((count + 1) / 3).min(count - hard);
    let mut tiers = vec![BotDifficulty::Easy; count];
    tiers[..hard].fill(BotDifficulty::Hard);
    tiers[hard..hard + medium].fill(BotDifficulty::Medium);
    for index in (1..count).rev() {
        let offset = index * 8;
        let random = u64::from_le_bytes(entropy[offset..offset + 8].try_into().unwrap());
        tiers.swap(index, (random % (index as u64 + 1)) as usize);
    }
    tiers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roster_has_three_hard_five_medium_seven_easy() {
        let entropy = (0..120).map(|n| n as u8).collect::<Vec<_>>();
        let tiers = shuffled_difficulties(15, &entropy);
        for (tier, count) in [
            (BotDifficulty::Hard, 3),
            (BotDifficulty::Medium, 5),
            (BotDifficulty::Easy, 7),
        ] {
            assert_eq!(tiers.iter().filter(|&&t| t == tier).count(), count);
        }
        assert_eq!(tiers, shuffled_difficulties(15, &entropy));
        assert_ne!(tiers, shuffled_difficulties(15, &[0; 120]));
    }

    #[test]
    fn arbitrary_roster_sizes_are_bounded_and_single_bot_retains_old_ai() {
        for count in 0..=200 {
            let tiers = shuffled_difficulties(count, &vec![0; count * 8]);
            assert_eq!(tiers.len(), count);
            assert_eq!(
                tiers.iter().filter(|&&t| t == BotDifficulty::Hard).count(),
                count.div_ceil(5)
            );
        }
        assert_eq!(shuffled_difficulties(1, &[0; 8]), [BotDifficulty::Hard]);
    }
}

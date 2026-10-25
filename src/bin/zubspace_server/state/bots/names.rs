/// Short ASCII names fit both Continuum's 20-byte roster field and our
/// 14-character player-label convention. This pool is cosmetic; it never
/// consumes the gameplay/prize RNG stream.
const BOT_NAMES: [&str; 40] = [
    "Jekyll",
    "Hyde",
    "Pinocchio",
    "Big Bad Wolf",
    "Little Red",
    "Ace",
    "Mr. Beaver",
    "Gizmo",
    "Wombat",
    "Zaphod",
    "Marvin",
    "Rincewind",
    "Moon Goose",
    "Space Toast",
    "Nibbler",
    "Rocket Rat",
    "Astro Duck",
    "Captain Nope",
    "Luna Tick",
    "Cosmic Bob",
    "Doodlebug",
    "Star Pickle",
    "Orbit Otter",
    "Comet Kid",
    "Nebula Ned",
    "Fizzpop",
    "Moonbeam",
    "Squidward",
    "Hobbes",
    "Calvin",
    "Waldo",
    "Tinkerbell",
    "Puck",
    "Cheshire",
    "Mad Hatter",
    "Mothman",
    "Stardust",
    "Sprocket",
    "Wile E.",
    "Buzzkill",
];

#[cfg(test)]
pub(super) fn bot_names(count: usize) -> Vec<String> {
    bot_names_avoiding(count, &std::collections::BTreeSet::new())
}

pub(super) fn bot_names_avoiding(
    count: usize,
    reserved: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    let mut pool = BOT_NAMES;
    let mut entropy = [0_u8; 40 * 8];
    if getrandom::fill(&mut entropy).is_ok() {
        // Fisher–Yates selection without replacement. A tiny modulo bias is
        // immaterial for display names; uniqueness holds for every permutation.
        for index in (1..pool.len()).rev() {
            let random = u64::from_le_bytes(entropy[index * 8..index * 8 + 8].try_into().unwrap());
            pool.swap(index, (random % (index as u64 + 1)) as usize);
        }
    } else {
        // Entropy availability must not prevent an arena starting. The ordered
        // fallback remains unique and does not affect any game state RNG.
        tracing::warn!("bot name shuffle unavailable; using ordered name pool");
    }
    let mut used = reserved.clone();
    let mut names = Vec::with_capacity(count);
    for name in pool {
        if used.insert(crate::identity::name_key(name)) {
            names.push(name.to_owned());
            if names.len() == count {
                return names;
            }
        }
    }
    let mut number = 1;
    while names.len() < count {
        let name = format!("Bot {number}");
        number += 1;
        if used.insert(crate::identity::name_key(&name)) {
            names.push(name);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn forty_names_are_unique_short_and_protocol_safe() {
        assert_eq!(BOT_NAMES.len(), 40);
        assert_eq!(BOT_NAMES.into_iter().collect::<BTreeSet<_>>().len(), 40);
        for name in BOT_NAMES {
            assert!(
                name.is_ascii() && !name.is_empty() && name.len() <= 14,
                "{name}"
            );
            assert!(
                name.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b' ' || b == b'.')
            );
        }
    }

    #[test]
    fn selection_uses_entire_pool_without_replacement_then_unique_numbered_fallback() {
        assert!(bot_names(0).is_empty());
        let names = bot_names(45);
        assert_eq!(names.iter().collect::<BTreeSet<_>>().len(), 45);
        assert_eq!(
            names[..40]
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BOT_NAMES.into_iter().collect()
        );
        assert_eq!(&names[40..], ["Bot 1", "Bot 2", "Bot 3", "Bot 4", "Bot 5"]);
    }
}

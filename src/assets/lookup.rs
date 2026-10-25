use super::*;

impl Assets {
    pub(crate) fn chat_sound(&self, sound: u8) -> Option<&Sound> {
        self.chat_sounds
            .get(chat_sound_index(sound)?)
            .and_then(Option::as_ref)
    }

    pub(crate) fn sheet_for_tile(&self, id: u8) -> Option<&SpriteSheet> {
        match id {
            FLAG_TILE_ID => Some(&self.flag),
            GOAL_TILE_ID => Some(&self.goal),
            216 => Some(&self.asteroid_small1),
            217 => Some(&self.asteroid_large),
            218 => Some(&self.asteroid_small2),
            219 => Some(&self.station),
            WORMHOLE_TILE_ID => Some(&self.wormhole),
            _ => None,
        }
    }
}

pub(crate) fn chat_sound_index(sound: u8) -> Option<usize> {
    let index = sound.checked_sub(1)? as usize;
    (index < 26).then_some(index)
}

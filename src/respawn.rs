pub(crate) const MIN_RESPAWN_DELAY_SECONDS: f32 = 2.0;
pub(crate) const RESPAWN_EXPLOSION_LIFETIME_SECONDS: f32 = 1.25;

pub(crate) fn respawn_delay_seconds(enter_delay_seconds: f32) -> f32 {
    let configured_delay = if enter_delay_seconds <= 0.0 {
        0.0
    } else {
        enter_delay_seconds + RESPAWN_EXPLOSION_LIFETIME_SECONDS
    };
    configured_delay.max(MIN_RESPAWN_DELAY_SECONDS)
}

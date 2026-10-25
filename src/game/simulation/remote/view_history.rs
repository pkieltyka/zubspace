use super::*;

pub(super) fn captures_remote_weapon_view(
    networked: bool,
    scope: AuthorityScopeIdentity,
    projectile: &RemoteProjectile,
) -> bool {
    networked
        && scope.player_id != Some(projectile.source.identity.player_id())
        && projectile.source.arena_scope.is_some_and(|arena| {
            scope.server_session_id == Some(arena.server_session_id)
                && scope.arena_stream_id == Some(arena.arena_stream_id)
        })
}

pub(super) fn record_weapon_view_step(
    history: &mut crate::remote_weapon::RemoteWeaponViewHistory,
    projectile: &RemoteProjectile,
    start_tick: u32,
    start_age: f32,
    expiry_tick: u32,
    elapsed: f32,
) {
    let tick = crate::continuum::continuum_tick_add(start_tick, (elapsed * 100.0).round() as u32);
    let mut sample = *projectile;
    sample.age = (start_age + elapsed).max(0.0);
    let expiry_tick = if sample.ttl <= 0.0 { tick } else { expiry_tick };
    sample.ttl = crate::continuum::ticks_to_seconds_i32(
        crate::continuum::continuum_tick_diff(expiry_tick, tick).max(0),
    );
    sample.presentation_clock = Some(crate::remote_weapon::RemoteProjectileClock {
        pose_tick: tick,
        expiry_tick,
        render_partial_seconds: 0.0,
        motion_frozen: sample
            .presentation_clock
            .is_some_and(|clock| clock.motion_frozen),
    });
    history.record(&sample, tick);
}

impl Game {
    pub(in crate::game) fn record_remote_weapon_view(
        &mut self,
        projectile: &RemoteProjectile,
        tick: u32,
    ) {
        if captures_remote_weapon_view(
            self.networked_play,
            self.client_authority.view().scope,
            projectile,
        ) {
            self.remote_weapon_view_history.record(projectile, tick);
        }
    }
}

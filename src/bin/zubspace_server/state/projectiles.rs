use super::{ServerProjectile, ServerState};

const MAX_PROJECTILES_PER_PLAYER: usize = 128;
const MAX_ACTIVE_PROJECTILES: usize = 32_768;

impl ServerState {
    pub(crate) fn note_owner_weapon_group(
        &mut self,
        source: crate::realtime_event::RealtimeWeaponEvent,
    ) {
        if source.source_session_id == 0 || source.shot_group_id == 0 {
            return;
        }
        let session = crate::authority_identity::PlayerSessionIdentity::new(
            source.source_player_id,
            crate::authority_identity::PlayerSessionId::new(source.source_session_id),
        );
        let latest = self.latest_owner_weapon_groups.entry(session).or_default();
        *latest = (*latest).max(source.shot_group_id);
    }

    pub(crate) fn can_record_explosion_fan(&self, owner_id: u16, count: usize) -> bool {
        self.active_projectiles.can_record_group_addition(
            owner_id,
            count,
            MAX_PROJECTILES_PER_PLAYER,
            MAX_ACTIVE_PROJECTILES,
        )
    }

    pub(crate) fn projectile_cancelled_by_owner(&self, projectile: &ServerProjectile) -> bool {
        projectile.presentation_source.is_some_and(|source| {
            if source.source_session_id == 0 {
                return false;
            }
            let session = crate::authority_identity::PlayerSessionIdentity::new(
                source.source_player_id,
                crate::authority_identity::PlayerSessionId::new(source.source_session_id),
            );
            source.source_player_id == projectile.owner_id
                && source.source_life_seq == projectile.owner_life_seq
                && source.shot_group_id == projectile.shot_group_id
                && self
                    .owner_weapon_cancellation_watermarks
                    .get(&session)
                    .is_some_and(|through| projectile.shot_group_id <= *through)
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn record_projectile(&mut self, projectile: ServerProjectile) {
        if let Some(source) = projectile.presentation_source {
            self.note_owner_weapon_group(source);
        }
        if self.projectile_cancelled_by_owner(&projectile) {
            return;
        }
        self.active_projectiles.record(
            projectile,
            MAX_PROJECTILES_PER_PLAYER,
            MAX_ACTIVE_PROJECTILES,
        );
    }

    pub(crate) fn record_projectile_group(&mut self, mut projectiles: Vec<ServerProjectile>) {
        for projectile in &projectiles {
            if let Some(source) = projectile.presentation_source {
                self.note_owner_weapon_group(source);
            }
        }
        projectiles.retain(|projectile| !self.projectile_cancelled_by_owner(projectile));
        self.active_projectiles.record_group(
            projectiles,
            MAX_PROJECTILES_PER_PLAYER,
            MAX_ACTIVE_PROJECTILES,
        );
    }

    pub(crate) fn expire_projectiles_older_than_by_weapon(
        &mut self,
        tick: u32,
        default_max_age_ticks: u32,
        mine_max_age_ticks: u32,
    ) {
        self.active_projectiles.expire_older_than_by_weapon(
            tick,
            default_max_age_ticks,
            mine_max_age_ticks,
        );
    }
}

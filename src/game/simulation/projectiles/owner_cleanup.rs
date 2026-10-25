use super::*;

impl Game {
    pub(in crate::game) fn remove_safe_zone_owner_weapons(&mut self) {
        let local_owner_safe = weapon_owner_position_in_safe_tile(self.map(), Some(self.ship.pos));
        if local_owner_safe {
            self.bullets.clear();
            self.bombs.clear();
            self.mines.clear();
            self.shrapnel.clear();
            self.thors.clear();
            self.decoys.retain(|decoy| decoy.remote_source.is_some());
        }

        // A delayed remote safe-position sample cannot cancel a newer shot
        // fired after egress. Network flight is retired by the authoritative
        // session/shot-group safety cutoff, independently of actor AOI.
        if self.networked_play {
            return;
        }

        // The roster does not carry the remote connection/session id needed
        // until a current-life authenticated 0x7d fact binds it. Only an exact
        // runtime/life match may remove that source's weapons; an absent bind
        // or a replacement occupant fails closed.
        let map = self.map();
        let safe_remote_sources: BTreeSet<RemoteWeaponSourceIdentity> = self
            .network_world
            .players
            .values()
            .filter(|player| weapon_owner_position_in_safe_tile(map, player.position))
            .filter_map(|player| {
                Some(RemoteWeaponSourceIdentity::Authenticated {
                    player_id: player.player_id,
                    session_id: player.source_session_id?,
                    life_seq: player.life_seq?,
                })
            })
            .collect();
        remove_exact_safe_remote_sources(
            &mut self.remote_projectiles,
            &mut self.decoys,
            &safe_remote_sources,
        );
    }
}

fn remove_exact_safe_remote_sources(
    projectiles: &mut Vec<RemoteProjectile>,
    decoys: &mut Vec<Decoy>,
    safe_sources: &BTreeSet<RemoteWeaponSourceIdentity>,
) {
    projectiles.retain(|projectile| !safe_sources.contains(&projectile.source.identity));
    decoys.retain(|decoy| {
        decoy
            .remote_source
            .is_none_or(|source| !safe_sources.contains(&source.identity))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(identity: RemoteWeaponSourceIdentity, bomb_bounces: u8) -> RemoteWeaponSource {
        let mut tuning = ShipTuning::continuum_skirmish_default();
        tuning.bomb_bounces = bomb_bounces;
        RemoteWeaponSource::capture(
            identity,
            RemoteWeaponArenaScope::authenticated(u64::MAX, u64::MAX),
            vec2(100.0, 200.0),
            vec2(4.0, -3.0),
            0.75,
            7,
            0,
            tuning,
            true,
        )
        .unwrap()
    }

    #[test]
    fn owner_replacement_and_session_life_wrap_cannot_remove_old_bomb_mine_or_decoy() {
        let old_identity =
            RemoteWeaponSourceIdentity::authenticated(9, u64::MAX, u32::MAX).unwrap();
        let replacement_identity = RemoteWeaponSourceIdentity::authenticated(9, 1, 1).unwrap();
        let old_source = source(old_identity, 7);
        let arena = ArenaTuning::continuum_skirmish_default();
        let tuning = arena.ship(0);
        let mut projectiles = vec![
            remote_weapon_projectiles(old_source, 3, tuning, &arena).remove(0),
            remote_weapon_projectiles(old_source, 3 | (1 << 15), tuning, &arena).remove(0),
        ];
        let mut decoys = vec![Decoy {
            authority_binding: None,
            presentation_clock: None,
            prediction: None,
            pos: old_source.fire_position,
            vel: old_source.fire_velocity,
            frequency: old_source.frequency,
            orientation: old_source.fire_orientation,
            initial_orientation: old_source.fire_orientation,
            remote_source: Some(old_source),
            last_repel_mutation_event_id: None,
            authoritative_repel_state: None,
            ship_type: old_source.ship as usize,
            owner_name: "old".to_owned(),
            owner_bounty: 0,
            ttl: 10.0,
            age: 0.0,
        }];

        remove_exact_safe_remote_sources(
            &mut projectiles,
            &mut decoys,
            &BTreeSet::from([replacement_identity]),
        );

        assert_eq!(projectiles.len(), 2);
        assert_eq!(decoys.len(), 1);
        assert!(projectiles.iter().all(|projectile| {
            projectile.source.identity == old_identity
                && projectile.source.bomb_bounces == 7
                && projectile.ttl > 0.0
        }));
        assert_eq!(decoys[0].remote_source, Some(old_source));

        remove_exact_safe_remote_sources(
            &mut projectiles,
            &mut decoys,
            &BTreeSet::from([old_identity]),
        );
        assert!(projectiles.is_empty());
        assert!(decoys.is_empty());
    }
}

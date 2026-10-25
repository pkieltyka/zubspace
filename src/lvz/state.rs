use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use crate::{LvzModifyPacket, LvzTogglePacket};

use super::{
    LVZ_DISPLAY_DEATH, LVZ_DISPLAY_ENTER_ARENA, LVZ_DISPLAY_ENTER_ZONE, LVZ_DISPLAY_KILL,
    LVZ_DISPLAY_SERVER_CONTROLLED, LVZ_DISPLAY_SHOW_ALWAYS, LvzObjectDefinition, LvzRenderCache,
    LvzRenderableObject, lvz_display_time_seconds,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct NetworkLvzState {
    pub(crate) enabled_objects: BTreeSet<u16>,
    pub(crate) enabled_object_ages: BTreeMap<u16, f32>,
    pub(crate) disabled_objects: BTreeSet<u16>,
    pub(crate) modified_objects: BTreeMap<u16, LvzModifyPacket>,
    zone_entry_age_seconds: f32,
    arena_entry_age_seconds: f32,
    kill_age_seconds: Option<f32>,
    death_age_seconds: Option<f32>,
}

impl NetworkLvzState {
    pub(crate) fn apply_toggle(&mut self, packet: &LvzTogglePacket) {
        for toggle in &packet.toggles {
            if toggle.enabled {
                self.enabled_objects.insert(toggle.object_id);
                self.enabled_object_ages.insert(toggle.object_id, 0.0);
                self.disabled_objects.remove(&toggle.object_id);
            } else {
                self.enabled_objects.remove(&toggle.object_id);
                self.enabled_object_ages.remove(&toggle.object_id);
                self.disabled_objects.insert(toggle.object_id);
            }
        }
    }

    pub(crate) fn apply_modify(&mut self, packet: LvzModifyPacket) {
        self.modified_objects.insert(packet.object_id, packet);
    }

    /// Whether any server modification is in effect. The per-pass draw bands are
    /// built from each object's merged class and layer, and a modify packet can
    /// change both, so a pass may only use them while this is false.
    pub(crate) fn has_modifications(&self) -> bool {
        !self.modified_objects.is_empty()
    }

    pub(crate) fn trigger_kill(&mut self) {
        self.kill_age_seconds = Some(0.0);
    }

    pub(crate) fn trigger_death(&mut self) {
        self.death_age_seconds = Some(0.0);
    }

    pub(crate) fn update(&mut self, dt: f32, render_cache: &LvzRenderCache) {
        self.update_entry_ages(dt);
        self.update_triggered_display_ages(dt);
        if self.enabled_object_ages.is_empty() {
            return;
        }

        let mut expired = Vec::new();
        let object_ids: Vec<u16> = self.enabled_object_ages.keys().copied().collect();
        for object_id in object_ids {
            let Some(age) = self.enabled_object_ages.get_mut(&object_id) else {
                continue;
            };
            *age += dt;
            let age = *age;
            let Some(display_time) = render_cache.expiration_seconds_for_object(object_id, self)
            else {
                continue;
            };
            if display_time > 0.0 && age >= display_time {
                expired.push(object_id);
            }
        }

        for object_id in expired {
            self.enabled_objects.remove(&object_id);
            self.enabled_object_ages.remove(&object_id);
            self.disabled_objects.insert(object_id);
        }
    }

    #[cfg(test)]
    pub(crate) fn object_is_active(&self, object: &LvzObjectDefinition) -> bool {
        self.enabled_objects.contains(&object.object_id)
            || (object.display_mode == LVZ_DISPLAY_SHOW_ALWAYS
                && !self.disabled_objects.contains(&object.object_id))
            || (!self.disabled_objects.contains(&object.object_id)
                && (self.entry_display_mode_is_active(object)
                    || self.triggered_display_mode_is_active(object)))
    }

    pub(crate) fn renderable_is_active(
        &self,
        renderable: &LvzRenderableObject,
        object: &LvzObjectDefinition,
    ) -> bool {
        if self.disabled_objects.contains(&object.object_id) {
            return false;
        }
        if self.enabled_objects.contains(&object.object_id) {
            return renderable.first_for_id;
        }
        object.display_mode == LVZ_DISPLAY_SHOW_ALWAYS
            || self.entry_display_mode_is_active(object)
            || self.triggered_display_mode_is_active(object)
    }

    pub(crate) fn renderable_animation_time(
        &self,
        renderable: &LvzRenderableObject,
        fallback_time: f32,
    ) -> f32 {
        if renderable.first_for_id {
            if let Some(age) = self.enabled_object_ages.get(&renderable.object.object_id) {
                return *age;
            }
        }
        if let Some(age) = self.triggered_display_mode_age(renderable.object.display_mode) {
            return age;
        }
        fallback_time
    }

    /// The object as this frame should draw it. Borrowed unless a server
    /// modification actually applies to it: the LVZ passes call this for every
    /// object they visit, and an owned return made every visit copy the
    /// definition whether or not anything had modified the object.
    pub(crate) fn effective_renderable_object<'a>(
        &self,
        renderable: &'a LvzRenderableObject,
    ) -> Cow<'a, LvzObjectDefinition> {
        let Some(modification) = renderable
            .first_for_id
            .then(|| self.modified_objects.get(&renderable.object.object_id))
            .flatten()
        else {
            return Cow::Borrowed(&renderable.object);
        };
        Cow::Owned(renderable.object.modified(Some(modification)))
    }

    #[cfg(test)]
    pub(crate) fn update_with_objects<'a, I>(&mut self, dt: f32, objects: I)
    where
        I: IntoIterator<Item = &'a LvzObjectDefinition>,
    {
        self.update_entry_ages(dt);
        self.update_triggered_display_ages(dt);
        if self.enabled_object_ages.is_empty() {
            return;
        }

        let mut object_by_id: BTreeMap<u16, Vec<&LvzObjectDefinition>> = BTreeMap::new();
        for object in objects {
            object_by_id
                .entry(object.object_id)
                .or_default()
                .push(object);
        }
        let mut expired = Vec::new();
        let object_ids: Vec<u16> = self.enabled_object_ages.keys().copied().collect();
        for object_id in object_ids {
            let Some(age) = self.enabled_object_ages.get_mut(&object_id) else {
                continue;
            };
            *age += dt;
            let age = *age;
            let Some(objects) = object_by_id.get(&object_id) else {
                continue;
            };
            let object = objects[0].modified(self.modified_objects.get(&object_id));
            let display_time = lvz_display_time_seconds(object.display_time);
            if display_time > 0.0 && age >= display_time {
                expired.push(object_id);
            }
        }

        for object_id in expired {
            self.enabled_objects.remove(&object_id);
            self.enabled_object_ages.remove(&object_id);
            self.disabled_objects.insert(object_id);
        }
    }

    pub(crate) fn reset_arena_entry_age(&mut self) {
        self.arena_entry_age_seconds = 0.0;
    }

    fn update_entry_ages(&mut self, dt: f32) {
        self.zone_entry_age_seconds += dt.max(0.0);
        self.arena_entry_age_seconds += dt.max(0.0);
    }

    fn update_triggered_display_ages(&mut self, dt: f32) {
        let dt = dt.max(0.0);
        if let Some(age) = self.kill_age_seconds.as_mut() {
            *age += dt;
        }
        if let Some(age) = self.death_age_seconds.as_mut() {
            *age += dt;
        }
    }

    fn entry_display_mode_is_active(&self, object: &LvzObjectDefinition) -> bool {
        let age = match object.display_mode {
            LVZ_DISPLAY_ENTER_ZONE => self.zone_entry_age_seconds,
            LVZ_DISPLAY_ENTER_ARENA => self.arena_entry_age_seconds,
            _ => return false,
        };
        let display_time = lvz_display_time_seconds(object.display_time);
        display_time <= 0.0 || age < display_time
    }

    fn triggered_display_mode_age(&self, display_mode: u8) -> Option<f32> {
        match display_mode {
            LVZ_DISPLAY_KILL => self.kill_age_seconds,
            LVZ_DISPLAY_DEATH => self.death_age_seconds,
            LVZ_DISPLAY_SERVER_CONTROLLED => None,
            _ => None,
        }
    }

    fn triggered_display_mode_is_active(&self, object: &LvzObjectDefinition) -> bool {
        let Some(age) = self.triggered_display_mode_age(object.display_mode) else {
            return false;
        };
        let display_time = lvz_display_time_seconds(object.display_time);
        display_time <= 0.0 || age < display_time
    }
}

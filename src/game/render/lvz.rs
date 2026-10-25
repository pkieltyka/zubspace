use super::*;
use crate::lvz::{LvzRenderableObject, lvz_map_object_world_position, lvz_object_animation_frame};

impl Game {
    pub(super) fn draw_lvz_map_objects_for_layers(&self, layer_start: u8, layer_end: u8) {
        if self.lvz_render_cache.objects.is_empty() {
            return;
        }

        let time = get_time() as f32;
        let half_screen = vec2(screen_width(), screen_height()) * 0.5;
        let min = self.camera.target - half_screen - vec2(256.0, 256.0);
        let max = self.camera.target + half_screen + vec2(256.0, 256.0);

        // Three walks, narrowest first: the cells the viewport covers, then the
        // band that owns these layers, then the whole table.
        let visible = self.lvz_render_cache.map_band_visible_objects(
            &self.network_world.lvz,
            layer_start,
            layer_end,
            min,
            max,
        );
        if let Some(indices) = &visible {
            for index in indices {
                let Some(renderable) = self.lvz_render_cache.renderable_at(*index) else {
                    continue;
                };
                self.draw_lvz_map_renderable(renderable, layer_start, layer_end, min, max, time);
            }
            return;
        }

        match self.map_band_objects(layer_start, layer_end) {
            Some(band) => {
                for index in band
                    .iter()
                    .chain(self.lvz_render_cache.unbanded_objects().iter())
                {
                    let Some(renderable) = self.lvz_render_cache.renderable_at(*index) else {
                        continue;
                    };
                    self.draw_lvz_map_renderable(
                        renderable,
                        layer_start,
                        layer_end,
                        min,
                        max,
                        time,
                    );
                }
            }
            None => {
                for renderable in &self.lvz_render_cache.objects {
                    self.draw_lvz_map_renderable(
                        renderable,
                        layer_start,
                        layer_end,
                        min,
                        max,
                        time,
                    );
                }
            }
        }
    }

    /// The band objects this map pass is the sole owner of, or `None` when it has
    /// to walk the whole table. The cache declines the band while a server
    /// modification is in effect, because a modify packet can move an object to
    /// another class or layer than the merged one the bands were built from.
    fn map_band_objects(&self, layer_start: u8, layer_end: u8) -> Option<&[u32]> {
        self.lvz_render_cache
            .map_band_objects(&self.network_world.lvz, layer_start, layer_end)
    }

    fn screen_band_objects(&self, layer_start: u8, layer_end: u8) -> Option<&[u32]> {
        self.lvz_render_cache
            .screen_band_objects(&self.network_world.lvz, layer_start, layer_end)
    }

    fn draw_lvz_map_renderable(
        &self,
        renderable: &LvzRenderableObject,
        layer_start: u8,
        layer_end: u8,
        min: Vec2,
        max: Vec2,
        time: f32,
    ) {
        let object = self
            .network_world
            .lvz
            .effective_renderable_object(renderable);
        if !object.map_object
            || !self
                .network_world
                .lvz
                .renderable_is_active(renderable, &object)
        {
            return;
        }
        if !lvz_layer_in_range(object.layer, layer_start, layer_end) {
            return;
        }

        let Some(image) = renderable.images.get(object.image_number as usize) else {
            return;
        };
        // Merge-time resolution. Holding the frame size here is what lets
        // the viewport cull run before the pass touches the texture table.
        let Some(resolved) = self
            .lvz_render_cache
            .resolved_image(renderable.image_slot(object.image_number))
        else {
            return;
        };

        let x_count = image.x_count.max(1) as f32;
        let y_count = image.y_count.max(1) as f32;
        let frame_size = vec2(resolved.width / x_count, resolved.height / y_count);
        let pos = lvz_map_object_world_position(object.x_map, object.y_map);
        if pos.x > max.x
            || pos.y > max.y
            || pos.x + frame_size.x < min.x
            || pos.y + frame_size.y < min.y
        {
            return;
        }

        let Some(texture) = self
            .lvz_render_cache
            .texture_for_name(&resolved.texture_name)
        else {
            return;
        };
        let frame_count = (x_count * y_count).max(1.0) as usize;
        let animation_time = self
            .network_world
            .lvz
            .renderable_animation_time(renderable, time);
        let frame = lvz_object_animation_frame(animation_time, image.animation_time, frame_count);
        let source = lvz_source_for_frame(frame_size, frame, x_count as usize);

        draw_texture_ex(
            &texture.texture,
            pos.x,
            pos.y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(frame_size),
                source: Some(source),
                ..Default::default()
            },
        );
    }

    pub(super) fn draw_lvz_screen_objects_for_layers(&self, layer_start: u8, layer_end: u8) {
        if self.lvz_render_cache.objects.is_empty() {
            return;
        }

        let time = get_time() as f32;
        match self.screen_band_objects(layer_start, layer_end) {
            Some(band) => {
                for index in band
                    .iter()
                    .chain(self.lvz_render_cache.unbanded_objects().iter())
                {
                    let Some(renderable) = self.lvz_render_cache.renderable_at(*index) else {
                        continue;
                    };
                    self.draw_lvz_screen_renderable(renderable, layer_start, layer_end, time);
                }
            }
            None => {
                for renderable in &self.lvz_render_cache.objects {
                    self.draw_lvz_screen_renderable(renderable, layer_start, layer_end, time);
                }
            }
        }
    }

    fn draw_lvz_screen_renderable(
        &self,
        renderable: &LvzRenderableObject,
        layer_start: u8,
        layer_end: u8,
        time: f32,
    ) {
        let object = self
            .network_world
            .lvz
            .effective_renderable_object(renderable);
        if object.map_object
            || !self
                .network_world
                .lvz
                .renderable_is_active(renderable, &object)
        {
            return;
        }
        if !lvz_layer_in_range(object.layer, layer_start, layer_end) {
            return;
        }

        let Some(image) = renderable.images.get(object.image_number as usize) else {
            return;
        };
        let Some(resolved) = self
            .lvz_render_cache
            .resolved_image(renderable.image_slot(object.image_number))
        else {
            return;
        };

        let x_count = image.x_count.max(1) as f32;
        let y_count = image.y_count.max(1) as f32;
        let frame_size = vec2(resolved.width / x_count, resolved.height / y_count);
        let frame_count = (x_count * y_count).max(1.0) as usize;
        let animation_time = self
            .network_world
            .lvz
            .renderable_animation_time(renderable, time);
        let Some(texture) = self
            .lvz_render_cache
            .texture_for_name(&resolved.texture_name)
        else {
            return;
        };
        let frame = lvz_object_animation_frame(animation_time, image.animation_time, frame_count);
        let source = lvz_source_for_frame(frame_size, frame, x_count as usize);
        let base_x = lvz_screen_reference_point(object.x_type);
        let base_y = lvz_screen_reference_point(object.y_type);
        let pos = vec2(
            base_x.x + object.x_screen as f32,
            base_y.y + object.y_screen as f32,
        );

        draw_texture_ex(
            &texture.texture,
            pos.x,
            pos.y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(frame_size),
                source: Some(source),
                ..Default::default()
            },
        );
    }
}

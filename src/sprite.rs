use crate::bm2::{Transparency, texture_from_bm2};
use macroquad::prelude::*;

pub(crate) struct SpriteSheet {
    pub(crate) texture: Texture2D,
    pub(crate) frame_size: Vec2,
    duration: f32,
}

impl SpriteSheet {
    pub(crate) fn from_bm2(data: &[u8], frame_size: Vec2, duration: f32) -> Self {
        let texture = texture_from_bm2(data, Transparency::Black);
        texture.set_filter(FilterMode::Nearest);
        Self {
            texture,
            frame_size,
            duration,
        }
    }

    pub(crate) fn frame_count(&self) -> usize {
        let columns = (self.texture.width() / self.frame_size.x) as usize;
        let rows = (self.texture.height() / self.frame_size.y) as usize;
        columns * rows
    }

    pub(crate) fn source_at(&self, time: f32) -> Rect {
        self.source_at_with_count(time, self.frame_count())
    }

    pub(crate) fn source_at_with_count(&self, time: f32, frame_count: usize) -> Rect {
        let frame_count = frame_count.max(1);
        let frame =
            ((time.rem_euclid(self.duration) / self.duration) * frame_count as f32) as usize;
        self.source_for_frame(frame)
    }

    pub(crate) fn source_at_range(
        &self,
        time: f32,
        frame_start: usize,
        frame_count: usize,
    ) -> Rect {
        let frame_count = frame_count.max(1);
        let frame =
            ((time.clamp(0.0, self.duration) / self.duration) * frame_count as f32) as usize;
        self.source_for_frame(frame_start + frame.min(frame_count - 1))
    }

    pub(crate) fn source_loop_range(
        &self,
        time: f32,
        frame_start: usize,
        frame_count: usize,
    ) -> Rect {
        let frame_count = frame_count.max(1);
        let frame =
            ((time.rem_euclid(self.duration) / self.duration) * frame_count as f32) as usize;
        self.source_for_frame(frame_start + frame.min(frame_count - 1))
    }

    pub(crate) fn source_loop_range_with_duration(
        &self,
        time: f32,
        frame_start: usize,
        frame_count: usize,
        duration: f32,
    ) -> Rect {
        let frame_count = frame_count.max(1);
        let duration = duration.max(f32::EPSILON);
        let frame = ((time.rem_euclid(duration) / duration) * frame_count as f32) as usize;
        self.source_for_frame(frame_start + frame.min(frame_count - 1))
    }

    pub(crate) fn source_for_frame(&self, frame: usize) -> Rect {
        let columns = (self.texture.width() / self.frame_size.x) as usize;

        Rect::new(
            (frame % columns) as f32 * self.frame_size.x,
            (frame / columns) as f32 * self.frame_size.y,
            self.frame_size.x,
            self.frame_size.y,
        )
    }
}

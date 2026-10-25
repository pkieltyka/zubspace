use crate::bm2::{Transparency, texture_from_bm2};
use macroquad::prelude::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextColor {
    White,
    Green,
    Blue,
    DarkRed,
    Yellow,
}

impl TextColor {
    fn row(self) -> usize {
        match self {
            Self::White => 0,
            Self::Green => 1,
            Self::Blue => 2,
            Self::DarkRed => 3,
            Self::Yellow => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextAlign {
    Left,
    Center,
    Right,
}

pub(crate) struct BitmapFont {
    pub(crate) texture: Texture2D,
    frame_size: Vec2,
    frames_per_color: usize,
}

impl BitmapFont {
    pub(crate) fn from_bm2(data: &[u8], frame_size: Vec2, frames_per_color: usize) -> Self {
        let texture = texture_from_bm2(data, Transparency::Black);
        texture.set_filter(FilterMode::Nearest);
        Self {
            texture,
            frame_size,
            frames_per_color,
        }
    }

    pub(crate) fn draw_scaled(
        &self,
        text: &str,
        x: f32,
        y: f32,
        color: TextColor,
        align: TextAlign,
        scale: f32,
    ) {
        self.draw_tinted_scaled(text, x, y, color, WHITE, align, scale);
    }

    pub(crate) fn draw_tinted_scaled(
        &self,
        text: &str,
        x: f32,
        y: f32,
        color: TextColor,
        tint: Color,
        align: TextAlign,
        scale: f32,
    ) {
        let scale = scale.max(0.25);
        let glyph_size = self.frame_size * scale;
        let width = self.measure_width_scaled(text, scale);
        let mut cursor = vec2(
            match align {
                TextAlign::Left => x,
                TextAlign::Center => x - width * 0.5,
                TextAlign::Right => x - width,
            },
            y,
        );
        let start_x = cursor.x;

        for byte in text.bytes() {
            if byte == b'\n' {
                cursor.x = start_x;
                cursor.y += glyph_size.y;
                continue;
            }

            if let Some(source) = self.source_for_byte(byte, color) {
                draw_texture_ex(
                    &self.texture,
                    cursor.x,
                    cursor.y,
                    tint,
                    DrawTextureParams {
                        source: Some(source),
                        dest_size: Some(glyph_size),
                        ..Default::default()
                    },
                );
            }
            cursor.x += glyph_size.x;
        }
    }

    pub(crate) fn measure_width(&self, text: &str) -> f32 {
        let max_line_len = text.split('\n').map(str::len).max().unwrap_or(0);
        max_line_len as f32 * self.frame_size.x
    }

    pub(crate) fn measure_width_scaled(&self, text: &str, scale: f32) -> f32 {
        self.measure_width(text) * scale.max(0.25)
    }

    pub(crate) fn line_height(&self) -> f32 {
        self.frame_size.y
    }

    pub(crate) fn line_height_scaled(&self, scale: f32) -> f32 {
        self.line_height() * scale.max(0.25)
    }

    fn source_for_byte(&self, byte: u8, color: TextColor) -> Option<Rect> {
        let char_index = ascii_font_index(byte)?;
        let frame = color.row() * self.frames_per_color + char_index;
        let columns = (self.texture.width() / self.frame_size.x) as usize;
        Some(Rect::new(
            (frame % columns) as f32 * self.frame_size.x,
            (frame / columns) as f32 * self.frame_size.y,
            self.frame_size.x,
            self.frame_size.y,
        ))
    }
}

fn ascii_font_index(byte: u8) -> Option<usize> {
    if (b' '..=b'~').contains(&byte) {
        Some(usize::from(byte - b' '))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_font_indices_match_reference_game_tallfont_order() {
        assert_eq!(ascii_font_index(b' '), Some(0));
        assert_eq!(ascii_font_index(b'!'), Some(1));
        assert_eq!(ascii_font_index(b'A'), Some(33));
        assert_eq!(ascii_font_index(b'~'), Some(94));
        assert_eq!(ascii_font_index(b'\n'), None);
        assert_eq!(ascii_font_index(0x80), None);
    }
}

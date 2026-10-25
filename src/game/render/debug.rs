use super::*;
use crate::tile::{is_door_tile, is_solid_tile_with_doors};

impl Game {
    pub(super) fn draw_hidden_solid_tile_debug_overlay(&self) {
        if !self.developer_overlay || !crate::browser_metrics::debug_mode() {
            return;
        }

        let map = self.map();
        let half_screen = vec2(screen_width(), screen_height()) * 0.5;
        let min = self.camera.target - half_screen - vec2(TILE_SIZE * 2.0, TILE_SIZE * 2.0);
        let max = self.camera.target + half_screen + vec2(TILE_SIZE * 2.0, TILE_SIZE * 2.0);
        let min_x = (min.x / TILE_SIZE).floor().max(0.0) as i32;
        let min_y = (min.y / TILE_SIZE).floor().max(0.0) as i32;
        let max_x = (max.x / TILE_SIZE).ceil().min(map.width as f32) as i32;
        let max_y = (max.y / TILE_SIZE).ceil().min(map.height as f32) as i32;

        for y in min_y..max_y {
            for x in min_x..max_x {
                let id = map.tile(x, y);
                if !hidden_solid_debug_tile_visible(map, id) {
                    continue;
                }

                let tile_pos = vec2(x as f32 * TILE_SIZE, y as f32 * TILE_SIZE);
                draw_rectangle(
                    tile_pos.x,
                    tile_pos.y,
                    TILE_SIZE,
                    TILE_SIZE,
                    Color::from_rgba(255, 40, 70, 48),
                );
                draw_rectangle_lines(
                    tile_pos.x,
                    tile_pos.y,
                    TILE_SIZE,
                    TILE_SIZE,
                    1.0,
                    Color::from_rgba(255, 90, 110, 160),
                );
            }
        }
    }
}

fn hidden_solid_debug_tile_visible(map: &ArenaMap, id: u8) -> bool {
    is_solid_tile_with_doors(id, map.door_seed)
        && map.render_tile(id).is_none()
        && !is_door_tile(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_solid_debug_overlay_marks_only_solid_tiles_without_art() {
        let map = ArenaMap::fallback_skirmish();

        assert!(hidden_solid_debug_tile_visible(&map, 243));
        assert!(!hidden_solid_debug_tile_visible(&map, 241));
        assert!(!hidden_solid_debug_tile_visible(&map, 1));
        assert!(!hidden_solid_debug_tile_visible(&map, DOOR_FIRST_TILE_ID));
    }
}

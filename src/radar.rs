use macroquad::prelude::*;

use crate::{GOAL_TILE_ID, SAFE_TILE_ID, TILE_SIZE};

const SMALL_RADAR_BORDER: f32 = 6.0;
const SMALL_RADAR_SCREEN_SPAN: f32 = 3.0;
const REFERENCE_GAME_RADAR_SECTORS: f32 = 20.0;
const REFERENCE_GAME_RADAR_ARENA_TILES: f32 = 1024.0;
const REFERENCE_GAME_TARGET_BOUNTY: u16 = 20;

pub(crate) struct RadarPoint {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RadarView {
    pub(crate) dim_px: f32,
    pub(crate) min_tile: Vec2,
    pub(crate) tile_span: Vec2,
}

impl RadarView {
    pub(crate) fn max_tile(self) -> Vec2 {
        self.min_tile + self.tile_span
    }

    pub(crate) fn contains_tile(self, x: usize, y: usize) -> bool {
        let x = x as f32;
        let y = y as f32;
        let max = self.max_tile();
        x >= self.min_tile.x && y >= self.min_tile.y && x <= max.x && y <= max.y
    }

    pub(crate) fn project_tile(
        self,
        tile_x: f32,
        tile_y: f32,
        origin_x: f32,
        origin_y: f32,
    ) -> Vec2 {
        let relative = (vec2(tile_x, tile_y) - self.min_tile) / self.tile_span;
        vec2(origin_x, origin_y) + relative * self.dim_px
    }

    pub(crate) fn project_world(self, pos: Vec2, origin_x: f32, origin_y: f32) -> Vec2 {
        self.project_tile(pos.x / TILE_SIZE, pos.y / TILE_SIZE, origin_x, origin_y)
    }
}

pub(crate) fn small_radar_view(
    map_width: usize,
    map_height: usize,
    ship_pos: Vec2,
    screen_width: f32,
    screen_height: f32,
    map_zoom: u16,
) -> RadarView {
    let map_zoom = map_zoom.max(1) as f32;
    let surface_width = screen_width.max(1.0) as u32;
    let dim_px = ((((surface_width / 6) / 4) * 8) / 2).max(32) as f32;
    let full_dim = (screen_width.max(1.0) * 8.0 / map_zoom).max(dim_px);
    let map_tile_dim = map_width.min(map_height).max(1) as f32;
    let mapzoom_tile_span = (map_tile_dim * dim_px / full_dim).clamp(1.0, map_tile_dim);
    let screen_tile_span =
        vec2(screen_width.max(1.0), screen_height.max(1.0)) / TILE_SIZE * SMALL_RADAR_SCREEN_SPAN;
    let tile_span = vec2(
        mapzoom_tile_span.min(screen_tile_span.x),
        mapzoom_tile_span.min(screen_tile_span.y),
    )
    .clamp(
        Vec2::ONE,
        vec2(map_width.max(1) as f32, map_height.max(1) as f32),
    );
    let center = ship_pos / TILE_SIZE;
    let max_min = vec2(
        (map_width as f32 - tile_span.x).max(0.0),
        (map_height as f32 - tile_span.y).max(0.0),
    );
    let min_tile = vec2(
        (center.x - tile_span.x * 0.5).clamp(0.0, max_min.x),
        (center.y - tile_span.y * 0.5).clamp(0.0, max_min.y),
    );

    RadarView {
        dim_px,
        min_tile,
        tile_span,
    }
}

/// Tile edge length of one small-radar index cell.
const RADAR_INDEX_CELL_TILES: usize = 32;

/// Cell-bucketed view of `ArenaMap::radar_points`.
///
/// The small radar draws a window at most a few hundred tiles wide, while the
/// bundled Chaos map is 1024x1024 tiles and yields 26,390 radar points (measured
/// from `assets/zones/chaos-zone/bdhadar.lvl`). Scanning every point on every
/// frame reads almost all of those entries to draw the two or three thousand
/// that can actually be on screen. This index groups the same points by cell so
/// a frame touches only the cells its view overlaps.
#[derive(Clone, Debug)]
pub(crate) struct RadarPointIndex {
    cells_wide: usize,
    cells_high: usize,
    /// Point indices grouped by cell, cell-major.
    ordering: Vec<u32>,
    /// One range into `ordering` per cell.
    cells: Vec<std::ops::Range<usize>>,
}

impl RadarPointIndex {
    pub(crate) fn build(points: &[RadarPoint], map_width: usize, map_height: usize) -> Self {
        let cells_wide = map_width.div_ceil(RADAR_INDEX_CELL_TILES).max(1);
        let cells_high = map_height.div_ceil(RADAR_INDEX_CELL_TILES).max(1);
        let cell_count = cells_wide * cells_high;
        let mut counts = vec![0usize; cell_count];
        for point in points {
            counts[Self::cell_of(point.x, point.y, cells_wide, cells_high)] += 1;
        }
        let mut cells = Vec::with_capacity(cell_count);
        let mut cursor = Vec::with_capacity(cell_count);
        let mut start = 0usize;
        for count in &counts {
            cells.push(start..start + count);
            cursor.push(start);
            start += count;
        }
        let mut ordering = vec![0u32; points.len()];
        for (position, point) in points.iter().enumerate() {
            let cell = Self::cell_of(point.x, point.y, cells_wide, cells_high);
            ordering[cursor[cell]] = position as u32;
            cursor[cell] += 1;
        }
        Self {
            cells_wide,
            cells_high,
            ordering,
            cells,
        }
    }

    fn cell_of(x: usize, y: usize, cells_wide: usize, cells_high: usize) -> usize {
        let column = (x / RADAR_INDEX_CELL_TILES).min(cells_wide - 1);
        let row = (y / RADAR_INDEX_CELL_TILES).min(cells_high - 1);
        row * cells_wide + column
    }

    /// Visit every point whose cell overlaps `view`.
    ///
    /// This is always a superset of the visible points — a boundary cell is
    /// visited whole — so the caller must still apply
    /// [`RadarView::contains_tile`]. That keeps the rendered set identical to a
    /// full scan of `points`.
    pub(crate) fn visit_view_candidates(
        &self,
        points: &[RadarPoint],
        view: RadarView,
        mut visit: impl FnMut(&RadarPoint),
    ) {
        if self.cells_wide == 0 {
            return;
        }
        let max = view.max_tile();
        let first_column = Self::column_of(view.min_tile.x, self.cells_wide);
        let last_column = Self::column_of(max.x, self.cells_wide);
        let first_row = Self::column_of(view.min_tile.y, self.cells_high);
        let last_row = Self::column_of(max.y, self.cells_high);
        for row in first_row..=last_row {
            for column in first_column..=last_column {
                for slot in self.cells[row * self.cells_wide + column].clone() {
                    visit(&points[self.ordering[slot] as usize]);
                }
            }
        }
    }

    fn column_of(tile: f32, cells: usize) -> usize {
        ((tile.max(0.0) as usize) / RADAR_INDEX_CELL_TILES).min(cells - 1)
    }
}

pub(crate) fn small_radar_origin(screen_width: f32, screen_height: f32, dim_px: f32) -> Vec2 {
    vec2(
        screen_width - dim_px - SMALL_RADAR_BORDER,
        screen_height - dim_px - SMALL_RADAR_BORDER,
    )
}

pub(crate) fn radar_time_sector_label(
    minutes_since_midnight: Option<u16>,
    ship_pos: Vec2,
) -> String {
    let sector = radar_sector_label(ship_pos);
    match minutes_since_midnight {
        Some(minutes) => format!("{}  {}", radar_time_label(minutes), sector),
        None => sector,
    }
}

fn radar_time_label(minutes_since_midnight: u16) -> String {
    // The reference game's `Radar::RenderTime` uses `if (hour > 12) hour -= 12;` with
    // no special-case for hour 0, so midnight reads `0:00am` (not `12:00am`).
    let minutes_since_midnight = minutes_since_midnight % (24 * 60);
    let hour24 = minutes_since_midnight / 60;
    let minute = minutes_since_midnight % 60;
    let pm = hour24 >= 12;
    let hour12 = if hour24 > 12 { hour24 - 12 } else { hour24 };
    format!("{hour12}:{minute:02}{}", if pm { "pm" } else { "am" })
}

fn radar_sector_label(ship_pos: Vec2) -> String {
    let sector_size = REFERENCE_GAME_RADAR_ARENA_TILES / REFERENCE_GAME_RADAR_SECTORS;
    let tile = ship_pos / TILE_SIZE;
    let sector_x = (tile.x / sector_size)
        .floor()
        .clamp(0.0, REFERENCE_GAME_RADAR_SECTORS - 1.0) as u8;
    let sector_y = (tile.y / sector_size)
        .floor()
        .clamp(0.0, REFERENCE_GAME_RADAR_SECTORS - 1.0) as u8
        + 1;
    format!("{}{sector_y}", char::from(b'A' + sector_x))
}

pub(crate) fn draw_radar_view_dot(
    view: &RadarView,
    origin_x: f32,
    origin_y: f32,
    pos: Vec2,
    color: Color,
    size: f32,
) {
    let tile = pos / TILE_SIZE;
    let max = view.max_tile();
    if tile.x < view.min_tile.x || tile.y < view.min_tile.y || tile.x > max.x || tile.y > max.y {
        return;
    }

    let screen = view.project_tile(tile.x, tile.y, origin_x, origin_y);
    draw_rectangle(
        screen.x - size * 0.5,
        screen.y - size * 0.5,
        size,
        size,
        color,
    );
}

pub(crate) fn radar_bomb_visible(see_bomb_level: u8, bomb_level: u8) -> bool {
    see_bomb_level > 0 && bomb_level >= see_bomb_level
}

pub(crate) fn radar_tile_color(id: u8) -> Option<Color> {
    if id == 0 || id > 241 {
        return None;
    }

    if id == SAFE_TILE_ID {
        return Some(Color::from_rgba(24, 82, 24, 255));
    }

    if id == GOAL_TILE_ID {
        return Some(radar_goal_tile_color(false));
    }

    if (162..=169).contains(&id) {
        return Some(Color::from_rgba(173, 173, 173, 255));
    }

    Some(Color::from_rgba(90, 90, 90, 255))
}

pub(crate) fn radar_goal_tile_color(team_goal: bool) -> Color {
    if team_goal {
        Color::from_rgba(33, 156, 173, 255)
    } else {
        Color::from_rgba(8, 57, 255, 255)
    }
}

pub(crate) fn radar_player_style(
    is_self: bool,
    same_frequency: bool,
    bounty: u16,
    carried_flags: u16,
    carrying_ball: bool,
    flagger_on_radar: bool,
) -> (Color, f32) {
    let carrying_objective = carried_flags > 0 || carrying_ball;
    let size = if carrying_objective { 3.0 } else { 2.0 };
    // The reference game's render/Colors has distinct entries for `RadarSelf` (27)
    // and `RadarTeam` (29). We pick a slightly brighter/whiter yellow for
    // self so it visually pops against teammates when overlapping.
    let color = if is_self {
        Color::from_rgba(255, 255, 220, 255)
    } else if same_frequency {
        Color::from_rgba(255, 255, 57, 255)
    } else if carrying_ball || (carried_flags > 0 && flagger_on_radar) {
        Color::from_rgba(255, 57, 74, 255)
    } else if bounty >= REFERENCE_GAME_TARGET_BOUNTY {
        Color::from_rgba(165, 165, 255, 255)
    } else {
        Color::from_rgba(82, 82, 214, 255)
    };

    (color, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radar_time_label_matches_reference_game_style() {
        // The reference game's Radar::RenderTime prints midnight as 0:00am (no
        // special-case for hour 0).
        assert_eq!(radar_time_label(0), "0:00am");
        assert_eq!(radar_time_label(9 * 60 + 7), "9:07am");
        assert_eq!(radar_time_label(12 * 60), "12:00pm");
        assert_eq!(radar_time_label(15 * 60 + 42), "3:42pm");
        assert_eq!(radar_time_label(23 * 60 + 59), "11:59pm");
    }

    #[test]
    fn radar_sector_label_uses_twenty_by_twenty_arena_grid() {
        assert_eq!(radar_sector_label(vec2(0.0, 0.0)), "A1");
        assert_eq!(
            radar_sector_label(vec2(512.0 * TILE_SIZE, 512.0 * TILE_SIZE)),
            "K11"
        );
        assert_eq!(
            radar_sector_label(vec2(1023.0 * TILE_SIZE, 1023.0 * TILE_SIZE)),
            "T20"
        );
        assert_eq!(
            radar_sector_label(vec2(2048.0 * TILE_SIZE, 2048.0 * TILE_SIZE)),
            "T20"
        );
    }

    #[test]
    fn radar_time_sector_label_omits_time_when_browser_time_is_unavailable() {
        assert_eq!(radar_time_sector_label(None, vec2(0.0, 0.0)), "A1");
        assert_eq!(
            radar_time_sector_label(
                Some(15 * 60 + 42),
                vec2(512.0 * TILE_SIZE, 512.0 * TILE_SIZE)
            ),
            "3:42pm  K11"
        );
    }

    #[test]
    fn radar_player_style_matches_reference_game_priority() {
        // Self uses a distinct lighter yellow (the reference game's `RadarSelf` vs
        // `RadarTeam`) so self+teammate overlap is visually distinguishable.
        assert_eq!(
            radar_player_style(true, false, 0, 0, false, false),
            (Color::from_rgba(255, 255, 220, 255), 2.0)
        );
        assert_eq!(
            radar_player_style(false, true, 99, 1, false, true),
            (Color::from_rgba(255, 255, 57, 255), 3.0)
        );
        assert_eq!(
            radar_player_style(false, false, 99, 0, false, false),
            (Color::from_rgba(165, 165, 255, 255), 2.0)
        );
        assert_eq!(
            radar_player_style(false, false, 99, 1, false, true),
            (Color::from_rgba(255, 57, 74, 255), 3.0)
        );
        assert_eq!(
            radar_player_style(false, false, 10, 1, false, false),
            (Color::from_rgba(82, 82, 214, 255), 3.0)
        );
        assert_eq!(
            radar_player_style(false, false, 10, 0, true, false),
            (Color::from_rgba(255, 57, 74, 255), 3.0)
        );
    }

    fn chaos_shaped_radar_points() -> (Vec<RadarPoint>, usize, usize) {
        // Chaos-shaped density: a 1024x1024-tile map carrying ~25,500 radar
        // points, close to the 26,390 measured from the bundled Chaos map.
        let (width, height) = (1024usize, 1024usize);
        let points = (0..width * height)
            .filter(|index| index % 41 == 0)
            .map(|index| RadarPoint {
                x: index % width,
                y: index / width,
                color: Color::from_rgba(90, 90, 90, (index % 251) as u8),
            })
            .collect();
        (points, width, height)
    }

    #[test]
    fn radar_point_index_visits_exactly_the_visible_points() {
        let (points, width, height) = chaos_shaped_radar_points();
        let index = RadarPointIndex::build(&points, width, height);
        let views = [
            // A corner view, an opposite-edge view that runs past the map, a
            // degenerate one-tile view, and a whole-map view.
            RadarView {
                dim_px: 240.0,
                min_tile: vec2(0.0, 0.0),
                tile_span: vec2(270.0, 187.0),
            },
            RadarView {
                dim_px: 240.0,
                min_tile: vec2(760.0, 840.0),
                tile_span: vec2(270.0, 187.0),
            },
            RadarView {
                dim_px: 240.0,
                min_tile: vec2(512.0, 512.0),
                tile_span: vec2(1.0, 1.0),
            },
            RadarView {
                dim_px: 900.0,
                min_tile: vec2(0.0, 0.0),
                tile_span: vec2(1023.0, 1023.0),
            },
        ];
        for (number, view) in views.iter().enumerate() {
            let mut scanned: Vec<(usize, usize)> = points
                .iter()
                .filter(|point| view.contains_tile(point.x, point.y))
                .map(|point| (point.x, point.y))
                .collect();
            let mut indexed: Vec<(usize, usize)> = Vec::new();
            index.visit_view_candidates(&points, *view, |point| {
                if view.contains_tile(point.x, point.y) {
                    indexed.push((point.x, point.y));
                }
            });
            scanned.sort_unstable();
            indexed.sort_unstable();
            assert_eq!(
                indexed, scanned,
                "view {number} rendered a different point set than the full scan"
            );
        }
    }

    #[test]
    fn radar_point_index_skips_cells_outside_the_small_radar_view() {
        let (points, width, height) = chaos_shaped_radar_points();
        let index = RadarPointIndex::build(&points, width, height);
        let view = RadarView {
            dim_px: 240.0,
            min_tile: vec2(400.0, 400.0),
            tile_span: vec2(270.0, 187.0),
        };
        let mut candidates = 0usize;
        index.visit_view_candidates(&points, view, |_| candidates += 1);
        assert!(candidates > 0, "the view covers a populated area");
        assert!(
            candidates < points.len() / 2,
            "the index visited {candidates} of {} points for a one-viewport view",
            points.len()
        );
    }
}

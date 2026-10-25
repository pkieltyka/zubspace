use super::*;
use crate::tile::{is_door_tile, is_solid_tile_with_doors};

#[derive(Clone, Copy, Debug, PartialEq)]
struct DebugContactTile {
    tile: IVec2,
    id: u8,
    rendered: bool,
}

impl Game {
    pub(super) fn draw_developer_overlay(&self) {
        if !self.developer_overlay || !crate::browser_metrics::debug_mode() {
            return;
        }

        let lines = self.developer_overlay_lines();
        let text_scale = 1.0;
        let line_h = self.assets.tall_font.line_height_scaled(text_scale);
        let width = 420.0_f32.min(screen_width() - 24.0).max(260.0);
        let height = 18.0 + line_h * lines.len() as f32;
        let x = 12.0;
        let y = 104.0_f32.min((screen_height() - height - 12.0).max(12.0));

        draw_rectangle(x, y, width, height, Color::from_rgba(5, 8, 12, 215));
        draw_rectangle_lines(
            x,
            y,
            width,
            height,
            1.0,
            Color::from_rgba(100, 150, 170, 210),
        );

        let mut row_y = y + 10.0;
        for (index, line) in lines.iter().enumerate() {
            let color = if index == 0 {
                TextColor::Yellow
            } else {
                TextColor::White
            };
            self.assets.tall_font.draw_scaled(
                line,
                x + 10.0,
                row_y,
                color,
                TextAlign::Left,
                text_scale,
            );
            row_y += line_h;
        }
    }

    fn developer_overlay_lines(&self) -> Vec<String> {
        let map_name = self.map().name.as_str();
        let zone = zone_label_for_path(map_name);
        let lvz_count = self.current_zone_lvz_archive_count();
        let settings = zone_settings_path_for_map(map_name, ZONE_SETTINGS_PATHS).unwrap_or("none");
        let network = self.developer_network_status();
        let network_metrics = self.developer_network_metrics();
        let tile = world_to_tile(self.ship.pos);
        let tile_id = self.map().tile(tile.x, tile.y);
        let solid = is_solid_tile_with_doors(tile_id, self.map().door_seed);
        let rendered = debug_tile_has_visible_art(self.map(), tile_id);
        let contact =
            nearest_ship_contact_tile(self.map(), self.ship.pos, self.ship_tuning().radius_pixels);
        let contact_line = contact
            .map(|contact| {
                format!(
                    "Contact  {},{}  id {}  drawn {}",
                    contact.tile.x, contact.tile.y, contact.id, contact.rendered
                )
            })
            .unwrap_or_else(|| "Contact  none".to_string());
        vec![
            format!("Dev  Zone {zone}"),
            format!("Map  {map_name}"),
            format!("Settings  {settings}"),
            format!(
                "LVZ  {} archives  {} objects  {} textures",
                lvz_count,
                self.lvz_render_cache.objects.len(),
                self.lvz_render_cache.textures.len()
            ),
            format!(
                "Pos  {:05},{:05}  Freq {}",
                self.ship.pos.x.round().max(0.0) as i32,
                self.ship.pos.y.round().max(0.0) as i32,
                self.player_frequency()
            ),
            format!(
                "Tile  {},{}  id {}  solid {}  drawn {}",
                tile.x, tile.y, tile_id, solid, rendered
            ),
            contact_line,
            format!("Net  {network}"),
            format!("Rate {network_metrics}"),
        ]
    }

    fn current_zone_lvz_archive_count(&self) -> usize {
        let Some(map_zone) = zone_directory(self.map().name.as_str()) else {
            return 0;
        };
        self.assets
            .lvz_archives
            .iter()
            .filter(|(path, _)| zone_directory(path) == Some(map_zone))
            .count()
    }

    #[cfg(target_arch = "wasm32")]
    fn developer_network_status(&self) -> String {
        let Some(browser) = &self.browser_network else {
            return "local".to_string();
        };
        let status = match browser.client.transport().status() {
            BrowserRtcStatus::Idle => "idle",
            BrowserRtcStatus::Connecting => "connecting",
            BrowserRtcStatus::Open => {
                if self.client_authority.view().readiness.joined {
                    "joined"
                } else if browser.login_sent {
                    "login"
                } else {
                    "open"
                }
            }
            BrowserRtcStatus::Closed => "closed",
            BrowserRtcStatus::Error => "error",
            BrowserRtcStatus::Unknown(_) => "unknown",
        };
        let server_id = self
            .client_authority
            .view()
            .scope
            .player_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "-".to_string());
        let browser_id = browser.player_identity.as_str();
        format!(
            "{}  {status}  pid {browser_id}  sid {server_id}  players {}",
            browser.client.transport().transport_name(),
            self.network_world.players.len()
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn developer_network_status(&self) -> String {
        "local".to_string()
    }

    #[cfg(target_arch = "wasm32")]
    fn developer_network_metrics(&self) -> String {
        let Some(browser) = &self.browser_network else {
            return "-".to_string();
        };
        let stats = browser.stats;
        let rtt_ms = stats
            .rtt_ticks
            .map(|ticks| (ticks * 10).to_string())
            .unwrap_or_else(|| "-".to_string());
        let jitter_ms = stats
            .jitter_ticks
            .map(|ticks| (ticks * 10).to_string())
            .unwrap_or_else(|| "-".to_string());
        format!(
            "tx/rx {:.0}/{:.0}pps  pos {:.0}/{:.0}pps  rtt {rtt_ms}ms  jit {jitter_ms}ms  stale {}  bufdrop {} ({:.0}/s)  ext {}",
            stats.packets_sent_per_second,
            stats.packets_received_per_second,
            stats.positions_sent_per_second,
            stats.remote_positions_received_per_second,
            self.network_world.stats.remote_position_stale_drops,
            stats.dropped_realtime_packets,
            stats.dropped_realtime_packets_per_second,
            self.network_world.stats.remote_extrapolated_frames
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn developer_network_metrics(&self) -> String {
        "-".to_string()
    }
}

fn zone_label_for_path(path: &str) -> &str {
    zone_directory(path)
        .and_then(|directory| directory.rsplit('/').next())
        .unwrap_or(path)
}

fn zone_directory(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(directory, _)| directory)
}

fn nearest_ship_contact_tile(
    map: &ArenaMap,
    pos: Vec2,
    radius_px: f32,
) -> Option<DebugContactTile> {
    let min_tile_x = ((pos.x - radius_px - TILE_SIZE) / TILE_SIZE).floor() as i32;
    let min_tile_y = ((pos.y - radius_px - TILE_SIZE) / TILE_SIZE).floor() as i32;
    let max_tile_x = ((pos.x + radius_px + TILE_SIZE) / TILE_SIZE).floor() as i32;
    let max_tile_y = ((pos.y + radius_px + TILE_SIZE) / TILE_SIZE).floor() as i32;
    let ship_min = debug_pixel_rounded(pos) - vec2(radius_px, radius_px);
    let ship_max = debug_pixel_rounded(pos) + vec2(radius_px, radius_px);

    let mut nearest: Option<(DebugContactTile, f32)> = None;
    for y in min_tile_y..=max_tile_y {
        for x in min_tile_x..=max_tile_x {
            let id = map.tile(x, y);
            if !is_solid_tile_with_doors(id, map.door_seed) {
                continue;
            }
            if !debug_aabb_overlaps_tile(ship_min, ship_max, x, y) {
                continue;
            }

            let tile_center = vec2((x as f32 + 0.5) * TILE_SIZE, (y as f32 + 0.5) * TILE_SIZE);
            let distance_squared = pos.distance_squared(tile_center);
            let contact = DebugContactTile {
                tile: ivec2(x, y),
                id,
                rendered: debug_tile_has_visible_art(map, id),
            };
            if nearest
                .map(|(_, best_distance)| distance_squared < best_distance)
                .unwrap_or(true)
            {
                nearest = Some((contact, distance_squared));
            }
        }
    }

    nearest.map(|(contact, _)| contact)
}

fn debug_pixel_rounded(pos: Vec2) -> Vec2 {
    vec2(pos.x as i32 as f32, pos.y as i32 as f32)
}

fn debug_tile_has_visible_art(map: &ArenaMap, id: u8) -> bool {
    map.render_tile(id).is_some() || is_door_tile(id)
}

fn debug_aabb_overlaps_tile(ship_min: Vec2, ship_max: Vec2, tile_x: i32, tile_y: i32) -> bool {
    let tile_min = vec2(tile_x as f32 * TILE_SIZE, tile_y as f32 * TILE_SIZE);
    let tile_max = tile_min + vec2(TILE_SIZE, TILE_SIZE);

    crate::gameplay_rules::reference_game_player_hull_overlaps_tile(
        ship_min.x, ship_min.y, ship_max.x, ship_max.y, tile_min.x, tile_min.y, tile_max.x,
        tile_max.y,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developer_overlay_zone_label_uses_zone_folder() {
        assert_eq!(
            zone_label_for_path("assets/zones/chaos-zone/bdhadar.lvl"),
            "chaos-zone"
        );
        assert_eq!(zone_label_for_path("arena.lvl"), "arena.lvl");
    }

    #[test]
    fn developer_overlay_zone_directory_matches_asset_paths() {
        assert_eq!(
            zone_directory("assets/zones/extreme/doomsy8.lvl"),
            Some("assets/zones/extreme")
        );
        assert_eq!(zone_directory("arena.lvl"), None);
    }

    #[test]
    fn nearest_ship_contact_tile_reports_adjacent_invisible_solid() {
        let mut map = ArenaMap::fallback_skirmish();
        map.tiles.fill(0);
        map.tiles[10 * map.width + 11] = 243;
        let pos = vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE);

        assert_eq!(
            nearest_ship_contact_tile(&map, pos, SHIP_RADIUS),
            Some(DebugContactTile {
                tile: ivec2(11, 10),
                id: 243,
                rendered: false,
            })
        );
    }

    #[test]
    fn debug_contact_reports_door_tiles_as_drawn() {
        let map = ArenaMap::fallback_skirmish();

        assert!(debug_tile_has_visible_art(&map, DOOR_FIRST_TILE_ID));
    }
}

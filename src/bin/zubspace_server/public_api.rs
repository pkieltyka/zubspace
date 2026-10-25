//! Public Part A API: live zone summaries over the existing single-zone runtime.
//! This module does not admit players, switch zones, or expose private identities.

use super::*;
use serde_json::{Value, json};

/// Keep the public status summary ahead of diagnostics without changing JSON
/// map ordering elsewhere. `serde` is not a direct dependency, so the object
/// is assembled from each value's compact `serde_json` form.
fn ordered_response_json(status: &serde_json::Map<String, Value>, fields: &[&str]) -> String {
    let summary = fields
        .iter()
        .filter_map(|&key| status.get(key).map(|value| (key, value)));
    let diagnostics = status
        .iter()
        .filter(|(key, _)| !fields.contains(&key.as_str()))
        .map(|(key, value)| (key.as_str(), value));
    let fields = summary
        .chain(diagnostics)
        .map(|(key, value)| format!("{}:{value}", Value::from(key)))
        .collect::<Vec<_>>();
    format!("{{{}}}", fields.join(","))
}

fn status_response_json(status: &serde_json::Map<String, Value>) -> String {
    ordered_response_json(
        status,
        &[
            "ok",
            "service",
            "version",
            "uptime",
            "zones",
            "zones_open",
            "total_players",
        ],
    )
}

const ZONE_FIELDS: [&str; 10] = [
    "id",
    "display_name",
    "status",
    "default",
    "listed",
    "total_players",
    "max_players",
    "map",
    "maxed_spawn_loadout",
    "free_for_all",
];

#[derive(Clone, Copy)]
enum Route<'a> {
    Status,
    Players,
    Zones,
    Zone(&'a str),
    ZonePlayers(&'a str),
    ZoneStatus(&'a str),
}

fn route(path: &str) -> Option<Route<'_>> {
    match path {
        "/v1/status" => Some(Route::Status),
        "/v1/players" => Some(Route::Players),
        "/v1/zones" => Some(Route::Zones),
        _ => {
            let suffix = path.strip_prefix("/v1/zones/")?;
            let (id, tail) = suffix.split_once('/').unwrap_or((suffix, ""));
            let bytes = id.as_bytes();
            if bytes.is_empty()
                || bytes.len() > 15
                || !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit()
                || !bytes[bytes.len() - 1].is_ascii_lowercase()
                    && !bytes[bytes.len() - 1].is_ascii_digit()
                || !bytes
                    .iter()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
            {
                return None;
            }
            match tail {
                "" if !suffix.ends_with('/') => Some(Route::Zone(id)),
                "players" => Some(Route::ZonePlayers(id)),
                "status" => Some(Route::ZoneStatus(id)),
                _ => None,
            }
        }
    }
}

/// Capture all mutable public facts in one lock acquisition. JSON encoding and
/// socket writes happen only after the guard is dropped. The catalog is already
/// bounded and validated at startup; requests never parse maps or LVZ archives.
struct Snapshot {
    catalog: Option<Arc<str>>,
    active: String,
    max_players: usize,
    free_for_all: bool,
    maxed_spawn_loadout: bool,
    players: Vec<(u16, String, bool)>,
    diagnostics: serde_json::Map<String, Value>,
    arena_tick: u64,
}

impl Snapshot {
    fn capture(state: &Arc<Mutex<ServerState>>, started_at: Instant, diagnostics: bool) -> Self {
        let state = state.lock().expect("server state should not be poisoned");
        Self {
            catalog: state.zone_catalog_json.clone(),
            active: state.config.default_zone.clone(),
            max_players: state.config.max_players,
            free_for_all: state.config.free_for_all,
            maxed_spawn_loadout: state.config.maxed_spawn_loadout,
            players: state
                .clients
                .values()
                .filter(|client| client.joined)
                .map(|client| (client.player.id, client.player.name.clone(), false))
                .chain(
                    state
                        .bots
                        .iter()
                        .map(|bot| (bot.player.id, bot.player.name.clone(), true)),
                )
                .collect(),
            diagnostics: if diagnostics {
                server_status_snapshot(&state, started_at)
            } else {
                serde_json::Map::new()
            },
            arena_tick: state.arena_tick,
        }
    }

    fn zones(&self) -> Option<Vec<Value>> {
        let catalog: Value = serde_json::from_str(self.catalog.as_deref()?)
            .expect("startup catalog must remain valid JSON");
        Some(
            catalog["zones"]
                .as_array()
                .expect("startup catalog zones")
                .iter()
                .map(|metadata| {
                    let mut zone = metadata.clone();
                    let open = metadata["id"] == self.active;
                    let object = zone.as_object_mut().expect("startup zone descriptor");
                    object.insert(
                        "status".into(),
                        json!(if open { "open" } else { "standby" }),
                    );
                    object.insert("default".into(), json!(open));
                    object.insert("listed".into(), json!(metadata["id"] == "chaos-zone"));
                    object.insert(
                        "total_players".into(),
                        json!(if open { self.players.len() } else { 0 }),
                    );
                    object.insert("max_players".into(), json!(self.max_players));
                    object.insert("free_for_all".into(), json!(self.free_for_all));
                    object.insert(
                        "maxed_spawn_loadout".into(),
                        json!(self.maxed_spawn_loadout),
                    );
                    zone
                })
                .collect(),
        )
    }

    fn roster(&mut self, zone: Option<&str>) -> String {
        self.players.sort_unstable_by_key(|(id, _, _)| *id);
        let players: Vec<_> = self
            .players
            .iter()
            .filter(|_| zone.is_none_or(|id| id == self.active))
            .map(|(id, name, bot)| {
                format!(
                    "{{\"id\":{id},\"name\":{},\"bot\":{bot},\"zone\":{}}}",
                    json!(name),
                    json!(self.active),
                )
            })
            .collect();
        format!(
            "{{\"total_players\":{},\"players\":[{}]}}",
            players.len(),
            players.join(",")
        )
    }
}

fn process_field(key: &str) -> bool {
    matches!(
        key,
        "ok" | "service"
            | "version"
            | "uptime"
            | "tick_rate"
            | "client_debug_trace"
            | "observer_network_profile"
            | "clients"
            | "connection_quality"
    ) || key.starts_with("rtc_")
}

pub(super) fn handle(
    stream: &mut TcpStream,
    method: &str,
    path: &str,
    state: &Arc<Mutex<ServerState>>,
    started_at: Instant,
) -> Option<io::Result<()>> {
    let route = route(path)?;
    if method == "OPTIONS" {
        return Some(write_http_response(
            stream,
            "204 No Content",
            "application/json",
            "",
            &[
                ("Access-Control-Allow-Origin", "*"),
                ("Access-Control-Allow-Methods", "GET, OPTIONS"),
                ("Access-Control-Allow-Headers", "Content-Type"),
            ],
        ));
    }
    if method != "GET" {
        return Some(write_http_response(
            stream,
            "405 Method Not Allowed",
            "application/json",
            r#"{"ok":false,"error":"method_not_allowed"}"#,
            &[
                ("Access-Control-Allow-Origin", "*"),
                ("Allow", "GET, OPTIONS"),
            ],
        ));
    }
    let mut snapshot = Snapshot::capture(
        state,
        started_at,
        matches!(route, Route::Status | Route::ZoneStatus(_)),
    );
    let zones = snapshot.zones();
    let body = match route {
        Route::Status => {
            snapshot.diagnostics.retain(|key, _| process_field(key));
            snapshot
                .diagnostics
                .insert("total_players".into(), json!(snapshot.players.len()));
            snapshot
                .diagnostics
                .insert("zones".into(), json!(zones.as_ref().map_or(0, Vec::len)));
            snapshot.diagnostics.insert(
                "zones_open".into(),
                json!(zones.as_ref().map_or(0, |zones| {
                    zones.iter().filter(|zone| zone["status"] == "open").count()
                })),
            );
            Value::Object(snapshot.diagnostics)
        }
        Route::Players => {
            let body = snapshot.roster(None);
            return Some(write_http_json(stream, "200 OK", &body));
        }
        _ => {
            let Some(zones) = zones else {
                return Some(write_http_json(
                    stream,
                    "503 Service Unavailable",
                    r#"{"ok":false,"error":"zone_catalog_unavailable"}"#,
                ));
            };
            let id = match route {
                Route::Zones => None,
                Route::Zone(id) | Route::ZonePlayers(id) | Route::ZoneStatus(id) => Some(id),
                _ => unreachable!(),
            };
            if let Some(id) = id {
                let Some(zone) = zones.into_iter().find(|zone| zone["id"] == id) else {
                    return Some(write_http_json(
                        stream,
                        "404 Not Found",
                        r#"{"ok":false,"error":"unknown_zone"}"#,
                    ));
                };
                match route {
                    Route::Zone(_) => zone,
                    Route::ZonePlayers(_) => {
                        let body = snapshot.roster(Some(id));
                        return Some(write_http_json(stream, "200 OK", &body));
                    }
                    Route::ZoneStatus(_) => {
                        let mut status = if id == snapshot.active {
                            snapshot.diagnostics.retain(|key, _| !process_field(key));
                            snapshot
                                .diagnostics
                                .insert("arena_tick".into(), json!(snapshot.arena_tick));
                            Value::Object(snapshot.diagnostics)
                        } else {
                            json!({})
                        };
                        status["ok"] = json!(true);
                        status["zone"] = json!(id);
                        status["status"] = zone["status"].clone();
                        status["total"] = zone["total_players"].clone();
                        status
                    }
                    _ => unreachable!(),
                }
            } else {
                json!({"total_players":snapshot.players.len(),"zones":zones})
            }
        }
    };
    let body = match (route, &body) {
        (Route::Status, Value::Object(status)) => status_response_json(status),
        (Route::Zone(_), Value::Object(zone)) => ordered_response_json(zone, &ZONE_FIELDS),
        (Route::Zones, Value::Object(catalog)) => {
            let zones = catalog["zones"]
                .as_array()
                .expect("catalog zones must be an array")
                .iter()
                .map(|zone| {
                    ordered_response_json(zone.as_object().expect("zone descriptor"), &ZONE_FIELDS)
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{{\"total_players\":{},\"zones\":[{zones}]}}",
                catalog["total_players"]
            )
        }
        _ => body.to_string(),
    };
    Some(write_http_json(stream, "200 OK", &body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_response_puts_summary_first_and_round_trips() {
        let status = json!({
            "client_debug_trace": false,
            "ok": true,
            "rtc_sessions": 3,
            "service": "zubspace-server",
            "total_players": 18,
            "uptime": 42,
            "version": "abc\"def",
            "zones": 2,
            "zones_open": 1,
        });
        let Value::Object(status) = status else {
            unreachable!()
        };
        let body = status_response_json(&status);
        assert!(body.starts_with(
            r#"{"ok":true,"service":"zubspace-server","version":"abc\"def","uptime":42,"zones":2,"zones_open":1,"total_players":18,"#
        ));
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            Value::Object(status)
        );
    }
}

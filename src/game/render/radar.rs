use super::*;

mod full_map;
mod small;

fn powerball_radar_style(kind: NetworkPowerballRadarKind) -> (Color, f32) {
    match kind {
        NetworkPowerballRadarKind::Loose => (Color::from_rgba(255, 235, 95, 255), 3.0),
        NetworkPowerballRadarKind::TeamCarrier => (Color::from_rgba(95, 190, 255, 255), 3.0),
        NetworkPowerballRadarKind::EnemyCarrier => (Color::from_rgba(255, 70, 70, 255), 3.0),
    }
}

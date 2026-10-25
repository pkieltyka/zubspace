/// Player connections use the client-id range below the server-owned bot ids.
/// Keep admission/configuration limits tied to this boundary so allocation can
/// never wrap onto a live client.
pub(crate) const SERVER_BOT_ID_BASE: u16 = 0x0300;
pub(crate) const MAX_PLAYER_CLIENTS: usize = SERVER_BOT_ID_BASE as usize;
pub(crate) const MAX_SERVER_BOTS: usize = 0x0400 - SERVER_BOT_ID_BASE as usize;
/// Includes bots, joined humans and pending human admission reservations.
pub(crate) const MAX_ARENA_PLAYERS: usize = 500;

#[path = "player_events/chat.rs"]
mod chat;
#[path = "player_events/combat.rs"]
mod combat;
#[path = "player_events/prizes.rs"]
mod prizes;
#[path = "player_events/spectate.rs"]
mod spectate;

pub(crate) use chat::{CHAT_MESSAGE_REQUEST, relay_chat};
#[cfg(test)]
pub(crate) use chat::{
    CHAT_PACKET, CHAT_TYPE_PRIVATE, CHAT_TYPE_PUBLIC, CHAT_TYPE_TEAM, chat_packet_from_request,
    relay_chat_at,
};
pub(crate) use combat::{DEATH_REQUEST, roll_death_green_prize_id, toggle_send_damage};
#[cfg(test)]
pub(crate) use combat::{PLAYER_DEATH, TOGGLE_SEND_DAMAGE, relay_death};
pub(crate) use prizes::PrizeRecipientAck;
pub(crate) use prizes::apply_prize_effect_application;
pub(crate) use prizes::player_prize_from_claim;
pub(crate) use prizes::prize_pickup_ack_from_claim;
#[cfg(test)]
pub(crate) use prizes::relay_prize_pickup_at_tick;
pub(crate) use prizes::relay_prize_pickup_for_runtime_at_tick;
pub(crate) use prizes::{TAKE_PRIZE_REQUEST, relay_prize_pickup};
#[cfg(test)]
pub(crate) use spectate::spectate_packet;
pub(crate) use spectate::{SPECTATE_REQUEST, update_spectate};

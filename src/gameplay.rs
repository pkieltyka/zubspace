use super::*;

mod bots;
mod movement;
mod player_input;
mod player_movement;
#[cfg(any(test, target_arch = "wasm32"))]
mod requests;
mod ship_status;
#[cfg(test)]
mod spawn;
mod ticks;
mod weapons;

pub(crate) use bots::*;
pub(crate) use movement::*;
pub(crate) use player_input::*;
#[allow(unused_imports)] // consumed in Phase 1c-2 (server simulation).
pub(crate) use player_movement::*;
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) use requests::*;
pub(crate) use ship_status::*;
#[cfg(test)]
pub(crate) use spawn::*;
pub(crate) use ticks::*;
pub(crate) use weapons::*;

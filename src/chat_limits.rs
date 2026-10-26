//! Shared bounded chat policy for browser drafts and server relays.
pub(crate) const CHAT_MAX_CHARS: usize = 256;
pub(crate) const CHAT_MAX_BYTES: usize = CHAT_MAX_CHARS * 4;

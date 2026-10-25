use super::*;

impl NetworkWorld {
    pub(super) fn apply_player_voice_event(&mut self, voice: &PlayerVoice) {
        self.voice_messages.push(voice.clone());
    }

    pub(super) fn apply_file_transferred_event(&mut self, file: &ServerFileTransfer) {
        self.transferred_files.push(file.clone());
    }

    pub(super) fn apply_arena_directory_event(&mut self, listing: &ArenaDirectoryListing) {
        self.arena_directory = listing.arenas.clone();
    }

    pub(super) fn apply_zone_banner_advertised_event(
        &mut self,
        advertisement: &ZoneBannerAdvertisement,
    ) {
        self.last_zone_banner = Some(advertisement.clone());
    }

    pub(super) fn apply_client_file_requested_event(&mut self, request: &ClientFileRequest) {
        self.client_file_requests.push(request.clone());
    }

    pub(super) fn apply_personal_banner_changed_event(&mut self, change: &PersonalBannerChange) {
        self.personal_banner = Some(change.banner);
    }

    pub(super) fn apply_player_banner_changed_event(&mut self, change: &PlayerBannerChange) {
        if let Some(player) = self.players.get_mut(&change.player_id) {
            player.banner = Some(change.banner);
        }
    }

    pub(super) fn apply_chat_event(&mut self, chat: &ChatPacket) {
        let sender_name = self
            .players
            .get(&chat.sender_id)
            .map(|player| player.name.clone());
        self.chat.push(NetworkChatEntry {
            chat_type: chat.chat_type,
            sound: chat.sound,
            sender_id: chat.sender_id,
            sender_name,
            message: chat.message.clone(),
        });
        if self.chat.len() > Self::CHAT_HISTORY_LIMIT {
            let overflow = self.chat.len() - Self::CHAT_HISTORY_LIMIT;
            self.chat.drain(0..overflow);
        }
    }
}

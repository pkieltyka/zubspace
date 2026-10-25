use crate::protocol::ChatMessageRequest;

pub(crate) const CHAT_MAX_CHARS: usize = 96;
pub(crate) const CHAT_HISTORY_LIMIT: usize = 64;
pub(crate) const CHAT_SENT_HISTORY_LIMIT: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AudioVolumeCategory {
    Effects,
    Chat,
    Voice,
    Music,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AudioVolumeCommand {
    pub(crate) category: AudioVolumeCategory,
    pub(crate) value: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalChatMessage {
    pub(crate) request: ChatMessageRequest,
    echo_prefix: &'static str,
}

impl LocalChatMessage {
    fn public(message: impl Into<String>) -> Self {
        Self {
            request: ChatMessageRequest::public(message),
            echo_prefix: "You",
        }
    }

    fn team(message: impl Into<String>) -> Self {
        Self {
            request: ChatMessageRequest::team(message),
            echo_prefix: "You (team)",
        }
    }

    fn private(target: u16, message: impl Into<String>) -> Self {
        Self {
            request: ChatMessageRequest::private(target, message),
            echo_prefix: "You ->",
        }
    }

    pub(crate) fn echo_text(&self) -> String {
        if self.request.chat_type == ChatMessageRequest::PRIVATE_TYPE {
            format!(
                "{} #{}: {}",
                self.echo_prefix, self.request.target, self.request.message
            )
        } else {
            format!("{}: {}", self.echo_prefix, self.request.message)
        }
    }
}

#[derive(Default)]
pub(crate) struct ChatState {
    pub(crate) active: bool,
    pub(crate) input: String,
    pub(crate) messages: Vec<String>,
    pub(crate) message_colors: Vec<crate::font::TextColor>,
    sent_history: Vec<String>,
    history_cursor: Option<usize>,
}

impl ChatState {
    pub(crate) fn push_message(&mut self, message: String, color: crate::font::TextColor) {
        push_chat_message(&mut self.messages, message, CHAT_HISTORY_LIMIT);
        self.message_colors.push(color);
        let overflow = self.message_colors.len().saturating_sub(CHAT_HISTORY_LIMIT);
        self.message_colors.drain(..overflow);
    }

    pub(crate) fn begin_input(&mut self) {
        self.active = true;
        self.input.clear();
        self.history_cursor = None;
    }

    pub(crate) fn cancel_input(&mut self) {
        self.active = false;
        self.input.clear();
        self.history_cursor = None;
    }

    pub(crate) fn push_char(&mut self, ch: char) {
        self.history_cursor = None;
        push_chat_char(&mut self.input, ch);
    }

    pub(crate) fn backspace(&mut self) {
        self.history_cursor = None;
        self.input.pop();
    }

    pub(crate) fn submit_input(&mut self) -> Option<String> {
        let message = self.input.trim().to_string();
        self.active = false;
        self.input.clear();
        self.history_cursor = None;
        if message.is_empty() {
            return None;
        }

        push_chat_message(
            &mut self.sent_history,
            message.clone(),
            CHAT_SENT_HISTORY_LIMIT,
        );
        Some(message)
    }

    pub(crate) fn recall_previous(&mut self) {
        if self.sent_history.is_empty() {
            return;
        }

        let next = match self.history_cursor {
            Some(0) => 0,
            Some(index) => index - 1,
            None => self.sent_history.len() - 1,
        };
        self.history_cursor = Some(next);
        self.input = self.sent_history[next].clone();
    }

    pub(crate) fn recall_next(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };

        if index + 1 >= self.sent_history.len() {
            self.history_cursor = None;
            self.input.clear();
            return;
        }

        let next = index + 1;
        self.history_cursor = Some(next);
        self.input = self.sent_history[next].clone();
    }
}

pub(crate) fn push_chat_char(input: &mut String, ch: char) {
    if input.chars().count() >= CHAT_MAX_CHARS {
        return;
    }

    if !ch.is_control() {
        input.push(ch);
    }
}

pub(crate) fn push_chat_message(messages: &mut Vec<String>, message: String, limit: usize) {
    if limit == 0 {
        messages.clear();
        return;
    }

    messages.push(message);
    let overflow = messages.len().saturating_sub(limit);
    if overflow > 0 {
        messages.drain(0..overflow);
    }
}

#[cfg(test)]
pub(crate) fn parse_local_chat_input(input: &str) -> Result<LocalChatMessage, &'static str> {
    parse_local_chat_input_with_resolver(input, |_| Err("private chat target must be a player id"))
}

pub(crate) fn parse_local_chat_input_with_resolver(
    input: &str,
    mut resolve_private_target: impl FnMut(&str) -> Result<u16, &'static str>,
) -> Result<LocalChatMessage, &'static str> {
    let input = input.trim();
    if input.is_empty() {
        return Err("chat message is empty");
    }

    if let Some(message) = input.strip_prefix("//") {
        return team_message(message);
    }
    if let Some(message) = input.strip_prefix('\'') {
        return team_message(message);
    }

    let Some(command_text) = input.strip_prefix('/') else {
        return Ok(LocalChatMessage::public(input));
    };

    let (command, rest) = split_first_word(command_text);
    match command.to_ascii_lowercase().as_str() {
        "t" | "team" => team_message(rest),
        command if is_private_chat_command(command) => {
            private_message(rest, &mut resolve_private_target)
        }
        "s" | "sound" => sound_message(rest),
        _ => Ok(LocalChatMessage::public(input)),
    }
}

pub(crate) fn parse_frequency_change_input(input: &str) -> Result<Option<u16>, &'static str> {
    let input = input.trim();
    let Some(rest) = input.strip_prefix('=') else {
        return Ok(None);
    };
    if rest.is_empty() || !rest.chars().all(|ch| ch.is_ascii_digit()) {
        return Err("frequency change needs a number");
    }
    rest.parse::<u16>()
        .map(Some)
        .map_err(|_| "frequency is out of range")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NetworkInfoCommand {
    Ping,
    Lag,
}

pub(crate) fn parse_network_info_command(input: &str) -> Option<NetworkInfoCommand> {
    match input.trim().to_ascii_lowercase().as_str() {
        "?ping" => Some(NetworkInfoCommand::Ping),
        "?lag" => Some(NetworkInfoCommand::Lag),
        _ => None,
    }
}

pub(crate) fn network_info_messages(
    command: NetworkInfoCommand,
    stats: Option<[u32; 5]>,
) -> Vec<String> {
    let mut messages = match stats {
        Some([current, average, low, high, _]) => vec![format!(
            "RTC ping: {current} ms (avg {average}, low {low}, high {high}; last 60 samples)."
        )],
        None => vec!["RTC ping unavailable: waiting for a connected, current measurement.".into()],
    };
    if command == NetworkInfoCommand::Lag {
        if let Some([_, _, _, _, variation]) = stats {
            messages.push(format!(
                "RTT variation: {variation} ms. Packet loss: unavailable in client diagnostics."
            ));
        } else {
            messages.push("Packet loss: unavailable in client diagnostics.".into());
        }
    }
    messages
}

pub(crate) fn parse_status_command(input: &str) -> bool {
    input.trim().eq_ignore_ascii_case("?status")
}

pub(crate) fn ship_status_percent(upgrade_step: f32, maximum: f32, current: f32) -> u32 {
    if upgrade_step <= 0.0 {
        return 100;
    }
    let maximum_upgrades = (maximum / upgrade_step).floor() as u32;
    if maximum_upgrades == 0 {
        return 100;
    }
    let current_upgrades = (current / upgrade_step).floor().max(0.0) as u32;
    (current_upgrades * 100) / maximum_upgrades
}

pub(crate) fn status_command_message(
    recharge: u32,
    thruster: u32,
    speed: u32,
    rotation: u32,
    shrapnel: usize,
) -> String {
    format!(
        "Recharge:{recharge}%  Thruster:{thruster}%  Speed:{speed}%  Rotation:{rotation}%  Shrapnel:{shrapnel}"
    )
}

pub(crate) fn parse_audio_volume_command(
    input: &str,
) -> Result<Option<AudioVolumeCommand>, &'static str> {
    let input = input.trim();
    let Some(rest) = input.strip_prefix('?') else {
        return Ok(None);
    };

    let (command, value) = rest
        .split_once('=')
        .map(|(command, value)| (command.trim(), value.trim()))
        .unwrap_or_else(|| split_first_word(rest));
    let category = match command.to_ascii_lowercase().as_str() {
        "sound" => AudioVolumeCategory::Effects,
        "chat" => AudioVolumeCategory::Chat,
        "voice" => AudioVolumeCategory::Voice,
        "music" => AudioVolumeCategory::Music,
        _ => return Ok(None),
    };
    if value.is_empty() {
        return Err("volume command needs a value from 0 to 10");
    }
    let value = value
        .parse::<u8>()
        .map_err(|_| "volume command needs a value from 0 to 10")?;
    if value > 10 {
        return Err("volume command needs a value from 0 to 10");
    }

    Ok(Some(AudioVolumeCommand { category, value }))
}

pub(crate) fn autocomplete_private_chat_target<'a>(
    input: &str,
    names: impl IntoIterator<Item = &'a str>,
) -> Result<Option<String>, &'static str> {
    let Some(command_text) = input.strip_prefix('/') else {
        return Ok(None);
    };

    let (command, rest) = split_first_word(command_text);
    if !is_private_chat_command(command) {
        return Ok(None);
    }

    let (target, message) = split_first_word(rest);
    if target.is_empty() || !message.is_empty() {
        return Ok(None);
    }

    let target_lower = target.to_ascii_lowercase();
    let mut prefix_match = None;
    for name in names {
        if name.is_empty() {
            continue;
        }

        let name_lower = name.to_ascii_lowercase();
        if name_lower == target_lower {
            return Ok(Some(format!("/{command} {name} ")));
        }
        if name_lower.starts_with(&target_lower) {
            if prefix_match.is_some() {
                return Err("chat autocomplete player name is ambiguous");
            }
            prefix_match = Some(name);
        }
    }

    prefix_match
        .map(|name| format!("/{command} {name} "))
        .map(Some)
        .ok_or("chat autocomplete found no player")
}

fn is_private_chat_command(command: &str) -> bool {
    matches!(
        command.to_ascii_lowercase().as_str(),
        "p" | "pm" | "msg" | "private"
    )
}

fn team_message(message: &str) -> Result<LocalChatMessage, &'static str> {
    let message = message.trim();
    if message.is_empty() {
        return Err("team chat needs a message");
    }
    Ok(LocalChatMessage::team(message))
}

fn private_message(
    rest: &str,
    resolve_private_target: &mut impl FnMut(&str) -> Result<u16, &'static str>,
) -> Result<LocalChatMessage, &'static str> {
    let (target, message) = split_first_word(rest);
    if target.is_empty() {
        return Err("private chat needs a target");
    }
    let target = target
        .parse::<u16>()
        .or_else(|_| resolve_private_target(target))?;
    let message = message.trim();
    if message.is_empty() {
        return Err("private chat needs a message");
    }
    Ok(LocalChatMessage::private(target, message))
}

fn sound_message(rest: &str) -> Result<LocalChatMessage, &'static str> {
    let (sound, message) = split_first_word(rest);
    let sound = sound
        .parse::<u8>()
        .map_err(|_| "chat sound id must be 0-255")?;
    let message = message.trim();
    if message.is_empty() {
        return Err("chat sound needs a message");
    }
    Ok(LocalChatMessage::public(message).with_sound(sound))
}

fn split_first_word(input: &str) -> (&str, &str) {
    let input = input.trim_start();
    input
        .split_once(char::is_whitespace)
        .map(|(first, rest)| (first, rest.trim_start()))
        .unwrap_or((input, ""))
}

pub(crate) fn format_network_chat_message(
    chat_type: u8,
    sender_name: Option<&str>,
    sender_id: u16,
    message: &str,
) -> String {
    let sender = sender_name
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("#{sender_id}"));
    match chat_type {
        ChatMessageRequest::TEAM_TYPE => format!("{sender} (team): {message}"),
        ChatMessageRequest::PRIVATE_TYPE => format!("{sender} -> you: {message}"),
        _ => format!("{sender}: {message}"),
    }
}

impl LocalChatMessage {
    fn with_sound(mut self, sound: u8) -> Self {
        self.request = self.request.with_sound(sound);
        self
    }
}

#[cfg(test)]
mod network_info_tests {
    use super::*;

    #[test]
    fn network_info_commands_are_exact_local_commands() {
        assert_eq!(
            parse_network_info_command(" ?PiNg\t"),
            Some(NetworkInfoCommand::Ping)
        );
        assert_eq!(
            parse_network_info_command("?lag"),
            Some(NetworkInfoCommand::Lag)
        );
        for ordinary in ["ping", "?ping now", "hello ?lag", "/ping", "?lagging", ""] {
            assert_eq!(parse_network_info_command(ordinary), None);
        }
    }

    #[test]
    fn network_info_replies_label_rtt_and_do_not_invent_packet_loss() {
        let stats = Some([44, 50, 40, 70, 6]);
        let ping = network_info_messages(NetworkInfoCommand::Ping, stats);
        assert_eq!(ping.len(), 1);
        assert!(ping[0].contains("44 ms (avg 50, low 40, high 70"));
        let lag = network_info_messages(NetworkInfoCommand::Lag, stats);
        assert_eq!(lag.len(), 2);
        assert!(lag[1].contains("variation: 6 ms"));
        assert!(lag[1].contains("Packet loss: unavailable"));
        let missing = network_info_messages(NetworkInfoCommand::Lag, None);
        assert!(missing[0].contains("unavailable"));
        assert!(!missing.join(" ").contains("0 ms"));
    }
}

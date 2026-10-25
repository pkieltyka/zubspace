//! Targets for high-rate, per-event log lines: one line per shot, projectile,
//! hit, damage or acknowledgement. Each target sits directly under the module
//! that emits it, so a filter naming that module (as the E2E harnesses use)
//! still matches, while prod's `log_filter` switches only these lines off.
//! Emitting modules assert the placement at compile time with [`is_child_of`].

pub(crate) const SERVER_TICK_COMBAT: &str = "zubspace_server::server_tick::combat";
pub(crate) const SERVER_DAMAGE_COMBAT: &str = "zubspace_server::server_damage::combat";
pub(crate) const POSITIONS_COMBAT: &str = "zubspace_server::arena::positions::combat";
pub(crate) const WEAPONS_COMBAT: &str = "zubspace_server::arena::positions::weapons::combat";
pub(crate) const RELIABLE_ACKS: &str = "zubspace_server::reliable::acks";
pub(crate) const RTC_ACKS: &str = "zubspace_server::rtc::acks";

#[cfg(test)]
pub(crate) const PER_EVENT_TARGETS: [&str; 6] = [
    SERVER_TICK_COMBAT,
    SERVER_DAMAGE_COMBAT,
    POSITIONS_COMBAT,
    WEAPONS_COMBAT,
    RELIABLE_ACKS,
    RTC_ACKS,
];

/// Whether `target` is `module` followed by exactly one more path segment.
pub(crate) const fn is_child_of(target: &str, module: &str) -> bool {
    let (target, module) = (target.as_bytes(), module.as_bytes());
    if target.len() <= module.len() + 2 {
        return false;
    }
    let mut index = 0;
    while index < module.len() {
        if target[index] != module[index] {
            return false;
        }
        index += 1;
    }
    if target[index] != b':' || target[index + 1] != b':' {
        return false;
    }
    index += 2;
    while index < target.len() {
        if target[index] == b':' {
            return false;
        }
        index += 1;
    }
    true
}

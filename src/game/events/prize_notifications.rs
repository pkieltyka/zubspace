use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrizeNotificationLimit {
    Min,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrizeNotification {
    pub(super) kind: PrizeKind,
    pub(super) negative: bool,
    limit: Option<PrizeNotificationLimit>,
}

impl PrizeNotification {
    pub(crate) fn new(kind: PrizeKind, negative: bool) -> Self {
        Self {
            kind,
            negative,
            limit: None,
        }
    }

    pub(super) fn limited(kind: PrizeKind, negative: bool) -> Self {
        Self {
            kind,
            negative,
            limit: Some(if negative {
                PrizeNotificationLimit::Min
            } else {
                PrizeNotificationLimit::Max
            }),
        }
    }
}

impl From<(PrizeKind, bool)> for PrizeNotification {
    fn from((kind, negative): (PrizeKind, bool)) -> Self {
        Self::new(kind, negative)
    }
}

pub(super) fn changed_prize_notification<T: PartialEq>(
    kind: PrizeKind,
    negative: bool,
    before: T,
    after: T,
) -> Option<PrizeNotification> {
    if before == after {
        Some(PrizeNotification::limited(kind, negative))
    } else {
        Some(PrizeNotification::new(kind, negative))
    }
}

pub(super) fn capability_prize_notification_entry(
    kind: PrizeKind,
    was_capable: bool,
    negative: bool,
) -> Option<PrizeNotification> {
    capability_prize_notification(kind, was_capable, negative)
        .map(PrizeNotification::from)
        .or_else(|| Some(PrizeNotification::limited(kind, negative)))
}

pub(super) fn status_prize_notification_entry(
    kind: PrizeKind,
    available: bool,
    was_capable: bool,
    negative: bool,
) -> Option<PrizeNotification> {
    status_prize_notification(kind, available, was_capable, negative)
        .map(|notification| {
            if available && notification == (kind, negative) && was_capable != negative {
                PrizeNotification::limited(kind, negative)
            } else {
                PrizeNotification::from(notification)
            }
        })
        .or_else(|| available.then(|| PrizeNotification::limited(kind, negative)))
}

pub(crate) fn prize_notification_text_for_entry(
    notification: PrizeNotification,
    damage: bool,
) -> String {
    let mut message = prize_notification_text(notification.kind, notification.negative, damage);
    if message.is_empty() {
        return message;
    }

    if let Some(limit) = notification.limit {
        message.push_str(match limit {
            PrizeNotificationLimit::Min => " MIN",
            PrizeNotificationLimit::Max => " MAX",
        });
    }
    message
}

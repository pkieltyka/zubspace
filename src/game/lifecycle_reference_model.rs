//! Independent, test-only specification for client authority sequencing.
//!
//! This module intentionally does not call production lifecycle code. It is a
//! compact executable contract used to pin ordering, idempotence, scope, wrap,
//! and bounded-state behavior before production ownership moves.

use std::collections::VecDeque;

const READY_JOINED: u8 = 1 << 0;
const READY_SETTINGS: u8 = 1 << 1;
const READY_SECURITY: u8 = 1 << 2;
const READY_MAP: u8 = 1 << 3;
const READY_FILES: u8 = 1 << 4;
const READY_ALL: u8 = READY_JOINED | READY_SETTINGS | READY_SECURITY | READY_MAP | READY_FILES;
const MAX_DEFERRED_FACTS: usize = 16;
const MAX_CLOSED_TRANSITIONS: usize = 32;
const MAX_HISTORY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Generation {
    session: u64,
    stream: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scope {
    generation: Generation,
    player: u16,
    life: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Local,
    Realtime,
    Event,
    Reliable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadyFact {
    Joined,
    Settings,
    Security,
    Map,
    Files,
}

impl ReadyFact {
    fn bit(self) -> u8 {
        match self {
            Self::Joined => READY_JOINED,
            Self::Settings => READY_SETTINGS,
            Self::Security => READY_SECURITY,
            Self::Map => READY_MAP,
            Self::Files => READY_FILES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BaselineKind {
    Initial,
    Ship,
    Frequency,
    Respawn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Disconnected,
    Bootstrapping(Generation),
    AwaitingReadiness(Scope),
    AwaitingSubmission {
        scope: Scope,
        kind: BaselineKind,
    },
    AwaitingApplication {
        scope: Scope,
        kind: BaselineKind,
        delivery: u32,
        sequence: u16,
        deadline: u32,
    },
    Live(Scope),
    CutoverRequested {
        scope: Scope,
        kind: BaselineKind,
    },
    Dead {
        scope: Scope,
        respawn_at: u32,
    },
    CorrectionAwaitingSubmission {
        scope: Scope,
        application: u32,
        sequence: u16,
    },
    CorrectionAwaitingApplication {
        scope: Scope,
        application: u32,
        sequence: u16,
        delivery: u32,
        deadline: u32,
    },
    Failed(Option<Scope>),
}

impl Phase {
    fn scope(self) -> Option<Scope> {
        match self {
            Self::AwaitingReadiness(scope)
            | Self::AwaitingSubmission { scope, .. }
            | Self::AwaitingApplication { scope, .. }
            | Self::Live(scope)
            | Self::CutoverRequested { scope, .. }
            | Self::Dead { scope, .. }
            | Self::CorrectionAwaitingSubmission { scope, .. }
            | Self::CorrectionAwaitingApplication { scope, .. } => Some(scope),
            Self::Failed(scope) => scope,
            Self::Disconnected | Self::Bootstrapping(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Bootstrap(Generation),
    PlayerId {
        generation: Generation,
        player: u16,
    },
    Ready {
        generation: Generation,
        fact: ReadyFact,
    },
    Lifecycle {
        scope: Scope,
        alive: bool,
    },
    SubmitBaseline {
        scope: Scope,
        kind: BaselineKind,
        delivery: u32,
        sequence: u16,
        now: u32,
        timeout: u32,
    },
    TransportAck {
        scope: Scope,
        delivery: u32,
    },
    RetryDurable {
        scope: Scope,
        delivery: u32,
    },
    ApplicationReceipt {
        scope: Scope,
        kind: BaselineKind,
        delivery: u32,
        sequence: u16,
    },
    RequestCutover {
        scope: Scope,
        kind: BaselineKind,
    },
    Death {
        scope: Scope,
        respawn_at: u32,
    },
    CorrectionProposed {
        scope: Scope,
        application: u32,
        sequence: u16,
    },
    SubmitCorrection {
        scope: Scope,
        application: u32,
        sequence: u16,
        delivery: u32,
        now: u32,
        timeout: u32,
    },
    CorrectionReceipt {
        scope: Scope,
        application: u32,
        sequence: u16,
        delivery: u32,
    },
    Gameplay {
        scope: Scope,
        sequence: u16,
    },
    Tick(u32),
    Disconnect {
        session: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EventKind {
    Bootstrap,
    PlayerId,
    Ready,
    Lifecycle,
    SubmitBaseline,
    TransportAck,
    RetryDurable,
    ApplicationReceipt,
    RequestCutover,
    Death,
    CorrectionProposed,
    SubmitCorrection,
    CorrectionReceipt,
    Gameplay,
    Tick,
    Disconnect,
}

impl Event {
    fn kind(self) -> EventKind {
        match self {
            Self::Bootstrap(_) => EventKind::Bootstrap,
            Self::PlayerId { .. } => EventKind::PlayerId,
            Self::Ready { .. } => EventKind::Ready,
            Self::Lifecycle { .. } => EventKind::Lifecycle,
            Self::SubmitBaseline { .. } => EventKind::SubmitBaseline,
            Self::TransportAck { .. } => EventKind::TransportAck,
            Self::RetryDurable { .. } => EventKind::RetryDurable,
            Self::ApplicationReceipt { .. } => EventKind::ApplicationReceipt,
            Self::RequestCutover { .. } => EventKind::RequestCutover,
            Self::Death { .. } => EventKind::Death,
            Self::CorrectionProposed { .. } => EventKind::CorrectionProposed,
            Self::SubmitCorrection { .. } => EventKind::SubmitCorrection,
            Self::CorrectionReceipt { .. } => EventKind::CorrectionReceipt,
            Self::Gameplay { .. } => EventKind::Gameplay,
            Self::Tick(_) => EventKind::Tick,
            Self::Disconnect { .. } => EventKind::Disconnect,
        }
    }

    fn scope(self) -> Option<Scope> {
        match self {
            Self::Lifecycle { scope, .. }
            | Self::SubmitBaseline { scope, .. }
            | Self::TransportAck { scope, .. }
            | Self::RetryDurable { scope, .. }
            | Self::ApplicationReceipt { scope, .. }
            | Self::RequestCutover { scope, .. }
            | Self::Death { scope, .. }
            | Self::CorrectionProposed { scope, .. }
            | Self::SubmitCorrection { scope, .. }
            | Self::CorrectionReceipt { scope, .. }
            | Self::Gameplay { scope, .. } => Some(scope),
            Self::Bootstrap(_)
            | Self::PlayerId { .. }
            | Self::Ready { .. }
            | Self::Tick(_)
            | Self::Disconnect { .. } => None,
        }
    }

    fn delivery(self) -> Option<u32> {
        match self {
            Self::SubmitBaseline { delivery, .. }
            | Self::TransportAck { delivery, .. }
            | Self::RetryDurable { delivery, .. }
            | Self::ApplicationReceipt { delivery, .. }
            | Self::SubmitCorrection { delivery, .. }
            | Self::CorrectionReceipt { delivery, .. } => Some(delivery),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RejectReason {
    StaleGeneration,
    StaleScope,
    WrongPlayer,
    ImpossiblePhase,
    IdentityMismatch,
    Cancelled,
    Capacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disposition {
    Applied,
    Replacement,
    Observed,
    Duplicate,
    Deferred,
    TimedOut,
    Rejected(RejectReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClosedKind {
    Baseline(BaselineKind),
    Correction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClosedTransition {
    scope: Scope,
    kind: ClosedKind,
    delivery: u32,
    sequence: u16,
    completed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HistoryEntry {
    source: Source,
    event: EventKind,
    scope: Option<Scope>,
    prior: Phase,
    disposition: Disposition,
    resulting: Phase,
    delivery: Option<u32>,
    deadline: Option<u32>,
}

#[derive(Debug)]
struct ReferenceSequencer {
    generation: Option<Generation>,
    player: Option<u16>,
    readiness: u8,
    authority_segment: u64,
    phase: Phase,
    deferred_lifecycle: VecDeque<(Source, Scope, bool)>,
    closed: VecDeque<ClosedTransition>,
    history: VecDeque<HistoryEntry>,
}

impl Default for ReferenceSequencer {
    fn default() -> Self {
        Self {
            generation: None,
            player: None,
            readiness: 0,
            authority_segment: 0,
            phase: Phase::Disconnected,
            deferred_lifecycle: VecDeque::new(),
            closed: VecDeque::new(),
            history: VecDeque::new(),
        }
    }
}

impl ReferenceSequencer {
    fn observe(&mut self, source: Source, event: Event) -> Disposition {
        let prior = self.phase;
        let disposition = self.apply(source, event);
        let deadline = match self.phase {
            Phase::AwaitingApplication { deadline, .. }
            | Phase::CorrectionAwaitingApplication { deadline, .. } => Some(deadline),
            _ => None,
        };
        self.push_history(HistoryEntry {
            source,
            event: event.kind(),
            scope: event.scope(),
            prior,
            disposition,
            resulting: self.phase,
            delivery: event.delivery(),
            deadline,
        });
        disposition
    }

    fn apply(&mut self, source: Source, event: Event) -> Disposition {
        match event {
            Event::Bootstrap(generation) => self.observe_bootstrap(generation),
            Event::PlayerId { generation, player } => {
                if self.generation != Some(generation) {
                    return Disposition::Rejected(RejectReason::StaleGeneration);
                }
                if self.player == Some(player) {
                    return Disposition::Duplicate;
                }
                if self.player.is_some() {
                    return Disposition::Rejected(RejectReason::WrongPlayer);
                }
                self.player = Some(player);
                self.drain_deferred_lifecycle();
                Disposition::Applied
            }
            Event::Ready { generation, fact } => {
                if self.generation != Some(generation) {
                    return Disposition::Rejected(RejectReason::StaleGeneration);
                }
                let bit = fact.bit();
                if self.readiness & bit != 0 {
                    return Disposition::Duplicate;
                }
                self.readiness |= bit;
                if self.readiness == READY_ALL
                    && let Phase::AwaitingReadiness(scope) = self.phase
                {
                    self.phase = Phase::AwaitingSubmission {
                        scope,
                        kind: BaselineKind::Initial,
                    };
                }
                Disposition::Applied
            }
            Event::Lifecycle { scope, alive } => self.observe_lifecycle(source, scope, alive, true),
            Event::SubmitBaseline {
                scope,
                kind,
                delivery,
                sequence,
                now,
                timeout,
            } => match self.phase {
                Phase::AwaitingSubmission {
                    scope: expected,
                    kind: expected_kind,
                } if expected == scope && expected_kind == kind => {
                    self.phase = Phase::AwaitingApplication {
                        scope,
                        kind,
                        delivery,
                        sequence,
                        deadline: now.wrapping_add(timeout),
                    };
                    Disposition::Applied
                }
                Phase::AwaitingApplication {
                    scope: expected,
                    kind: expected_kind,
                    delivery: expected_delivery,
                    sequence: expected_sequence,
                    ..
                } if expected == scope
                    && expected_kind == kind
                    && expected_delivery == delivery
                    && expected_sequence == sequence =>
                {
                    Disposition::Duplicate
                }
                _ => self.reject_for_scope(scope),
            },
            Event::TransportAck { scope, delivery } => match self.phase {
                Phase::AwaitingApplication {
                    scope: expected,
                    delivery: expected_delivery,
                    ..
                }
                | Phase::CorrectionAwaitingApplication {
                    scope: expected,
                    delivery: expected_delivery,
                    ..
                } if expected == scope && expected_delivery == delivery => Disposition::Observed,
                _ => self.reject_for_scope(scope),
            },
            Event::RetryDurable { scope, delivery } => match self.phase {
                Phase::AwaitingApplication {
                    scope: expected,
                    delivery: expected_delivery,
                    ..
                }
                | Phase::CorrectionAwaitingApplication {
                    scope: expected,
                    delivery: expected_delivery,
                    ..
                } if expected == scope && expected_delivery == delivery => Disposition::Observed,
                _ => self.reject_for_scope(scope),
            },
            Event::ApplicationReceipt {
                scope,
                kind,
                delivery,
                sequence,
            } => self.complete_baseline(scope, kind, delivery, sequence),
            Event::RequestCutover { scope, kind } => {
                if matches!(kind, BaselineKind::Initial | BaselineKind::Respawn) {
                    return Disposition::Rejected(RejectReason::ImpossiblePhase);
                }
                match self.phase {
                    Phase::Live(expected) if expected == scope => {
                        self.phase = Phase::CutoverRequested { scope, kind };
                        Disposition::Applied
                    }
                    Phase::CutoverRequested {
                        scope: expected,
                        kind: expected_kind,
                    } if expected == scope && expected_kind == kind => Disposition::Duplicate,
                    _ => self.reject_for_scope(scope),
                }
            }
            Event::Death { scope, respawn_at } => match self.phase {
                Phase::Live(expected)
                | Phase::CorrectionAwaitingSubmission {
                    scope: expected, ..
                }
                | Phase::CorrectionAwaitingApplication {
                    scope: expected, ..
                } if expected == scope => {
                    self.cancel_active_transition();
                    self.phase = Phase::Dead { scope, respawn_at };
                    Disposition::Applied
                }
                Phase::Dead {
                    scope: expected,
                    respawn_at: expected_at,
                } if expected == scope && expected_at == respawn_at => Disposition::Duplicate,
                _ => self.reject_for_scope(scope),
            },
            Event::CorrectionProposed {
                scope,
                application,
                sequence,
            } => match self.phase {
                Phase::Live(expected) if expected == scope => {
                    self.phase = Phase::CorrectionAwaitingSubmission {
                        scope,
                        application,
                        sequence,
                    };
                    Disposition::Applied
                }
                Phase::CorrectionAwaitingSubmission {
                    scope: expected,
                    application: expected_application,
                    sequence: expected_sequence,
                } if expected == scope
                    && expected_application == application
                    && expected_sequence == sequence =>
                {
                    Disposition::Duplicate
                }
                _ => self.reject_for_scope(scope),
            },
            Event::SubmitCorrection {
                scope,
                application,
                sequence,
                delivery,
                now,
                timeout,
            } => match self.phase {
                Phase::CorrectionAwaitingSubmission {
                    scope: expected,
                    application: expected_application,
                    sequence: expected_sequence,
                } if expected == scope
                    && expected_application == application
                    && expected_sequence == sequence =>
                {
                    self.phase = Phase::CorrectionAwaitingApplication {
                        scope,
                        application,
                        sequence,
                        delivery,
                        deadline: now.wrapping_add(timeout),
                    };
                    Disposition::Applied
                }
                _ => self.reject_for_scope(scope),
            },
            Event::CorrectionReceipt {
                scope,
                application,
                sequence,
                delivery,
            } => self.complete_correction(scope, application, sequence, delivery),
            Event::Gameplay { scope, .. } => match self.phase {
                Phase::Live(expected) if expected == scope => Disposition::Applied,
                _ => self.reject_for_scope(scope),
            },
            Event::Tick(now) => {
                let closed = match self.phase {
                    Phase::AwaitingApplication {
                        scope,
                        kind,
                        delivery,
                        sequence,
                        deadline,
                    } if tick_reached(now, deadline) => Some(ClosedTransition {
                        scope,
                        kind: ClosedKind::Baseline(kind),
                        delivery,
                        sequence,
                        completed: false,
                    }),
                    Phase::CorrectionAwaitingApplication {
                        scope,
                        application: _,
                        sequence,
                        delivery,
                        deadline,
                    } if tick_reached(now, deadline) => Some(ClosedTransition {
                        scope,
                        kind: ClosedKind::Correction,
                        delivery,
                        sequence,
                        completed: false,
                    }),
                    _ => None,
                };
                if let Some(closed) = closed {
                    self.push_closed(closed);
                    self.phase = Phase::Failed(Some(closed.scope));
                    Disposition::TimedOut
                } else {
                    Disposition::Observed
                }
            }
            Event::Disconnect { session } => {
                if self
                    .generation
                    .is_some_and(|generation| generation.session == session)
                {
                    self.cancel_active_transition();
                    self.generation = None;
                    self.player = None;
                    self.readiness = 0;
                    self.deferred_lifecycle.clear();
                    self.authority_segment = self.authority_segment.wrapping_add(1);
                    self.phase = Phase::Disconnected;
                    Disposition::Applied
                } else {
                    Disposition::Rejected(RejectReason::StaleGeneration)
                }
            }
        }
    }

    fn observe_bootstrap(&mut self, generation: Generation) -> Disposition {
        if self.generation == Some(generation) {
            return Disposition::Duplicate;
        }
        if self
            .generation
            .is_some_and(|current| !generation_is_newer(generation, current))
        {
            return Disposition::Rejected(RejectReason::StaleGeneration);
        }
        self.cancel_active_transition();
        self.generation = Some(generation);
        self.player = None;
        self.readiness = 0;
        self.deferred_lifecycle.clear();
        self.authority_segment = self.authority_segment.wrapping_add(1);
        self.phase = Phase::Bootstrapping(generation);
        Disposition::Replacement
    }

    fn observe_lifecycle(
        &mut self,
        source: Source,
        scope: Scope,
        alive: bool,
        allow_defer: bool,
    ) -> Disposition {
        if self.generation != Some(scope.generation) {
            return Disposition::Rejected(RejectReason::StaleGeneration);
        }
        let Some(player) = self.player else {
            if !allow_defer {
                return Disposition::Rejected(RejectReason::ImpossiblePhase);
            }
            if self.deferred_lifecycle.len() == MAX_DEFERRED_FACTS {
                return Disposition::Rejected(RejectReason::Capacity);
            }
            if self
                .deferred_lifecycle
                .iter()
                .any(|(_, deferred_scope, deferred_alive)| {
                    *deferred_scope == scope && *deferred_alive == alive
                })
            {
                return Disposition::Duplicate;
            }
            self.deferred_lifecycle.push_back((source, scope, alive));
            return Disposition::Deferred;
        };
        if player != scope.player {
            return Disposition::Rejected(RejectReason::WrongPlayer);
        }

        let current = self.phase.scope();
        match current {
            None => {
                self.install_first_lifecycle(scope, alive);
                Disposition::Applied
            }
            Some(expected) if expected == scope => match (self.phase, alive) {
                (Phase::Live(_), false)
                | (Phase::CorrectionAwaitingSubmission { .. }, false)
                | (Phase::CorrectionAwaitingApplication { .. }, false) => {
                    self.cancel_active_transition();
                    self.phase = Phase::Dead {
                        scope,
                        respawn_at: 0,
                    };
                    Disposition::Applied
                }
                (Phase::Dead { .. }, true) => Disposition::Rejected(RejectReason::ImpossiblePhase),
                _ => Disposition::Duplicate,
            },
            Some(expected) if life_is_newer(scope.life, expected.life) => {
                let kind = match self.phase {
                    Phase::Dead { .. } => BaselineKind::Respawn,
                    Phase::CutoverRequested { kind, .. } => kind,
                    _ => return Disposition::Rejected(RejectReason::ImpossiblePhase),
                };
                self.cancel_active_transition();
                if alive {
                    self.phase = Phase::AwaitingSubmission { scope, kind };
                } else {
                    self.phase = Phase::Dead {
                        scope,
                        respawn_at: 0,
                    };
                }
                Disposition::Replacement
            }
            Some(_) => Disposition::Rejected(RejectReason::StaleScope),
        }
    }

    fn install_first_lifecycle(&mut self, scope: Scope, alive: bool) {
        if alive {
            self.phase = if self.readiness == READY_ALL {
                Phase::AwaitingSubmission {
                    scope,
                    kind: BaselineKind::Initial,
                }
            } else {
                Phase::AwaitingReadiness(scope)
            };
        } else {
            self.phase = Phase::Dead {
                scope,
                respawn_at: 0,
            };
        }
    }

    fn drain_deferred_lifecycle(&mut self) {
        let deferred = std::mem::take(&mut self.deferred_lifecycle);
        for (source, scope, alive) in deferred {
            let _ = self.observe_lifecycle(source, scope, alive, false);
        }
    }

    fn complete_baseline(
        &mut self,
        scope: Scope,
        kind: BaselineKind,
        delivery: u32,
        sequence: u16,
    ) -> Disposition {
        if self.generation != Some(scope.generation) {
            return Disposition::Rejected(RejectReason::StaleGeneration);
        }
        if let Some(closed) =
            self.find_closed(scope, ClosedKind::Baseline(kind), delivery, sequence)
        {
            return if closed.completed {
                Disposition::Duplicate
            } else {
                Disposition::Rejected(RejectReason::Cancelled)
            };
        }
        match self.phase {
            Phase::AwaitingApplication {
                scope: expected,
                kind: expected_kind,
                delivery: expected_delivery,
                sequence: expected_sequence,
                ..
            } if expected == scope
                && expected_kind == kind
                && expected_delivery == delivery
                && expected_sequence == sequence =>
            {
                self.push_closed(ClosedTransition {
                    scope,
                    kind: ClosedKind::Baseline(kind),
                    delivery,
                    sequence,
                    completed: true,
                });
                self.phase = Phase::Live(scope);
                Disposition::Applied
            }
            Phase::AwaitingApplication {
                scope: expected, ..
            } if expected == scope => Disposition::Rejected(RejectReason::IdentityMismatch),
            _ => self.reject_for_scope(scope),
        }
    }

    fn complete_correction(
        &mut self,
        scope: Scope,
        application: u32,
        sequence: u16,
        delivery: u32,
    ) -> Disposition {
        if self.generation != Some(scope.generation) {
            return Disposition::Rejected(RejectReason::StaleGeneration);
        }
        if let Some(closed) = self.find_closed(scope, ClosedKind::Correction, delivery, sequence) {
            return if closed.completed {
                Disposition::Duplicate
            } else {
                Disposition::Rejected(RejectReason::Cancelled)
            };
        }
        match self.phase {
            Phase::CorrectionAwaitingApplication {
                scope: expected,
                application: expected_application,
                sequence: expected_sequence,
                delivery: expected_delivery,
                ..
            } if expected == scope
                && expected_application == application
                && expected_sequence == sequence
                && expected_delivery == delivery =>
            {
                self.push_closed(ClosedTransition {
                    scope,
                    kind: ClosedKind::Correction,
                    delivery,
                    sequence,
                    completed: true,
                });
                self.phase = Phase::Live(scope);
                Disposition::Applied
            }
            Phase::CorrectionAwaitingApplication {
                scope: expected, ..
            } if expected == scope => Disposition::Rejected(RejectReason::IdentityMismatch),
            _ => self.reject_for_scope(scope),
        }
    }

    fn cancel_active_transition(&mut self) {
        let closed = match self.phase {
            Phase::AwaitingApplication {
                scope,
                kind,
                delivery,
                sequence,
                ..
            } => Some(ClosedTransition {
                scope,
                kind: ClosedKind::Baseline(kind),
                delivery,
                sequence,
                completed: false,
            }),
            Phase::CorrectionAwaitingApplication {
                scope,
                sequence,
                delivery,
                ..
            } => Some(ClosedTransition {
                scope,
                kind: ClosedKind::Correction,
                delivery,
                sequence,
                completed: false,
            }),
            _ => None,
        };
        if let Some(closed) = closed {
            self.push_closed(closed);
        }
    }

    fn reject_for_scope(&self, scope: Scope) -> Disposition {
        if self.generation != Some(scope.generation) {
            Disposition::Rejected(RejectReason::StaleGeneration)
        } else if self.player != Some(scope.player) || self.phase.scope() != Some(scope) {
            Disposition::Rejected(RejectReason::StaleScope)
        } else {
            Disposition::Rejected(RejectReason::ImpossiblePhase)
        }
    }

    fn find_closed(
        &self,
        scope: Scope,
        kind: ClosedKind,
        delivery: u32,
        sequence: u16,
    ) -> Option<ClosedTransition> {
        self.closed.iter().copied().find(|closed| {
            closed.scope == scope
                && closed.kind == kind
                && closed.delivery == delivery
                && closed.sequence == sequence
        })
    }

    fn push_closed(&mut self, transition: ClosedTransition) {
        if self.closed.len() == MAX_CLOSED_TRANSITIONS {
            self.closed.pop_front();
        }
        self.closed.push_back(transition);
    }

    fn push_history(&mut self, entry: HistoryEntry) {
        if self.history.len() == MAX_HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(entry);
    }
}

fn generation_is_newer(candidate: Generation, current: Generation) -> bool {
    if candidate.session == current.session {
        serial_u64_is_newer(candidate.stream, current.stream)
    } else {
        serial_u64_is_newer(candidate.session, current.session)
    }
}

fn serial_u64_is_newer(candidate: u64, current: u64) -> bool {
    candidate != current && candidate.wrapping_sub(current) < (1_u64 << 63)
}

fn life_is_newer(candidate: u32, current: u32) -> bool {
    candidate != current && candidate.wrapping_sub(current) < (1_u32 << 31)
}

fn tick_reached(now: u32, deadline: u32) -> bool {
    now.wrapping_sub(deadline) < (1_u32 << 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENERATION: Generation = Generation {
        session: 7,
        stream: 11,
    };
    const SCOPE: Scope = Scope {
        generation: GENERATION,
        player: 23,
        life: 41,
    };

    fn observe(model: &mut ReferenceSequencer, event: Event) -> Disposition {
        let source = match event {
            Event::Gameplay { .. } => Source::Realtime,
            Event::Lifecycle { .. } | Event::Ready { .. } => Source::Event,
            Event::Bootstrap(_)
            | Event::PlayerId { .. }
            | Event::TransportAck { .. }
            | Event::RetryDurable { .. }
            | Event::ApplicationReceipt { .. }
            | Event::CorrectionProposed { .. }
            | Event::CorrectionReceipt { .. } => Source::Reliable,
            Event::SubmitBaseline { .. }
            | Event::RequestCutover { .. }
            | Event::Death { .. }
            | Event::SubmitCorrection { .. }
            | Event::Tick(_)
            | Event::Disconnect { .. } => Source::Local,
        };
        model.observe(source, event)
    }

    fn readiness_events(generation: Generation) -> [Event; 5] {
        [
            Event::Ready {
                generation,
                fact: ReadyFact::Joined,
            },
            Event::Ready {
                generation,
                fact: ReadyFact::Settings,
            },
            Event::Ready {
                generation,
                fact: ReadyFact::Security,
            },
            Event::Ready {
                generation,
                fact: ReadyFact::Map,
            },
            Event::Ready {
                generation,
                fact: ReadyFact::Files,
            },
        ]
    }

    fn establish_awaiting_initial(model: &mut ReferenceSequencer, scope: Scope) {
        assert_eq!(
            observe(model, Event::Bootstrap(scope.generation)),
            Disposition::Replacement
        );
        assert_eq!(
            observe(
                model,
                Event::PlayerId {
                    generation: scope.generation,
                    player: scope.player,
                }
            ),
            Disposition::Applied
        );
        for event in readiness_events(scope.generation) {
            assert_eq!(observe(model, event), Disposition::Applied);
        }
        assert_eq!(
            observe(model, Event::Lifecycle { scope, alive: true }),
            Disposition::Applied
        );
        assert_eq!(
            model.phase,
            Phase::AwaitingSubmission {
                scope,
                kind: BaselineKind::Initial,
            }
        );
    }

    fn establish_live(model: &mut ReferenceSequencer, scope: Scope) {
        establish_awaiting_initial(model, scope);
        assert_eq!(
            observe(
                model,
                Event::SubmitBaseline {
                    scope,
                    kind: BaselineKind::Initial,
                    delivery: 19,
                    sequence: 65535,
                    now: u32::MAX - 2,
                    timeout: 6,
                }
            ),
            Disposition::Applied
        );
        assert_eq!(
            observe(
                model,
                Event::ApplicationReceipt {
                    scope,
                    kind: BaselineKind::Initial,
                    delivery: 19,
                    sequence: 65535,
                }
            ),
            Disposition::Applied
        );
        assert_eq!(model.phase, Phase::Live(scope));
    }

    fn permutations<const N: usize>(mut values: [Event; N]) -> Vec<[Event; N]> {
        fn generate<const N: usize>(
            values: &mut [Event; N],
            start: usize,
            out: &mut Vec<[Event; N]>,
        ) {
            if start == N {
                out.push(*values);
                return;
            }
            for index in start..N {
                values.swap(start, index);
                generate(values, start + 1, out);
                values.swap(start, index);
            }
        }
        let mut out = Vec::new();
        generate(&mut values, 0, &mut out);
        out
    }

    #[test]
    fn bootstrap_join_readiness_permutations_converge_step_by_step() {
        let events = [
            Event::PlayerId {
                generation: GENERATION,
                player: SCOPE.player,
            },
            Event::Ready {
                generation: GENERATION,
                fact: ReadyFact::Joined,
            },
            Event::Ready {
                generation: GENERATION,
                fact: ReadyFact::Settings,
            },
            Event::Ready {
                generation: GENERATION,
                fact: ReadyFact::Security,
            },
            Event::Ready {
                generation: GENERATION,
                fact: ReadyFact::Map,
            },
            Event::Ready {
                generation: GENERATION,
                fact: ReadyFact::Files,
            },
            Event::Lifecycle {
                scope: SCOPE,
                alive: true,
            },
        ];

        for permutation in permutations(events) {
            let mut model = ReferenceSequencer::default();
            assert_eq!(
                observe(&mut model, Event::Bootstrap(GENERATION)),
                Disposition::Replacement
            );
            for event in permutation {
                let disposition = observe(&mut model, event);
                assert!(matches!(
                    disposition,
                    Disposition::Applied | Disposition::Deferred
                ));
                assert!(model.history.len() <= MAX_HISTORY);
                assert!(model.deferred_lifecycle.len() <= MAX_DEFERRED_FACTS);
                assert_ne!(model.phase, Phase::Live(SCOPE));
            }
            assert_eq!(
                model.phase,
                Phase::AwaitingSubmission {
                    scope: SCOPE,
                    kind: BaselineKind::Initial,
                }
            );
        }
    }

    #[test]
    fn durable_submission_transport_ack_and_receipt_are_distinct() {
        let mut model = ReferenceSequencer::default();
        establish_awaiting_initial(&mut model, SCOPE);
        let submit = Event::SubmitBaseline {
            scope: SCOPE,
            kind: BaselineKind::Initial,
            delivery: 28,
            sequence: 25044,
            now: 100,
            timeout: 20,
        };
        assert_eq!(observe(&mut model, submit), Disposition::Applied);
        let awaiting = model.phase;
        assert_eq!(observe(&mut model, submit), Disposition::Duplicate);
        assert_eq!(
            observe(
                &mut model,
                Event::TransportAck {
                    scope: SCOPE,
                    delivery: 28,
                }
            ),
            Disposition::Observed
        );
        assert_eq!(model.phase, awaiting);
        assert_eq!(
            observe(
                &mut model,
                Event::RetryDurable {
                    scope: SCOPE,
                    delivery: 28,
                }
            ),
            Disposition::Observed
        );
        assert_eq!(model.phase, awaiting);

        let receipt = Event::ApplicationReceipt {
            scope: SCOPE,
            kind: BaselineKind::Initial,
            delivery: 28,
            sequence: 25044,
        };
        assert_eq!(observe(&mut model, receipt), Disposition::Applied);
        assert_eq!(model.phase, Phase::Live(SCOPE));
        assert_eq!(observe(&mut model, receipt), Disposition::Duplicate);
        assert_eq!(
            observe(
                &mut model,
                Event::Lifecycle {
                    scope: SCOPE,
                    alive: true,
                }
            ),
            Disposition::Duplicate
        );
        assert_eq!(model.phase, Phase::Live(SCOPE));
    }

    #[test]
    fn missing_transport_ack_does_not_block_application_receipt_and_missing_receipt_times_out() {
        let mut model = ReferenceSequencer::default();
        establish_awaiting_initial(&mut model, SCOPE);
        let submit = Event::SubmitBaseline {
            scope: SCOPE,
            kind: BaselineKind::Initial,
            delivery: u32::MAX,
            sequence: u16::MAX,
            now: u32::MAX - 2,
            timeout: 5,
        };
        assert_eq!(observe(&mut model, submit), Disposition::Applied);
        assert_eq!(observe(&mut model, Event::Tick(1)), Disposition::Observed);
        assert_eq!(
            observe(
                &mut model,
                Event::ApplicationReceipt {
                    scope: SCOPE,
                    kind: BaselineKind::Initial,
                    delivery: u32::MAX,
                    sequence: u16::MAX,
                }
            ),
            Disposition::Applied
        );

        let mut timed_out = ReferenceSequencer::default();
        establish_awaiting_initial(&mut timed_out, SCOPE);
        assert_eq!(observe(&mut timed_out, submit), Disposition::Applied);
        assert_eq!(
            observe(&mut timed_out, Event::Tick(2)),
            Disposition::TimedOut
        );
        let failed = timed_out.phase;
        assert_eq!(
            observe(
                &mut timed_out,
                Event::ApplicationReceipt {
                    scope: SCOPE,
                    kind: BaselineKind::Initial,
                    delivery: u32::MAX,
                    sequence: u16::MAX,
                }
            ),
            Disposition::Rejected(RejectReason::Cancelled)
        );
        assert_eq!(timed_out.phase, failed);
    }

    #[test]
    fn ship_and_frequency_cutovers_reject_predecessors_in_both_snapshot_orders() {
        for kind in [BaselineKind::Ship, BaselineKind::Frequency] {
            for duplicate_before_receipt in [false, true] {
                let mut model = ReferenceSequencer::default();
                establish_live(&mut model, SCOPE);
                assert_eq!(
                    observe(&mut model, Event::RequestCutover { scope: SCOPE, kind }),
                    Disposition::Applied
                );
                let successor = Scope {
                    life: SCOPE.life.wrapping_add(1),
                    ..SCOPE
                };
                assert_eq!(
                    observe(
                        &mut model,
                        Event::Lifecycle {
                            scope: successor,
                            alive: true,
                        }
                    ),
                    Disposition::Replacement
                );
                assert_eq!(
                    observe(
                        &mut model,
                        Event::SubmitBaseline {
                            scope: successor,
                            kind,
                            delivery: 30,
                            sequence: 0,
                            now: 8,
                            timeout: 10,
                        }
                    ),
                    Disposition::Applied
                );
                if duplicate_before_receipt {
                    assert_eq!(
                        observe(
                            &mut model,
                            Event::Lifecycle {
                                scope: successor,
                                alive: true,
                            }
                        ),
                        Disposition::Duplicate
                    );
                }
                assert_eq!(
                    observe(
                        &mut model,
                        Event::Gameplay {
                            scope: SCOPE,
                            sequence: u16::MAX,
                        }
                    ),
                    Disposition::Rejected(RejectReason::StaleScope)
                );
                assert_eq!(
                    observe(
                        &mut model,
                        Event::ApplicationReceipt {
                            scope: successor,
                            kind,
                            delivery: 30,
                            sequence: 0,
                        }
                    ),
                    Disposition::Applied
                );
                assert_eq!(model.phase, Phase::Live(successor));
                if !duplicate_before_receipt {
                    assert_eq!(
                        observe(
                            &mut model,
                            Event::Lifecycle {
                                scope: successor,
                                alive: true,
                            }
                        ),
                        Disposition::Duplicate
                    );
                }
                assert_eq!(model.phase, Phase::Live(successor));
            }
        }
    }

    #[test]
    fn death_respawn_and_zero_delay_keep_old_life_inert() {
        for respawn_at in [0, 500] {
            let mut model = ReferenceSequencer::default();
            establish_live(&mut model, SCOPE);
            assert_eq!(
                observe(
                    &mut model,
                    Event::Death {
                        scope: SCOPE,
                        respawn_at,
                    }
                ),
                Disposition::Applied
            );
            assert_eq!(
                observe(
                    &mut model,
                    Event::Gameplay {
                        scope: SCOPE,
                        sequence: 55,
                    }
                ),
                Disposition::Rejected(RejectReason::ImpossiblePhase)
            );
            let successor = Scope {
                life: SCOPE.life.wrapping_add(1),
                ..SCOPE
            };
            assert_eq!(
                observe(
                    &mut model,
                    Event::Lifecycle {
                        scope: successor,
                        alive: true,
                    }
                ),
                Disposition::Replacement
            );
            assert_eq!(
                model.phase,
                Phase::AwaitingSubmission {
                    scope: successor,
                    kind: BaselineKind::Respawn,
                }
            );
            assert_eq!(
                observe(
                    &mut model,
                    Event::Gameplay {
                        scope: successor,
                        sequence: 56,
                    }
                ),
                Disposition::Rejected(RejectReason::ImpossiblePhase)
            );
            assert_eq!(
                observe(
                    &mut model,
                    Event::Gameplay {
                        scope: SCOPE,
                        sequence: 57,
                    }
                ),
                Disposition::Rejected(RejectReason::StaleScope)
            );
        }
    }

    #[test]
    fn correction_is_same_life_scoped_idempotent_and_cancelled_by_death() {
        let mut model = ReferenceSequencer::default();
        establish_live(&mut model, SCOPE);
        assert_eq!(
            observe(
                &mut model,
                Event::CorrectionProposed {
                    scope: SCOPE,
                    application: u32::MAX,
                    sequence: u16::MAX,
                }
            ),
            Disposition::Applied
        );
        assert_eq!(
            observe(
                &mut model,
                Event::SubmitCorrection {
                    scope: SCOPE,
                    application: u32::MAX,
                    sequence: u16::MAX,
                    delivery: u32::MAX,
                    now: 10,
                    timeout: 20,
                }
            ),
            Disposition::Applied
        );
        let receipt = Event::CorrectionReceipt {
            scope: SCOPE,
            application: u32::MAX,
            sequence: u16::MAX,
            delivery: u32::MAX,
        };
        assert_eq!(observe(&mut model, receipt), Disposition::Applied);
        assert_eq!(observe(&mut model, receipt), Disposition::Duplicate);
        assert_eq!(model.phase, Phase::Live(SCOPE));

        assert_eq!(
            observe(
                &mut model,
                Event::CorrectionProposed {
                    scope: SCOPE,
                    application: 0,
                    sequence: 0,
                }
            ),
            Disposition::Applied
        );
        assert_eq!(
            observe(
                &mut model,
                Event::SubmitCorrection {
                    scope: SCOPE,
                    application: 0,
                    sequence: 0,
                    delivery: 0,
                    now: 20,
                    timeout: 20,
                }
            ),
            Disposition::Applied
        );
        assert_eq!(
            observe(
                &mut model,
                Event::Death {
                    scope: SCOPE,
                    respawn_at: 40,
                }
            ),
            Disposition::Applied
        );
        let dead = model.phase;
        assert_eq!(
            observe(
                &mut model,
                Event::CorrectionReceipt {
                    scope: SCOPE,
                    application: 0,
                    sequence: 0,
                    delivery: 0,
                }
            ),
            Disposition::Rejected(RejectReason::Cancelled)
        );
        assert_eq!(model.phase, dead);
    }

    #[test]
    fn disconnect_reconnect_reused_numeric_ids_do_not_reopen_old_scope() {
        let mut model = ReferenceSequencer::default();
        establish_live(&mut model, SCOPE);
        assert_eq!(
            observe(
                &mut model,
                Event::Disconnect {
                    session: GENERATION.session,
                }
            ),
            Disposition::Applied
        );
        assert_eq!(model.phase, Phase::Disconnected);
        let replacement_generation = Generation {
            session: GENERATION.session + 1,
            stream: GENERATION.stream,
        };
        let replacement_scope = Scope {
            generation: replacement_generation,
            player: SCOPE.player,
            life: SCOPE.life,
        };
        establish_awaiting_initial(&mut model, replacement_scope);
        assert_eq!(
            observe(
                &mut model,
                Event::ApplicationReceipt {
                    scope: SCOPE,
                    kind: BaselineKind::Initial,
                    delivery: 19,
                    sequence: u16::MAX,
                }
            ),
            Disposition::Rejected(RejectReason::StaleGeneration)
        );
        assert_eq!(
            model.phase,
            Phase::AwaitingSubmission {
                scope: replacement_scope,
                kind: BaselineKind::Initial,
            }
        );
    }

    #[test]
    fn session_stream_life_and_protocol_identities_wrap_without_aliasing() {
        let maximum_generation = Generation {
            session: u64::MAX,
            stream: u64::MAX,
        };
        let maximum_scope = Scope {
            generation: maximum_generation,
            player: u16::MAX,
            life: u32::MAX,
        };
        let mut model = ReferenceSequencer::default();
        establish_live(&mut model, maximum_scope);
        assert_eq!(model.authority_segment, 1);
        assert_eq!(
            observe(
                &mut model,
                Event::Death {
                    scope: maximum_scope,
                    respawn_at: u32::MAX,
                }
            ),
            Disposition::Applied
        );
        let wrapped_scope = Scope {
            life: 0,
            ..maximum_scope
        };
        assert_eq!(
            observe(
                &mut model,
                Event::Lifecycle {
                    scope: wrapped_scope,
                    alive: true,
                }
            ),
            Disposition::Replacement
        );
        assert_eq!(
            observe(
                &mut model,
                Event::SubmitBaseline {
                    scope: wrapped_scope,
                    kind: BaselineKind::Respawn,
                    delivery: 0,
                    sequence: 0,
                    now: u32::MAX,
                    timeout: 2,
                }
            ),
            Disposition::Applied
        );
        assert_eq!(
            observe(
                &mut model,
                Event::ApplicationReceipt {
                    scope: wrapped_scope,
                    kind: BaselineKind::Respawn,
                    delivery: 0,
                    sequence: 0,
                }
            ),
            Disposition::Applied
        );

        assert_eq!(
            observe(&mut model, Event::Disconnect { session: u64::MAX }),
            Disposition::Applied
        );
        let wrapped_generation = Generation {
            session: 0,
            stream: 0,
        };
        assert_eq!(
            observe(&mut model, Event::Bootstrap(wrapped_generation)),
            Disposition::Replacement
        );
    }

    #[test]
    fn predecessor_bootstrap_and_gameplay_never_roll_back_successor() {
        let mut model = ReferenceSequencer::default();
        establish_live(&mut model, SCOPE);
        let successor_generation = Generation {
            session: GENERATION.session + 1,
            stream: 0,
        };
        assert_eq!(
            observe(&mut model, Event::Bootstrap(successor_generation)),
            Disposition::Replacement
        );
        let successor_phase = model.phase;
        assert_eq!(
            observe(&mut model, Event::Bootstrap(GENERATION)),
            Disposition::Rejected(RejectReason::StaleGeneration)
        );
        assert_eq!(
            observe(
                &mut model,
                Event::Gameplay {
                    scope: SCOPE,
                    sequence: 0,
                }
            ),
            Disposition::Rejected(RejectReason::StaleGeneration)
        );
        assert_eq!(model.phase, successor_phase);
    }

    #[test]
    fn pending_and_diagnostic_state_is_bounded_under_hostile_repetition() {
        let mut model = ReferenceSequencer::default();
        assert_eq!(
            observe(&mut model, Event::Bootstrap(GENERATION)),
            Disposition::Replacement
        );
        for life in 0..(MAX_DEFERRED_FACTS as u32 + 8) {
            let disposition = observe(
                &mut model,
                Event::Lifecycle {
                    scope: Scope { life, ..SCOPE },
                    alive: true,
                },
            );
            if life < MAX_DEFERRED_FACTS as u32 {
                assert_eq!(disposition, Disposition::Deferred);
            } else {
                assert_eq!(disposition, Disposition::Rejected(RejectReason::Capacity));
            }
        }
        assert_eq!(model.deferred_lifecycle.len(), MAX_DEFERRED_FACTS);

        for tick in 0..(MAX_HISTORY as u32 * 3) {
            let _ = observe(&mut model, Event::Tick(tick));
        }
        assert_eq!(model.history.len(), MAX_HISTORY);

        let mut completed = ReferenceSequencer::default();
        establish_live(&mut completed, SCOPE);
        for identity in 0..(MAX_CLOSED_TRANSITIONS as u32 + 8) {
            assert_eq!(
                observe(
                    &mut completed,
                    Event::CorrectionProposed {
                        scope: SCOPE,
                        application: identity,
                        sequence: identity as u16,
                    }
                ),
                Disposition::Applied
            );
            assert_eq!(
                observe(
                    &mut completed,
                    Event::SubmitCorrection {
                        scope: SCOPE,
                        application: identity,
                        sequence: identity as u16,
                        delivery: identity,
                        now: identity,
                        timeout: 20,
                    }
                ),
                Disposition::Applied
            );
            assert_eq!(
                observe(
                    &mut completed,
                    Event::CorrectionReceipt {
                        scope: SCOPE,
                        application: identity,
                        sequence: identity as u16,
                        delivery: identity,
                    }
                ),
                Disposition::Applied
            );
        }
        assert_eq!(completed.closed.len(), MAX_CLOSED_TRANSITIONS);
        assert_eq!(completed.history.len(), MAX_HISTORY);
    }
}

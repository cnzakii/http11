// Portions of this state-machine implementation are adapted from h11 0.16.0.
// Copyright (c) 2016 Nathaniel J. Smith <njs@pobox.com> and other contributors.
// Licensed under the MIT License; see LICENSE.h11.
// Upstream source:
// https://github.com/python-hyper/h11/blob/1c5b07581f058886c8bdd87adababd7d959dc7ca/h11/_state.py

//! HTTP/1.1 request and response lifecycle state.
//!
//! A connection tracks client and server message progress, reuse, and pending
//! CONNECT or Upgrade decisions. Client and server are protocol roles, not
//! local and remote endpoints.
//!
//! An event first advances one role. Joint-state rules then run to a fixed
//! point; protocol switching takes priority over mandatory closure. Rejected
//! events leave the complete connection state unchanged.

/// The client or server actor in an HTTP/1 exchange.
///
/// A role is protocol-relative: it does not mean local or remote. A
/// [`crate::Connection`] maps these actors to its local and peer endpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// Sends requests and receives responses.
    Client,
    /// Receives requests and sends responses.
    Server,
}

impl Role {
    pub(crate) const fn peer(self) -> Self {
        match self {
            Self::Client => Self::Server,
            Self::Server => Self::Client,
        }
    }
}

/// Observable lifecycle state of one HTTP/1 protocol actor.
///
/// Client and server share the enum, but each role reaches only the states
/// meaningful to its half of the exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    /// Ready for a new request or, on a server, an early error response.
    Idle,
    /// The server owes a final response.
    SendResponse,
    /// The actor may send or receive body data followed by end-of-message.
    SendBody,
    /// The actor finished its message and the connection may be reusable.
    Done,
    /// The client finished a request that proposed a protocol switch.
    MightSwitchProtocol,
    /// HTTP processing ended because another protocol took over.
    SwitchedProtocol,
    /// The actor finished, but connection reuse is forbidden.
    MustClose,
    /// The actor closed its side of the connection.
    Closed,
    /// A protocol error permanently poisoned this actor.
    Error,
}

// CONNECT and Upgrade remain independent until the server accepts or rejects
// them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SwitchProposals {
    connect: bool,
    upgrade: bool,
}

impl SwitchProposals {
    pub(crate) const NONE: Self = Self {
        connect: false,
        upgrade: false,
    };
    #[cfg(test)]
    const CONNECT: Self = Self {
        connect: true,
        upgrade: false,
    };
    #[cfg(test)]
    const UPGRADE: Self = Self {
        connect: false,
        upgrade: true,
    };

    const fn contains(self, kind: SwitchKind) -> bool {
        match kind {
            SwitchKind::Connect => self.connect,
            SwitchKind::Upgrade => self.upgrade,
        }
    }

    pub(crate) const fn from_flags(connect: bool, upgrade: bool) -> Self {
        Self { connect, upgrade }
    }

    const fn is_empty(self) -> bool {
        !self.connect && !self.upgrade
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SwitchKind {
    Connect,
    Upgrade,
}

// These are semantic events after message and header validation.
// `ProtocolSwitch` means a valid 101 Upgrade or successful CONNECT response,
// not an unvalidated response head.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StateEvent {
    Request(SwitchProposals),
    InformationalResponse,
    Response,
    Data,
    EndOfMessage,
    ProtocolSwitch(SwitchKind),
    ConnectionClosed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InvalidTransition;

// All fields commit together so a rejected event cannot leave partial state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ConnectionState {
    client: State,
    server: State,
    keep_alive: bool,
    proposals: SwitchProposals,
}

impl ConnectionState {
    pub(crate) const fn new() -> Self {
        Self {
            client: State::Idle,
            server: State::Idle,
            keep_alive: true,
            proposals: SwitchProposals::NONE,
        }
    }

    pub(crate) fn state(self, role: Role) -> State {
        match role {
            Role::Client => self.client,
            Role::Server => self.server,
        }
    }

    pub(crate) fn process_event(
        &mut self,
        role: Role,
        event: StateEvent,
    ) -> Result<(), InvalidTransition> {
        // Apply to a copy so rejection cannot commit a partial transition.
        let mut next = *self;
        let current = match role {
            Role::Client => next.client,
            Role::Server => next.server,
        };
        let transitioned = direct_transition(role, current, event).ok_or(InvalidTransition)?;

        match (role, event) {
            (Role::Client, StateEvent::Request(proposals)) => {
                if next.server != State::Idle {
                    return Err(InvalidTransition);
                }
                // A request starts the server's response obligation.
                next.proposals = proposals;
                next.server = State::SendResponse;
            }
            (Role::Server, StateEvent::ProtocolSwitch(kind)) => {
                // Upgrade and CONNECT can switch only in response to the
                // corresponding request proposal.
                // https://www.rfc-editor.org/rfc/rfc9110.html#section-7.8
                // https://www.rfc-editor.org/rfc/rfc9110.html#section-9.3.6
                if !next.proposals.contains(kind) {
                    return Err(InvalidTransition);
                }
            }
            (Role::Server, StateEvent::Response) => {
                // RFC 9931 Section 8 requires a rejected CONNECT to close by
                // default; this state layer has no external trusted-client
                // policy that could opt out of that mitigation.
                // https://www.rfc-editor.org/rfc/rfc9931.html#section-8
                if next.proposals.contains(SwitchKind::Connect) {
                    next.keep_alive = false;
                }
                // An ordinary final response denies every pending switch. A
                // response before a valid Request is also a close-only path.
                next.proposals = SwitchProposals::NONE;
                if current == State::Idle {
                    next.keep_alive = false;
                }
            }
            _ => {}
        }

        match role {
            Role::Client => next.client = transitioned,
            Role::Server => next.server = transitioned,
        }
        next.stabilize();
        *self = next;
        Ok(())
    }

    pub(crate) fn disable_keep_alive(&mut self) {
        self.keep_alive = false;
        self.stabilize();
    }

    pub(crate) const fn keep_alive(self) -> bool {
        self.keep_alive
    }

    pub(crate) fn process_error(&mut self, role: Role) {
        match role {
            Role::Client => self.client = State::Error,
            Role::Server => self.server = State::Error,
        }
        self.stabilize();
    }

    pub(crate) fn start_next_cycle(&mut self) -> Result<(), InvalidTransition> {
        if self.client != State::Done
            || self.server != State::Done
            || !self.keep_alive
            || !self.proposals.is_empty()
        {
            return Err(InvalidTransition);
        }
        *self = Self::new();
        Ok(())
    }

    // Resolve coupled rules to a fixed point. Switching must win over closing.
    fn stabilize(&mut self) {
        loop {
            let before = *self;

            // A completed request waits for the server's switch decision.
            if self.client == State::Done && !self.proposals.is_empty() {
                self.client = State::MightSwitchProtocol;
            }
            if self.client == State::MightSwitchProtocol && self.server == State::SwitchedProtocol {
                self.client = State::SwitchedProtocol;
            } else if self.client == State::MightSwitchProtocol && self.proposals.is_empty() {
                self.client = State::Done;
            }
            if self.client == State::SwitchedProtocol && self.server == State::SwitchedProtocol {
                self.proposals = SwitchProposals::NONE;
            }

            // Disabled keep-alive affects an actor only after its message ends.
            if !self.keep_alive {
                if self.client == State::Done {
                    self.client = State::MustClose;
                }
                if self.server == State::Done {
                    self.server = State::MustClose;
                }
            }

            // Closing or poisoning one actor prevents the peer from starting
            // another message once its current work is complete.
            match (self.client, self.server) {
                (State::Closed, State::Done | State::Idle) | (State::Error, State::Done) => {
                    self.server = State::MustClose
                }
                (State::Done | State::Idle, State::Closed) | (State::Done, State::Error) => {
                    self.client = State::MustClose
                }
                _ => {}
            }

            if *self == before {
                return;
            }
        }
    }
}

// Event-triggered transitions affect one role. Cross-region effects are
// committed by `process_event` and converged by `stabilize`.
fn direct_transition(role: Role, state: State, event: StateEvent) -> Option<State> {
    match role {
        Role::Client => match state {
            State::Idle => match event {
                StateEvent::Request(_) => Some(State::SendBody),
                StateEvent::ConnectionClosed => Some(State::Closed),
                event => reject(event),
            },
            State::SendBody => match event {
                StateEvent::Data => Some(State::SendBody),
                StateEvent::EndOfMessage => Some(State::Done),
                event => reject(event),
            },
            State::Done | State::MustClose | State::Closed => match event {
                StateEvent::ConnectionClosed => Some(State::Closed),
                event => reject(event),
            },
            State::SendResponse
            | State::MightSwitchProtocol
            | State::SwitchedProtocol
            | State::Error => reject(event),
        },
        Role::Server => match state {
            State::Idle => match event {
                StateEvent::Response => Some(State::SendBody),
                StateEvent::ConnectionClosed => Some(State::Closed),
                event => reject(event),
            },
            State::SendResponse => match event {
                StateEvent::InformationalResponse => Some(State::SendResponse),
                StateEvent::Response => Some(State::SendBody),
                StateEvent::ProtocolSwitch(_) => Some(State::SwitchedProtocol),
                event => reject(event),
            },
            State::SendBody => match event {
                StateEvent::Data => Some(State::SendBody),
                StateEvent::EndOfMessage => Some(State::Done),
                event => reject(event),
            },
            State::Done | State::MustClose | State::Closed => match event {
                StateEvent::ConnectionClosed => Some(State::Closed),
                event => reject(event),
            },
            State::MightSwitchProtocol | State::SwitchedProtocol | State::Error => reject(event),
        },
    }
}

// One exhaustive rejection point keeps new events compiler-visible without
// spelling every invalid state/event pair.
fn reject(event: StateEvent) -> Option<State> {
    match event {
        StateEvent::Request(_)
        | StateEvent::InformationalResponse
        | StateEvent::Response
        | StateEvent::Data
        | StateEvent::EndOfMessage
        | StateEvent::ProtocolSwitch(_)
        | StateEvent::ConnectionClosed => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Role::*;
    use State::*;
    use StateEvent as Event;
    use StateEvent::*;

    const STATES: [State; 9] = [
        State::Idle,
        State::SendResponse,
        State::SendBody,
        State::Done,
        State::MightSwitchProtocol,
        State::SwitchedProtocol,
        State::MustClose,
        State::Closed,
        State::Error,
    ];
    const EVENTS: [StateEvent; 8] = [
        StateEvent::Request(SwitchProposals::NONE),
        StateEvent::InformationalResponse,
        StateEvent::Response,
        StateEvent::Data,
        StateEvent::EndOfMessage,
        StateEvent::ProtocolSwitch(SwitchKind::Connect),
        StateEvent::ProtocolSwitch(SwitchKind::Upgrade),
        StateEvent::ConnectionClosed,
    ];

    // Message sequence: RFC 9112 Sections 2.1 and 9.2.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-2.1
    // Accepted Upgrade: RFC 9110 Section 7.8.
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-7.8
    // Successful CONNECT: RFC 9110 Section 9.3.6.
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-9.3.6
    // Repeated close and a response from Server/Idle are local lifecycle
    // policies consistent with RFC 9112 Sections 2.2 and 9.6, not an RFC
    // automaton. https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-9.6
    const DIRECT_CONTRACT: &[(Role, State, StateEvent, State)] = &[
        (Client, Idle, Request(SwitchProposals::NONE), SendBody),
        (Client, Idle, ConnectionClosed, Closed),
        (Client, SendBody, Data, SendBody),
        (Client, SendBody, EndOfMessage, Done),
        (Client, Done, ConnectionClosed, Closed),
        (Client, MustClose, ConnectionClosed, Closed),
        (Client, Closed, ConnectionClosed, Closed),
        (Server, Idle, Response, SendBody),
        (Server, Idle, ConnectionClosed, Closed),
        (Server, SendResponse, InformationalResponse, SendResponse),
        (Server, SendResponse, Response, SendBody),
        (
            Server,
            SendResponse,
            ProtocolSwitch(SwitchKind::Connect),
            SwitchedProtocol,
        ),
        (
            Server,
            SendResponse,
            ProtocolSwitch(SwitchKind::Upgrade),
            SwitchedProtocol,
        ),
        (Server, SendBody, Data, SendBody),
        (Server, SendBody, EndOfMessage, Done),
        (Server, Done, ConnectionClosed, Closed),
        (Server, MustClose, ConnectionClosed, Closed),
        (Server, Closed, ConnectionClosed, Closed),
    ];

    #[test]
    fn direct_transitions_match_the_protocol_contract() {
        for role in [Role::Client, Role::Server] {
            for state in STATES {
                for event in EVENTS {
                    let expected = DIRECT_CONTRACT.iter().find_map(
                        |&(case_role, case_state, case_event, next)| {
                            (role == case_role && state == case_state && event == case_event)
                                .then_some(next)
                        },
                    );
                    assert_eq!(
                        direct_transition(role, state, event),
                        expected,
                        "{role:?} {state:?} {event:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_complete_exchange_can_be_reused() {
        let mut state = ConnectionState::new();
        state
            .process_event(Role::Client, Event::Request(SwitchProposals::NONE))
            .unwrap();
        assert_eq!(
            (state.client, state.server),
            (State::SendBody, State::SendResponse)
        );

        state.process_event(Role::Client, Event::Data).unwrap();
        state
            .process_event(Role::Client, Event::EndOfMessage)
            .unwrap();
        state
            .process_event(Role::Server, Event::InformationalResponse)
            .unwrap();
        state.process_event(Role::Server, Event::Response).unwrap();
        state.process_event(Role::Server, Event::Data).unwrap();
        state
            .process_event(Role::Server, Event::EndOfMessage)
            .unwrap();

        assert_eq!((state.client, state.server), (State::Done, State::Done));
        state.start_next_cycle().unwrap();
        assert_eq!(state, ConnectionState::new());
    }

    #[test]
    fn invalid_input_is_rejected_atomically() {
        let mut state = ConnectionState::new();
        state
            .process_event(Role::Client, Event::Request(SwitchProposals::NONE))
            .unwrap();
        let before = state;

        assert_eq!(
            state.process_event(Role::Client, Event::Request(SwitchProposals::NONE)),
            Err(InvalidTransition)
        );
        assert_eq!(state, before);
        assert_eq!(
            state.process_event(Role::Server, Event::ProtocolSwitch(SwitchKind::Upgrade)),
            Err(InvalidTransition)
        );
        assert_eq!(state, before);
    }

    // RFC 9112 Section 9.3: a non-persistent connection cannot begin another
    // cycle. https://www.rfc-editor.org/rfc/rfc9112.html#section-9.3
    #[test]
    fn disabling_keep_alive_forces_close_after_each_message() {
        let mut state = ConnectionState::new();
        state
            .process_event(Role::Client, Event::Request(SwitchProposals::NONE))
            .unwrap();
        state.disable_keep_alive();
        state
            .process_event(Role::Client, Event::EndOfMessage)
            .unwrap();
        assert_eq!(state.client, State::MustClose);

        state.process_event(Role::Server, Event::Response).unwrap();
        state
            .process_event(Role::Server, Event::EndOfMessage)
            .unwrap();
        assert_eq!(state.server, State::MustClose);
        assert_eq!(state.start_next_cycle(), Err(InvalidTransition));
    }

    // RFC 9110 Sections 7.8 and 9.3.6: only a switch proposed by the request
    // can win. https://www.rfc-editor.org/rfc/rfc9110.html#section-7.8
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-9.3.6
    #[test]
    fn proposed_upgrade_and_connect_can_switch_protocols() {
        for (proposals, kind) in [
            (SwitchProposals::UPGRADE, SwitchKind::Upgrade),
            (SwitchProposals::CONNECT, SwitchKind::Connect),
        ] {
            let mut state = ConnectionState::new();
            state
                .process_event(Role::Client, Event::Request(proposals))
                .unwrap();
            state.disable_keep_alive();
            state
                .process_event(Role::Server, Event::ProtocolSwitch(kind))
                .unwrap();
            assert_eq!(
                (state.client, state.server),
                (State::SendBody, State::SwitchedProtocol)
            );

            state
                .process_event(Role::Client, Event::EndOfMessage)
                .unwrap();
            assert_eq!(
                (state.client, state.server),
                (State::SwitchedProtocol, State::SwitchedProtocol)
            );
            assert_eq!(state.proposals, SwitchProposals::NONE);
        }
    }

    #[test]
    fn switch_selection_must_match_one_of_the_request_proposals() {
        // RFC 9110 Sections 7.8 and 9.3.6 allow a CONNECT request to also
        // carry Upgrade. The server may select either proposal, but it cannot
        // invent a switch that the request did not offer.
        // https://www.rfc-editor.org/rfc/rfc9110.html#section-7.8
        // https://www.rfc-editor.org/rfc/rfc9110.html#section-9.3.6
        for selected in [SwitchKind::Connect, SwitchKind::Upgrade] {
            let mut state = ConnectionState::new();
            state
                .process_event(
                    Role::Client,
                    Event::Request(SwitchProposals::from_flags(true, true)),
                )
                .unwrap();
            state
                .process_event(Role::Server, Event::ProtocolSwitch(selected))
                .unwrap();
            assert_eq!(state.server, State::SwitchedProtocol);
        }

        for (proposed, selected) in [
            (SwitchProposals::CONNECT, SwitchKind::Upgrade),
            (SwitchProposals::UPGRADE, SwitchKind::Connect),
        ] {
            let mut state = ConnectionState::new();
            state
                .process_event(Role::Client, Event::Request(proposed))
                .unwrap();
            let before = state;
            assert_eq!(
                state.process_event(Role::Server, Event::ProtocolSwitch(selected)),
                Err(InvalidTransition)
            );
            assert_eq!(state, before);
        }
    }

    #[test]
    fn denied_switch_restores_the_close_rule() {
        let mut state = ConnectionState::new();
        state
            .process_event(Role::Client, Event::Request(SwitchProposals::UPGRADE))
            .unwrap();
        state.disable_keep_alive();
        state
            .process_event(Role::Client, Event::EndOfMessage)
            .unwrap();
        assert_eq!(state.client, State::MightSwitchProtocol);

        state.process_event(Role::Server, Event::Response).unwrap();
        assert_eq!(
            (state.client, state.server),
            (State::MustClose, State::SendBody)
        );
        assert_eq!(state.proposals, SwitchProposals::NONE);
    }

    // RFC 9112 Section 2.2 recommends 400 plus close for a malformed request.
    // The direct-response path models that class and therefore disables reuse.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2
    #[test]
    fn direct_error_response_and_protocol_errors_require_close() {
        let mut response = ConnectionState::new();
        response
            .process_event(Role::Server, Event::Response)
            .unwrap();
        assert!(!response.keep_alive);
        response
            .process_event(Role::Server, Event::EndOfMessage)
            .unwrap();
        assert_eq!(response.server, State::MustClose);

        let mut error = ConnectionState::new();
        error
            .process_event(Role::Client, Event::Request(SwitchProposals::NONE))
            .unwrap();
        error
            .process_event(Role::Client, Event::EndOfMessage)
            .unwrap();
        error.process_error(Role::Server);
        assert_eq!(
            (error.client, error.server),
            (State::MustClose, State::Error)
        );

        let mut closed = ConnectionState {
            client: State::Done,
            server: State::Done,
            keep_alive: true,
            proposals: SwitchProposals::NONE,
        };
        closed
            .process_event(Role::Server, Event::ConnectionClosed)
            .unwrap();
        assert_eq!(
            (closed.client, closed.server),
            (State::MustClose, State::Closed)
        );
    }
}

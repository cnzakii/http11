#![no_main]

mod common;

use common::{MAX_EVENTS, Outcome, Terminal, connection, finish, poll, seed_wire};
use h11r::Role;
use libfuzzer_sys::fuzz_target;

fn run(role: Role, control: u8, wire: &[u8], chunk_size: usize) -> Outcome {
    let mut connection = connection(role, control);
    let mut events = Vec::new();
    let mut body = Vec::new();
    let mut terminal = Terminal::Complete;

    for chunk in wire.chunks(chunk_size) {
        connection.receive_data(chunk).unwrap();
        terminal = poll(
            &mut connection,
            &mut events,
            &mut body,
            wire.len().saturating_mul(2).saturating_add(MAX_EVENTS),
        );
        if matches!(terminal, Terminal::Error { .. }) {
            break;
        }
    }
    if matches!(terminal, Terminal::Complete) {
        connection.receive_data(&[]).unwrap();
        terminal = poll(
            &mut connection,
            &mut events,
            &mut body,
            wire.len().saturating_mul(2).saturating_add(MAX_EVENTS),
        );
    }
    finish(&mut connection, events, body, terminal)
}

fuzz_target!(|input: &[u8]| {
    let Some((&control, wire)) = input.split_first() else {
        return;
    };
    let role = if control & 0x40 == 0 {
        Role::Server
    } else {
        Role::Client
    };
    let wire = if wire.is_empty() {
        seed_wire(role, control)
    } else {
        wire
    };
    let whole = run(role, control, wire, wire.len().max(1));
    let fragmented = run(role, control, wire, usize::from(control & 0x1f) + 1);

    assert_eq!(whole.terminal, fragmented.terminal);
    if whole.terminal == Terminal::Complete {
        assert_eq!(whole.events, fragmented.events);
        assert_eq!(whole.body, fragmented.body);
        assert_eq!(whole.states, fragmented.states);
        assert_eq!(whole.trailing, fragmented.trailing);
    }
});

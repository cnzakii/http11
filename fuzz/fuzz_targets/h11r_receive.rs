#![no_main]

mod common;

use common::{MAX_EVENTS, Terminal, connection, poll, seed_wire};
use h11r::Role;
use libfuzzer_sys::fuzz_target;

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
    let mut connection = connection(role, control);
    let chunk_size = usize::from(control & 0x1f) + 1;
    let budget = wire.len().saturating_mul(2).saturating_add(MAX_EVENTS);
    let mut events = Vec::new();
    let mut body = Vec::new();

    for chunk in wire.chunks(chunk_size) {
        connection.receive_data(chunk).unwrap();
        if matches!(
            poll(&mut connection, &mut events, &mut body, budget),
            Terminal::Error { .. }
        ) {
            return;
        }
    }
    connection.receive_data(&[]).unwrap();
    poll(&mut connection, &mut events, &mut body, budget);
});

#![no_main]

mod common;

use common::{MAX_EVENTS, assert_message, header};
use h11r::{Connection, Event, Limits, NextEvent, Request, Response, Role, Version};
use h11r::{Method, StatusCode};
use libfuzzer_sys::fuzz_target;

fn fuzz_value(input: &[u8]) -> Vec<u8> {
    input.iter().take(64).map(|byte| b'!' + byte % 94).collect()
}

fn framing(chunked: bool, body_len: usize) -> (Vec<u8>, Vec<u8>) {
    if chunked {
        (b"Transfer-Encoding".to_vec(), b"chunked".to_vec())
    } else {
        (
            b"Content-Length".to_vec(),
            body_len.to_string().into_bytes(),
        )
    }
}

fn decode(mut connection: Connection, wire: &[u8], body: &[u8]) {
    connection.receive_data(wire).unwrap();
    let mut events = Vec::new();
    for _ in 0..MAX_EVENTS {
        match connection.next_event().unwrap() {
            NextEvent::Event(event) => {
                let complete = matches!(event, Event::EndOfMessage(_));
                events.push(event);
                if complete {
                    break;
                }
            }
            NextEvent::NeedData | NextEvent::Paused => break,
        }
    }
    assert_message(&events, body);
}

fn request_round_trip(control: u8, value: &[u8], body: &[u8]) {
    let chunked = control & 1 != 0;
    let mut headers = vec![
        header(b"Host", b"example.test"),
        framing(chunked, body.len()),
    ];
    if !value.is_empty() {
        headers.push(header(b"X-Fuzz", value));
    }
    let request = Request {
        method: Method::from_bytes(if control & 2 == 0 { b"GET" } else { b"POST" }).unwrap(),
        target: if control & 4 == 0 {
            b"/".to_vec()
        } else {
            b"/items?q=1".to_vec()
        },
        headers,
        http_version: Version::Http11,
    };
    let mut sender = Connection::new(Role::Client, Limits::default());
    let mut wire = sender.send_request(&request).unwrap();
    wire.extend(sender.send_data(body).unwrap());
    wire.extend(sender.end_of_message(&[]).unwrap());
    decode(
        Connection::new(Role::Server, Limits::default()),
        &wire,
        body,
    );
}

fn response_round_trip(control: u8, value: &[u8], body: &[u8]) {
    let request = Request {
        method: Method::from_bytes(b"GET").unwrap(),
        target: b"/".to_vec(),
        headers: vec![header(b"Host", b"example.test")],
        http_version: Version::Http11,
    };
    let mut receiver = Connection::new(Role::Client, Limits::default());
    let request_wire = receiver.send_request(&request).unwrap();
    receiver.end_of_message(&[]).unwrap();

    let chunked = control & 1 != 0;
    let mut headers = vec![framing(chunked, body.len())];
    if !value.is_empty() {
        headers.push(header(b"X-Fuzz", value));
    }
    let response = Response {
        status: StatusCode::from_u16(200).unwrap(),
        reason: b"OK".to_vec(),
        headers,
        http_version: Version::Http11,
    };
    let mut sender = Connection::new(Role::Server, Limits::default());
    sender.receive_data(&request_wire).unwrap();
    assert!(matches!(
        sender.next_event().unwrap(),
        NextEvent::Event(Event::Request(_))
    ));
    assert!(matches!(
        sender.next_event().unwrap(),
        NextEvent::Event(Event::EndOfMessage(_))
    ));
    let mut wire = sender.send_response(&response).unwrap();
    wire.extend(sender.send_data(body).unwrap());
    wire.extend(sender.end_of_message(&[]).unwrap());
    decode(receiver, &wire, body);
}

fuzz_target!(|input: &[u8]| {
    let Some((&control, rest)) = input.split_first() else {
        return;
    };
    let Some((&value_len, payload)) = rest.split_first() else {
        return;
    };
    let split = payload.len().min(usize::from(value_len) % 65);
    let value = fuzz_value(&payload[..split]);
    let body = &payload[split..];
    if control & 0x80 == 0 {
        request_round_trip(control, &value, body);
    } else {
        response_round_trip(control, &value, body);
    }
});

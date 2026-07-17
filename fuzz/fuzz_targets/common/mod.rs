// Each fuzz binary imports only the helpers needed by its own oracle.
#![allow(dead_code)]

use h11r::Method;
use h11r::{
    Connection, EndOfMessage, Event, Header, Limits, NextEvent, Request, Role, State, Version,
};

pub(crate) const MAX_EVENTS: usize = 32;

pub(crate) fn seed_wire(role: Role, control: u8) -> &'static [u8] {
    if role == Role::Client {
        match control & 0x03 {
            1 => b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n",
            2 => b"HTTP/1.1 200 Connection Established\r\n\r\n",
            3 => b"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: next-protocol\r\n\r\n",
            _ => b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nbody",
        }
    } else {
        match control & 0x03 {
            1 => b"POST / HTTP/1.1\r\nHost: example.test\r\nContent-Length: 4\r\n\r\nbody",
            2 => b"POST / HTTP/1.1\r\nHost: example.test\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nbody\r\n0\r\n\r\n",
            3 => b"POST / HTTP/1.1\r\nHost: example.test\r\nTransfer-Encoding: chunked\r\nContent-Length: 4\r\n\r\nbody",
            _ => b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n",
        }
    }
}

pub(crate) fn connection(role: Role, control: u8) -> Connection {
    let limits = if control & 0x80 == 0 {
        Limits::default()
    } else {
        Limits::new(512, 8).unwrap()
    };
    let mut connection = Connection::new(role, limits);
    if role == Role::Client {
        connection.send_request(&client_request(control)).unwrap();
        if control & 0x03 == 3 {
            connection.end_of_message(&[]).unwrap();
        }
    }
    connection
}

fn client_request(control: u8) -> Request {
    let (method, headers) = match control & 0x03 {
        1 => (b"HEAD".as_slice(), vec![header(b"Host", b"example.test")]),
        2 => (
            b"CONNECT".as_slice(),
            vec![header(b"Host", b"example.test:443")],
        ),
        3 => (
            b"GET".as_slice(),
            vec![
                header(b"Host", b"example.test"),
                header(b"Connection", b"upgrade"),
                header(b"Upgrade", b"next-protocol"),
            ],
        ),
        _ => (b"GET".as_slice(), vec![header(b"Host", b"example.test")]),
    };
    Request {
        method: Method::from_bytes(method).unwrap(),
        target: if method == b"CONNECT" {
            b"example.test:443".to_vec()
        } else {
            b"/".to_vec()
        },
        headers,
        http_version: Version::Http11,
    }
}

pub(crate) fn header(name: &[u8], value: &[u8]) -> Header {
    (name.to_vec(), value.to_vec())
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Terminal {
    Complete,
    Error { status: Option<u16> },
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Outcome {
    pub(crate) terminal: Terminal,
    pub(crate) events: Vec<Event>,
    pub(crate) body: Vec<u8>,
    pub(crate) states: (State, State),
    pub(crate) trailing: Vec<u8>,
}

pub(crate) fn poll(
    connection: &mut Connection,
    events: &mut Vec<Event>,
    body: &mut Vec<u8>,
    budget: usize,
) -> Terminal {
    for _ in 0..budget {
        match connection.next_event() {
            Ok(NextEvent::Event(Event::Data(data))) => body.extend_from_slice(&data.data),
            Ok(NextEvent::Event(event)) => {
                let closed = matches!(event, Event::ConnectionClosed);
                events.push(event);
                if closed {
                    return Terminal::Complete;
                }
            }
            Ok(NextEvent::NeedData | NextEvent::Paused) => return Terminal::Complete,
            Err(error) => {
                return Terminal::Error {
                    status: error.suggested_status_code(),
                };
            }
        }
    }
    panic!("next_event made unbounded progress without new input")
}

pub(crate) fn finish(
    connection: &mut Connection,
    events: Vec<Event>,
    body: Vec<u8>,
    terminal: Terminal,
) -> Outcome {
    let trailing = connection.trailing_data().0.to_vec();
    Outcome {
        terminal,
        events,
        body,
        states: (connection.local_state(), connection.peer_state()),
        trailing,
    }
}

pub(crate) fn assert_message(events: &[Event], expected_body: &[u8]) {
    assert!(matches!(
        events.first(),
        Some(Event::Request(_) | Event::Response(_))
    ));
    assert!(matches!(
        events.last(),
        Some(Event::EndOfMessage(EndOfMessage { .. }))
    ));
    let body: Vec<u8> = events
        .iter()
        .filter_map(|event| match event {
            Event::Data(data) => Some(data.data.as_slice()),
            _ => None,
        })
        .flatten()
        .copied()
        .collect();
    assert_eq!(body, expected_body);
}

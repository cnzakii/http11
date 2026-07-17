use h11r::{
    Connection, Data, EndOfMessage, Event, Header, Limits, NextEvent, Request, Response, Role,
    State, Version,
};
use h11r::{Method, StatusCode};

fn header(name: &[u8], value: &[u8]) -> Header {
    (name.to_vec(), value.to_vec())
}

fn data(value: &[u8], chunk_start: bool, chunk_end: bool) -> Data {
    Data {
        data: value.to_vec(),
        chunk_start,
        chunk_end,
    }
}

fn event(connection: &mut Connection) -> Event {
    match connection.next_event().expect("valid peer input") {
        NextEvent::Event(event) => event,
        other => panic!("expected event, got {other:?}"),
    }
}

#[test]
fn public_states_follow_a_complete_reusable_exchange() {
    // RFC 9112 Sections 2.1 and 9.3:
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-2.1
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-9.3
    let mut server = Connection::new(Role::Server, Limits::default());
    assert_eq!(
        (server.local_state(), server.peer_state()),
        (State::Idle, State::Idle)
    );

    server
        .receive_data(b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));
    assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
    assert_eq!(
        (server.local_state(), server.peer_state()),
        (State::SendResponse, State::Done)
    );

    let bytes = server
        .send_response(&Response {
            status: StatusCode::from_u16(204).unwrap(),
            reason: b"No Content".to_vec(),
            headers: vec![],
            http_version: Version::Http11,
        })
        .unwrap();
    assert_eq!(bytes, b"HTTP/1.1 204 No Content\r\n\r\n");
    assert!(server.end_of_message(&[]).unwrap().is_empty());
    assert_eq!(
        (server.local_state(), server.peer_state()),
        (State::Done, State::Done)
    );

    server.start_next_cycle().unwrap();
    assert_eq!(
        (server.local_state(), server.peer_state()),
        (State::Idle, State::Idle)
    );
}

#[test]
fn segmented_request_preserves_headers_and_content_length_body() {
    // RFC 9112 Sections 2.2 and 6.3:
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-6.3
    let mut server = Connection::new(Role::Server, Limits::default());
    for part in [
        b"POST /items HTTP/1.1\r\nHost: ex".as_slice(),
        b"ample.test\r\nX-Tag: one\r\nX-Tag: two\r\nContent-Length: 5\r\n\r\nhe",
    ] {
        server.receive_data(part).unwrap();
    }

    let Event::Request(request) = event(&mut server) else {
        panic!("expected request")
    };
    assert_eq!(request.method.as_bytes(), b"POST");
    assert_eq!(request.target, b"/items");
    assert_eq!(request.http_version, Version::Http11);
    assert_eq!(
        request.headers,
        vec![
            header(b"Host", b"example.test"),
            header(b"X-Tag", b"one"),
            header(b"X-Tag", b"two"),
            header(b"Content-Length", b"5"),
        ]
    );
    assert_eq!(event(&mut server), Event::Data(data(b"he", false, false)));
    assert_eq!(server.next_event().unwrap(), NextEvent::NeedData);

    server.receive_data(b"llo").unwrap();
    assert_eq!(event(&mut server), Event::Data(data(b"llo", false, false)));
    assert_eq!(
        event(&mut server),
        Event::EndOfMessage(EndOfMessage::default())
    );
}

#[test]
fn incomplete_invalid_heads_have_one_error_across_transport_splits() {
    // Transport segmentation cannot change a parser diagnosis. In particular,
    // EOF must give httparse one final chance to reject an invalid prefix.
    let cases = [b"\n~\x8a".as_slice(), b"\r\nHTTP/X".as_slice()];
    for wire in cases {
        let run = |chunk_size: usize| {
            let mut client = Connection::new(Role::Client, Limits::default());
            client
                .send_request(&Request {
                    method: Method::from_bytes(b"GET").unwrap(),
                    target: b"/".to_vec(),
                    headers: vec![header(b"Host", b"example.test")],
                    http_version: Version::Http11,
                })
                .unwrap();
            client.end_of_message(&[]).unwrap();
            for chunk in wire.chunks(chunk_size) {
                client.receive_data(chunk).unwrap();
                if let Err(error) = client.next_event() {
                    return error.to_string();
                }
            }
            client.receive_data(b"").unwrap();
            client.next_event().unwrap_err().to_string()
        };
        assert_eq!(run(wire.len()), run(1), "wire={wire:?}");
    }
}

#[test]
fn chunked_body_reports_chunk_boundaries_and_trailers() {
    // RFC 9112 Sections 7.1 and 7.1.2:
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-7.1
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-7.1.2
    let mut server = Connection::new(Role::Server, Limits::default());
    server
        .receive_data(
            b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\nDigest: ok\r\n\r\n",
        )
        .unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));
    assert_eq!(event(&mut server), Event::Data(data(b"abc", true, true)));
    assert_eq!(
        event(&mut server),
        Event::EndOfMessage(EndOfMessage {
            trailers: vec![header(b"Digest", b"ok")]
        })
    );
}

#[test]
fn head_limit_also_bounds_incomplete_chunk_size_lines() {
    // Chunk extensions are unbounded protocol syntax, so the field-section
    // byte budget also caps an incomplete chunk-size control line.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-7.1.1
    let limits = Limits::new(64, 8).unwrap();
    let mut chunked = Connection::new(Role::Server, limits);
    chunked
        .receive_data(b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut chunked), Event::Request(_)));
    assert_eq!(chunked.next_event().unwrap(), NextEvent::NeedData);

    let mut body = b"1;".to_vec();
    body.extend(std::iter::repeat_n(b'a', 62));
    body.extend_from_slice(b"\r\nx\r\n0\r\n\r\n");
    let mut events = Vec::new();
    for byte in body {
        chunked.receive_data(&[byte]).unwrap();
        loop {
            match chunked.next_event().unwrap() {
                NextEvent::Event(event) => events.push(event),
                NextEvent::NeedData => break,
                NextEvent::Paused => panic!("chunked request cannot pause"),
            }
        }
    }
    assert!(events.iter().any(|event| matches!(event, Event::Data(_))));
    assert!(matches!(events.last(), Some(Event::EndOfMessage(_))));

    let mut oversized_chunk = Connection::new(Role::Server, limits);
    oversized_chunk
        .receive_data(b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut oversized_chunk), Event::Request(_)));
    oversized_chunk.receive_data(&[b'1'; 65]).unwrap();
    assert_eq!(
        oversized_chunk
            .next_event()
            .unwrap_err()
            .suggested_status_code(),
        Some(400)
    );
}

#[test]
fn independent_inbound_limits_are_segmentation_invariant() {
    // RFC 9112 leaves concrete resource limits to implementations. A complete
    // section and every transport segmentation of it must have one decision.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2
    let cases = [
        (
            Limits::new(24, 10).unwrap(),
            b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n".as_slice(),
            431,
        ),
        (
            Limits::new(128, 1).unwrap(),
            b"GET / HTTP/1.1\r\nHost: x\r\nX: y\r\n\r\n",
            431,
        ),
    ];

    for (limits, wire, status) in cases {
        for split in 0..=wire.len() {
            let mut connection = Connection::new(Role::Server, limits);
            if split > 0 {
                connection.receive_data(&wire[..split]).unwrap();
            }
            if split < wire.len() {
                connection.receive_data(&wire[split..]).unwrap();
            }
            let error = connection.next_event().unwrap_err();
            assert_eq!(
                error.suggested_status_code(),
                Some(status),
                "status={status}, split={split}"
            );
            assert_eq!(connection.peer_state(), State::Error);
        }
    }

    let mut client = Connection::new(Role::Client, Limits::new(8, 10).unwrap());
    client
        .send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: b"/".to_vec(),
            headers: vec![header(b"Host", b"x")],
            http_version: Version::Http11,
        })
        .unwrap();
    client.end_of_message(&[]).unwrap();
    client.receive_data(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
    assert_eq!(
        client.next_event().unwrap_err().suggested_status_code(),
        None
    );
}

#[test]
fn send_framing_and_end_of_message_are_explicit() {
    // RFC 9112 Sections 6.3 and 7.1:
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-6.3
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-7.1
    let mut client = Connection::new(Role::Client, Limits::default());
    let head = client
        .send_request(&Request {
            method: Method::from_bytes(b"POST").unwrap(),
            target: b"/upload".to_vec(),
            headers: vec![
                header(b"Host", b"example.test"),
                header(b"Transfer-Encoding", b"chunked"),
            ],
            http_version: Version::Http11,
        })
        .unwrap();
    assert_eq!(
        head,
        b"POST /upload HTTP/1.1\r\nHost: example.test\r\nTransfer-Encoding: chunked\r\n\r\n"
    );
    assert_eq!(client.send_data(b"abc").unwrap(), b"3\r\nabc\r\n");
    assert_eq!(
        client.end_of_message(&[header(b"Digest", b"ok")]).unwrap(),
        b"0\r\nDigest: ok\r\n\r\n"
    );
}

#[test]
fn data_parts_preserve_the_callers_body_object() {
    // This is an API/performance contract, not an RFC rule: framing bytes are
    // owned by the connection while the body object passes through unchanged.
    let mut client = Connection::new(Role::Client, Limits::default());
    client
        .send_request(&Request {
            method: Method::from_bytes(b"POST").unwrap(),
            target: b"/".to_vec(),
            headers: vec![
                header(b"Host", b"x"),
                header(b"Transfer-Encoding", b"chunked"),
            ],
            http_version: Version::Http11,
        })
        .unwrap();
    let body = b"payload".to_vec();
    let parts = client.send_data_parts(body).unwrap();
    assert_eq!(parts.prefix, b"7\r\n");
    assert_eq!(parts.data, b"payload");
    assert_eq!(parts.suffix, b"\r\n");
}

#[test]
fn only_status_100_clears_continue_waiting() {
    // RFC 9110 Section 10.1.1 ties the expectation to a 100 response, a final
    // response, or the client proceeding without waiting. A different 1xx,
    // such as 103, does not satisfy it.
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-10.1.1
    let mut client = Connection::new(Role::Client, Limits::default());
    client
        .send_request(&Request {
            method: Method::from_bytes(b"POST").unwrap(),
            target: b"/".to_vec(),
            headers: vec![
                header(b"Host", b"x"),
                header(b"Content-Length", b"1"),
                header(b"Expect", b"100-continue"),
            ],
            http_version: Version::Http11,
        })
        .unwrap();
    assert!(client.client_is_waiting_for_100_continue());

    client
        .receive_data(b"HTTP/1.1 103 Early Hints\r\n\r\n")
        .unwrap();
    assert!(matches!(
        event(&mut client),
        Event::InformationalResponse(_)
    ));
    assert!(client.client_is_waiting_for_100_continue());

    client
        .receive_data(b"HTTP/1.1 100 Continue\r\n\r\n")
        .unwrap();
    assert!(matches!(
        event(&mut client),
        Event::InformationalResponse(_)
    ));
    assert!(!client.client_is_waiting_for_100_continue());
}

#[test]
fn continue_waiting_is_gated_by_the_request_http_version() {
    // RFC 9110 Section 10.1.1 defines 100-continue for an HTTP/1.1 request.
    // The same rule applies whether the request is local or received.
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-10.1.1
    for (version, wire_version, expected) in [
        (Version::Http10, b"1.0".as_slice(), false),
        (Version::Http11, b"1.1".as_slice(), true),
    ] {
        let mut client = Connection::new(Role::Client, Limits::default());
        client
            .send_request(&Request {
                method: Method::from_bytes(b"POST").unwrap(),
                target: b"/".to_vec(),
                headers: vec![
                    header(b"Host", b"x"),
                    header(b"Content-Length", b"1"),
                    header(b"Expect", b"100-continue"),
                ],
                http_version: version,
            })
            .unwrap();
        assert_eq!(
            client.client_is_waiting_for_100_continue(),
            expected,
            "local HTTP/{:?}",
            wire_version
        );

        let mut wire = b"POST / HTTP/".to_vec();
        wire.extend_from_slice(wire_version);
        wire.extend_from_slice(b"\r\nHost: x\r\nContent-Length: 1\r\nExpect: 100-continue\r\n\r\n");
        let mut server = Connection::new(Role::Server, Limits::default());
        server.receive_data(&wire).unwrap();
        assert!(matches!(event(&mut server), Event::Request(_)));
        assert_eq!(
            server.client_is_waiting_for_100_continue(),
            expected,
            "peer HTTP/{:?}",
            wire_version
        );
    }
}

#[test]
fn rfc_body_length_precedence_and_incomplete_eof() {
    // RFC 9112 Section 6.3 defines this ordered algorithm:
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-6.3
    let mut client = Connection::new(Role::Client, Limits::default());
    client
        .send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: b"/".to_vec(),
            headers: vec![header(b"Host", b"x")],
            http_version: Version::Http11,
        })
        .unwrap();
    client.end_of_message(&[]).unwrap();
    client
        .receive_data(b"HTTP/1.1 200 OK\r\nContent-Length: nope\r\n\r\n")
        .unwrap();
    assert!(client.next_event().is_err());

    let mut close_delimited = Connection::new(Role::Client, Limits::default());
    close_delimited
        .send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: b"/".to_vec(),
            headers: vec![header(b"Host", b"x")],
            http_version: Version::Http11,
        })
        .unwrap();
    close_delimited.end_of_message(&[]).unwrap();
    close_delimited
        .receive_data(b"HTTP/1.1 200 OK\r\n\r\nbody")
        .unwrap();
    assert!(matches!(event(&mut close_delimited), Event::Response(_)));
    assert_eq!(
        event(&mut close_delimited),
        Event::Data(data(b"body", false, false))
    );
    close_delimited.receive_data(b"").unwrap();
    assert!(matches!(
        event(&mut close_delimited),
        Event::EndOfMessage(_)
    ));
    assert!(close_delimited.start_next_cycle().is_err());

    let mut incomplete = Connection::new(Role::Server, Limits::default());
    incomplete
        .receive_data(b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\n\r\na")
        .unwrap();
    assert!(matches!(event(&mut incomplete), Event::Request(_)));
    assert!(matches!(event(&mut incomplete), Event::Data(_)));
    incomplete.receive_data(b"").unwrap();
    assert!(incomplete.next_event().is_err());
}

#[test]
fn content_length_and_no_body_response_framing_follow_the_ordered_algorithm() {
    // RFC 9112 Section 6.3 makes method/status precedence higher than framing
    // fields and accepts repeated decimal Content-Length values only when all
    // values agree.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-6.3
    for fields in [
        b"Content-Length: 2\r\nContent-Length: 2\r\n".as_slice(),
        b"Content-Length: 2, 2\r\n",
    ] {
        let mut wire = b"POST / HTTP/1.1\r\nHost: x\r\n".to_vec();
        wire.extend_from_slice(fields);
        wire.extend_from_slice(b"\r\nok");
        let mut server = Connection::new(Role::Server, Limits::default());
        server.receive_data(&wire).unwrap();
        assert!(matches!(event(&mut server), Event::Request(_)));
        assert_eq!(event(&mut server), Event::Data(data(b"ok", false, false)));
        assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
    }

    for fields in [
        b"Content-Length: 2\r\nContent-Length: 3\r\n".as_slice(),
        b"Content-Length: 2, 3\r\n",
        b"Content-Length: 999999999999999999999999999999\r\n",
    ] {
        let mut wire = b"POST / HTTP/1.1\r\nHost: x\r\n".to_vec();
        wire.extend_from_slice(fields);
        wire.extend_from_slice(b"\r\n");
        let mut server = Connection::new(Role::Server, Limits::default());
        server.receive_data(&wire).unwrap();
        assert_eq!(
            server.next_event().unwrap_err().suggested_status_code(),
            Some(400)
        );
    }

    for (method, status, fields) in [
        (b"HEAD".as_slice(), 200, b"Content-Length: 8\r\n".as_slice()),
        (b"HEAD", 200, b"Transfer-Encoding: chunked\r\n"),
        (b"GET", 304, b"Content-Length: 8\r\n"),
        (b"GET", 304, b"Transfer-Encoding: chunked\r\n"),
    ] {
        let mut client = Connection::new(Role::Client, Limits::default());
        client
            .send_request(&Request {
                method: Method::from_bytes(method).unwrap(),
                target: b"/".to_vec(),
                headers: vec![header(b"Host", b"x")],
                http_version: Version::Http11,
            })
            .unwrap();
        client.end_of_message(&[]).unwrap();
        let mut wire = format!("HTTP/1.1 {status} Test\r\n").into_bytes();
        wire.extend_from_slice(fields);
        wire.extend_from_slice(b"\r\nnext");
        client.receive_data(&wire).unwrap();
        assert!(matches!(event(&mut client), Event::Response(_)));
        assert!(matches!(event(&mut client), Event::EndOfMessage(_)));
        assert_eq!(client.trailing_data().0, b"next");
    }
}

#[test]
fn unsupported_request_transfer_coding_suggests_not_implemented() {
    // RFC 9112 Section 6.1 recommends 501 when a server does not understand a
    // request transfer coding. A non-final chunked coding is malformed framing
    // instead and remains a 400.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-6.1
    for fields in [
        b"Transfer-Encoding: gzip\r\n".as_slice(),
        b"Transfer-Encoding: gzip, chunked\r\n",
    ] {
        let mut wire = b"POST / HTTP/1.1\r\nHost: x\r\n".to_vec();
        wire.extend_from_slice(fields);
        wire.extend_from_slice(b"\r\n");
        let mut server = Connection::new(Role::Server, Limits::default());
        server.receive_data(&wire).unwrap();
        assert_eq!(
            server.next_event().unwrap_err().suggested_status_code(),
            Some(501)
        );
    }

    let mut server = Connection::new(Role::Server, Limits::default());
    server
        .receive_data(b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked, gzip\r\n\r\n")
        .unwrap();
    assert_eq!(
        server.next_event().unwrap_err().suggested_status_code(),
        Some(400)
    );
}

#[test]
fn expect_continue_is_sent_only_when_request_content_will_follow() {
    // RFC 9110 Section 10.1.1 forbids a client from generating 100-continue
    // for a request without content.
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-10.1.1
    for headers in [
        vec![header(b"Host", b"x"), header(b"Expect", b"100-continue")],
        vec![
            header(b"Host", b"x"),
            header(b"Content-Length", b"0"),
            header(b"Expect", b"100-continue"),
        ],
    ] {
        let mut client = Connection::new(Role::Client, Limits::default());
        assert!(
            client
                .send_request(&Request {
                    method: Method::from_bytes(b"POST").unwrap(),
                    target: b"/".to_vec(),
                    headers,
                    http_version: Version::Http11,
                })
                .is_err()
        );
        assert_eq!(client.local_state(), State::Idle);
    }
}

#[test]
fn pipeline_pauses_until_both_sides_start_the_next_cycle() {
    // RFC 9112 Sections 9.2 and 9.3.2 require response ordering. The explicit
    // Paused boundary prevents a second request from overwriting the active
    // response cycle.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-9.3.2
    let mut server = Connection::new(Role::Server, Limits::default());
    server
        .receive_data(b"GET /1 HTTP/1.1\r\nHost: x\r\n\r\nGET /2 HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));
    assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
    assert_eq!(server.next_event().unwrap(), NextEvent::Paused);
    server
        .send_response(&Response {
            status: StatusCode::from_u16(204).unwrap(),
            reason: vec![],
            headers: vec![],
            http_version: Version::Http11,
        })
        .unwrap();
    server.end_of_message(&[]).unwrap();
    server.start_next_cycle().unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));
}

#[test]
fn proposed_protocol_switch_pauses_peer_parsing_until_the_server_decides() {
    // RFC 9110 Sections 7.8 and 9.3.6: bytes after a completed switch proposal
    // are ambiguous until the server accepts or rejects the proposal.
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-7.8
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-9.3.6
    for wire in [
        b"CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\ntunnel"
            .as_slice(),
        b"GET /chat HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: next-protocol\r\n\r\nprotocol"
            .as_slice(),
    ] {
        let mut server = Connection::new(Role::Server, Limits::default());
        server.receive_data(wire).unwrap();
        assert!(matches!(event(&mut server), Event::Request(_)));
        assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
        assert_eq!(server.peer_state(), State::MightSwitchProtocol);
        assert_eq!(server.next_event().unwrap(), NextEvent::Paused);
    }
}

#[test]
fn accepted_upgrade_exposes_trailing_protocol_bytes() {
    // RFC 9110 Section 7.8: a 101 response completes the HTTP upgrade and all
    // following bytes belong to the switched protocol.
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-7.8
    let mut client = Connection::new(Role::Client, Limits::default());
    client
        .send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: b"/chat".to_vec(),
            headers: vec![
                header(b"Host", b"x"),
                header(b"Connection", b"Upgrade"),
                header(b"Upgrade", b"next-protocol"),
            ],
            http_version: Version::Http11,
        })
        .unwrap();
    client.end_of_message(&[]).unwrap();
    client
        .receive_data(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: next-protocol\r\n\r\nprotocol-data")
        .unwrap();
    assert!(matches!(
        event(&mut client),
        Event::InformationalResponse(_)
    ));
    assert_eq!(client.next_event().unwrap(), NextEvent::Paused);
    assert_eq!(client.trailing_data(), (b"protocol-data".as_slice(), false));
}

#[test]
fn upgrade_negotiation_enforces_required_headers_and_response_order() {
    // RFC 9110 Section 7.8 requires Connection: upgrade, an Upgrade protocol
    // selected from the request, and 100 Continue before 101 when expected.
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-7.8
    let request = Request {
        method: Method::from_bytes(b"GET").unwrap(),
        target: b"/chat".to_vec(),
        headers: vec![
            header(b"Host", b"x"),
            header(b"Connection", b"Upgrade"),
            header(b"Upgrade", b"next-protocol"),
        ],
        http_version: Version::Http11,
    };

    for headers in [
        vec![],
        vec![header(b"Connection", b"Upgrade")],
        vec![header(b"Upgrade", b"next-protocol")],
        vec![
            header(b"Connection", b"Upgrade"),
            header(b"Upgrade", b"other-protocol"),
        ],
    ] {
        let mut server = Connection::new(Role::Server, Limits::default());
        server
            .receive_data(
                b"GET /chat HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: next-protocol\r\n\r\n",
            )
            .unwrap();
        assert!(matches!(event(&mut server), Event::Request(_)));
        assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
        assert!(
            server
                .send_informational_response(&Response {
                    status: StatusCode::from_u16(101).unwrap(),
                    reason: b"Switching Protocols".to_vec(),
                    headers,
                    http_version: Version::Http11,
                })
                .is_err()
        );
        assert_eq!(server.local_state(), State::SendResponse);
    }

    for wire in [
        b"HTTP/1.1 101 Switching Protocols\r\n\r\n".as_slice(),
        b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: other-protocol\r\n\r\n",
    ] {
        let mut client = Connection::new(Role::Client, Limits::default());
        client.send_request(&request).unwrap();
        client.end_of_message(&[]).unwrap();
        client.receive_data(wire).unwrap();
        assert!(client.next_event().is_err());
        assert_eq!(client.peer_state(), State::Error);
    }

    for headers in [
        vec![header(b"Host", b"x"), header(b"Upgrade", b"next-protocol")],
        vec![header(b"Host", b"x"), header(b"Connection", b"Upgrade")],
    ] {
        let mut client = Connection::new(Role::Client, Limits::default());
        assert!(
            client
                .send_request(&Request {
                    method: Method::from_bytes(b"GET").unwrap(),
                    target: b"/".to_vec(),
                    headers,
                    http_version: Version::Http11,
                })
                .is_err()
        );
        assert_eq!(client.local_state(), State::Idle);
    }

    let mut server = Connection::new(Role::Server, Limits::default());
    server
        .receive_data(
            b"POST /chat HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: next-protocol\r\nExpect: 100-continue\r\nContent-Length: 1\r\n\r\n",
        )
        .unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));
    let switching = Response {
        status: StatusCode::from_u16(101).unwrap(),
        reason: b"Switching Protocols".to_vec(),
        headers: vec![
            header(b"Connection", b"Upgrade"),
            header(b"Upgrade", b"next-protocol"),
        ],
        http_version: Version::Http11,
    };
    assert!(server.send_informational_response(&switching).is_err());
    server
        .send_informational_response(&Response {
            status: StatusCode::from_u16(100).unwrap(),
            reason: b"Continue".to_vec(),
            headers: vec![],
            http_version: Version::Http11,
        })
        .unwrap();
    assert!(server.send_informational_response(&switching).is_ok());
}

#[test]
fn send_rejects_status_line_injection_and_forbidden_framing_fields() {
    // RFC 9112 Sections 4, 6.1, and 6.2 constrain status-line syntax and
    // prohibit framing fields on responses that cannot contain a body.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-4
    let mut server = Connection::new(Role::Server, Limits::default());
    for (status, reason, headers) in [
        (200, b"OK\r\nInjected: yes".as_slice(), vec![]),
        (
            204,
            b"No Content".as_slice(),
            vec![header(b"Transfer-Encoding", b"chunked")],
        ),
        (
            204,
            b"No Content".as_slice(),
            vec![header(b"Content-Length", b"0")],
        ),
    ] {
        assert!(
            server
                .send_response(&Response {
                    status: StatusCode::from_u16(status).unwrap(),
                    reason: reason.to_vec(),
                    headers,
                    http_version: Version::Http11,
                })
                .is_err()
        );
        assert_eq!(server.local_state(), State::Idle);
    }
}

#[test]
fn http10_messages_are_close_only_even_with_a_legacy_keep_alive_token() {
    // RFC 9112 Section 9.3 permits HTTP/1.0 persistence only through a legacy
    // option with additional recipient constraints. This engine parses 1.0
    // messages but deliberately does not implement that optional mechanism.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-9.3
    let mut server = Connection::new(Role::Server, Limits::default());
    server
        .receive_data(b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));
    assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
    assert_eq!(server.peer_state(), State::MustClose);

    let mut client = Connection::new(Role::Client, Limits::default());
    client
        .send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: b"/".to_vec(),
            headers: vec![header(b"Host", b"x")],
            http_version: Version::Http11,
        })
        .unwrap();
    client.end_of_message(&[]).unwrap();
    client
        .receive_data(b"HTTP/1.0 204 No Content\r\nConnection: keep-alive\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut client), Event::Response(_)));
    assert!(matches!(event(&mut client), Event::EndOfMessage(_)));
    assert_eq!(client.peer_state(), State::MustClose);
}

#[test]
fn http10_peer_never_gets_automatic_chunked_framing() {
    // RFC 9112 Section 6.1 forbids a server from sending Transfer-Encoding
    // unless the corresponding request indicates HTTP/1.1 or later.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-6.1
    let mut server = Connection::new(Role::Server, Limits::default());
    server.receive_data(b"GET / HTTP/1.0\r\n\r\n").unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));
    assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
    let head = server
        .send_response(&Response {
            status: StatusCode::from_u16(200).unwrap(),
            reason: b"OK".to_vec(),
            headers: vec![],
            http_version: Version::Http11,
        })
        .unwrap();
    assert_eq!(head, b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
    assert_eq!(server.send_data(b"body").unwrap(), b"body");
    assert!(server.end_of_message(&[]).unwrap().is_empty());
    assert_eq!(server.local_state(), State::MustClose);
}

#[test]
fn responses_normalize_the_connection_close_signal() {
    // RFC 9112 Sections 9.3 and 9.6 require the wire response to communicate
    // that a connection will not persist.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-9.3
    let mut server = Connection::new(Role::Server, Limits::default());
    server
        .receive_data(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));
    assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
    assert_eq!(
        server
            .send_response(&Response {
                status: StatusCode::from_u16(204).unwrap(),
                reason: b"No Content".to_vec(),
                headers: vec![header(b"Connection", b"keep-alive")],
                http_version: Version::Http11,
            })
            .unwrap(),
        b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n"
    );

    let mut early = Connection::new(Role::Server, Limits::default());
    assert_eq!(
        early
            .send_response(&Response {
                status: StatusCode::from_u16(408).unwrap(),
                reason: b"Request Timeout".to_vec(),
                headers: vec![header(b"Content-Length", b"0")],
                http_version: Version::Http11,
            })
            .unwrap(),
        b"HTTP/1.1 408 Request Timeout\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
}

#[test]
fn request_target_forms_round_trip() {
    // RFC 9112 Section 3.2 defines origin-, absolute-, authority-, and
    // asterisk-form request targets.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-3.2
    for (method, target) in [
        (b"GET".as_slice(), b"/items?q=1".as_slice()),
        (b"GET", b"https://example.test/items"),
        (b"CONNECT", b"example.test:443"),
        (b"OPTIONS", b"*"),
    ] {
        let mut client = Connection::new(Role::Client, Limits::default());
        let wire = client
            .send_request(&Request {
                method: Method::from_bytes(method).unwrap(),
                target: target.to_vec(),
                headers: vec![header(b"Host", b"example.test")],
                http_version: Version::Http11,
            })
            .unwrap_or_else(|error| panic!("request target {target:?}: {error}"));
        let mut server = Connection::new(Role::Server, Limits::default());
        server.receive_data(&wire).unwrap();
        let Event::Request(request) = event(&mut server) else {
            panic!("expected request")
        };
        assert_eq!(request.target, target);
    }
}

#[test]
fn request_target_rejects_bytes_outside_its_field_boundary() {
    for target in [
        b"".as_slice(),
        b"/bad target",
        b"/bad\taccess",
        b"/bad\r\nInjected: yes",
        b"/bad\x7f",
        b"/bad\xff",
    ] {
        let mut client = Connection::new(Role::Client, Limits::default());
        let result = client.send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: target.to_vec(),
            headers: vec![header(b"Host", b"example.test")],
            http_version: Version::Http11,
        });
        assert!(result.is_err(), "target {target:?}");
        assert_eq!(client.local_state(), State::Idle);
    }
}

#[test]
fn host_field_uses_uri_host_and_optional_port_grammar() {
    // RFC 9112 Section 3.2 requires 400 for missing, duplicate, or invalid
    // Host. RFC 9110 Section 7.2 defines Host as uri-host plus optional port.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-3.2
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-7.2
    for host in [
        b"".as_slice(),
        b"example.test",
        b"example.test:8080",
        b"127.0.0.1",
        b"[2001:db8::1]:443",
    ] {
        let mut client = Connection::new(Role::Client, Limits::default());
        client
            .send_request(&Request {
                method: Method::from_bytes(b"GET").unwrap(),
                target: b"/".to_vec(),
                headers: vec![header(b"Host", host)],
                http_version: Version::Http11,
            })
            .unwrap_or_else(|error| panic!("valid Host {host:?}: {error}"));
    }

    for host in [
        b"bad host".as_slice(),
        b"user@example.test",
        b"example.test/path",
        b"[2001:db8::1",
        b"example.test:not-a-port",
    ] {
        let mut client = Connection::new(Role::Client, Limits::default());
        assert!(
            client
                .send_request(&Request {
                    method: Method::from_bytes(b"GET").unwrap(),
                    target: b"/".to_vec(),
                    headers: vec![header(b"Host", host)],
                    http_version: Version::Http11,
                })
                .is_err(),
            "local Host {host:?}"
        );

        let mut wire = b"GET / HTTP/1.1\r\nHost: ".to_vec();
        wire.extend_from_slice(host);
        wire.extend_from_slice(b"\r\n\r\n");
        let mut server = Connection::new(Role::Server, Limits::default());
        server.receive_data(&wire).unwrap();
        assert_eq!(
            server.next_event().unwrap_err().suggested_status_code(),
            Some(400),
            "peer Host {host:?}"
        );
    }

    for wire in [
        b"GET / HTTP/1.1\r\n\r\n".as_slice(),
        b"GET / HTTP/1.0\r\nHost: one\r\nHost: two\r\n\r\n",
    ] {
        let mut server = Connection::new(Role::Server, Limits::default());
        server.receive_data(wire).unwrap();
        assert_eq!(
            server.next_event().unwrap_err().suggested_status_code(),
            Some(400)
        );
    }
}

#[test]
fn token_field_value_and_reason_phrase_have_distinct_octet_grammars() {
    // RFC 9110 Sections 5.5 and 5.6.2 define token and field-content. RFC 9112
    // Section 4 separately allows SP and HTAB in a reason phrase. A single
    // SP/HTAB is line OWS, not a one-byte field value.
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-5.5
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-4
    for byte in u8::MIN..=u8::MAX {
        let token = byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte);
        let field_value = matches!(byte, b'!'..=b'~' | 0x80..=0xff);
        let reason_phrase = matches!(byte, b'\t' | b' '..=b'~' | 0x80..=0xff);

        let mut with_name = Connection::new(Role::Client, Limits::default());
        assert_eq!(
            with_name
                .send_request(&Request {
                    method: Method::from_bytes(b"GET").unwrap(),
                    target: b"/".to_vec(),
                    headers: vec![header(b"Host", b"x"), header(&[byte], b"value")],
                    http_version: Version::Http11,
                })
                .is_ok(),
            token,
            "header name byte 0x{byte:02x}"
        );

        let mut with_value = Connection::new(Role::Client, Limits::default());
        assert_eq!(
            with_value
                .send_request(&Request {
                    method: Method::from_bytes(b"GET").unwrap(),
                    target: b"/".to_vec(),
                    headers: vec![header(b"Host", b"x"), header(b"X-Test", &[byte])],
                    http_version: Version::Http11,
                })
                .is_ok(),
            field_value,
            "header value byte 0x{byte:02x}"
        );

        let mut with_reason = Connection::new(Role::Server, Limits::default());
        assert_eq!(
            with_reason
                .send_response(&Response {
                    status: StatusCode::from_u16(204).unwrap(),
                    reason: vec![byte],
                    headers: vec![],
                    http_version: Version::Http11,
                })
                .is_ok(),
            reason_phrase,
            "reason phrase byte 0x{byte:02x}"
        );

        let mut from_peer = Connection::new(Role::Client, Limits::default());
        from_peer
            .send_request(&Request {
                method: Method::from_bytes(b"GET").unwrap(),
                target: b"/".to_vec(),
                headers: vec![header(b"Host", b"x")],
                http_version: Version::Http11,
            })
            .unwrap();
        let mut wire = b"HTTP/1.1 204 ".to_vec();
        wire.push(byte);
        wire.extend_from_slice(b"\r\n\r\n");
        from_peer.receive_data(&wire).unwrap();
        let received = from_peer.next_event();
        assert_eq!(
            received.is_ok(),
            reason_phrase || byte == b'\n',
            "received reason phrase byte 0x{byte:02x}"
        );
        if reason_phrase || byte == b'\n' {
            let NextEvent::Event(Event::Response(response)) = received.unwrap() else {
                panic!("expected response")
            };
            // On receive, LF is an accepted line delimiter rather than a
            // reason-phrase octet (RFC 9112 Section 2.2).
            if byte == b'\n' {
                assert!(response.reason.is_empty());
            } else {
                assert_eq!(response.reason, [byte]);
            }
        }
    }

    for value in [b" leading".as_slice(), b"trailing ", b"\tboth\t"] {
        let mut client = Connection::new(Role::Client, Limits::default());
        assert!(
            client
                .send_request(&Request {
                    method: Method::from_bytes(b"GET").unwrap(),
                    target: b"/".to_vec(),
                    headers: vec![header(b"Host", b"x"), header(b"X-Test", value)],
                    http_version: Version::Http11,
                })
                .is_err()
        );
    }
}

#[test]
fn chunk_extensions_are_ignored_only_after_validating_their_grammar() {
    // RFC 9112 Section 7.1.1 requires recipients to ignore unknown chunk
    // extensions, but they still have to be syntactically valid chunk-ext.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-7.1.1
    let prefix = b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n";
    for body in [
        b"1;foo\r\na\r\n0\r\n\r\n".as_slice(),
        b"1 ; foo = bar ; quoted = \"a\\\"b\"\r\na\r\n0 ; done = yes\r\n\r\n",
    ] {
        let mut wire = prefix.to_vec();
        wire.extend_from_slice(body);
        let mut server = Connection::new(Role::Server, Limits::default());
        server.receive_data(&wire).unwrap();
        assert!(matches!(event(&mut server), Event::Request(_)));
        assert!(matches!(event(&mut server), Event::Data(_)));
        assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
    }

    for line in [
        b"1;\r\n".as_slice(),
        b"1;foo=\r\n",
        b"1;foo=\"unterminated\r\n",
        b"1;foo=a b\r\n",
        b"1;\0\r\n",
    ] {
        let mut wire = prefix.to_vec();
        wire.extend_from_slice(line);
        let mut server = Connection::new(Role::Server, Limits::default());
        server.receive_data(&wire).unwrap();
        assert!(matches!(event(&mut server), Event::Request(_)));
        assert!(server.next_event().is_err(), "chunk line {line:?}");
    }
}

#[test]
fn trailer_field_lines_exclude_ows_from_the_parsed_value() {
    // RFC 9112 Sections 5.1 and 7.1.2 apply the same field-line parser to
    // headers and trailers. Leading and trailing OWS is not part of the value.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-7.1.2
    let mut server = Connection::new(Role::Server, Limits::default());
    server
        .receive_data(
            b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-Test:\t value \t\r\n\r\n",
        )
        .unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));
    assert_eq!(
        event(&mut server),
        Event::EndOfMessage(EndOfMessage {
            trailers: vec![header(b"X-Test", b"value")],
        })
    );
}

#[test]
fn sender_rejects_core_fields_that_cannot_be_trailers() {
    // RFC 9110 Section 6.5.1 forbids generating a trailer unless the field's
    // definition permits it. These core fields affect framing, routing, or the
    // connection before the trailer section can be read.
    // https://www.rfc-editor.org/rfc/rfc9110.html#section-6.5.1
    for name in [
        b"Content-Length".as_slice(),
        b"Transfer-Encoding",
        b"Host",
        b"Connection",
        b"Trailer",
        b"Upgrade",
        b"Expect",
        b"TE",
    ] {
        let mut client = Connection::new(Role::Client, Limits::default());
        client
            .send_request(&Request {
                method: Method::from_bytes(b"POST").unwrap(),
                target: b"/".to_vec(),
                headers: vec![
                    header(b"Host", b"x"),
                    header(b"Transfer-Encoding", b"chunked"),
                ],
                http_version: Version::Http11,
            })
            .unwrap();
        assert!(
            client.end_of_message(&[header(name, b"value")]).is_err(),
            "trailer {name:?}"
        );
        assert_eq!(client.local_state(), State::SendBody);
    }
}

#[test]
fn response_obs_fold_is_normalized_but_request_obs_fold_is_rejected() {
    // RFC 9112 Section 5.2 requires a user agent receiving obs-fold in a
    // response to replace it with SP. A server may reject obs-fold in a request;
    // this engine chooses that strict alternative.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-5.2
    let mut client = Connection::new(Role::Client, Limits::new(65536, 1).unwrap());
    client
        .send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: b"/".to_vec(),
            headers: vec![header(b"Host", b"x")],
            http_version: Version::Http11,
        })
        .unwrap();
    client.end_of_message(&[]).unwrap();
    client
        .receive_data(b"HTTP/1.1 204 No Content\r\nX-Test: one\r\n two\t\r\n\r\n")
        .unwrap();
    let Event::Response(response) = event(&mut client) else {
        panic!("expected response")
    };
    assert_eq!(response.headers, vec![header(b"X-Test", b"one two")]);

    let mut first_line = Connection::new(Role::Client, Limits::default());
    first_line
        .send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: b"/".to_vec(),
            headers: vec![header(b"Host", b"x")],
            http_version: Version::Http11,
        })
        .unwrap();
    first_line.end_of_message(&[]).unwrap();
    first_line
        .receive_data(b"HTTP/1.1 204 No Content\r\n X-Test: hidden\r\n\r\n")
        .unwrap();
    assert!(first_line.next_event().is_err());

    let mut trailers = Connection::new(Role::Client, Limits::new(65536, 1).unwrap());
    trailers
        .send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: b"/".to_vec(),
            headers: vec![header(b"Host", b"x")],
            http_version: Version::Http11,
        })
        .unwrap();
    trailers.end_of_message(&[]).unwrap();
    trailers
        .receive_data(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-Test: one\r\n two\r\n\r\n",
        )
        .unwrap();
    assert!(matches!(event(&mut trailers), Event::Response(_)));
    assert_eq!(
        event(&mut trailers),
        Event::EndOfMessage(EndOfMessage {
            trailers: vec![header(b"X-Test", b"one two")],
        })
    );

    let mut server = Connection::new(Role::Server, Limits::default());
    server
        .receive_data(b"GET / HTTP/1.1\r\nHost: x\r\nX-Test: one\r\n two\r\n\r\n")
        .unwrap();
    assert_eq!(
        server.next_event().unwrap_err().suggested_status_code(),
        Some(400)
    );
}

#[test]
fn parser_accepted_leading_empty_lines_are_segmentation_invariant() {
    // RFC 9112 Section 2.2 recommends ignoring at least one leading empty line
    // before a request-line. httparse accepts any number, so Connection must
    // preserve that decision across transport segmentation.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2
    fn outcome<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> Result<(), Option<u16>> {
        let mut server = Connection::new(Role::Server, Limits::default());
        for part in parts {
            server.receive_data(part).unwrap();
            match server.next_event() {
                Ok(NextEvent::NeedData) => {}
                Ok(NextEvent::Event(Event::Request(_))) => return Ok(()),
                Ok(other) => panic!("unexpected result: {other:?}"),
                Err(error) => return Err(error.suggested_status_code()),
            }
        }
        panic!("request outcome remained incomplete")
    }

    for count in [0, 1, 2, 16] {
        let mut wire = b"\r\n".repeat(count);
        wire.extend_from_slice(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(outcome([wire.as_slice()]), Ok(()), "count={count}");
        assert_eq!(outcome(wire.chunks(1)), Ok(()), "count={count}");
        for split in 1..wire.len() {
            assert_eq!(
                outcome([&wire[..split], &wire[split..]]),
                Ok(()),
                "count={count}, split={split}"
            );
        }
    }
}

#[test]
fn parser_accepted_lf_is_preserved_for_heads_and_trailers() {
    // RFC 9112 Section 2.2 permits recipients to recognize LF as a line ending.
    // This engine follows httparse for heads and trailer field sections.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2
    let mut server = Connection::new(Role::Server, Limits::default());
    server.receive_data(b"GET / HTTP/1.1\nHost: x\n\n").unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));

    let mut client = Connection::new(Role::Client, Limits::default());
    client
        .send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: b"/".to_vec(),
            headers: vec![header(b"Host", b"x")],
            http_version: Version::Http11,
        })
        .unwrap();
    client.end_of_message(&[]).unwrap();
    client
        .receive_data(b"HTTP/1.1 204 All Good\nX-Test: one\n two\t\n\n")
        .unwrap();
    let Event::Response(response) = event(&mut client) else {
        panic!("expected response")
    };
    assert_eq!(response.reason, b"All Good");
    assert_eq!(response.headers, vec![header(b"X-Test", b"one two")]);

    let mut trailers = Connection::new(Role::Server, Limits::default());
    trailers
        .receive_data(
            b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-Test: y\n\n",
        )
        .unwrap();
    assert!(matches!(event(&mut trailers), Event::Request(_)));
    assert_eq!(
        event(&mut trailers),
        Event::EndOfMessage(EndOfMessage {
            trailers: vec![header(b"X-Test", b"y")],
        })
    );
}

#[test]
fn connection_close_disables_reuse_when_sent_by_either_actor() {
    // RFC 9112 Section 9.3 makes persistence a property of the exchanged
    // messages, not merely of bytes received by one endpoint. Exercise the
    // same Connection option through both public send paths.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-9.3
    let mut client = Connection::new(Role::Client, Limits::default());
    client
        .send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: b"/".to_vec(),
            headers: vec![header(b"Host", b"x"), header(b"Connection", b"close")],
            http_version: Version::Http11,
        })
        .unwrap();
    client.end_of_message(&[]).unwrap();
    client
        .receive_data(b"HTTP/1.1 204 No Content\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut client), Event::Response(_)));
    assert!(matches!(event(&mut client), Event::EndOfMessage(_)));
    assert_eq!(client.local_state(), State::MustClose);
    assert!(client.start_next_cycle().is_err());

    let mut server = Connection::new(Role::Server, Limits::default());
    server
        .receive_data(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));
    assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
    server
        .send_response(&Response {
            status: StatusCode::from_u16(204).unwrap(),
            reason: b"No Content".to_vec(),
            headers: vec![header(b"Connection", b"close")],
            http_version: Version::Http11,
        })
        .unwrap();
    server.end_of_message(&[]).unwrap();
    assert_eq!(server.local_state(), State::MustClose);
    assert!(server.start_next_cycle().is_err());
}

#[test]
fn transfer_encoding_is_gated_by_the_peer_http_version() {
    // RFC 9112 Section 6.1 forbids treating Transfer-Encoding as valid HTTP/1.0
    // framing and forbids sending it to a peer known only to speak HTTP/1.0.
    // Cover both directions and both local/remote paths as one version gate.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-6.1
    let mut inbound_request = Connection::new(Role::Server, Limits::default());
    inbound_request
        .receive_data(b"POST / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n")
        .unwrap();
    assert!(inbound_request.next_event().is_err());

    let mut inbound_response = Connection::new(Role::Client, Limits::default());
    inbound_response
        .send_request(&Request {
            method: Method::from_bytes(b"GET").unwrap(),
            target: b"/".to_vec(),
            headers: vec![header(b"Host", b"x")],
            http_version: Version::Http11,
        })
        .unwrap();
    inbound_response.end_of_message(&[]).unwrap();
    inbound_response
        .receive_data(b"HTTP/1.0 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n")
        .unwrap();
    assert!(inbound_response.next_event().is_err());

    let mut outbound_request = Connection::new(Role::Client, Limits::default());
    assert!(
        outbound_request
            .send_request(&Request {
                method: Method::from_bytes(b"POST").unwrap(),
                target: b"/".to_vec(),
                headers: vec![header(b"Transfer-Encoding", b"chunked")],
                http_version: Version::Http10,
            })
            .is_err()
    );
    assert_eq!(outbound_request.local_state(), State::Idle);

    let mut outbound_response = Connection::new(Role::Server, Limits::default());
    outbound_response
        .receive_data(b"GET / HTTP/1.0\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut outbound_response), Event::Request(_)));
    assert!(matches!(
        event(&mut outbound_response),
        Event::EndOfMessage(_)
    ));
    let head = outbound_response
        .send_response(&Response {
            status: StatusCode::from_u16(200).unwrap(),
            reason: b"OK".to_vec(),
            headers: vec![header(b"Transfer-Encoding", b"chunked")],
            http_version: Version::Http11,
        })
        .unwrap();
    assert_eq!(head, b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
    assert_eq!(outbound_response.send_data(b"body").unwrap(), b"body");
    assert!(outbound_response.end_of_message(&[]).unwrap().is_empty());
    assert_eq!(outbound_response.local_state(), State::MustClose);

    for headers in [vec![], vec![header(b"Transfer-Encoding", b"chunked")]] {
        let mut http10_response = Connection::new(Role::Server, Limits::default());
        http10_response
            .receive_data(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        assert!(matches!(event(&mut http10_response), Event::Request(_)));
        assert!(matches!(
            event(&mut http10_response),
            Event::EndOfMessage(_)
        ));
        let head = http10_response
            .send_response(&Response {
                status: StatusCode::from_u16(200).unwrap(),
                reason: b"OK".to_vec(),
                headers,
                http_version: Version::Http10,
            })
            .unwrap();
        assert_eq!(head, b"HTTP/1.0 200 OK\r\nConnection: close\r\n\r\n");
        assert_eq!(http10_response.send_data(b"body").unwrap(), b"body");
        assert!(http10_response.end_of_message(&[]).unwrap().is_empty());
        assert_eq!(http10_response.local_state(), State::MustClose);
    }
}

#[test]
fn eof_preserves_complete_pipelined_messages_already_in_the_buffer() {
    // RFC 9112 Sections 9.3.2 and 9.6: closure forbids future transport data,
    // but does not erase complete pipelined requests received before EOF.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-9.3.2
    let mut server = Connection::new(Role::Server, Limits::default());
    server
        .receive_data(b"GET /1 HTTP/1.1\r\nHost: x\r\n\r\nGET /2 HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    server.receive_data(b"").unwrap();

    assert!(matches!(event(&mut server), Event::Request(_)));
    assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
    server
        .send_response(&Response {
            status: StatusCode::from_u16(204).unwrap(),
            reason: vec![],
            headers: vec![],
            http_version: Version::Http11,
        })
        .unwrap();
    server.end_of_message(&[]).unwrap();
    server.start_next_cycle().unwrap();

    let Event::Request(request) = event(&mut server) else {
        panic!("expected buffered request")
    };
    assert_eq!(request.target, b"/2");
    assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
    assert!(matches!(event(&mut server), Event::ConnectionClosed));
}

#[test]
fn eof_is_idempotent_but_cannot_be_followed_by_bytes() {
    // RFC 9112 Section 9.6 makes transport closure terminal. Repeating the
    // same EOF notification changes nothing; bytes after it are impossible.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-9.6
    let mut server = Connection::new(Role::Server, Limits::default());
    server.receive_data(b"").unwrap();
    server.receive_data(b"").unwrap();
    assert!(matches!(event(&mut server), Event::ConnectionClosed));
    assert!(matches!(event(&mut server), Event::ConnectionClosed));
    assert!(server.receive_data(b"GET /").is_err());
}

#[test]
fn chunked_message_is_invariant_under_transport_segmentation() {
    // RFC 9112 Sections 2.1 and 7.1 define HTTP message and chunk boundaries;
    // transport read boundaries have no protocol meaning.
    // https://www.rfc-editor.org/rfc/rfc9112.html#section-7.1
    let wire = b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n3;tag=yes\r\nabc\r\n0\r\nDigest: ok\r\n\r\n";
    for size in 1..=wire.len() {
        let mut server = Connection::new(Role::Server, Limits::default());
        let mut events = Vec::new();
        for part in wire.chunks(size) {
            server.receive_data(part).unwrap();
            loop {
                match server.next_event().unwrap() {
                    NextEvent::Event(event) => events.push(event),
                    NextEvent::NeedData => break,
                    NextEvent::Paused => panic!("server cannot pause before its response"),
                }
            }
        }
        assert!(matches!(events.first(), Some(Event::Request(_))));
        assert_eq!(
            events.last(),
            Some(&Event::EndOfMessage(EndOfMessage {
                trailers: vec![header(b"Digest", b"ok")],
            }))
        );
        let chunks = &events[1..events.len() - 1];
        let body: Vec<_> = chunks
            .iter()
            .flat_map(|event| match event {
                Event::Data(data) => data.data.iter().copied(),
                _ => panic!("body contained a non-data event"),
            })
            .collect();
        assert_eq!(body, b"abc");
        assert!(matches!(chunks.first(), Some(Event::Data(data)) if data.chunk_start));
        assert!(matches!(chunks.last(), Some(Event::Data(data)) if data.chunk_end));
    }
}

#[test]
fn rejected_connect_cannot_reuse_the_h11r_connection() {
    // RFC 9931 Section 8 updates HTTP/1.1: a proxy server rejecting CONNECT
    // must close the connection unless it has external knowledge that the
    // client did not send tunnel bytes optimistically. This engine has no such
    // external policy input, so rejection always disables reuse.
    // https://www.rfc-editor.org/rfc/rfc9931.html#section-8
    let request = Request {
        method: Method::from_bytes(b"CONNECT").unwrap(),
        target: b"example.test:443".to_vec(),
        headers: vec![header(b"Host", b"example.test:443")],
        http_version: Version::Http11,
    };
    let response = Response {
        status: StatusCode::from_u16(403).unwrap(),
        reason: b"Forbidden".to_vec(),
        headers: vec![header(b"Content-Length", b"0")],
        http_version: Version::Http11,
    };

    let mut server = Connection::new(Role::Server, Limits::default());
    server
        .receive_data(b"CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut server), Event::Request(_)));
    assert!(matches!(event(&mut server), Event::EndOfMessage(_)));
    server.send_response(&response).unwrap();
    server.end_of_message(&[]).unwrap();
    assert_eq!(server.local_state(), State::MustClose);
    assert!(server.start_next_cycle().is_err());

    let mut client = Connection::new(Role::Client, Limits::default());
    client.send_request(&request).unwrap();
    client.end_of_message(&[]).unwrap();
    client
        .receive_data(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
        .unwrap();
    assert!(matches!(event(&mut client), Event::Response(_)));
    assert!(matches!(event(&mut client), Event::EndOfMessage(_)));
    assert_eq!(client.peer_state(), State::MustClose);
    assert!(client.start_next_cycle().is_err());
}

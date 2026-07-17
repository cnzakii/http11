//! Measures representative HTTP/1 server paths through the public Rust API.
//!
//! Common workloads and diagnostic stress cases are named separately. The
//! httparse target is a syntax-only lower bound, not a Connection comparison.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use h11r::Method;
use h11r::StatusCode;
use h11r::{Connection, Event, Header, Limits, NextEvent, Request, Response, Role, State, Version};
use std::hint::black_box;

const REQUEST: &[u8] = b"GET /items?q=1 HTTP/1.1\r\n\
Host: example.test\r\n\
User-Agent: h11r-benchmark\r\n\
Accept: */*\r\n\
Connection: keep-alive\r\n\r\n";
const FIXED_HEAD: &[u8] = b"POST /items HTTP/1.1\r\n\
Host: example.test\r\n\
Content-Length: 1024\r\n\r\n";
const CHUNKED_HEAD: &[u8] = b"POST /items HTTP/1.1\r\n\
Host: example.test\r\n\
Transfer-Encoding: chunked\r\n\r\n";
const BODY: &[u8] = &[b'x'; 1024];
const LARGE_BODY: &[u8] = &[b'x'; 64 * 1024];

fn header(name: &[u8], value: &[u8]) -> Header {
    (name.to_vec(), value.to_vec())
}

fn response(status: u16, headers: Vec<Header>) -> Response {
    Response {
        status: StatusCode::from_u16(status).unwrap(),
        reason: if status == 204 {
            b"No Content".as_slice()
        } else {
            b"OK".as_slice()
        }
        .to_vec(),
        headers,
        http_version: Version::Http11,
    }
}

fn fixed_wire() -> Vec<u8> {
    [FIXED_HEAD, BODY].concat()
}

fn chunked_wire() -> Vec<u8> {
    let mut wire = CHUNKED_HEAD.to_vec();
    wire.extend_from_slice(b"400\r\n");
    wire.extend_from_slice(BODY);
    wire.extend_from_slice(b"\r\n0\r\n\r\n");
    wire
}

fn head_with_fields(count: usize) -> Vec<u8> {
    let mut wire = b"GET / HTTP/1.1\r\nHost: example.test\r\n".to_vec();
    for index in 1..count {
        wire.extend_from_slice(format!("X-{index}: value\r\n").as_bytes());
    }
    wire.extend_from_slice(b"\r\n");
    wire
}

fn receive_head(wire: &[u8]) -> [NextEvent; 2] {
    let mut connection = Connection::new(Role::Server, Limits::default());
    connection.receive_data(black_box(wire)).unwrap();
    [
        connection.next_event().unwrap(),
        connection.next_event().unwrap(),
    ]
}

fn receive_body(wire: &[u8]) -> [NextEvent; 3] {
    let mut connection = Connection::new(Role::Server, Limits::default());
    connection.receive_data(black_box(wire)).unwrap();
    [
        connection.next_event().unwrap(),
        connection.next_event().unwrap(),
        connection.next_event().unwrap(),
    ]
}

fn receive_fragmented(wire: &[u8], size: usize) -> [NextEvent; 2] {
    let mut connection = Connection::new(Role::Server, Limits::default());
    let mut events = [None, None];
    let mut count = 0;
    for part in wire.chunks(size) {
        connection.receive_data(black_box(part)).unwrap();
        loop {
            match connection.next_event().unwrap() {
                NextEvent::NeedData => break,
                NextEvent::Event(event) => {
                    events[count] = Some(NextEvent::Event(event));
                    count += 1;
                }
                NextEvent::Paused => panic!("request parsing paused before a response"),
            }
        }
    }
    events.map(|event| event.expect("request and end-of-message events"))
}

fn reusable_cycle(connection: &mut Connection, reply: &Response) {
    connection.receive_data(black_box(REQUEST)).unwrap();
    black_box(connection.next_event().unwrap());
    black_box(connection.next_event().unwrap());
    black_box(connection.send_response(reply).unwrap());
    black_box(connection.end_of_message(&[]).unwrap());
    connection.start_next_cycle().unwrap();
}

fn client_with_fixed_body() -> Connection {
    let mut connection = Connection::new(Role::Client, Limits::default());
    connection
        .send_request(&Request {
            method: Method::from_bytes(b"POST").unwrap(),
            target: b"/upload".to_vec(),
            headers: vec![
                header(b"Host", b"example.test"),
                header(b"Content-Length", b"65536"),
            ],
            http_version: Version::Http11,
        })
        .unwrap();
    connection
}

fn server_exchange(reply: &Response) -> [Vec<u8>; 3] {
    let mut connection = Connection::new(Role::Server, Limits::default());
    connection.receive_data(black_box(REQUEST)).unwrap();
    black_box(connection.next_event().unwrap());
    black_box(connection.next_event().unwrap());
    [
        connection.send_response(reply).unwrap(),
        connection.send_data(BODY).unwrap(),
        connection.end_of_message(&[]).unwrap(),
    ]
}

fn check_workloads() {
    let [request, end] = receive_head(REQUEST);
    let NextEvent::Event(Event::Request(request)) = request else {
        panic!("expected request event")
    };
    assert_eq!(request.method.as_bytes(), b"GET");
    assert_eq!(request.target, b"/items?q=1");
    assert!(matches!(end, NextEvent::Event(Event::EndOfMessage(_))));

    for wire in [&fixed_wire(), &chunked_wire()] {
        let [request, data, end] = receive_body(wire);
        assert!(matches!(request, NextEvent::Event(Event::Request(_))));
        assert!(matches!(data, NextEvent::Event(Event::Data(ref value)) if value.data == BODY));
        assert!(matches!(end, NextEvent::Event(Event::EndOfMessage(_))));
    }

    for size in [1, 32] {
        let [request, end] = receive_fragmented(REQUEST, size);
        assert!(matches!(request, NextEvent::Event(Event::Request(_))));
        assert!(matches!(end, NextEvent::Event(Event::EndOfMessage(_))));
    }

    let no_content = response(204, vec![]);
    let mut reusable = Connection::new(Role::Server, Limits::default());
    reusable_cycle(&mut reusable, &no_content);
    assert_eq!(
        (reusable.local_state(), reusable.peer_state()),
        (State::Idle, State::Idle)
    );

    let reply = response(200, vec![header(b"Content-Length", b"1024")]);
    let bytes = server_exchange(&reply).concat();
    assert_eq!(
        bytes,
        [
            b"HTTP/1.1 200 OK\r\nContent-Length: 1024\r\n\r\n".as_slice(),
            BODY
        ]
        .concat()
    );

    let mut copy = client_with_fixed_body();
    assert_eq!(copy.send_data(LARGE_BODY).unwrap(), LARGE_BODY);
    let mut parts = client_with_fixed_body();
    let parts = parts.send_data_parts(LARGE_BODY).unwrap();
    assert!(parts.prefix.is_empty() && parts.suffix.is_empty());
    assert_eq!(parts.data, LARGE_BODY);
}

fn benchmarks(c: &mut Criterion) {
    check_workloads();
    let fixed = fixed_wire();
    let chunked = chunked_wire();
    let no_content = response(204, vec![]);
    let reply = response(200, vec![header(b"Content-Length", b"1024")]);

    let mut receive = c.benchmark_group("common/receive");
    receive.bench_function("fresh_head", |b| {
        b.iter(|| black_box(receive_head(REQUEST)))
    });
    receive.bench_function("four_fragments", |b| {
        b.iter(|| black_box(receive_fragmented(REQUEST, 32)))
    });
    receive.bench_function("content_length_1k", |b| {
        b.iter(|| black_box(receive_body(&fixed)))
    });
    receive.bench_function("chunked_1k", |b| {
        b.iter(|| black_box(receive_body(&chunked)))
    });
    receive.finish();

    c.bench_function("common/reusable_server_cycle", |b| {
        let mut connection = Connection::new(Role::Server, Limits::default());
        b.iter(|| reusable_cycle(&mut connection, &no_content));
    });
    c.bench_function("common/fresh_server_exchange_1k", |b| {
        b.iter(|| black_box(server_exchange(&reply)))
    });

    let mut body = c.benchmark_group("common/send_body_64k");
    body.throughput(Throughput::Bytes(LARGE_BODY.len() as u64));
    body.bench_function("copy", |b| {
        b.iter_batched(
            client_with_fixed_body,
            |mut connection| black_box(connection.send_data(black_box(LARGE_BODY)).unwrap()),
            BatchSize::SmallInput,
        )
    });
    body.bench_function("parts", |b| {
        b.iter_batched(
            client_with_fixed_body,
            |mut connection| black_box(connection.send_data_parts(black_box(LARGE_BODY)).unwrap()),
            BatchSize::SmallInput,
        )
    });
    body.finish();

    let heads = [(4, head_with_fields(4)), (100, head_with_fields(100))];
    let mut fields = c.benchmark_group("diagnostic/header_fields");
    for (count, wire) in &heads {
        fields.bench_with_input(BenchmarkId::from_parameter(count), wire, |b, wire| {
            b.iter(|| black_box(receive_head(black_box(wire))))
        });
    }
    fields.finish();

    c.bench_function("diagnostic/byte_fragmented_head", |b| {
        b.iter(|| black_box(receive_fragmented(REQUEST, 1)))
    });

    let mut substrate = c.benchmark_group("diagnostic/request_head");
    substrate.throughput(Throughput::Bytes(REQUEST.len() as u64));
    substrate.bench_function("httparse_substrate", |b| {
        b.iter(|| {
            let mut headers = [httparse::EMPTY_HEADER; 16];
            let mut request = httparse::Request::new(&mut headers);
            black_box(request.parse(black_box(REQUEST)).unwrap());
        });
    });
    substrate.finish();
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);

from __future__ import annotations

import http.client
import socket

import h11 as reference
import h11r as candidate


def test_candidate_client_exchanges_with_h11_server() -> None:
    client = candidate.Connection(candidate.Role.CLIENT)
    server = reference.Connection(reference.SERVER)

    request_wire = client.send_request(
        "POST",
        "/upload",
        [("Host", "example.test"), ("Transfer-Encoding", "chunked")],
    )
    request_wire += client.send_data(b"body") + client.end_of_message()
    server.receive_data(request_wire)

    request = server.next_event()
    body = server.next_event()
    end = server.next_event()
    assert isinstance(request, reference.Request)
    assert (request.method, request.target) == (b"POST", b"/upload")
    assert isinstance(body, reference.Data) and body.data == b"body"
    assert isinstance(end, reference.EndOfMessage)

    response_wire = server.send(
        reference.Response(status_code=200, headers=[("Content-Length", "2")])
    )
    response_wire += server.send(reference.Data(data=b"ok"))
    response_wire += server.send(reference.EndOfMessage())
    client.receive_data(response_wire)

    response = client.next_event()
    body = client.next_event()
    end = client.next_event()
    assert isinstance(response, candidate.Response) and response.status_code == 200
    assert isinstance(body, candidate.Data) and body.data == b"ok"
    assert isinstance(end, candidate.EndOfMessage)


def test_h11_client_exchanges_with_candidate_server() -> None:
    client = reference.Connection(reference.CLIENT)
    server = candidate.Connection(candidate.Role.SERVER)

    request_wire = client.send(
        reference.Request(
            method="GET",
            target="/items",
            headers=[("Host", "example.test")],
        )
    )
    request_wire += client.send(reference.EndOfMessage())
    server.receive_data(request_wire)

    request = server.next_event()
    end = server.next_event()
    assert isinstance(request, candidate.Request)
    assert (request.method, request.target) == (b"GET", b"/items")
    assert isinstance(end, candidate.EndOfMessage)

    response_wire = server.send_response(
        200, [("Transfer-Encoding", "chunked")], reason="OK"
    )
    response_wire += server.send_data(b"body") + server.end_of_message()
    client.receive_data(response_wire)

    response = client.next_event()
    body = client.next_event()
    end = client.next_event()
    assert isinstance(response, reference.Response) and response.status_code == 200
    assert isinstance(body, reference.Data) and body.data == b"body"
    assert isinstance(end, reference.EndOfMessage)


def test_stdlib_http_client_exchanges_over_a_socket() -> None:
    server = candidate.Connection(candidate.Role.SERVER)
    client_socket, server_socket = socket.socketpair()
    with client_socket, server_socket:
        client_socket.settimeout(2)
        server_socket.settimeout(2)
        client = http.client.HTTPConnection("example.test")
        client.sock = client_socket
        client.request("POST", "/echo", body=b"body")

        events = []
        while not events or not isinstance(events[-1], candidate.EndOfMessage):
            event = server.next_event()
            if event is candidate.ReceiveStatus.NEED_DATA:
                server.receive_data(server_socket.recv(4096))
            else:
                events.append(event)

        assert isinstance(events[0], candidate.Request)
        assert (events[0].method, events[0].target) == (b"POST", b"/echo")
        assert isinstance(events[1], candidate.Data) and events[1].data == b"body"

        response_wire = server.send_response(200, [("Content-Length", "4")])
        response_wire += server.send_data(b"pong") + server.end_of_message()
        server_socket.sendall(response_wire)

        response = client.getresponse()
        assert response.status == 200
        assert response.read() == b"pong"
        client.close()

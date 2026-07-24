from collections.abc import Iterable
from typing import ClassVar, Protocol, TypeAlias, TypeVar, final

__version__: str

class _ByteSized(Protocol):
    @property
    def nbytes(self) -> int: ...

_Buffer: TypeAlias = bytes | bytearray | memoryview
_DataT = TypeVar("_DataT", bound=_Buffer | _ByteSized)
_HeaderInput: TypeAlias = Iterable[tuple[_Buffer | str, _Buffer | str]]
_Headers: TypeAlias = tuple[tuple[bytes, bytes], ...]

@final
class Role:
    CLIENT: ClassVar[Role]
    SERVER: ClassVar[Role]

@final
class State:
    IDLE: ClassVar[State]
    SEND_RESPONSE: ClassVar[State]
    SEND_BODY: ClassVar[State]
    DONE: ClassVar[State]
    MIGHT_SWITCH_PROTOCOL: ClassVar[State]
    SWITCHED_PROTOCOL: ClassVar[State]
    MUST_CLOSE: ClassVar[State]
    CLOSED: ClassVar[State]
    ERROR: ClassVar[State]

@final
class ReceiveStatus:
    NEED_DATA: ClassVar[ReceiveStatus]
    PAUSED: ClassVar[ReceiveStatus]

class ProtocolError(Exception): ...
class LocalProtocolError(ProtocolError): ...

class RemoteProtocolError(ProtocolError):
    suggested_status_code: int | None

@final
class Request:
    @property
    def method(self) -> bytes: ...
    @property
    def target(self) -> bytes: ...
    @property
    def headers(self) -> _Headers: ...
    @property
    def http_version(self) -> bytes: ...

@final
class InformationalResponse:
    @property
    def status_code(self) -> int: ...
    @property
    def reason(self) -> bytes: ...
    @property
    def headers(self) -> _Headers: ...
    @property
    def http_version(self) -> bytes: ...

@final
class Response:
    @property
    def status_code(self) -> int: ...
    @property
    def reason(self) -> bytes: ...
    @property
    def headers(self) -> _Headers: ...
    @property
    def http_version(self) -> bytes: ...

@final
class Data:
    @property
    def data(self) -> bytes: ...
    @property
    def chunk_start(self) -> bool: ...
    @property
    def chunk_end(self) -> bool: ...

@final
class EndOfMessage:
    def __new__(cls, trailers: _HeaderInput | None = None) -> EndOfMessage: ...
    @property
    def trailers(self) -> _Headers: ...

@final
class ConnectionClosed:
    def __init__(self) -> None: ...

_Event: TypeAlias = (
    Request | InformationalResponse | Response | Data | EndOfMessage | ConnectionClosed
)

@final
class Connection:
    def __new__(
        cls,
        role: Role,
        *,
        max_head_bytes: int = 65536,
        max_header_count: int = 100,
    ) -> Connection: ...
    def receive_data(self, data: _Buffer) -> None: ...
    def next_event(self) -> _Event | ReceiveStatus: ...
    def send_request(
        self,
        method: _Buffer | str,
        target: _Buffer | str,
        headers: _HeaderInput,
        *,
        http_version: _Buffer | str | None = None,
    ) -> bytes: ...
    def send_informational_response(
        self,
        status_code: int,
        headers: _HeaderInput | None = None,
        *,
        reason: _Buffer | str | None = None,
        http_version: _Buffer | str | None = None,
    ) -> bytes: ...
    def send_response(
        self,
        status_code: int,
        headers: _HeaderInput | None = None,
        *,
        reason: _Buffer | str | None = None,
        http_version: _Buffer | str | None = None,
    ) -> bytes: ...
    def send_data(self, data: _Buffer) -> bytes: ...
    def send_data_parts(self, data: _DataT) -> tuple[bytes, _DataT, bytes]: ...
    def end_of_message(self, trailers: _HeaderInput | None = None) -> bytes: ...
    def start_next_cycle(self) -> None: ...
    def close(self) -> None: ...
    @property
    def local_state(self) -> State: ...
    @property
    def peer_state(self) -> State: ...
    @property
    def peer_http_version(self) -> bytes | None: ...
    @property
    def client_is_waiting_for_100_continue(self) -> bool: ...
    @property
    def trailing_data(self) -> tuple[bytes, bool]: ...

__all__ = [
    "Connection",
    "ConnectionClosed",
    "Data",
    "EndOfMessage",
    "InformationalResponse",
    "LocalProtocolError",
    "ProtocolError",
    "ReceiveStatus",
    "RemoteProtocolError",
    "Request",
    "Response",
    "Role",
    "State",
    "__version__",
]

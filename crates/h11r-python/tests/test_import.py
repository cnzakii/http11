from __future__ import annotations

import h11r
import h11r._core


def test_public_import_surface() -> None:
    assert h11r.__all__ == [
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
    assert h11r.__version__
    assert h11r._core.__version__ == h11r.__version__
    assert h11r.Connection.__module__ == "h11r"
    for name in h11r.__all__:
        assert hasattr(h11r._core, name)

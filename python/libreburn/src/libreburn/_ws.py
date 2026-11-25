# SPDX-License-Identifier: AGPL-3.0-or-later
"""A small WebSocket client (RFC 6455) for the event stream, using only the stdlib.

It supports what `/api/v1/events` needs: text frames from the server, pings,
and close. It is not a general-purpose WebSocket library.
"""

from __future__ import annotations

import base64
import hashlib
import os
import socket
import struct
from collections.abc import Iterator
from urllib.parse import urlsplit

_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


class WebSocketError(OSError):
    """The WebSocket handshake or stream failed."""


def _recv_exact(sock: socket.socket, n: int) -> bytes:
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise WebSocketError("connection closed by the service")
        buf += chunk
    return bytes(buf)


def _send_frame(sock: socket.socket, opcode: int, payload: bytes) -> None:
    # Clients must mask every frame.
    mask = os.urandom(4)
    header = bytearray([0x80 | opcode])
    n = len(payload)
    if n < 126:
        header.append(0x80 | n)
    elif n < 1 << 16:
        header.append(0x80 | 126)
        header += struct.pack("!H", n)
    else:
        header.append(0x80 | 127)
        header += struct.pack("!Q", n)
    masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
    sock.sendall(bytes(header) + mask + masked)


def connect(url: str, headers: dict[str, str], timeout: float | None) -> socket.socket:
    """Open a WebSocket to an `http://` or `ws://` URL and return the socket."""
    parts = urlsplit(url)
    if parts.scheme in ("https", "wss"):
        raise WebSocketError("TLS is not supported; put the service behind a local tunnel")
    host = parts.hostname or "127.0.0.1"
    port = parts.port or 80
    path = parts.path + (f"?{parts.query}" if parts.query else "")
    sock = socket.create_connection((host, port), timeout=timeout)
    key = base64.b64encode(os.urandom(16)).decode()
    lines = [
        f"GET {path} HTTP/1.1",
        f"Host: {host}:{port}",
        "Upgrade: websocket",
        "Connection: Upgrade",
        f"Sec-WebSocket-Key: {key}",
        "Sec-WebSocket-Version: 13",
        *(f"{k}: {v}" for k, v in headers.items()),
    ]
    sock.sendall(("\r\n".join(lines) + "\r\n\r\n").encode())
    response = b""
    while b"\r\n\r\n" not in response:
        chunk = sock.recv(1024)
        if not chunk:
            raise WebSocketError("the service closed the connection during the handshake")
        response += chunk
    head, _, rest = response.partition(b"\r\n\r\n")
    status = head.split(b"\r\n", 1)[0].decode(errors="replace")
    if " 101 " not in status + " ":
        sock.close()
        raise WebSocketError(f"handshake refused: {status}")
    expected = base64.b64encode(hashlib.sha1((key + _GUID).encode()).digest()).decode()
    if expected.encode() not in head:
        sock.close()
        raise WebSocketError("handshake reply had the wrong Sec-WebSocket-Accept")
    if rest:
        raise WebSocketError("unexpected data after the handshake")
    return sock


def messages(sock: socket.socket) -> Iterator[str]:
    """Yield text messages until the server closes the stream."""
    fragments: list[bytes] = []
    try:
        while True:
            b0, b1 = _recv_exact(sock, 2)
            opcode = b0 & 0x0F
            n = b1 & 0x7F
            if n == 126:
                (n,) = struct.unpack("!H", _recv_exact(sock, 2))
            elif n == 127:
                (n,) = struct.unpack("!Q", _recv_exact(sock, 8))
            mask = _recv_exact(sock, 4) if b1 & 0x80 else b""
            payload = _recv_exact(sock, n)
            if mask:
                payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
            if opcode == 0x8:
                _send_frame(sock, 0x8, payload[:2])
                return
            if opcode == 0x9:
                _send_frame(sock, 0xA, payload)
                continue
            if opcode in (0x0, 0x1):
                fragments.append(payload)
                if b0 & 0x80:
                    yield b"".join(fragments).decode()
                    fragments = []
    finally:
        sock.close()

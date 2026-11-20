"""The listener's WebSocket client (stdlib only): the handshake and its
refusals, frame reading, and per-socket send locks.
"""

import base64
import os
import socket
import ssl
import struct
import threading
import weakref
import time
from urllib.parse import urlparse
from credentials import validate_server_scope
from kit_version import with_kit_version


class HandshakeRefused(ConnectionError):
    """A WebSocket handshake the server answered with something other than
    101. ``status`` is the HTTP status (or ``None``); ``body`` holds at most
    the first 1 KiB of the answer's body."""

    def __init__(self, message, status=None, body=b""):
        super().__init__(message)
        self.status = status
        self.body = body


def refusal_says_suspended(error):
    """Did the server refuse this handshake because the membership is
    suspended (403 ``membership suspended``)? A 403 from anything else, such
    as a proxy, does not count."""
    return (
        isinstance(error, HandshakeRefused)
        and error.status == 403
        and error.body.strip() == b"membership suspended"
    )


def read_refusal(s, buf):
    """Return (status, body) of a refused handshake; the body is read up to
    its Content-Length, at most 1 KiB, and only as far as it arrives."""
    head, _, body = buf.partition(b"\r\n\r\n")
    lines = head.split(b"\r\n")
    parts = lines[0].split()
    status = int(parts[1]) if len(parts) > 1 and parts[1].isdigit() else None
    length = 0
    for line in lines[1:]:
        name, _, value = line.partition(b":")
        if name.strip().lower() == b"content-length" and value.strip().isdigit():
            length = min(int(value.strip()), 1024)
    try:
        while len(body) < length:
            chunk = s.recv(length - len(body))
            if not chunk:
                break
            body += chunk
    except OSError:
        pass
    return status, body[:1024]


def connect(url, protocols=None):
    validate_server_scope(url)
    # Display only: the Master sees which kit this agent runs (kit_version.py).
    u = urlparse(with_kit_version(url))
    host = u.hostname
    port = u.port or (443 if u.scheme == "wss" else 80)
    path = u.path + ("?" + u.query if u.query else "")
    s = socket.create_connection((host, port), timeout=10)
    if u.scheme == "wss":
        context = ssl.create_default_context()
        # Never below TLS 1.2 (the default since Python 3.10; explicit here so
        # an older interpreter cannot negotiate TLS 1.0/1.1).
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        s = context.wrap_socket(s, server_hostname=host)
    key = base64.b64encode(os.urandom(16)).decode()
    protocol_header = ""
    if protocols:
        protocol_header = "Sec-WebSocket-Protocol: " + ", ".join(protocols) + "\r\n"
    req = (
        f"GET {path} HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n"
        f"{protocol_header}\r\n"
    )
    s.sendall(req.encode())
    # Read handshake response headers.
    buf = b""
    while b"\r\n\r\n" not in buf:
        chunk = s.recv(1024)
        if not chunk:
            raise ConnectionError("closed during handshake")
        buf += chunk
    if b" 101 " not in buf.split(b"\r\n", 1)[0]:
        status, body = read_refusal(s, buf)
        raise HandshakeRefused(
            "handshake failed: " + buf.split(b"\r\n", 1)[0].decode(errors="replace"),
            status,
            body,
        )
    # A healthy server sends WebSocket pings regularly. Keep a bounded read
    # timeout so a half-open nginx/TCP connection cannot make an agent look
    # alive locally while it has vanished from the server roster forever.
    s.settimeout(20)
    return s, buf.split(b"\r\n\r\n", 1)[1]


# One writer at a time per socket: the reading thread answers pings while the
# "replying" thread may send a typing frame; interleaved bytes would corrupt
# the stream.
_SEND_LOCKS = weakref.WeakKeyDictionary()


_SEND_LOCKS_GUARD = threading.Lock()


def send_lock_for(s):
    with _SEND_LOCKS_GUARD:
        lock = _SEND_LOCKS.get(s)
        if lock is None:
            lock = _SEND_LOCKS[s] = threading.Lock()
        return lock


def send_control_frame(s, opcode, payload=b""):
    """Send one masked client control frame (pong/close)."""
    payload = bytes(payload)
    if len(payload) > 125:
        raise ValueError("WebSocket control payload is too large")
    mask = os.urandom(4)
    masked = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
    with send_lock_for(s):
        s.sendall(bytes([0x80 | opcode, 0x80 | len(payload)]) + mask + masked)


def send_text_frame(s, text):
    """Send one masked client text frame (small JSON control messages)."""
    payload = text.encode("utf-8")
    if len(payload) > 125:
        raise ValueError("text frame is larger than this listener sends")
    mask = os.urandom(4)
    masked = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
    with send_lock_for(s):
        s.sendall(bytes([0x81, 0x80 | len(payload)]) + mask + masked)


def frames(s, leftover=b"", on_activity=None):
    """Yield decoded text payloads from the socket."""
    buf = leftover

    def read_more():
        data = s.recv(4096)
        if not data:
            raise ConnectionError("WebSocket closed")
        if on_activity is not None:
            on_activity()
        return data

    while True:
        # Ensure at least 2 header bytes.
        while len(buf) < 2:
            buf += read_more()
        b0, b1 = buf[0], buf[1]
        opcode = b0 & 0x0F
        masked = b1 & 0x80
        ln = b1 & 0x7F
        idx = 2
        if ln == 126:
            while len(buf) < 4:
                buf += read_more()
            ln = struct.unpack(">H", buf[2:4])[0]; idx = 4
        elif ln == 127:
            while len(buf) < 10:
                buf += read_more()
            ln = struct.unpack(">Q", buf[2:10])[0]; idx = 10
        mask = b""
        if masked:
            while len(buf) < idx + 4:
                buf += read_more()
            mask = buf[idx:idx + 4]; idx += 4
        while len(buf) < idx + ln:
            buf += read_more()
        payload = bytearray(buf[idx:idx + ln])
        buf = buf[idx + ln:]
        if masked:
            for i in range(len(payload)):
                payload[i] ^= mask[i % 4]
        if opcode == 0x8:      # close
            return
        if opcode == 0x9:      # ping
            send_control_frame(s, 0xA, payload)
            continue
        if opcode in (0x1, 0x2):
            yield payload.decode("utf-8", "replace")

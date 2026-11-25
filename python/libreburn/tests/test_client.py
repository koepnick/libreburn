# SPDX-License-Identifier: AGPL-3.0-or-later
import json
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, HTTPServer

import pytest

from libreburn import Client, LibreburnError


class _Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        body = {
            "/api/v1/health": b"ok",
            "/api/v1/version": json.dumps({"api": "v1"}).encode(),
        }.get(self.path)
        self.send_response(200 if body else 404)
        self.end_headers()
        self.wfile.write(body or b"")

    def log_message(self, format: str, *args: object) -> None:
        pass


@pytest.fixture
def server() -> Iterator[str]:
    httpd = HTTPServer(("127.0.0.1", 0), _Handler)
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    yield f"http://127.0.0.1:{httpd.server_address[1]}"
    httpd.shutdown()


def test_health_and_version(server: str) -> None:
    client = Client(server)
    assert client.health()
    assert client.version()["api"] == "v1"


def test_unreachable_service_gives_a_coded_error() -> None:
    with pytest.raises(LibreburnError, match="LB-SDK-001"):
        Client("http://127.0.0.1:9", timeout=0.5).health()

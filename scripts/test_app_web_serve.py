#!/usr/bin/env python3
"""HTTP regression for reloading the packaged app's client-side routes."""

from functools import partial
from http.client import HTTPConnection
from http.server import ThreadingHTTPServer
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Thread
import unittest

from app_web_serve import AppHandler


class ReloadTest(unittest.TestCase):
    def test_html_navigation_falls_back_without_masking_missing_assets(self):
        with TemporaryDirectory() as directory:
            Path(directory, "index.html").write_bytes(b"app shell")
            Path(directory, "app.js").write_bytes(b"script")
            handler = partial(AppHandler, directory=directory)
            with ThreadingHTTPServer(("127.0.0.1", 0), handler) as server:
                thread = Thread(target=server.serve_forever)
                thread.start()
                try:
                    for path, accept, status, body in [
                        ("/sessions", "text/html", 200, b"app shell"),
                        ("/session/B-1/child/bot%2Ev2?from=link", "text/html, */*", 200, b"app shell"),
                        ("/app.js", "text/html", 200, b"script"),
                        ("/missing.js", "text/html", 404, None),
                        ("/sessions", "*/*", 404, None),
                    ]:
                        with self.subTest(path=path, accept=accept):
                            conn = HTTPConnection(*server.server_address, timeout=5)
                            try:
                                conn.request("GET", path, headers={"Accept": accept})
                                response = conn.getresponse()
                                self.assertEqual(response.status, status)
                                if body is not None:
                                    self.assertEqual(response.read(), body)
                            finally:
                                conn.close()
                finally:
                    server.shutdown()
                    thread.join()


if __name__ == "__main__":
    unittest.main()

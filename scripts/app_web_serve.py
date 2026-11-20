#!/usr/bin/env python3
"""Serve the Corral web package on loopback."""

import argparse
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from os.path import exists


class AppHandler(SimpleHTTPRequestHandler):
    def send_head(self):
        # Use the URL, not the decoded file path: the app escapes dots in record IDs.
        path = self.path.partition("?")[0].rstrip("/")
        accepts_html = any(
            item.partition(";")[0].strip().lower() == "text/html"
            for item in self.headers.get("Accept", "").split(",")
        )
        if (
            accepts_html
            and "." not in path.rsplit("/", 1)[-1]
            and not exists(self.translate_path(self.path))
        ):
            self.path = "/index.html"
        return super().send_head()

    def end_headers(self):
        self.send_header("Vary", "Accept")
        super().end_headers()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", required=True)
    parser.add_argument("port", type=int, nargs="?", default=8890)
    args = parser.parse_args()
    handler = partial(AppHandler, directory=args.directory)
    with ThreadingHTTPServer(("127.0.0.1", args.port), handler) as server:
        server.serve_forever()


if __name__ == "__main__":
    main()

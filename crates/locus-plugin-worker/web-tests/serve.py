# SPDX-License-Identifier: AGPL-3.0-or-later
"""Serves the web plugin host test page and records its verdict (#254).

    python3 serve.py <site-dir> <port> <result-file>

Serves <site-dir> on 127.0.0.1:<port>. The page POSTs its report to /result,
which is written to <result-file>; CI waits for that file and checks it.
"""

import functools
import http.server
import sys

site, port, result = sys.argv[1], int(sys.argv[2]), sys.argv[3]


class Handler(http.server.SimpleHTTPRequestHandler):
    def do_POST(self):  # noqa: N802 (http.server's naming)
        if self.path != "/result":
            self.send_error(404)
            return
        length = min(int(self.headers.get("Content-Length", "0")), 1 << 20)
        with open(result, "wb") as f:
            f.write(self.rfile.read(length))
        self.send_response(204)
        self.end_headers()

    def log_message(self, *args):
        pass


handler = functools.partial(Handler, directory=site)
http.server.ThreadingHTTPServer(("127.0.0.1", port), handler).serve_forever()

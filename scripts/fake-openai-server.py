#!/usr/bin/env python3
"""Tiny OpenAI-compatible server for local TUI soaking tests."""

from __future__ import annotations

import argparse
import http.server
import json
import time


class Handler(http.server.BaseHTTPRequestHandler):
    content = "OK"
    delay_secs = 0.0

    def do_GET(self) -> None:
        if self.path != "/health":
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"ok":true}\n')

    def do_POST(self) -> None:
        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return

        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length)
        try:
            body = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            self.send_error(400, "invalid json")
            return

        if self.delay_secs > 0:
            time.sleep(self.delay_secs)

        if body.get("stream"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            chunks = [
                {
                    "id": "chatcmpl-tui-soak",
                    "object": "chat.completion.chunk",
                    "created": int(time.time()),
                    "model": body.get("model", "gpt-4o-mini"),
                    "choices": [
                        {
                            "index": 0,
                            "delta": {"role": "assistant", "content": self.content},
                            "finish_reason": None,
                        }
                    ],
                },
                {
                    "id": "chatcmpl-tui-soak",
                    "object": "chat.completion.chunk",
                    "created": int(time.time()),
                    "model": body.get("model", "gpt-4o-mini"),
                    "choices": [
                        {
                            "index": 0,
                            "delta": {},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {
                        "prompt_tokens": 1,
                        "completion_tokens": 1,
                        "total_tokens": 2,
                    },
                },
            ]
            for chunk in chunks:
                self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode("utf-8"))
                self.wfile.flush()
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
            return

        payload = {
            "id": "chatcmpl-tui-soak",
            "object": "chat.completion",
            "created": int(time.time()),
            "model": body.get("model", "gpt-4o-mini"),
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": self.content},
                    "finish_reason": "stop",
                }
            ],
            "usage": {
                "prompt_tokens": 1,
                "completion_tokens": 1,
                "total_tokens": 2,
            },
        }
        encoded = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, fmt: str, *args: object) -> None:
        print(f"{self.address_string()} - {fmt % args}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--content", default="OK")
    parser.add_argument("--delay-secs", type=float, default=0.0)
    args = parser.parse_args()

    Handler.content = args.content
    Handler.delay_secs = max(0.0, args.delay_secs)
    server = http.server.ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"fake OpenAI server listening on {args.host}:{args.port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()

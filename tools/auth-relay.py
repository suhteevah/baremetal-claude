#!/usr/bin/env python3
"""OAuth auth relay for ClaudioOS.

Opens your browser to Anthropic's console, you log in and create an API key,
then paste it here. The relay serves it over HTTP so the bare-metal OS can
fetch it from 10.0.2.2:8444.

Usage:
    python tools/auth-relay.py

The guest OS fetches: GET http://10.0.2.2:8444/token
"""

import http.server
import threading
import webbrowser
import sys
import json

TOKEN = None
PORT = 8444

class TokenHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/token" and TOKEN:
            body = json.dumps({"api_key": TOKEN}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path == "/token":
            body = b'{"status":"waiting"}'
            self.send_response(202)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, format, *args):
        pass  # quiet

def main():
    global TOKEN

    print("=" * 60)
    print("  ClaudioOS Auth Relay")
    print("=" * 60)
    print()
    print("Opening Anthropic console to create/copy an API key...")
    print()
    webbrowser.open("https://console.anthropic.com/settings/keys")
    print("1. Log in to your Anthropic account")
    print("2. Create a new API key (or copy existing)")
    print("3. Paste it below")
    print()

    TOKEN = input("Paste API key here: ").strip()
    if not TOKEN:
        print("No key entered. Exiting.")
        sys.exit(1)

    print()
    print(f"[relay] Token received ({len(TOKEN)} chars)")
    print(f"[relay] Serving on 0.0.0.0:{PORT}")
    print(f"[relay] Guest fetches: GET http://10.0.2.2:{PORT}/token")
    print(f"[relay] Waiting for ClaudioOS to fetch token...")
    print()

    server = http.server.HTTPServer(("0.0.0.0", PORT), TokenHandler)
    server.serve_forever()

if __name__ == "__main__":
    main()

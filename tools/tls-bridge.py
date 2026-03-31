#!/usr/bin/env python3
"""Stdin/stdout TLS bridge for QEMU guestfwd.

Reads HTTP from stdin (guest TCP), wraps in TLS, sends to upstream,
reads response, writes back to stdout.

Used with: -netdev user,guestfwd=tcp:10.0.2.100:443-cmd:"python tools/tls-bridge.py"
"""
import sys, ssl, socket, threading, os

# Determine upstream from first HTTP request's Host header
def main():
    # Read the full request from stdin
    stdin = sys.stdin.buffer if hasattr(sys.stdin, 'buffer') else sys.stdin
    stdout = sys.stdout.buffer if hasattr(sys.stdout, 'buffer') else sys.stdout

    data = b""
    while b"\r\n\r\n" not in data:
        chunk = stdin.read(1)
        if not chunk:
            return
        data += chunk

    # Check for body (Content-Length)
    content_length = 0
    for line in data.split(b"\r\n"):
        if line.lower().startswith(b"content-length:"):
            content_length = int(line.split(b":")[1].strip())
            break

    # Read body
    if content_length > 0:
        body = stdin.read(content_length)
        data += body

    # Extract host
    host = "api.anthropic.com"
    for line in data.split(b"\r\n"):
        if line.lower().startswith(b"host:"):
            host = line.split(b":", 1)[1].strip().decode()
            if ":" in host:
                host = host.split(":")[0]
            break

    # Connect upstream with TLS
    ctx = ssl.create_default_context()
    sock = ctx.wrap_socket(socket.socket(), server_hostname=host)
    sock.connect((host, 443))
    sock.sendall(data)

    # Read response and forward to stdout
    while True:
        try:
            chunk = sock.recv(8192)
            if not chunk:
                break
            stdout.write(chunk)
            stdout.flush()
        except:
            break

    sock.close()

if __name__ == "__main__":
    main()

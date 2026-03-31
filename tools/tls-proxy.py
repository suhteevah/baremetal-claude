#!/usr/bin/env python3
"""TLS termination proxy for ClaudioOS development.

Listens on a local TCP port and forwards to upstream HTTPS servers.
The bare-metal guest (which has no TLS stack yet) connects here with
plain HTTP, and this proxy handles the TLS handshake with the real server.

The guest sends a standard HTTP/1.1 request with a Host header — the proxy
uses that to determine the upstream server.

Usage:
    python tls-proxy.py [port]        (default: 8443)
"""

import socket, ssl, threading, sys

LISTEN_PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8443

def handle_client(client_sock, addr):
    """Handle one TCP connection from the guest."""
    try:
        # Read enough to parse the Host header
        data = b""
        while b"\r\n\r\n" not in data and len(data) < 8192:
            chunk = client_sock.recv(4096)
            if not chunk:
                client_sock.close()
                return
            data += chunk

        # Extract Host header to determine upstream
        host = "api.anthropic.com"  # default
        for line in data.split(b"\r\n"):
            if line.lower().startswith(b"host:"):
                host = line.split(b":", 1)[1].strip().decode("ascii")
                # Remove port if present
                if ":" in host:
                    host = host.split(":")[0]
                break

        print(f"[proxy] {addr} -> {host}:443")

        # Connect to upstream with TLS
        ctx = ssl.create_default_context()
        upstream = ctx.wrap_socket(
            socket.socket(socket.AF_INET, socket.SOCK_STREAM),
            server_hostname=host,
        )
        upstream.connect((host, 443))

        # Send the buffered request data
        upstream.sendall(data)

        # Bidirectional forwarding
        def forward(src, dst, name):
            try:
                while True:
                    chunk = src.recv(8192)
                    if not chunk:
                        break
                    dst.sendall(chunk)
            except (ConnectionError, OSError):
                pass
            try: src.shutdown(socket.SHUT_RD)
            except: pass
            try: dst.shutdown(socket.SHUT_WR)
            except: pass

        t1 = threading.Thread(target=forward, args=(client_sock, upstream, "guest->upstream"), daemon=True)
        t2 = threading.Thread(target=forward, args=(upstream, client_sock, "upstream->guest"), daemon=True)
        t1.start()
        t2.start()
        t1.join(timeout=30)
        t2.join(timeout=30)

    except Exception as e:
        print(f"[proxy] error handling {addr}: {e}")
    finally:
        try: client_sock.close()
        except: pass
        try: upstream.close()
        except: pass

def main():
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("0.0.0.0", LISTEN_PORT))
    server.listen(10)
    print(f"[proxy] TLS proxy listening on :{LISTEN_PORT}")
    print(f"[proxy] Guest should connect to 10.0.2.2:{LISTEN_PORT}")
    print(f"[proxy] Host header determines upstream (default: api.anthropic.com)")

    while True:
        client, addr = server.accept()
        threading.Thread(target=handle_client, args=(client, addr), daemon=True).start()

if __name__ == "__main__":
    main()

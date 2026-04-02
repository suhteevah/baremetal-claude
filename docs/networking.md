# ClaudioOS Network Stack

## Overview

ClaudioOS implements a complete network stack from NIC driver to HTTPS API calls,
all in `#![no_std]` Rust. No sockets API, no libc, no Linux kernel.

```
+-------------------------------------------------------------------+
|  claude.ai Chat API  |  Anthropic Messages API  |  SSH Daemon     |
+-------------------------------------------------------------------+
|  HTTP/1.1 Client (raw request/response, chunked encoding, SSE)    |
+-------------------------------------------------------------------+
|  TLS 1.3 (embedded-tls, AES-128-GCM-SHA256, 16-byte aligned)     |
+-------------------------------------------------------------------+
|  smoltcp TCP/IP (DHCP, DNS, TCP sockets, UDP sockets)             |
+-------------------------------------------------------------------+
|  NIC Driver: VirtIO-net (QEMU) or Intel e1000/e1000e (real HW)   |
+-------------------------------------------------------------------+
|  PCI Bus (BAR mapping, bus mastering, IRQ)                        |
+-------------------------------------------------------------------+
```

---

## VirtIO-net Driver

Used in QEMU with `-device virtio-net-pci,netdev=net0`.

**Spec**: VirtIO Legacy 0.9.5 (not modern 1.0+)

### Initialization

1. PCI enumeration finds vendor `0x1AF4`, device `0x1000`
2. Read BAR0 for I/O port base
3. Reset device (write 0 to STATUS register)
4. Set ACKNOWLEDGE + DRIVER status bits
5. Negotiate features (MAC address, status, etc.)
6. Set up virtqueues:
   - Queue 0: RX (receive packets from network)
   - Queue 1: TX (transmit packets to network)
7. Allocate DMA-accessible buffers (physical addresses via page table walk)
8. Set DRIVER_OK status

### Virtqueue Mechanics

```
Descriptor Table:  [addr, len, flags, next] x queue_size
Available Ring:    [flags, idx, ring[]] -- driver writes available buffer indices
Used Ring:         [flags, idx, ring[]] -- device writes completed buffer indices

TX: driver fills descriptor -> adds to available ring -> notifies device
RX: device fills descriptor -> adds to used ring -> fires interrupt
```

### Physical Address Translation

VirtIO requires physical addresses for DMA. The driver walks the x86_64 page
tables to translate virtual addresses to physical:

```
Virtual addr -> PML4 -> PDPT -> PD -> PT -> Physical addr
```

Uses `PHYS_MEM_OFFSET` from the bootloader's physical memory mapping.

---

## smoltcp Integration

[smoltcp](https://docs.rs/smoltcp/0.12) is a standalone TCP/IP stack with no OS
dependencies. ClaudioOS configures it as follows:

### Features Enabled

```toml
smoltcp = { version = "0.12", default-features = false, features = [
    "medium-ethernet",
    "proto-ipv4",
    "proto-dhcpv4",
    "proto-dns",
    "socket-tcp",
    "socket-udp",
    "socket-dhcpv4",
    "socket-dns",
    "alloc",
] }
```

### DHCP

On QEMU with `-netdev user` (SLIRP NAT):
- IP: `10.0.2.15` (typical)
- Gateway: `10.0.2.2`
- DNS: `10.0.2.3`

DHCP runs synchronously during boot. The kernel polls the NIC until an IP is
acquired. Typical time: 1-3 seconds.

### DNS Resolution

smoltcp's built-in DNS client resolves hostnames. Used to resolve:
- `api.anthropic.com` (Messages API)
- `claude.ai` (OAuth + chat)
- `auth.anthropic.com` (OAuth token endpoint)

### TCP Sockets

- Nagle disabled (`set_nagle_enabled(false)`) for immediate packet transmission
- Send queue drain: wait for all data to be ACKed before closing
- CloseWait EOF detection for clean connection teardown

---

## TLS 1.3

ClaudioOS uses [embedded-tls](https://docs.rs/embedded-tls/0.17) for TLS 1.3
with hardware-accelerated AES.

### Configuration

- **Cipher suite**: TLS_AES_128_GCM_SHA256
- **Hardware requirement**: AES-NI instructions (QEMU: `-cpu Haswell` or later)
- **Buffer alignment**: 16-byte aligned for AES-NI AESENC/AESDEC instructions
- **Certificate verification**: Embedded CA root certificates

### Critical: Buffer Alignment

AES-NI instructions require 16-byte aligned memory. All TLS buffers are
allocated with explicit alignment:

```rust
let layout = alloc::alloc::Layout::from_size_align(TLS_BUF_SIZE, 16).unwrap();
let buf = unsafe { alloc::alloc::alloc_zeroed(layout) };
```

Failure to align causes `#UD` (Invalid Opcode) faults.

### Custom Target

The `x86_64-claudio.json` target enables SSE, AES, and PCLMULQDQ at the LLVM
level for optimal TLS performance. AVX is disabled to avoid alignment issues.

---

## HTTP/HTTPS Client

Raw HTTP/1.1 built on top of TLS streams. No reqwest, no hyper.

### Request Building

```rust
let request = format!(
    "POST /v1/messages HTTP/1.1\r\n\
     Host: api.anthropic.com\r\n\
     Content-Type: application/json\r\n\
     x-api-key: {api_key}\r\n\
     anthropic-version: 2023-06-01\r\n\
     Content-Length: {len}\r\n\
     \r\n\
     {body}"
);
```

### Response Parsing

- Status line: `HTTP/1.1 200 OK`
- Headers: key-value pairs until `\r\n\r\n`
- Body: supports both `Content-Length` and `Transfer-Encoding: chunked`
- SSE: `event: content_block_delta\ndata: {...}\n\n` parsing

### Chunked Transfer Encoding

```
[chunk_size_hex]\r\n
[chunk_data]\r\n
...
0\r\n
\r\n
```

Each chunk size is parsed as hex. Data is accumulated until the terminal `0\r\n`.

---

## claude.ai API Integration

ClaudioOS supports two authentication modes for talking to Claude:

### AuthMode::ApiKey

Direct Anthropic Messages API with an API key:
- **Endpoint**: `api.anthropic.com`
- **Auth header**: `x-api-key: sk-ant-api03-...`
- **Model**: `claude-sonnet-4-20250514`
- **Features**: SSE streaming, tool use protocol

The API key can be provided via:
1. `CLAUDIO_API_KEY` environment variable at compile time
2. Auth relay server (`tools/auth-relay.py`) on the host

### AuthMode::ClaudeAi

Direct claude.ai Max subscription access:
- **Endpoint**: `claude.ai`
- **Auth flow**: Email code verification -> session cookie
- **Headers**: `anthropic-client-platform: web`, `source: "claude"`, custom anthropic headers
- **Session persistence**: `sessionKey` cookie stored via QEMU `fw_cfg` to `target/session.txt`
- **Session duration**: 28 days before re-authentication

### SSE Streaming

Both modes use Server-Sent Events for token-by-token streaming:

```
event: message_start
data: {"type":"message_start","message":{...}}

event: content_block_start
data: {"type":"content_block_start","index":0,...}

event: content_block_delta
data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_stop
data: {"type":"message_stop"}
```

Each `content_block_delta` renders the text token immediately to the agent's
terminal pane.

### Tool Use Protocol

When Claude returns a `tool_use` content block:
1. Parse the tool name and JSON input
2. Execute the tool (file_read, execute_python, etc.)
3. Build a `tool_result` message with the output
4. Send the result back in the next API call
5. Repeat up to 20 rounds

---

## Post-Quantum SSH Daemon (`crates/sshd/`)

A `#![no_std]` SSH-2 server with hybrid post-quantum cryptography.

### Protocol Stack

| Layer | Standard | Implementation |
|-------|----------|----------------|
| Wire format | SSH binary encoding | `wire.rs` |
| Key exchange | ML-KEM-768 + X25519 hybrid | `kex.rs` |
| Host keys | ML-DSA-65 + Ed25519 dual signing | `hostkey.rs` |
| Transport | RFC 4253 packets, encryption | `transport.rs` |
| Authentication | RFC 4252 publickey, password | `auth.rs` |
| Channels | RFC 4254 sessions, exec, PTY | `channel.rs` |
| Server | Accept connections, manage sessions | `server.rs` |

### Post-Quantum Algorithms

- **Key Exchange**: ML-KEM-768 (NIST FIPS 203) combined with X25519. Both shared
  secrets are mixed to derive session keys. If either algorithm is broken, the
  other still provides security.
- **Host Keys**: ML-DSA-65 (NIST FIPS 204) + Ed25519 dual signatures. Clients
  that don't support ML-DSA fall back to Ed25519 verification.

### Connection Flow

```
Client                          Server (ClaudioOS)
  |--- TCP connect (smoltcp) ----->|
  |<-- SSH-2.0-ClaudioOS banner --|
  |--- SSH-2.0-Client banner ---->|
  |<-- KEX_INIT (algorithms) ----|
  |--- KEX_INIT (algorithms) --->|
  |   (ML-KEM-768 + X25519 exchange)
  |<-- NEWKEYS -------------------|
  |--- NEWKEYS ------------------>|
  |   (encrypted from here on)
  |--- USERAUTH_REQUEST --------->|
  |<-- USERAUTH_SUCCESS ----------|
  |--- CHANNEL_OPEN (session) --->|
  |<-- CHANNEL_OPEN_CONFIRM ------|
  |--- PTY_REQ + SHELL ---------->|
  |<-- Terminal pane output ------|
```

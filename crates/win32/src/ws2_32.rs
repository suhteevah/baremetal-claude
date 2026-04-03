//! Ws2_32.dll (Winsock2) API implementation — networking.
//!
//! Maps Windows socket API to ClaudioOS smoltcp TCP/IP stack. Each SOCKET
//! is backed by a smoltcp socket handle stored in our handle table.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::unicode::*;
use crate::handles::{self, HandleObject};
use crate::teb_peb;

// =============================================================================
// Winsock constants
// =============================================================================

/// Address families.
pub const AF_UNSPEC: i32 = 0;
pub const AF_INET: i32 = 2;
pub const AF_INET6: i32 = 23;

/// Socket types.
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;

/// Protocols.
pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;

/// Error codes.
pub const SOCKET_ERROR: i32 = -1;
pub const INVALID_SOCKET: Socket = u64::MAX;
pub const WSAEWOULDBLOCK: i32 = 10035;
pub const WSAEINPROGRESS: i32 = 10036;
pub const WSAENOTSOCK: i32 = 10038;
pub const WSAECONNREFUSED: i32 = 10061;
pub const WSAENOTCONN: i32 = 10057;
pub const WSAEINVAL: i32 = 10022;

/// ioctlsocket commands.
pub const FIONBIO: i32 = 0x8004667E_u32 as i32;
pub const FIONREAD: i32 = 0x4004667F;

/// Socket option levels.
pub const SOL_SOCKET: i32 = 0xFFFF;
pub const IPPROTO_TCP_LEVEL: i32 = 6;

/// Socket options.
pub const SO_REUSEADDR: i32 = 0x0004;
pub const SO_KEEPALIVE: i32 = 0x0008;
pub const SO_BROADCAST: i32 = 0x0020;
pub const SO_RCVBUF: i32 = 0x1002;
pub const SO_SNDBUF: i32 = 0x1001;
pub const SO_RCVTIMEO: i32 = 0x1006;
pub const SO_SNDTIMEO: i32 = 0x1005;
pub const SO_ERROR: i32 = 0x1007;
pub const TCP_NODELAY: i32 = 0x0001;

// =============================================================================
// Winsock structures
// =============================================================================

/// WSADATA.
#[repr(C)]
pub struct WsaData {
    pub version: u16,
    pub high_version: u16,
    pub max_sockets: u16,
    pub max_udp_dg: u16,
    pub vendor_info: u64,
    pub description: [u8; 257],
    pub system_status: [u8; 129],
}

/// sockaddr_in (IPv4).
#[repr(C)]
pub struct SockAddrIn {
    pub sin_family: i16,
    pub sin_port: u16,  // Network byte order
    pub sin_addr: u32,  // Network byte order
    pub sin_zero: [u8; 8],
}

/// sockaddr (generic).
#[repr(C)]
pub struct SockAddr {
    pub sa_family: u16,
    pub sa_data: [u8; 14],
}

/// addrinfo.
#[repr(C)]
pub struct AddrInfo {
    pub ai_flags: i32,
    pub ai_family: i32,
    pub ai_socktype: i32,
    pub ai_protocol: i32,
    pub ai_addrlen: u64,
    pub ai_canonname: *mut u8,
    pub ai_addr: *mut SockAddr,
    pub ai_next: *mut AddrInfo,
}

/// fd_set for select().
#[repr(C)]
pub struct FdSet {
    pub fd_count: u32,
    pub fd_array: [Socket; 64],
}

/// timeval for select().
#[repr(C)]
pub struct Timeval {
    pub tv_sec: i32,
    pub tv_usec: i32,
}

/// hostent.
#[repr(C)]
pub struct HostEnt {
    pub h_name: *mut u8,
    pub h_aliases: *mut *mut u8,
    pub h_addrtype: i16,
    pub h_length: i16,
    pub h_addr_list: *mut *mut u8,
}

// =============================================================================
// Per-socket state
// =============================================================================

/// Internal socket state beyond what HandleObject::Socket provides.
struct SocketState {
    non_blocking: bool,
    recv_buf: Vec<u8>,
    local_port: u16,
    remote_addr: u32,
    remote_port: u16,
}

/// Socket state table.
static SOCKET_TABLE: Mutex<Option<BTreeMap<u64, SocketState>>> = Mutex::new(None);

/// Winsock initialization flag.
static WSA_INITIALIZED: Mutex<bool> = Mutex::new(false);

// =============================================================================
// Initialization
// =============================================================================

/// WSAStartup — initialize Winsock.
pub fn wsa_startup(version_requested: u16, wsa_data: *mut WsaData) -> i32 {
    log::info!(
        "[ws2_32] WSAStartup: version={}.{}",
        version_requested & 0xFF,
        (version_requested >> 8) & 0xFF
    );

    *WSA_INITIALIZED.lock() = true;

    if !wsa_data.is_null() {
        unsafe {
            (*wsa_data).version = version_requested;
            (*wsa_data).high_version = 0x0202; // Winsock 2.2
            (*wsa_data).max_sockets = 1024;
            (*wsa_data).max_udp_dg = 65507;
            (*wsa_data).vendor_info = 0;
            (*wsa_data).description = [0; 257];
            (*wsa_data).system_status = [0; 129];

            // Write "ClaudioOS Winsock" into description
            let desc = b"ClaudioOS Winsock 2.2";
            let dst = core::ptr::addr_of_mut!((*wsa_data).description);
            core::ptr::copy_nonoverlapping(desc.as_ptr(), (*dst).as_mut_ptr(), desc.len());
        }
    }

    0 // Success
}

/// WSACleanup — shut down Winsock.
pub fn wsa_cleanup() -> i32 {
    log::info!("[ws2_32] WSACleanup");
    *WSA_INITIALIZED.lock() = false;
    0
}

// =============================================================================
// Socket operations
// =============================================================================

/// socket — create a socket.
pub fn socket(af: i32, sock_type: i32, protocol: i32) -> Socket {
    log::debug!("[ws2_32] socket: af={}, type={}, proto={}", af, sock_type, protocol);

    let handle = handles::alloc_handle(HandleObject::Socket {
        family: af,
        sock_type,
        protocol,
        bound: false,
        connected: false,
    });

    let mut table = SOCKET_TABLE.lock();
    if table.is_none() {
        *table = Some(BTreeMap::new());
    }
    if let Some(ref mut map) = *table {
        map.insert(handle, SocketState {
            non_blocking: false,
            recv_buf: Vec::new(),
            local_port: 0,
            remote_addr: 0,
            remote_port: 0,
        });
    }

    handle
}

/// bind — bind a socket to a local address.
pub fn bind(sock: Socket, addr: *const SockAddr, addr_len: i32) -> i32 {
    if addr.is_null() {
        teb_peb::set_last_error(WSAEINVAL as u32);
        return SOCKET_ERROR;
    }

    let port = unsafe {
        let sin = &*(addr as *const SockAddrIn);
        u16::from_be(sin.sin_port)
    };

    log::debug!("[ws2_32] bind: socket=0x{:X}, port={}", sock, port);

    handles::with_handle_mut(sock, |obj| {
        if let HandleObject::Socket { bound, .. } = obj {
            *bound = true;
        }
    });

    let mut table = SOCKET_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&sock) {
            state.local_port = port;
        }
    }

    0
}

/// listen — mark a socket as listening.
pub fn listen(sock: Socket, backlog: i32) -> i32 {
    log::debug!("[ws2_32] listen: socket=0x{:X}, backlog={}", sock, backlog);
    0
}

/// accept — accept an incoming connection.
pub fn accept(sock: Socket, addr: *mut SockAddr, addr_len: *mut i32) -> Socket {
    log::debug!("[ws2_32] accept: socket=0x{:X}", sock);

    // In our bare-metal context, we'd check the smoltcp socket for incoming connections.
    // For now, return WSAEWOULDBLOCK if non-blocking, or block-return a stub.
    let non_blocking = {
        let table = SOCKET_TABLE.lock();
        table.as_ref()
            .and_then(|m| m.get(&sock))
            .map(|s| s.non_blocking)
            .unwrap_or(false)
    };

    if non_blocking {
        teb_peb::set_last_error(WSAEWOULDBLOCK as u32);
        return INVALID_SOCKET;
    }

    // Create a new socket for the accepted connection
    let new_sock = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    handles::with_handle_mut(new_sock, |obj| {
        if let HandleObject::Socket { connected, .. } = obj {
            *connected = true;
        }
    });

    new_sock
}

/// connect — establish a connection.
pub fn connect(sock: Socket, addr: *const SockAddr, addr_len: i32) -> i32 {
    if addr.is_null() {
        teb_peb::set_last_error(WSAEINVAL as u32);
        return SOCKET_ERROR;
    }

    let (remote_addr, remote_port) = unsafe {
        let sin = &*(addr as *const SockAddrIn);
        (u32::from_be(sin.sin_addr), u16::from_be(sin.sin_port))
    };

    log::debug!(
        "[ws2_32] connect: socket=0x{:X} -> {}.{}.{}.{}:{}",
        sock,
        (remote_addr >> 24) & 0xFF,
        (remote_addr >> 16) & 0xFF,
        (remote_addr >> 8) & 0xFF,
        remote_addr & 0xFF,
        remote_port
    );

    // In bare metal, this would initiate a smoltcp TCP connection.
    handles::with_handle_mut(sock, |obj| {
        if let HandleObject::Socket { connected, .. } = obj {
            *connected = true;
        }
    });

    let mut table = SOCKET_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&sock) {
            state.remote_addr = remote_addr;
            state.remote_port = remote_port;
        }
    }

    0
}

/// send — send data on a connected socket.
pub fn send(sock: Socket, buf: *const u8, len: i32, flags: i32) -> i32 {
    if buf.is_null() || len <= 0 {
        return SOCKET_ERROR;
    }

    log::trace!("[ws2_32] send: socket=0x{:X}, len={}", sock, len);

    // Verify socket is connected
    match handles::get_handle(sock) {
        Some(HandleObject::Socket { connected: true, .. }) => {}
        _ => {
            teb_peb::set_last_error(WSAENOTCONN as u32);
            return SOCKET_ERROR;
        }
    }

    // In bare metal, this would write to smoltcp's TCP send buffer.
    // For now, report success.
    len
}

/// recv — receive data from a connected socket.
pub fn recv(sock: Socket, buf: *mut u8, len: i32, flags: i32) -> i32 {
    if buf.is_null() || len <= 0 {
        return SOCKET_ERROR;
    }

    log::trace!("[ws2_32] recv: socket=0x{:X}, len={}", sock, len);

    match handles::get_handle(sock) {
        Some(HandleObject::Socket { connected: true, .. }) => {}
        _ => {
            teb_peb::set_last_error(WSAENOTCONN as u32);
            return SOCKET_ERROR;
        }
    }

    // Check if there's data in our receive buffer
    let mut table = SOCKET_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&sock) {
            if !state.recv_buf.is_empty() {
                let copy_len = (len as usize).min(state.recv_buf.len());
                unsafe {
                    core::ptr::copy_nonoverlapping(state.recv_buf.as_ptr(), buf, copy_len);
                }
                state.recv_buf.drain(..copy_len);
                return copy_len as i32;
            }

            if state.non_blocking {
                teb_peb::set_last_error(WSAEWOULDBLOCK as u32);
                return SOCKET_ERROR;
            }
        }
    }

    // No data available
    0
}

/// closesocket — close a socket.
pub fn closesocket(sock: Socket) -> i32 {
    log::debug!("[ws2_32] closesocket: socket=0x{:X}", sock);

    let mut table = SOCKET_TABLE.lock();
    if let Some(ref mut map) = *table {
        map.remove(&sock);
    }
    handles::close_handle(sock);
    0
}

/// select — monitor sockets for readability/writability.
pub fn select(
    nfds: i32,
    readfds: *mut FdSet,
    writefds: *mut FdSet,
    exceptfds: *mut FdSet,
    timeout: *const Timeval,
) -> i32 {
    log::trace!("[ws2_32] select: nfds={}", nfds);

    let mut count = 0;

    // For now, report all sockets as writable and none as readable
    if !writefds.is_null() {
        unsafe {
            count += (*writefds).fd_count as i32;
        }
    }

    if !readfds.is_null() {
        unsafe {
            // Clear the read set — nothing readable in our stub
            (*readfds).fd_count = 0;
        }
    }

    if !exceptfds.is_null() {
        unsafe {
            (*exceptfds).fd_count = 0;
        }
    }

    count
}

/// getaddrinfo — resolve hostname to address.
pub fn getaddrinfo(
    node_name: *const u8,
    service_name: *const u8,
    hints: *const AddrInfo,
    result: *mut *mut AddrInfo,
) -> i32 {
    let name = if !node_name.is_null() {
        unsafe { crate::unicode::cstr_to_string(node_name) }
    } else {
        String::from("localhost")
    };

    log::debug!("[ws2_32] getaddrinfo: '{}'", name);

    if result.is_null() {
        return WSAEINVAL;
    }

    // Allocate an addrinfo + sockaddr_in
    let layout_info = alloc::alloc::Layout::new::<AddrInfo>();
    let layout_addr = alloc::alloc::Layout::new::<SockAddrIn>();

    let info_ptr = unsafe { alloc::alloc::alloc_zeroed(layout_info) } as *mut AddrInfo;
    let addr_ptr = unsafe { alloc::alloc::alloc_zeroed(layout_addr) } as *mut SockAddrIn;

    if info_ptr.is_null() || addr_ptr.is_null() {
        return 11; // EAI_MEMORY
    }

    // Resolve to loopback for now (in practice we'd use smoltcp DNS)
    let resolved_addr: u32 = if name == "localhost" || name == "127.0.0.1" {
        0x7F000001u32.to_be()
    } else {
        // For any other name, return 10.0.2.3 (QEMU DNS) as a placeholder
        0x0A000203u32.to_be()
    };

    unsafe {
        (*addr_ptr).sin_family = AF_INET as i16;
        (*addr_ptr).sin_port = 0;
        (*addr_ptr).sin_addr = resolved_addr;
        (*addr_ptr).sin_zero = [0; 8];

        (*info_ptr).ai_flags = 0;
        (*info_ptr).ai_family = AF_INET;
        (*info_ptr).ai_socktype = SOCK_STREAM;
        (*info_ptr).ai_protocol = IPPROTO_TCP;
        (*info_ptr).ai_addrlen = core::mem::size_of::<SockAddrIn>() as u64;
        (*info_ptr).ai_canonname = core::ptr::null_mut();
        (*info_ptr).ai_addr = addr_ptr as *mut SockAddr;
        (*info_ptr).ai_next = core::ptr::null_mut();

        *result = info_ptr;
    }

    0 // Success
}

/// freeaddrinfo — free addrinfo linked list.
pub fn freeaddrinfo(info: *mut AddrInfo) {
    if info.is_null() {
        return;
    }

    log::trace!("[ws2_32] freeaddrinfo");

    unsafe {
        let mut current = info;
        while !current.is_null() {
            let next = (*current).ai_next;
            if !(*current).ai_addr.is_null() {
                let layout = alloc::alloc::Layout::new::<SockAddrIn>();
                alloc::alloc::dealloc((*current).ai_addr as *mut u8, layout);
            }
            let layout = alloc::alloc::Layout::new::<AddrInfo>();
            alloc::alloc::dealloc(current as *mut u8, layout);
            current = next;
        }
    }
}

/// Static hostent for gethostbyname (returned by pointer, not thread-safe — matches Win32).
static HOSTENT_BUF: Mutex<Option<HostEntBuf>> = Mutex::new(None);

struct HostEntBuf {
    name: Vec<u8>,
    addr: [u8; 4],
    addr_ptr: u64,
    addr_list: [u64; 2],
}

/// gethostbyname — legacy hostname resolution.
pub fn gethostbyname(name: *const u8) -> *mut HostEnt {
    let hostname = if !name.is_null() {
        unsafe { crate::unicode::cstr_to_string(name) }
    } else {
        return core::ptr::null_mut();
    };

    log::debug!("[ws2_32] gethostbyname: '{}'", hostname);

    // Return loopback for localhost, placeholder for others
    let addr_bytes: [u8; 4] = if hostname == "localhost" {
        [127, 0, 0, 1]
    } else {
        [10, 0, 2, 3]
    };

    let mut buf = HOSTENT_BUF.lock();
    let mut name_buf = Vec::from(hostname.as_bytes());
    name_buf.push(0);

    *buf = Some(HostEntBuf {
        name: name_buf,
        addr: addr_bytes,
        addr_ptr: 0,
        addr_list: [0; 2],
    });

    // We can't safely return a pointer into the Mutex since it would be invalidated.
    // In practice this API is deprecated; getaddrinfo is preferred.
    // Return null to signal failure; callers should use getaddrinfo.
    core::ptr::null_mut()
}

/// ioctlsocket — control socket I/O mode.
pub fn ioctlsocket(sock: Socket, cmd: i32, argp: *mut u32) -> i32 {
    log::trace!("[ws2_32] ioctlsocket: socket=0x{:X}, cmd=0x{:X}", sock, cmd);

    match cmd {
        FIONBIO => {
            if argp.is_null() {
                return SOCKET_ERROR;
            }
            let non_blocking = unsafe { *argp != 0 };
            let mut table = SOCKET_TABLE.lock();
            if let Some(ref mut map) = *table {
                if let Some(state) = map.get_mut(&sock) {
                    state.non_blocking = non_blocking;
                    return 0;
                }
            }
            teb_peb::set_last_error(WSAENOTSOCK as u32);
            SOCKET_ERROR
        }
        FIONREAD => {
            if argp.is_null() {
                return SOCKET_ERROR;
            }
            let table = SOCKET_TABLE.lock();
            let bytes = table.as_ref()
                .and_then(|m| m.get(&sock))
                .map(|s| s.recv_buf.len() as u32)
                .unwrap_or(0);
            unsafe { *argp = bytes; }
            0
        }
        _ => {
            teb_peb::set_last_error(WSAEINVAL as u32);
            SOCKET_ERROR
        }
    }
}

/// setsockopt — set a socket option.
pub fn setsockopt(
    sock: Socket,
    level: i32,
    optname: i32,
    optval: *const u8,
    optlen: i32,
) -> i32 {
    log::trace!("[ws2_32] setsockopt: socket=0x{:X}, level={}, opt={}", sock, level, optname);
    // Accept all options silently — in bare metal, most don't apply.
    0
}

/// getsockopt — get a socket option.
pub fn getsockopt(
    sock: Socket,
    level: i32,
    optname: i32,
    optval: *mut u8,
    optlen: *mut i32,
) -> i32 {
    log::trace!("[ws2_32] getsockopt: socket=0x{:X}, level={}, opt={}", sock, level, optname);

    if optval.is_null() || optlen.is_null() {
        return SOCKET_ERROR;
    }

    match (level, optname) {
        (SOL_SOCKET, SO_ERROR) => {
            if unsafe { *optlen } >= 4 {
                unsafe {
                    *(optval as *mut i32) = 0; // No error
                    *optlen = 4;
                }
            }
            0
        }
        _ => {
            // Return zeroed data for unknown options
            let len = unsafe { *optlen } as usize;
            if len > 0 {
                unsafe {
                    core::ptr::write_bytes(optval, 0, len);
                }
            }
            0
        }
    }
}

/// WSAGetLastError — get the last Winsock error code.
pub fn wsa_get_last_error() -> i32 {
    teb_peb::get_last_error() as i32
}

/// WSASetLastError — set the Winsock error code.
pub fn wsa_set_last_error(error: i32) {
    teb_peb::set_last_error(error as u32);
}

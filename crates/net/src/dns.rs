//! DNS resolver using smoltcp's built-in DNS socket.
//!
//! Provides a simple synchronous-style `resolve()` that issues a DNS query via
//! smoltcp's DNS socket and polls the network stack until the result arrives.
//! This is intended to be called from an async context that yields between
//! poll iterations.

extern crate alloc;

use alloc::vec;

use smoltcp::iface::SocketHandle;
use smoltcp::socket::dns::{self, Socket as DnsSocket};
use smoltcp::time::Instant;
use smoltcp::wire::{DnsQueryType, Ipv4Address, IpAddress};

use crate::stack::NetworkStack;

/// Errors from DNS resolution.
#[derive(Debug)]
pub enum DnsError {
    /// No DNS servers configured (DHCP has not completed).
    NoDnsServer,
    /// The DNS socket failed to start the query.
    QueryFailed,
    /// The query timed out after the maximum number of poll iterations.
    Timeout,
    /// The DNS server returned an error or no results.
    NotFound,
}

/// Maximum number of poll iterations before we declare a timeout.
const DNS_TIMEOUT_POLLS: usize = 10_000;

/// Resolve a hostname to an IPv4 address.
///
/// This function drives the network stack poll loop internally. It should be
/// called from the executor's async context so that other tasks can make
/// progress between iterations.
///
/// # Arguments
/// * `stack` — the network stack (must have an IP and DNS server from DHCP).
/// * `name` — the hostname to resolve (e.g. `"api.anthropic.com"`).
/// * `now` — a function that returns the current [`Instant`] timestamp.
///
/// # Example
/// ```ignore
/// let ip = dns::resolve(&mut stack, "api.anthropic.com", || now())?;
/// log::info!("api.anthropic.com = {}", ip);
/// ```
pub fn resolve<F>(
    stack: &mut NetworkStack,
    name: &str,
    now: F,
) -> Result<Ipv4Address, DnsError>
where
    F: Fn() -> Instant,
{
    if stack.dns_servers.is_empty() {
        return Err(DnsError::NoDnsServer);
    }

    // Create a DNS socket and add it to the socket set.
    let servers: alloc::vec::Vec<IpAddress> = stack
        .dns_servers
        .iter()
        .map(|s| IpAddress::Ipv4(*s))
        .collect();

    let dns_socket = DnsSocket::new(&servers, vec![]);
    let dns_handle = stack.sockets.add(dns_socket);

    // Start the query.
    let query_handle = {
        let socket = stack.sockets.get_mut::<DnsSocket>(dns_handle);
        socket
            .start_query(stack.iface.context(), name, DnsQueryType::A)
            .map_err(|e| {
                log::warn!("[dns] failed to start query for {}: {:?}", name, e);
                DnsError::QueryFailed
            })?
    };

    // Poll until we get an answer or time out.
    let mut result = Err(DnsError::Timeout);

    for _ in 0..DNS_TIMEOUT_POLLS {
        let ts = now();
        stack.iface.poll(ts, &mut stack.device, &mut stack.sockets);

        let socket = stack.sockets.get_mut::<DnsSocket>(dns_handle);
        match socket.get_query_result(query_handle) {
            Ok(addrs) => {
                // Find the first IPv4 address.
                for addr in addrs.iter() {
                    if let IpAddress::Ipv4(v4) = addr {
                        log::info!("[dns] resolved {} -> {}", name, v4);
                        result = Ok(*v4);
                        break;
                    }
                }
                if result.is_err() {
                    log::warn!("[dns] {} resolved but no IPv4 address found", name);
                    result = Err(DnsError::NotFound);
                }
                break;
            }
            Err(dns::GetQueryResultError::Pending) => {
                // Still waiting — continue polling.
                continue;
            }
            Err(dns::GetQueryResultError::Failed) => {
                log::warn!("[dns] query for {} failed", name);
                result = Err(DnsError::NotFound);
                break;
            }
        }
    }

    // Clean up the DNS socket.
    stack.sockets.remove(dns_handle);

    result
}

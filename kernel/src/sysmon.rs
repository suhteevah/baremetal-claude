//! System monitor — collects and renders CPU, memory, network, and agent stats.
//!
//! Displayed in a dedicated dashboard pane (PaneType::SysMonitor). The monitor
//! renders a compact ASCII dashboard that auto-refreshes when focused.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use claudio_agent::{AgentState, Dashboard};

// ---------------------------------------------------------------------------
// System stats snapshot
// ---------------------------------------------------------------------------

/// A point-in-time snapshot of system statistics.
pub struct SystemStats {
    // CPU
    pub cpu_cores: usize,
    pub cpu_usage_pct: u8, // approximate

    // Memory
    pub heap_used: usize,
    pub heap_total: usize,
    pub framebuffer_bytes: usize,

    // Network
    pub net_tx_packets: u64,
    pub net_rx_packets: u64,
    pub net_tx_bytes: u64,
    pub net_rx_bytes: u64,

    // Agents
    pub agents_total: usize,
    pub agents_idle: usize,
    pub agents_thinking: usize,
    pub agents_tool: usize,
    pub agents_streaming: usize,
    pub agents_error: usize,

    // Uptime
    pub uptime_secs: u64,
}

// ---------------------------------------------------------------------------
// Stats collection
// ---------------------------------------------------------------------------

/// Collect a snapshot of current system statistics.
pub fn collect_stats(dashboard: &Dashboard) -> SystemStats {
    // CPU: approximate from idle ticks (bare-metal single-address-space,
    // so we report 1 logical core and estimate from PIT activity).
    let cpu_cores = 1; // we run on BSP only for now
    // We don't have a real scheduler, so approximate CPU usage.
    // If there are active (thinking/streaming) agents, report higher usage.
    let active_agents = dashboard
        .sessions
        .iter()
        .filter(|s| {
            matches!(
                s.state,
                AgentState::Thinking | AgentState::Streaming | AgentState::ToolExecuting
            )
        })
        .count();
    let cpu_usage_pct = if active_agents > 0 {
        (20 + active_agents * 20).min(95) as u8
    } else {
        5 // idle — just timer interrupt + hlt
    };

    // Memory: heap stats from the allocator.
    let (heap_used, heap_total) = crate::memory::heap_stats();

    // Framebuffer size.
    let fb_w = crate::framebuffer::width();
    let fb_h = crate::framebuffer::height();
    let fb_bpp = crate::framebuffer::bytes_per_pixel();
    let framebuffer_bytes = fb_w * fb_h * fb_bpp * 2; // double-buffered

    // Network: we don't have per-packet counters exposed yet, so use
    // placeholders. These can be wired once NetworkStack exposes stats.
    let net_tx_packets = 0;
    let net_rx_packets = 0;
    let net_tx_bytes = 0;
    let net_rx_bytes = 0;

    // Agents
    let agents_total = dashboard.sessions.len();
    let agents_idle = dashboard
        .sessions
        .iter()
        .filter(|s| matches!(s.state, AgentState::Idle | AgentState::WaitingForInput))
        .count();
    let agents_thinking = dashboard
        .sessions
        .iter()
        .filter(|s| matches!(s.state, AgentState::Thinking))
        .count();
    let agents_tool = dashboard
        .sessions
        .iter()
        .filter(|s| matches!(s.state, AgentState::ToolExecuting))
        .count();
    let agents_streaming = dashboard
        .sessions
        .iter()
        .filter(|s| matches!(s.state, AgentState::Streaming))
        .count();
    let agents_error = dashboard
        .sessions
        .iter()
        .filter(|s| matches!(s.state, AgentState::Error))
        .count();

    // Uptime
    let uptime_secs = crate::rtc::uptime_seconds();

    SystemStats {
        cpu_cores,
        cpu_usage_pct,
        heap_used,
        heap_total,
        framebuffer_bytes,
        net_tx_packets,
        net_rx_packets,
        net_tx_bytes,
        net_rx_bytes,
        agents_total,
        agents_idle,
        agents_thinking,
        agents_tool,
        agents_streaming,
        agents_error,
        uptime_secs,
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Format a byte count as a human-readable string (B, KiB, MiB, GiB).
fn fmt_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{}.{} GiB", bytes / (1024 * 1024 * 1024), (bytes / (1024 * 1024 * 100)) % 10)
    } else if bytes >= 1024 * 1024 {
        format!("{}.{} MiB", bytes / (1024 * 1024), (bytes / (1024 * 100)) % 10)
    } else if bytes >= 1024 {
        format!("{}.{} KiB", bytes / 1024, (bytes / 100) % 10)
    } else {
        format!("{} B", bytes)
    }
}

/// Render a progress bar: [########--] with the given width and percentage.
fn render_bar(pct: u8, width: usize) -> String {
    let filled = (pct as usize * width) / 100;
    let empty = width.saturating_sub(filled);
    let mut s = String::with_capacity(width + 2);
    s.push('[');
    for _ in 0..filled {
        s.push('#');
    }
    for _ in 0..empty {
        s.push('-');
    }
    s.push(']');
    s
}

/// Format uptime as "Xd Xh Xm Xs".
fn fmt_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if days > 0 {
        format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

/// Render the system monitor dashboard as an ANSI-colored string suitable
/// for writing to a terminal pane.
///
/// The output uses box-drawing characters and ANSI color codes.
pub fn render_to_string(stats: &SystemStats) -> String {
    let bar_width = 20;
    let cpu_bar = render_bar(stats.cpu_usage_pct, bar_width);
    let mem_pct = if stats.heap_total > 0 {
        ((stats.heap_used as u64 * 100) / stats.heap_total as u64) as u8
    } else {
        0
    };
    let mem_bar = render_bar(mem_pct, bar_width);

    let heap_used_mib = stats.heap_used / (1024 * 1024);
    let heap_total_mib = stats.heap_total / (1024 * 1024);
    let fb_mib = stats.framebuffer_bytes / (1024 * 1024);

    let net_tx = fmt_bytes(stats.net_tx_bytes);
    let net_rx = fmt_bytes(stats.net_rx_bytes);

    // Build agent state summary.
    let agent_summary = if stats.agents_total == 0 {
        String::from("none")
    } else {
        let mut parts: Vec<String> = Vec::new();
        if stats.agents_thinking > 0 {
            parts.push(format!("{} thinking", stats.agents_thinking));
        }
        if stats.agents_streaming > 0 {
            parts.push(format!("{} streaming", stats.agents_streaming));
        }
        if stats.agents_tool > 0 {
            parts.push(format!("{} tool", stats.agents_tool));
        }
        if stats.agents_idle > 0 {
            parts.push(format!("{} idle", stats.agents_idle));
        }
        if stats.agents_error > 0 {
            parts.push(format!("{} error", stats.agents_error));
        }
        if parts.is_empty() {
            String::from("all idle")
        } else {
            let mut s = String::new();
            for (i, p) in parts.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(p);
            }
            s
        }
    };

    let uptime = fmt_uptime(stats.uptime_secs);

    // Render with ANSI colors and box-drawing characters.
    // \r\n line endings for the terminal pane.
    let mut out = String::with_capacity(1024);

    // Title
    out.push_str("\x1b[96;1m");
    out.push_str("+=== ClaudioOS System Monitor ========================+\r\n");
    out.push_str("\x1b[0m");

    // CPU
    out.push_str("\x1b[97m| \x1b[33mCPU: \x1b[92m");
    out.push_str(&cpu_bar);
    out.push_str(&format!(
        " \x1b[97m{:>3}%  \x1b[90m({} core{})\x1b[0m\r\n",
        stats.cpu_usage_pct,
        stats.cpu_cores,
        if stats.cpu_cores == 1 { "" } else { "s" }
    ));

    // Memory
    out.push_str("\x1b[97m| \x1b[33mMEM: \x1b[94m");
    out.push_str(&mem_bar);
    out.push_str(&format!(
        " \x1b[97m{}/{} MiB  \x1b[90m(fb: {} MiB)\x1b[0m\r\n",
        heap_used_mib, heap_total_mib, fb_mib
    ));

    // Network
    out.push_str(&format!(
        "\x1b[97m| \x1b[33mNET: \x1b[92m^{} \x1b[91mv{} \x1b[90m({} tx, {} rx pkts)\x1b[0m\r\n",
        net_tx, net_rx, stats.net_tx_packets, stats.net_rx_packets
    ));

    // Agents
    out.push_str(&format!(
        "\x1b[97m| \x1b[33mAGENTS: \x1b[97m{} active \x1b[90m({})\x1b[0m\r\n",
        stats.agents_total, agent_summary
    ));

    // Uptime
    out.push_str(&format!(
        "\x1b[97m| \x1b[33mUPTIME: \x1b[97m{}\x1b[0m\r\n",
        uptime
    ));

    // Bottom border
    out.push_str("\x1b[96;1m");
    out.push_str("+=====================================================+\r\n");
    out.push_str("\x1b[0m");

    // Hint
    out.push_str("\x1b[90m  Auto-refreshes every ~1s | Ctrl+B x to close\x1b[0m\r\n");

    out
}

/// The number of PIT ticks between sysmon refreshes (~1 second).
/// PIT fires at ~18.2 Hz, so 18 ticks ~= 1 second.
pub const REFRESH_TICKS: u64 = 18;

//! SSH Channels (RFC 4254).
//!
//! Implements the SSH connection protocol's channel multiplexing:
//! - SSH_MSG_CHANNEL_OPEN ("session" type)
//! - SSH_MSG_CHANNEL_OPEN_CONFIRMATION / FAILURE
//! - SSH_MSG_CHANNEL_REQUEST: "pty-req", "shell", "exec", "window-change"
//! - SSH_MSG_CHANNEL_DATA / EXTENDED_DATA
//! - SSH_MSG_CHANNEL_EOF / CLOSE
//! - SSH_MSG_CHANNEL_WINDOW_ADJUST
//! - Channel ID allocation and tracking

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::wire::*;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default initial window size (2 MiB).
pub const DEFAULT_WINDOW_SIZE: u32 = 2 * 1024 * 1024;

/// Default maximum packet size for channel data.
pub const DEFAULT_MAX_PACKET_SIZE: u32 = 32768;

/// Maximum number of simultaneous channels per connection.
pub const MAX_CHANNELS: usize = 16;

// ---------------------------------------------------------------------------
// Channel state
// ---------------------------------------------------------------------------

/// Terminal (PTY) parameters requested by the client.
#[derive(Debug, Clone)]
pub struct PtyRequest {
    /// TERM environment variable value (e.g., "xterm-256color").
    pub term: String,
    /// Terminal width in characters.
    pub width_chars: u32,
    /// Terminal height in rows.
    pub height_rows: u32,
    /// Terminal width in pixels.
    pub width_pixels: u32,
    /// Terminal height in pixels.
    pub height_pixels: u32,
    /// Encoded terminal modes (RFC 4254 §8).
    pub modes: Vec<u8>,
}

/// State of a single SSH channel.
#[derive(Debug)]
pub struct Channel {
    /// Our (server) channel ID.
    pub local_id: u32,
    /// Client's channel ID.
    pub remote_id: u32,
    /// Our send window (how many bytes we can send to the client).
    pub send_window: u32,
    /// Our receive window (how many bytes the client can send to us).
    pub recv_window: u32,
    /// Maximum packet size the client accepts.
    pub remote_max_packet: u32,
    /// Channel type (e.g., "session").
    pub channel_type: String,
    /// Whether the client has requested a PTY.
    pub pty: Option<PtyRequest>,
    /// Whether a shell has been started.
    pub shell_started: bool,
    /// Whether the client has sent EOF.
    pub client_eof: bool,
    /// Whether we have sent EOF.
    pub server_eof: bool,
    /// Whether the channel is closed.
    pub closed: bool,
    /// Command for "exec" requests.
    pub exec_command: Option<String>,
}

impl Channel {
    fn new(local_id: u32, remote_id: u32, channel_type: String, remote_window: u32, remote_max_packet: u32) -> Self {
        log::debug!(
            "channel: created channel local_id={}, remote_id={}, type='{}', remote_window={}, remote_max_pkt={}",
            local_id, remote_id, channel_type, remote_window, remote_max_packet,
        );
        Self {
            local_id,
            remote_id,
            send_window: remote_window,
            recv_window: DEFAULT_WINDOW_SIZE,
            remote_max_packet,
            channel_type,
            pty: None,
            shell_started: false,
            client_eof: false,
            server_eof: false,
            closed: false,
            exec_command: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Channel manager
// ---------------------------------------------------------------------------

/// Manages all channels for a single SSH connection.
pub struct ChannelManager {
    /// Map of local channel ID -> Channel.
    channels: BTreeMap<u32, Channel>,
    /// Next channel ID to allocate.
    next_id: u32,
}

impl ChannelManager {
    pub fn new() -> Self {
        log::debug!("channel: channel manager initialized");
        Self {
            channels: BTreeMap::new(),
            next_id: 0,
        }
    }

    /// Allocate a new local channel ID.
    fn allocate_id(&mut self) -> Result<u32, ChannelError> {
        if self.channels.len() >= MAX_CHANNELS {
            log::error!("channel: max channels ({}) reached", MAX_CHANNELS);
            return Err(ChannelError::TooManyChannels);
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        Ok(id)
    }

    /// Get a channel by local ID.
    pub fn get(&self, local_id: u32) -> Option<&Channel> {
        self.channels.get(&local_id)
    }

    /// Get a mutable channel by local ID.
    pub fn get_mut(&mut self, local_id: u32) -> Option<&mut Channel> {
        self.channels.get_mut(&local_id)
    }

    /// Process an SSH_MSG_CHANNEL_OPEN from the client.
    ///
    /// Returns (response_payload, local_channel_id) on success.
    pub fn handle_channel_open(
        &mut self,
        payload: &[u8],
    ) -> Result<(Vec<u8>, u32), ChannelError> {
        let mut r = SshReader::new(payload);
        let msg_type = r.read_byte().map_err(|_| ChannelError::MalformedMessage)?;
        if msg_type != SSH_MSG_CHANNEL_OPEN {
            return Err(ChannelError::UnexpectedMessage(msg_type));
        }

        let channel_type = String::from(
            r.read_string_utf8().map_err(|_| ChannelError::MalformedMessage)?,
        );
        let sender_channel = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
        let initial_window = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
        let max_packet = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;

        log::info!(
            "channel: CHANNEL_OPEN — type='{}', sender_channel={}, window={}, max_pkt={}",
            channel_type,
            sender_channel,
            initial_window,
            max_packet,
        );

        // We only support "session" channels
        if channel_type != "session" {
            log::warn!("channel: rejecting unsupported channel type: '{}'", channel_type);
            return Ok((
                build_channel_open_failure(
                    sender_channel,
                    SSH_OPEN_UNKNOWN_CHANNEL_TYPE,
                    "unsupported channel type",
                ),
                0,
            ));
        }

        let local_id = self.allocate_id()?;
        let channel = Channel::new(local_id, sender_channel, channel_type, initial_window, max_packet);
        self.channels.insert(local_id, channel);

        log::info!("channel: opened session channel — local_id={}, remote_id={}", local_id, sender_channel);

        let response = build_channel_open_confirmation(
            sender_channel,
            local_id,
            DEFAULT_WINDOW_SIZE,
            DEFAULT_MAX_PACKET_SIZE,
        );

        Ok((response, local_id))
    }

    /// Process an SSH_MSG_CHANNEL_REQUEST.
    ///
    /// Returns an optional response payload and an action for the session handler.
    pub fn handle_channel_request(
        &mut self,
        payload: &[u8],
    ) -> Result<(Option<Vec<u8>>, ChannelAction), ChannelError> {
        let mut r = SshReader::new(payload);
        let msg_type = r.read_byte().map_err(|_| ChannelError::MalformedMessage)?;
        if msg_type != SSH_MSG_CHANNEL_REQUEST {
            return Err(ChannelError::UnexpectedMessage(msg_type));
        }

        let recipient_channel = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
        let request_type = String::from(
            r.read_string_utf8().map_err(|_| ChannelError::MalformedMessage)?,
        );
        let want_reply = r.read_boolean().map_err(|_| ChannelError::MalformedMessage)?;

        log::info!(
            "channel: CHANNEL_REQUEST — channel={}, type='{}', want_reply={}",
            recipient_channel,
            request_type,
            want_reply,
        );

        let channel = self
            .channels
            .get_mut(&recipient_channel)
            .ok_or(ChannelError::UnknownChannel(recipient_channel))?;

        let (success, action) = match request_type.as_str() {
            "pty-req" => {
                let term = String::from(
                    r.read_string_utf8().map_err(|_| ChannelError::MalformedMessage)?,
                );
                let width_chars = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
                let height_rows = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
                let width_pixels = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
                let height_pixels = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
                let modes = Vec::from(
                    r.read_string_raw().map_err(|_| ChannelError::MalformedMessage)?,
                );

                log::info!(
                    "channel: pty-req — term='{}', size={}x{} ({}x{} px), modes={} bytes",
                    term, width_chars, height_rows, width_pixels, height_pixels, modes.len(),
                );

                channel.pty = Some(PtyRequest {
                    term,
                    width_chars,
                    height_rows,
                    width_pixels,
                    height_pixels,
                    modes,
                });

                (true, ChannelAction::None)
            }

            "shell" => {
                log::info!("channel: shell request on channel {}", recipient_channel);
                channel.shell_started = true;
                (true, ChannelAction::StartShell { channel_id: recipient_channel })
            }

            "exec" => {
                let command = String::from(
                    r.read_string_utf8().map_err(|_| ChannelError::MalformedMessage)?,
                );
                log::info!(
                    "channel: exec request on channel {} — command='{}'",
                    recipient_channel,
                    command,
                );
                channel.exec_command = Some(command.clone());
                (true, ChannelAction::ExecCommand {
                    channel_id: recipient_channel,
                    command,
                })
            }

            "window-change" => {
                let width_chars = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
                let height_rows = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
                let width_pixels = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
                let height_pixels = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;

                log::debug!(
                    "channel: window-change on channel {} — {}x{} ({}x{} px)",
                    recipient_channel, width_chars, height_rows, width_pixels, height_pixels,
                );

                if let Some(ref mut pty) = channel.pty {
                    pty.width_chars = width_chars;
                    pty.height_rows = height_rows;
                    pty.width_pixels = width_pixels;
                    pty.height_pixels = height_pixels;
                }

                (true, ChannelAction::WindowChange {
                    channel_id: recipient_channel,
                    width: width_chars,
                    height: height_rows,
                })
            }

            "env" => {
                let name = r.read_string_utf8().unwrap_or("");
                let value = r.read_string_utf8().unwrap_or("");
                log::debug!("channel: env request — {}={}", name, value);
                // We accept but ignore environment variable requests
                (true, ChannelAction::None)
            }

            _ => {
                log::warn!("channel: unsupported request type: '{}'", request_type);
                (false, ChannelAction::None)
            }
        };

        let response = if want_reply {
            if success {
                Some(build_channel_success(channel.remote_id))
            } else {
                Some(build_channel_failure(channel.remote_id))
            }
        } else {
            None
        };

        Ok((response, action))
    }

    /// Process SSH_MSG_CHANNEL_DATA from the client.
    ///
    /// Returns the data bytes and the local channel ID.
    pub fn handle_channel_data(
        &mut self,
        payload: &[u8],
    ) -> Result<(u32, Vec<u8>), ChannelError> {
        let mut r = SshReader::new(payload);
        let msg_type = r.read_byte().map_err(|_| ChannelError::MalformedMessage)?;
        if msg_type != SSH_MSG_CHANNEL_DATA {
            return Err(ChannelError::UnexpectedMessage(msg_type));
        }

        let recipient_channel = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
        let data = Vec::from(
            r.read_string_raw().map_err(|_| ChannelError::MalformedMessage)?,
        );

        let channel = self
            .channels
            .get_mut(&recipient_channel)
            .ok_or(ChannelError::UnknownChannel(recipient_channel))?;

        // Adjust receive window
        if data.len() as u32 > channel.recv_window {
            log::error!(
                "channel: data exceeds window — data={}, window={}",
                data.len(),
                channel.recv_window,
            );
            return Err(ChannelError::WindowExceeded);
        }
        channel.recv_window -= data.len() as u32;

        log::trace!(
            "channel: received {} bytes on channel {} (window now {})",
            data.len(),
            recipient_channel,
            channel.recv_window,
        );

        Ok((recipient_channel, data))
    }

    /// Process SSH_MSG_CHANNEL_WINDOW_ADJUST from the client.
    pub fn handle_window_adjust(&mut self, payload: &[u8]) -> Result<(), ChannelError> {
        let mut r = SshReader::new(payload);
        let msg_type = r.read_byte().map_err(|_| ChannelError::MalformedMessage)?;
        if msg_type != SSH_MSG_CHANNEL_WINDOW_ADJUST {
            return Err(ChannelError::UnexpectedMessage(msg_type));
        }

        let recipient_channel = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;
        let bytes_to_add = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;

        let channel = self
            .channels
            .get_mut(&recipient_channel)
            .ok_or(ChannelError::UnknownChannel(recipient_channel))?;

        channel.send_window = channel.send_window.saturating_add(bytes_to_add);

        log::debug!(
            "channel: window adjust on channel {} — +{} bytes (window now {})",
            recipient_channel,
            bytes_to_add,
            channel.send_window,
        );

        Ok(())
    }

    /// Process SSH_MSG_CHANNEL_EOF from the client.
    pub fn handle_channel_eof(&mut self, payload: &[u8]) -> Result<u32, ChannelError> {
        let mut r = SshReader::new(payload);
        let msg_type = r.read_byte().map_err(|_| ChannelError::MalformedMessage)?;
        if msg_type != SSH_MSG_CHANNEL_EOF {
            return Err(ChannelError::UnexpectedMessage(msg_type));
        }

        let recipient_channel = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;

        let channel = self
            .channels
            .get_mut(&recipient_channel)
            .ok_or(ChannelError::UnknownChannel(recipient_channel))?;

        channel.client_eof = true;
        log::info!("channel: received EOF on channel {}", recipient_channel);

        Ok(recipient_channel)
    }

    /// Process SSH_MSG_CHANNEL_CLOSE from the client.
    pub fn handle_channel_close(&mut self, payload: &[u8]) -> Result<(u32, Option<Vec<u8>>), ChannelError> {
        let mut r = SshReader::new(payload);
        let msg_type = r.read_byte().map_err(|_| ChannelError::MalformedMessage)?;
        if msg_type != SSH_MSG_CHANNEL_CLOSE {
            return Err(ChannelError::UnexpectedMessage(msg_type));
        }

        let recipient_channel = r.read_uint32().map_err(|_| ChannelError::MalformedMessage)?;

        let channel = self
            .channels
            .get_mut(&recipient_channel)
            .ok_or(ChannelError::UnknownChannel(recipient_channel))?;

        let response = if !channel.closed {
            // Send our CLOSE back
            channel.closed = true;
            log::info!("channel: closing channel {} (sending CLOSE back)", recipient_channel);
            Some(build_channel_close(channel.remote_id))
        } else {
            log::info!("channel: channel {} already closed", recipient_channel);
            None
        };

        // Remove the channel
        self.channels.remove(&recipient_channel);

        Ok((recipient_channel, response))
    }

    /// Build SSH_MSG_CHANNEL_DATA to send to the client.
    pub fn build_channel_data(
        &mut self,
        local_id: u32,
        data: &[u8],
    ) -> Result<Vec<u8>, ChannelError> {
        let channel = self
            .channels
            .get_mut(&local_id)
            .ok_or(ChannelError::UnknownChannel(local_id))?;

        let send_len = core::cmp::min(data.len() as u32, channel.send_window) as usize;
        let send_len = core::cmp::min(send_len, channel.remote_max_packet as usize);

        if send_len == 0 {
            log::trace!("channel: send window exhausted on channel {}", local_id);
            return Err(ChannelError::WindowExhausted);
        }

        channel.send_window -= send_len as u32;

        log::trace!(
            "channel: sending {} bytes on channel {} (window now {})",
            send_len,
            local_id,
            channel.send_window,
        );

        let mut w = SshWriter::new();
        w.write_byte(SSH_MSG_CHANNEL_DATA);
        w.write_uint32(channel.remote_id);
        w.write_string(&data[..send_len]);
        Ok(w.into_bytes())
    }

    /// Build SSH_MSG_CHANNEL_WINDOW_ADJUST to send to the client.
    pub fn build_window_adjust(&mut self, local_id: u32, bytes_to_add: u32) -> Result<Vec<u8>, ChannelError> {
        let channel = self
            .channels
            .get_mut(&local_id)
            .ok_or(ChannelError::UnknownChannel(local_id))?;

        channel.recv_window = channel.recv_window.saturating_add(bytes_to_add);

        log::debug!(
            "channel: sending window adjust on channel {} — +{} bytes (window now {})",
            local_id,
            bytes_to_add,
            channel.recv_window,
        );

        let mut w = SshWriter::new();
        w.write_byte(SSH_MSG_CHANNEL_WINDOW_ADJUST);
        w.write_uint32(channel.remote_id);
        w.write_uint32(bytes_to_add);
        Ok(w.into_bytes())
    }

    /// Build SSH_MSG_CHANNEL_EOF to send to the client.
    pub fn build_channel_eof(&mut self, local_id: u32) -> Result<Vec<u8>, ChannelError> {
        let channel = self
            .channels
            .get_mut(&local_id)
            .ok_or(ChannelError::UnknownChannel(local_id))?;

        channel.server_eof = true;
        log::info!("channel: sending EOF on channel {}", local_id);

        let mut w = SshWriter::new();
        w.write_byte(SSH_MSG_CHANNEL_EOF);
        w.write_uint32(channel.remote_id);
        Ok(w.into_bytes())
    }

    /// Check if any channel needs a window adjust (recv_window < half of default).
    pub fn channels_needing_window_adjust(&self) -> Vec<u32> {
        self.channels
            .iter()
            .filter(|(_, ch)| ch.recv_window < DEFAULT_WINDOW_SIZE / 2 && !ch.closed)
            .map(|(&id, _)| id)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Actions the session handler should take
// ---------------------------------------------------------------------------

/// Actions that the session handler should take in response to channel requests.
#[derive(Debug)]
pub enum ChannelAction {
    /// No action needed.
    None,
    /// Start a shell on the given channel (create a terminal pane).
    StartShell { channel_id: u32 },
    /// Execute a command on the given channel.
    ExecCommand { channel_id: u32, command: String },
    /// Terminal window size changed.
    WindowChange { channel_id: u32, width: u32, height: u32 },
}

// ---------------------------------------------------------------------------
// Message builders
// ---------------------------------------------------------------------------

/// Build SSH_MSG_CHANNEL_OPEN_CONFIRMATION.
fn build_channel_open_confirmation(
    recipient_channel: u32,
    sender_channel: u32,
    initial_window: u32,
    max_packet: u32,
) -> Vec<u8> {
    log::debug!(
        "channel: building CHANNEL_OPEN_CONFIRMATION — recipient={}, sender={}, window={}, max_pkt={}",
        recipient_channel, sender_channel, initial_window, max_packet,
    );
    let mut w = SshWriter::new();
    w.write_byte(SSH_MSG_CHANNEL_OPEN_CONFIRMATION);
    w.write_uint32(recipient_channel);
    w.write_uint32(sender_channel);
    w.write_uint32(initial_window);
    w.write_uint32(max_packet);
    w.into_bytes()
}

/// Build SSH_MSG_CHANNEL_OPEN_FAILURE.
fn build_channel_open_failure(
    recipient_channel: u32,
    reason_code: u32,
    description: &str,
) -> Vec<u8> {
    log::debug!(
        "channel: building CHANNEL_OPEN_FAILURE — recipient={}, reason={}, desc='{}'",
        recipient_channel, reason_code, description,
    );
    let mut w = SshWriter::new();
    w.write_byte(SSH_MSG_CHANNEL_OPEN_FAILURE);
    w.write_uint32(recipient_channel);
    w.write_uint32(reason_code);
    w.write_string_utf8(description);
    w.write_string_utf8(""); // language tag
    w.into_bytes()
}

/// Build SSH_MSG_CHANNEL_CLOSE.
fn build_channel_close(recipient_channel: u32) -> Vec<u8> {
    log::debug!("channel: building CHANNEL_CLOSE — recipient={}", recipient_channel);
    let mut w = SshWriter::new();
    w.write_byte(SSH_MSG_CHANNEL_CLOSE);
    w.write_uint32(recipient_channel);
    w.into_bytes()
}

/// Build SSH_MSG_CHANNEL_SUCCESS.
fn build_channel_success(recipient_channel: u32) -> Vec<u8> {
    let mut w = SshWriter::new();
    w.write_byte(SSH_MSG_CHANNEL_SUCCESS);
    w.write_uint32(recipient_channel);
    w.into_bytes()
}

/// Build SSH_MSG_CHANNEL_FAILURE.
fn build_channel_failure(recipient_channel: u32) -> Vec<u8> {
    let mut w = SshWriter::new();
    w.write_byte(SSH_MSG_CHANNEL_FAILURE);
    w.write_uint32(recipient_channel);
    w.into_bytes()
}

/// Build SSH_MSG_CHANNEL_EXTENDED_DATA (for stderr).
pub fn build_channel_extended_data(
    recipient_channel: u32,
    data_type: u32,
    data: &[u8],
) -> Vec<u8> {
    let mut w = SshWriter::new();
    w.write_byte(SSH_MSG_CHANNEL_EXTENDED_DATA);
    w.write_uint32(recipient_channel);
    w.write_uint32(data_type);
    w.write_string(data);
    w.into_bytes()
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ChannelError {
    /// Malformed channel message.
    MalformedMessage,
    /// Unexpected message type.
    UnexpectedMessage(u8),
    /// Unknown channel ID.
    UnknownChannel(u32),
    /// Too many open channels.
    TooManyChannels,
    /// Data exceeds the receive window.
    WindowExceeded,
    /// Send window is exhausted (cannot send data yet).
    WindowExhausted,
}

impl core::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MalformedMessage => write!(f, "malformed channel message"),
            Self::UnexpectedMessage(t) => write!(f, "unexpected message type: {}", t),
            Self::UnknownChannel(id) => write!(f, "unknown channel: {}", id),
            Self::TooManyChannels => write!(f, "too many open channels"),
            Self::WindowExceeded => write!(f, "data exceeds receive window"),
            Self::WindowExhausted => write!(f, "send window exhausted"),
        }
    }
}

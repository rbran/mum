//! [Command]s can be sent from a controller to mumd who might respond with a
//! [CommandResponse]. The commands and their responses are serializable and
//! can be sent in any way you want.

use crate::state::{Channel, Server};

use serde::{Deserialize, Serialize};

/// Sent by a controller to mumd who might respond with a [CommandResponse]. Not
/// all commands receive a response.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Command {
    /// No response.
    ChannelJoin {
        channel_identifier: String,
    },

    /// Response: [CommandResponse::ChannelList].
    ChannelList,

    /// Force reloading of config file from disk. No response.
    ConfigReload,

    /// Response: [CommandResponse::DeafenStatus]. Toggles if None.
    DeafenSelf(
        Option<bool>
    ),

    /// Set the outgoing audio volume (i.e. from you to the server). No response.
    InputVolumeSet(f32),

    /// Response: [CommandResponse::MuteStatus]. Toggles if None.
    MuteOther(String, Option<bool>),

    /// Response: [CommandResponse::MuteStatus]. Toggles if None.
    MuteSelf(Option<bool>),

    /// Set the master incoming audio volume (i.e. from the server to you).
    /// No response.
    OutputVolumeSet(f32),

    /// Response: [CommandResponse::Pong]. Used to test existance of a
    /// mumd-instance.
    Ping,

    /// Connect to the specified server. Response: [CommandResponse::ServerConnect].
    ServerConnect {
        host: String,
        port: u16,
        username: String,
        accept_invalid_cert: bool,
    },

    /// No response.
    ServerDisconnect,

    /// Send a server status request via UDP (e.g. not requiring a TCP connection).
    /// Response: [CommandResponse::ServerStatus].
    ServerStatus {
        host: String,
        port: u16,
    },

    /// Response: [CommandResponse::Status].
    Status,

    /// No response.
    UserVolumeSet(String, f32),
}

/// A response to a sent [Command].
#[derive(Debug, Deserialize, Serialize)]
pub enum CommandResponse {
    ChannelList {
        channels: Channel,
    },

    DeafenStatus {
        is_deafened: bool,
    },

    MuteStatus {
        is_muted: bool,
    },

    Pong,

    ServerConnect {
        welcome_message: Option<String>,
    },

    ServerStatus {
        version: u32,
        users: u32,
        max_users: u32,
        bandwidth: u32,
    },

    Status {
        server_state: Server,
    },
}

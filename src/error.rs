use serde::{Deserialize, Serialize};
use std::fmt;

use colored::*;
use log::*;

#[derive(Debug, Serialize, Deserialize)]
pub enum Error {
    DisconnectedError,
    AlreadyConnectedError,
    ChannelIdentifierError(String, ChannelIdentifierError),
    InvalidServerAddrError(String, u16),
    InvalidUsernameError(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::DisconnectedError => write!(f, "Not connected to a server"),
            Error::AlreadyConnectedError => {
                write!(f, "Already connected to a server")
            }
            Error::ChannelIdentifierError(id, kind) => {
                write!(f, "{}: {}", kind, id)
            }
            Error::InvalidServerAddrError(addr, port) => {
                write!(f, "Invalid server address: {}: {}", addr, port)
            }
            Error::InvalidUsernameError(username) => {
                write!(f, "Invalid username: {}", username)
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ChannelIdentifierError {
    Invalid,
    Ambiguous,
}

impl fmt::Display for ChannelIdentifierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelIdentifierError::Invalid => {
                write!(f, "Invalid channel identifier")
            }
            ChannelIdentifierError::Ambiguous => {
                write!(f, "Ambiguous channel identifier")
            }
        }
    }
}

pub fn setup_logger<T: Into<fern::Output>>(target: T, color: bool) {
    fern::Dispatch::new()
        .format(move |out, message, record| {
            let message = message.to_string();
            out.finish(format_args!(
                "{} {}:{}{}{}",
                //TODO runtime flag that disables color
                if color {
                    match record.level() {
                        Level::Error => "ERROR".red(),
                        Level::Warn => "WARN ".yellow(),
                        Level::Info => "INFO ".normal(),
                        Level::Debug => "DEBUG".green(),
                        Level::Trace => "TRACE".normal(),
                    }
                } else {
                    match record.level() {
                        Level::Error => "ERROR",
                        Level::Warn => "WARN ",
                        Level::Info => "INFO ",
                        Level::Debug => "DEBUG",
                        Level::Trace => "TRACE",
                    }
                    .normal()
                },
                record.file().unwrap(),
                record.line().unwrap(),
                if message.chars().any(|e| e == '\n') {
                    "\n"
                } else {
                    " "
                },
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .chain(target.into())
        .apply()
        .unwrap();
}

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;

pub const DEFAULT_INPUT_VOLUME: f32 = 1.0;
pub const DEFAULT_OUTPUT_VOLUME: f32 = 1.0;
pub const DEFAULT_PORT: u16 = 64738;
pub const DEFAULT_USERNAME: &str = "mymumd";
pub const DEFAULT_CHECK_CERT: bool = true;

#[derive(Debug, Deserialize, Serialize)]
struct TOMLConfig {
    pub input_volume: Option<f32>,
    pub output_volume: Option<f32>,
    pub host: String,
    pub port: Option<u16>,
    pub channel: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub check_cert: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub input_volume: f32,
    pub output_volume: f32,
    pub host: String,
    pub port: u16,
    pub channel: Option<String>,
    pub username: String,
    pub password: Option<String>,
    pub check_cert: bool,
    pub addr: SocketAddr,
}

pub fn to_socket_addr(host: &str, port: u16) -> Option<SocketAddr> {
    match (host, port).to_socket_addrs().map(|mut e| e.next()) {
        Ok(Some(addr)) => Some(addr),
        _ => None,
    }
}

impl From<TOMLConfig> for Config {
    fn from(input: TOMLConfig) -> Self {
        let port = input.port.unwrap_or(DEFAULT_PORT);
        let addr = to_socket_addr(&input.host, port).unwrap();
        Config {
            input_volume: input.input_volume.unwrap_or(DEFAULT_INPUT_VOLUME),
            output_volume: input.output_volume.unwrap_or(DEFAULT_OUTPUT_VOLUME),
            host: input.host,
            port,
            channel: input.channel,
            username: input.username.unwrap_or(DEFAULT_USERNAME.to_string()),
            password: input.password,
            check_cert: input.check_cert.unwrap_or(DEFAULT_CHECK_CERT),
            addr,
        }
    }
}

pub fn get_cfg_path() -> String {
    if let Ok(var) = std::env::var("XDG_CONFIG_HOME") {
        let path = format!("{}/mymumdrc", var);
        if Path::new(&path).exists() {
            return path;
        }
    } else if let Ok(var) = std::env::var("HOME") {
        let path = format!("{}/.config/mymumdrc", var);
        if Path::new(&path).exists() {
            return path;
        }
    }

    "/etc/mymumdrc".to_string()
}

pub fn read_cfg() -> Result<Config, Box<dyn Error>> {
    Ok(Config::from(toml::from_str::<TOMLConfig>(
        &fs::read_to_string(get_cfg_path())?,
    )?))
}

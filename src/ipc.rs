//! Line-delimited JSON protocol over a Unix domain socket.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use crate::config;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    List,
    Get { device: Option<usize> },
    Set {
        device: Option<usize>,
        disable_acceleration: Option<bool>,
        acceleration: Option<f64>,
        speed: Option<f64>,
    },
    Reload,
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub index: usize,
    pub name: String,
    pub vendor_id: Option<i64>,
    pub product_id: Option<i64>,
    pub disable_acceleration: bool,
    pub acceleration: f64,
    pub speed: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Pong { version: String },
    Devices { devices: Vec<DeviceInfo> },
    Error { message: String },
}

/// Connect to the daemon socket if it's running.
pub fn connect() -> Option<UnixStream> {
    UnixStream::connect(config::socket_path()).ok()
}

/// Send a single request and read a single response.
pub fn roundtrip(stream: &mut UnixStream, req: &Request) -> std::io::Result<Response> {
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    reader.read_line(&mut buf)?;
    serde_json::from_str(&buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

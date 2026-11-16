//! Handles SteamVR commands pressed in the headset lobby.
//!
//! ALVR runs inside SteamVR's vrserver process, so the headset cannot reach it while SteamVR is
//! closed. The dashboard polls the headset's lobby endpoint instead, and always opens the
//! connection itself, so no inbound firewall rule is needed.

use crate::steamvr_launcher::LAUNCHER;
use alvr_common::{info, warn};
use std::{
    io::{Read, Write},
    net::{IpAddr, SocketAddr, SocketAddrV6, TcpStream},
    thread,
    time::{Duration, Instant},
};

const PORT: u16 = 9946;
const REQUEST_MAGIC: &str = "ALVR-NCM-LOBBY";
const POLL_INTERVAL: Duration = Duration::from_millis(600);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const IO_TIMEOUT: Duration = Duration::from_millis(400);
const COMMAND_COOLDOWN: Duration = Duration::from_secs(2);
// Link local addresses need an interface index, which is not stored in the session
const MAX_SCOPE_ID: u32 = 64;

pub fn spawn_listener() {
    thread::spawn(|| {
        let mut endpoints = Vec::new();
        let mut last_command = None;

        loop {
            if endpoints.is_empty() {
                endpoints = discover_lobby_endpoints();
            }

            let mut reachable = Vec::with_capacity(endpoints.len());

            for endpoint in std::mem::take(&mut endpoints) {
                match poll_command(endpoint) {
                    Ok(Some(command)) => {
                        reachable.push(endpoint);
                        run_command(&command, &mut last_command);
                    }
                    Ok(None) => reachable.push(endpoint),
                    // The headset address or its interface index changed; discover it again
                    Err(_) => {}
                }
            }

            endpoints = reachable;

            thread::sleep(POLL_INTERVAL);
        }
    });
}

fn discover_lobby_endpoints() -> Vec<SocketAddrV6> {
    let session = crate::data_sources::get_read_only_local_session();
    let mut endpoints = Vec::new();

    for client in session.session().client_connections.values() {
        let Some(IpAddr::V6(ip)) = client.current_ip else {
            continue;
        };

        if !ip.is_unicast_link_local() {
            continue;
        }

        for scope_id in 1..=MAX_SCOPE_ID {
            let endpoint = SocketAddrV6::new(ip, PORT, 0, scope_id);
            if TcpStream::connect_timeout(&SocketAddr::V6(endpoint), CONNECT_TIMEOUT).is_ok() {
                endpoints.push(endpoint);
                break;
            }
        }
    }

    endpoints
}

fn poll_command(endpoint: SocketAddrV6) -> std::io::Result<Option<String>> {
    let mut stream = TcpStream::connect_timeout(&SocketAddr::V6(endpoint), CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    stream.write_all(REQUEST_MAGIC.as_bytes())?;
    stream.flush()?;

    let mut response = [0; 16];
    let length = stream.read(&mut response)?;
    let command = String::from_utf8_lossy(&response[..length]).trim().to_owned();

    Ok((!command.is_empty() && command != "none").then_some(command))
}

fn run_command(command: &str, last_command: &mut Option<(String, Instant)>) {
    if let Some((previous, at)) = last_command
        && previous == command
        && at.elapsed() < COMMAND_COOLDOWN
    {
        return;
    }
    *last_command = Some((command.to_owned(), Instant::now()));

    match command {
        "launch" => {
            info!("Launching SteamVR from the headset lobby");
            thread::spawn(|| LAUNCHER.lock().launch_steamvr());
        }
        "restart" => {
            info!("Restarting SteamVR from the headset lobby");
            thread::spawn(|| LAUNCHER.lock().restart_steamvr());
        }
        command => warn!("Ignoring unknown lobby command: {command}"),
    }
}

//! SteamVR commands requested from the headset lobby.
//!
//! The ALVR server runs inside SteamVR's vrserver process, so it cannot be reached while SteamVR
//! is closed. The dashboard polls this endpoint over the USB link instead, which also keeps the
//! PC firewall out of the way: every connection is opened by the PC.

use alvr_common::{info, parking_lot::Mutex, warn};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

pub const PORT: u16 = 9946;

const REQUEST_MAGIC: &str = "ALVR-NCM-LOBBY";
const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);

static PENDING_COMMANDS: Mutex<VecDeque<LobbyCommand>> = Mutex::new(VecDeque::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LobbyCommand {
    LaunchSteamvr,
    RestartSteamvr,
}

impl LobbyCommand {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::LaunchSteamvr => "launch",
            Self::RestartSteamvr => "restart",
        }
    }
}

/// Queues a command for the dashboard and returns immediately. Commands are best effort: when no
/// dashboard is polling, the command waits in the queue for the next poll.
pub fn request_steamvr(command: LobbyCommand) {
    info!("Lobby: requested {}", command.wire_name());

    PENDING_COMMANDS.lock().push_back(command);
}

/// Starts the endpoint the dashboard polls. Only lobby button presses add commands, so it stays
/// idle while a stream is running.
pub fn spawn_listener() {
    thread::spawn(|| {
        let listener = match TcpListener::bind(("::", PORT)) {
            Ok(listener) => listener,
            Err(error) => {
                warn!("Could not listen for lobby commands on port {PORT}: {error}");
                return;
            }
        };

        for request in listener.incoming() {
            let Ok(stream) = request else {
                continue;
            };

            handle_request(stream);
        }
    });
}

fn handle_request(mut stream: TcpStream) {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT)).ok();
    stream.set_write_timeout(Some(REQUEST_TIMEOUT)).ok();

    let mut request = [0; 32];
    let Ok(length) = stream.read(&mut request) else {
        return;
    };

    if !request[..length].starts_with(REQUEST_MAGIC.as_bytes()) {
        return;
    }

    let command = PENDING_COMMANDS.lock().pop_front();
    let response = command.map_or("none", LobbyCommand::wire_name);

    stream.write_all(response.as_bytes()).ok();
    stream.flush().ok();
}

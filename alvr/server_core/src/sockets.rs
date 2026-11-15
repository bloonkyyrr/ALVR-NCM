use alvr_common::{
    ToAny,
    anyhow::{Result, bail},
    warn,
};
use flume::TryRecvError;
use alvr_sockets::WIRED_CLIENT_HOSTNAME;
use mdns_sd::{Receiver, ScopedIp, ServiceDaemon, ServiceEvent};
use std::{
    collections::{HashMap, HashSet},
    net::{SocketAddr, SocketAddrV6},
};

pub struct WelcomeSocket {
    mdns_receiver: Receiver<ServiceEvent>,
}

impl WelcomeSocket {
    pub fn new() -> Result<Self> {
        let mdns_receiver = ServiceDaemon::new()?.browse(alvr_sockets::MDNS_SERVICE_TYPE)?;

        Ok(Self { mdns_receiver })
    }

    // Returns each client's hostname and candidate control addresses. When `wired_only` is set,
    // only clients announcing themselves on the headset's USB network are returned, under the
    // wired hostname.
    pub fn recv_all(&self, wired_only: bool) -> Result<HashMap<String, Vec<SocketAddr>>> {
        let mut clients = HashMap::new();

        loop {
            match self.mdns_receiver.try_recv() {
                Ok(event) => {
                    if let ServiceEvent::ServiceResolved(info) = event {
                        let announced_hostname = info
                            .get_property_val_str(alvr_sockets::MDNS_DEVICE_ID_KEY)
                            .unwrap_or_else(|| info.get_hostname());

                        let client_protocol = info
                            .get_property_val_str(alvr_sockets::MDNS_PROTOCOL_KEY)
                            .to_any()?;
                        let server_protocol = alvr_common::protocol_id();
                        let client_is_dev = client_protocol.contains("-dev");
                        let server_is_dev = server_protocol.contains("-dev");

                        if client_protocol != server_protocol {
                            let reason = if client_is_dev && server_is_dev {
                                "Please use matching nightly versions."
                            } else if client_is_dev {
                                "Please use nightly server or stable client."
                            } else if server_is_dev {
                                "Please use stable server or nightly client."
                            } else {
                                "Please use matching stable versions."
                            };
                            let protocols = format!(
                                "Protocols: server={server_protocol}, client={client_protocol}"
                            );
                            warn!(
                                "Found incompatible client {announced_hostname}! {reason}\n{protocols}"
                            );
                        }

                        let is_usb_network = info
                            .get_property_val_str(alvr_sockets::MDNS_USB_NETWORK_KEY)
                            == Some("true");
                        let addresses = control_addresses(info.get_addresses());
                        let Some((hostname, addresses)) =
                            client_entry(announced_hostname, is_usb_network, addresses, wired_only)
                        else {
                            continue;
                        };
                        clients.insert(hostname, addresses);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(e) => bail!(e),
            }
        }

        Ok(clients)
    }
}

/// Addresses to try for a discovered client. A client on the USB network announces an IPv6
/// link-local address, which the host may see once per interface its announcement reaches (for
/// example over Ethernet as well as over the USB cable), and it may also advertise other addresses
/// such as Wi-Fi. Link-local addresses come first so the connection stays on the USB interface,
/// and the caller tries the rest if that one does not answer.
fn control_addresses(addresses: &HashSet<ScopedIp>) -> Vec<SocketAddr> {
    let mut ordered = Vec::new();
    let mut remaining = Vec::new();

    for address in addresses {
        let address = control_address(address);
        if matches!(address, SocketAddr::V6(address) if address.ip().is_unicast_link_local()) {
            if !ordered.contains(&address) {
                ordered.push(address);
            }
        } else if !remaining.contains(&address) {
            remaining.push(address);
        }
    }

    ordered.extend(remaining);
    ordered
}

/// Hostname and control addresses announced by a client. When `wired_only` is set, clients that are
/// not on the headset's USB network are skipped, and USB clients are returned as the wired device.
fn client_entry(
    announced_hostname: &str,
    is_usb_network: bool,
    addresses: Vec<SocketAddr>,
    wired_only: bool,
) -> Option<(String, Vec<SocketAddr>)> {
    if wired_only && !is_usb_network {
        return None;
    }

    let hostname = if wired_only {
        WIRED_CLIENT_HOSTNAME.to_owned()
    } else {
        announced_hostname.to_owned()
    };

    Some((hostname, addresses))
}

fn control_address(address: &ScopedIp) -> SocketAddr {
    match address {
        ScopedIp::V6(address) => {
            // The USB network is IPv6 link-local, so keep the scope id of the interface the
            // client was discovered on. Otherwise the stream would leave through another link.
            let ip = *address.addr();
            let scope_id = if ip.is_unicast_link_local() {
                address.scope_id().index
            } else {
                0
            };
            SocketAddr::V6(SocketAddrV6::new(
                ip,
                alvr_sockets::CONTROL_PORT,
                0,
                scope_id,
            ))
        }
        _ => SocketAddr::new(address.to_ip_addr(), alvr_sockets::CONTROL_PORT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    const CLIENT_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(169, 254, 10, 2));

    fn announced(addresses: &[IpAddr]) -> Vec<SocketAddr> {
        control_addresses(&addresses.iter().copied().map(ScopedIp::from).collect())
    }

    #[test]
    fn wired_discovery_uses_the_usb_network_client() {
        let addresses = announced(&[CLIENT_IP]);
        let (hostname, addresses) = client_entry("quest-headset", true, addresses, true).unwrap();

        assert_eq!(hostname, WIRED_CLIENT_HOSTNAME);
        assert_eq!(addresses[0].ip(), CLIENT_IP);
        assert_eq!(addresses[0].port(), alvr_sockets::CONTROL_PORT);
    }

    #[test]
    fn wired_discovery_skips_clients_off_the_usb_network() {
        let addresses = announced(&[CLIENT_IP]);

        assert!(client_entry("quest-headset", false, addresses, true).is_none());
    }

    #[test]
    fn wifi_discovery_keeps_the_announced_hostname() {
        let addresses = announced(&[CLIENT_IP]);
        let (hostname, _) = client_entry("quest-headset", true, addresses, false).unwrap();

        assert_eq!(hostname, "quest-headset");
    }

    #[test]
    fn discovery_prefers_the_link_local_address() {
        let link_local = IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));
        let wifi = IpAddr::V4(Ipv4Addr::new(192, 159, 12, 120));

        let addresses = announced(&[wifi, link_local]);

        assert_eq!(addresses[0].ip(), link_local);
        assert_eq!(addresses.len(), 2);
    }

    #[test]
    fn link_local_ipv6_keeps_its_address_and_control_port() {
        let ip = IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));
        let scoped = ScopedIp::from(ip);
        let address = control_address(&scoped);

        assert_eq!(address.ip(), ip);
        assert_eq!(address.port(), alvr_sockets::CONTROL_PORT);
    }
}

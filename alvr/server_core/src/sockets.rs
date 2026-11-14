use alvr_common::{
    ToAny,
    anyhow::{Result, bail},
    warn,
};
use flume::TryRecvError;
use alvr_sockets::WIRED_CLIENT_HOSTNAME;
use mdns_sd::{Receiver, ScopedIp, ServiceDaemon, ServiceEvent};
use std::{
    collections::HashMap,
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

    // Returns each client's control address and hostname. When `wired_only` is set, only clients
    // announcing themselves on the headset's USB network are returned, under the wired hostname.
    pub fn recv_all(&self, wired_only: bool) -> Result<HashMap<String, SocketAddr>> {
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
                        let address = info.get_addresses().iter().next().to_any()?;
                        let Some((hostname, address)) =
                            client_entry(announced_hostname, is_usb_network, address, wired_only)
                        else {
                            continue;
                        };
                        clients.insert(hostname, address);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(e) => bail!(e),
            }
        }

        Ok(clients)
    }
}

/// Hostname and control address announced by a client. When `wired_only` is set, clients that are
/// not on the headset's USB network are skipped, and USB clients are returned as the wired device.
fn client_entry(
    announced_hostname: &str,
    is_usb_network: bool,
    address: &ScopedIp,
    wired_only: bool,
) -> Option<(String, SocketAddr)> {
    if wired_only && !is_usb_network {
        return None;
    }

    let hostname = if wired_only {
        WIRED_CLIENT_HOSTNAME.to_owned()
    } else {
        announced_hostname.to_owned()
    };

    Some((hostname, control_address(address)))
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

    #[test]
    fn wired_discovery_uses_the_usb_network_client() {
        let scoped = ScopedIp::from(CLIENT_IP);
        let (hostname, address) = client_entry("quest-headset", true, &scoped, true).unwrap();

        assert_eq!(hostname, WIRED_CLIENT_HOSTNAME);
        assert_eq!(address.ip(), CLIENT_IP);
        assert_eq!(address.port(), alvr_sockets::CONTROL_PORT);
    }

    #[test]
    fn wired_discovery_skips_clients_off_the_usb_network() {
        let scoped = ScopedIp::from(CLIENT_IP);

        assert!(client_entry("quest-headset", false, &scoped, true).is_none());
    }

    #[test]
    fn wifi_discovery_keeps_the_announced_hostname() {
        let scoped = ScopedIp::from(CLIENT_IP);
        let (hostname, _) = client_entry("quest-headset", true, &scoped, false).unwrap();

        assert_eq!(hostname, "quest-headset");
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

use alvr_common::{
    anyhow::{Result, bail},
    parking_lot::Mutex,
};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::net::IpAddr;

#[derive(Default)]
struct Announcement {
    local_ip: Option<IpAddr>,
    usb_network: bool,
    fullname: Option<String>,
}

pub struct AnnouncerSocket {
    hostname: String,
    daemon: ServiceDaemon,
    announcement: Mutex<Announcement>,
}

impl AnnouncerSocket {
    pub fn new(hostname: &str) -> Result<Self> {
        let daemon = ServiceDaemon::new()?;

        Ok(Self {
            daemon,
            hostname: hostname.to_owned(),
            announcement: Mutex::new(Announcement::default()),
        })
    }

    pub fn announce(&self) -> Result<()> {
        let local_ip = alvr_system_info::local_ip();
        if local_ip.is_unspecified() {
            bail!("IP is unspecified");
        }

        #[cfg(target_os = "android")]
        let usb_network = alvr_system_info::is_usb_network_bound();
        #[cfg(not(target_os = "android"))]
        let usb_network = false;

        let mut announcement = self.announcement.lock();

        // The connection loop announces repeatedly. Re-registering identical records would drop
        // and recreate the entry the server is resolving, so only publish changed announcements,
        // such as when the headset moves from Wi-Fi to the USB network.
        if announcement.local_ip == Some(local_ip) && announcement.usb_network == usb_network {
            return Ok(());
        }

        let protocol_id = alvr_common::protocol_id();
        let mut properties = vec![(alvr_sockets::MDNS_PROTOCOL_KEY, protocol_id.as_str())];
        if usb_network {
            properties.push((alvr_sockets::MDNS_USB_NETWORK_KEY, "true"));
        }

        let service_info = ServiceInfo::new(
            alvr_sockets::MDNS_SERVICE_TYPE,
            &format!("alvr{}", rand::random::<u16>()),
            &self.hostname,
            local_ip,
            5353,
            &properties[..],
        )?;
        let fullname = service_info.get_fullname().to_owned();

        // Replace the previous announcement. Otherwise the server keeps resolving the address of
        // the link the client was on before, such as Wi-Fi before the USB network came up.
        if let Some(previous_fullname) = announcement.fullname.take() {
            self.daemon.unregister(&previous_fullname).ok();
        }

        self.daemon.register(service_info)?;

        *announcement = Announcement {
            local_ip: Some(local_ip),
            usb_network,
            fullname: Some(fullname),
        };

        Ok(())
    }
}

use alvr_common::{
    anyhow::{Result, bail},
    parking_lot::Mutex,
};
use mdns_sd::{ServiceDaemon, ServiceInfo};

pub struct AnnouncerSocket {
    hostname: String,
    daemon: ServiceDaemon,
    last_fullname: Mutex<Option<String>>,
}

impl AnnouncerSocket {
    pub fn new(hostname: &str) -> Result<Self> {
        let daemon = ServiceDaemon::new()?;

        Ok(Self {
            daemon,
            hostname: hostname.to_owned(),
            last_fullname: Mutex::new(None),
        })
    }

    pub fn announce(&self) -> Result<()> {
        let local_ip = alvr_system_info::local_ip();
        if local_ip.is_unspecified() {
            bail!("IP is unspecified");
        }

        let protocol_id = alvr_common::protocol_id();
        let mut properties = vec![(alvr_sockets::MDNS_PROTOCOL_KEY, protocol_id.as_str())];
        #[cfg(target_os = "android")]
        let usb_network = alvr_system_info::is_usb_network_bound();
        #[cfg(not(target_os = "android"))]
        let usb_network = false;
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

        let mut last_fullname = self.last_fullname.lock();

        // Replace the previous announcement. Otherwise the server keeps resolving the address of
        // the link the client was on before, such as Wi-Fi before the USB network came up.
        if let Some(previous_fullname) = last_fullname.take() {
            self.daemon.unregister(&previous_fullname).ok();
        }

        self.daemon.register(service_info)?;
        *last_fullname = Some(fullname);

        Ok(())
    }
}

use alvr_common::{debug, info, parking_lot::Mutex, warn};
use jni::{
    Env, JavaVM,
    errors::Result as JniResult,
    jni_sig, jni_str,
    objects::{JObject, JObjectArray, JString},
    refs::Reference,
    strings::JNIStr,
    sys::jobject,
};
use std::{
    ffi::CStr,
    net::{IpAddr, Ipv4Addr},
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
    time::{Duration, Instant},
};

pub const MICROPHONE_PERMISSION: &str = "android.permission.RECORD_AUDIO";

const USB_NETWORK_RETRY_INTERVAL: Duration = Duration::from_secs(10);
// Meta reports the negotiated USB speed as the NCM network's link bandwidth. Anything below this
// is a USB 2 cable or port, which cannot carry a high bitrate stream.
const USB3_MIN_LINK_KBPS: i32 = 1_000_000;

static USB_NETWORK_REQUESTED: AtomicBool = AtomicBool::new(false);
static USB_NETWORK_BOUND: AtomicBool = AtomicBool::new(false);
static USB_LINK_KBPS: AtomicI32 = AtomicI32::new(0);
static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

/// Request Horizon OS's USB NCM network and bind ALVR's process to it when available.
/// This is intentionally best-effort: unsupported devices continue using Wi-Fi.
pub fn request_usb_network() -> bool {
    // Also re-check the link while bound, so that unplugging the cable falls back to Wi-Fi.
    if USB_NETWORK_BOUND.load(Ordering::Relaxed) && usb_network_ip().is_some() {
        return true;
    }

    // The system releases the request when the user denies the consent dialog (the callback then
    // gets onUnavailable) and when the cable is unplugged, so ask again after a delay instead of
    // giving up on USB networking for the rest of the session.
    let last_request = *LAST_REQUEST.lock();
    let request_due = !USB_NETWORK_REQUESTED.load(Ordering::Relaxed)
        || last_request.is_none_or(|last| last.elapsed() > USB_NETWORK_RETRY_INTERVAL);

    if request_due {
        let result = vm().attach_current_thread(|env| {
            let manager = get_system_service(env, "connectivity")?;
            let builder = env.new_object(
                jni_str!("android/net/NetworkRequest$Builder"),
                jni_sig!("()V"),
                &[],
            )?;
            let builder = env
                .call_method(
                    builder,
                    jni_str!("addTransportType"),
                    jni_sig!("(I)Landroid/net/NetworkRequest$Builder;"),
                    &[8.into()], // NetworkCapabilities.TRANSPORT_USB
                )?
                .l()?;
            let builder = env
                .call_method(
                    builder,
                    jni_str!("removeCapability"),
                    jni_sig!("(I)Landroid/net/NetworkRequest$Builder;"),
                    &[12.into()], // NET_CAPABILITY_INTERNET
                )?
                .l()?;
            let builder = env
                .call_method(
                    builder,
                    jni_str!("removeCapability"),
                    jni_sig!("(I)Landroid/net/NetworkRequest$Builder;"),
                    // NET_CAPABILITY_TRUSTED. The USB link is local-only, so a request that keeps
                    // the default capabilities never matches. Removing NOT_RESTRICTED (13) instead
                    // would make this a restricted network request, which ConnectivityService
                    // rejects for third-party apps.
                    &[14.into()],
                )?
                .l()?;
            let request = env
                .call_method(
                    builder,
                    jni_str!("build"),
                    jni_sig!("()Landroid/net/NetworkRequest;"),
                    &[],
                )?
                .l()?;
            let callback = env.new_object(
                jni_str!("android/net/ConnectivityManager$NetworkCallback"),
                jni_sig!("()V"),
                &[],
            )?;

            // Keep the request registered while the app is running. Android retains the callback
            // for the request and delivers the USB network asynchronously.
            env.call_method(
                &manager,
                jni_str!("requestNetwork"),
                jni_sig!("(Landroid/net/NetworkRequest;Landroid/net/ConnectivityManager$NetworkCallback;)V"),
                &[(&request).into(), (&callback).into()],
            )?;
            JniResult::Ok(())
        });

        match result {
            Ok(()) => {
                *LAST_REQUEST.lock() = Some(Instant::now());
                USB_NETWORK_REQUESTED.store(true, Ordering::Relaxed);
                info!("Requested the Horizon USB network (waiting for consent and the cable link)");
            }
            Err(error) => {
                warn!("Could not request Horizon USB networking: {error}");
                // Retry later instead of disabling USB networking for the session. The timestamp
                // also keeps a failing request from being retried on every poll.
                *LAST_REQUEST.lock() = Some(Instant::now());
                USB_NETWORK_REQUESTED.store(false, Ordering::Relaxed);
                return false;
            }
        }
    }

    // requestNetwork is asynchronous. The connection retry loop polls for the USB network, so
    // ALVR can continue advertising over Wi-Fi while the headset displays its consent prompt.
    bind_available_usb_network()
}

pub fn is_usb_network_bound() -> bool {
    USB_NETWORK_BOUND.load(Ordering::Relaxed)
}

/// Whether the USB network was requested. When it is still pending, the headset is waiting for
/// the consent dialog, the cable, or a link the current request can use.
pub fn is_usb_network_requested() -> bool {
    USB_NETWORK_REQUESTED.load(Ordering::Relaxed)
}

/// Negotiated speed of the USB NCM link, in Kbps. Used to warn about USB 2 cables.
pub fn usb_link_speed_kbps() -> Option<u32> {
    if !USB_NETWORK_BOUND.load(Ordering::Relaxed) {
        return None;
    }

    let link_kbps = USB_LINK_KBPS.load(Ordering::Relaxed);

    (link_kbps > 0).then_some(link_kbps as u32)
}

fn bind_available_usb_network() -> bool {
    let result = vm().attach_current_thread(|env| {
        let manager = get_system_service(env, "connectivity")?;
        let networks = env
            .call_method(
                &manager,
                jni_str!("getAllNetworks"),
                jni_sig!("()[Landroid/net/Network;"),
                &[],
            )?
            .l()?;
        let networks = env.cast_local::<JObjectArray<JObject>>(networks)?;
        for index in 0..networks.len(env)? {
            let network = networks.get_element(env, index)?;
            let capabilities = env
                .call_method(
                    &manager,
                    jni_str!("getNetworkCapabilities"),
                    jni_sig!("(Landroid/net/Network;)Landroid/net/NetworkCapabilities;"),
                    &[(&network).into()],
                )?
                .l()?;
            if capabilities.is_null() {
                continue;
            }

            let transports = network_transports(env, &capabilities)?;
            let interface_name = network_interface_name(env, &manager, &network)?
                .unwrap_or_else(|| "unknown".to_owned());

            debug!(
                "Network {interface_name}: transports: {}",
                transports.join(", ")
            );

            // Horizon OS reports the USB link as TRANSPORT_USB. Some builds only surface the NCM
            // gadget as an Ethernet network, and older ones do not report TRANSPORT_USB at all, so
            // also accept an Ethernet network or the gadget's usual interface name.
            let is_wired_link = transports.contains(&"usb")
                || transports.contains(&"ethernet")
                || interface_name.starts_with("usb");

            if is_wired_link {
                let bound = env
                    .call_method(
                        &manager,
                        jni_str!("bindProcessToNetwork"),
                        jni_sig!("(Landroid/net/Network;)Z"),
                        &[(&network).into()],
                    )?
                    .z()?;
                if bound {
                    // The NCM link bandwidth is the negotiated USB speed, which tells a USB 2
                    // cable or port from a USB 3 one.
                    let link_kbps = env
                        .call_method(
                            &capabilities,
                            jni_str!("getLinkDownstreamBandwidthKbps"),
                            jni_sig!("()I"),
                            &[],
                        )?
                        .i()?;

                    return JniResult::Ok(Some((interface_name, link_kbps)));
                }
            }
        }

        JniResult::Ok(None)
    });

    match result {
        Ok(Some((interface_name, link_kbps))) => {
            USB_LINK_KBPS.store(link_kbps, Ordering::Relaxed);

            if !USB_NETWORK_BOUND.swap(true, Ordering::Relaxed) {
                info!(
                    "Bound ALVR to the Horizon USB network {interface_name} ({link_kbps} Kbps)"
                );

                if link_kbps > 0 && link_kbps < USB3_MIN_LINK_KBPS {
                    warn!(
                        "USB link negotiated at {link_kbps} Kbps (USB 2). Use a USB 3 cable and port for high bitrate streaming"
                    );
                }
            }

            true
        }
        Ok(None) => {
            // The cable was unplugged or the link went down. Advertising falls back to Wi-Fi.
            USB_NETWORK_BOUND.store(false, Ordering::Relaxed);

            false
        }
        Err(error) => {
            warn!("Could not bind ALVR to Horizon USB networking: {error}");
            USB_NETWORK_BOUND.store(false, Ordering::Relaxed);

            false
        }
    }
}

fn network_transports<'a>(
    env: &mut Env<'a>,
    capabilities: &JObject<'a>,
) -> JniResult<Vec<&'static str>> {
    // NetworkCapabilities.TRANSPORT_* values. USB was added in API 31.
    const TRANSPORTS: [(i32, &str); 6] = [
        (0, "cellular"),
        (1, "wifi"),
        (2, "bluetooth"),
        (3, "ethernet"),
        (4, "vpn"),
        (8, "usb"),
    ];

    let mut names = Vec::new();
    for (transport, name) in TRANSPORTS {
        let has_transport = env
            .call_method(
                capabilities,
                jni_str!("hasTransport"),
                jni_sig!("(I)Z"),
                &[transport.into()],
            )?
            .z()?;
        if has_transport {
            names.push(name);
        }
    }

    JniResult::Ok(names)
}

fn network_interface_name<'a>(
    env: &mut Env<'a>,
    manager: &JObject<'a>,
    network: &JObject<'a>,
) -> JniResult<Option<String>> {
    let properties = env
        .call_method(
            manager,
            jni_str!("getLinkProperties"),
            jni_sig!("(Landroid/net/Network;)Landroid/net/LinkProperties;"),
            &[network.into()],
        )?
        .l()?;
    if properties.is_null() {
        return JniResult::Ok(None);
    }

    let interface_name = env
        .call_method(
            &properties,
            jni_str!("getInterfaceName"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )?
        .l()?;
    if interface_name.is_null() {
        return JniResult::Ok(None);
    }

    JniResult::Ok(Some(
        env.cast_local::<JString>(interface_name)?.to_string(),
    ))
}

pub fn vm() -> JavaVM {
    unsafe { JavaVM::from_raw(ndk_context::android_context().vm().cast()) }
}

pub fn context() -> jobject {
    ndk_context::android_context().context().cast()
}

/// Requests missing permissions together without waiting for the user's response.
pub fn try_get_permissions(permissions: &[&str]) {
    vm().attach_current_thread(|env| {
        let mut missing_permissions = Vec::new();
        for permission in permissions {
            let permission = env.new_string(permission)?;
            let permission_status = env
                .call_method(
                    unsafe { JObject::global_kind_from_raw(context()) },
                    jni_str!("checkSelfPermission"),
                    jni_sig!("(Ljava/lang/String;)I"),
                    &[(&permission).into()],
                )?
                .i()?;

            if permission_status != 0 {
                missing_permissions.push(permission);
            }
        }

        if !missing_permissions.is_empty() {
            let permissions =
                JObjectArray::<JString>::new(env, missing_permissions.len(), JString::null())?;
            for (index, permission) in missing_permissions.iter().enumerate() {
                permissions.set_element(env, index, permission)?;
            }

            env.call_method(
                unsafe { JObject::global_kind_from_raw(context()) },
                jni_str!("requestPermissions"),
                jni_sig!("([Ljava/lang/String;I)V"),
                &[(&permissions).into(), 0.into()],
            )?;
            // todo: handle case where permission is rejected
        }

        JniResult::Ok(())
    })
    .unwrap();
}

pub fn try_get_permission(permission: &str) {
    try_get_permissions(&[permission]);
}

pub fn build_string(ty: &CStr) -> String {
    vm().attach_current_thread(|env| {
        let jname = env
            .get_static_field(
                jni_str!("android/os/Build"),
                JNIStr::from_cstr(ty).unwrap(),
                jni_sig!("Ljava/lang/String;"),
            )?
            .l()?;
        JniResult::Ok(env.cast_local::<JString>(jname)?.to_string())
    })
    .unwrap()
}

pub fn device_name() -> String {
    build_string(c"DEVICE")
}

pub fn model_name() -> String {
    build_string(c"MODEL")
}

pub fn manufacturer_name() -> String {
    build_string(c"MANUFACTURER")
}

pub fn product_name() -> String {
    build_string(c"PRODUCT")
}

fn get_system_service<'a>(env: &mut Env<'a>, service_name: &str) -> JniResult<JObject<'a>> {
    let service_str = env.new_string(service_name)?;

    env.call_method(
        unsafe { JObject::global_kind_from_raw(context()) },
        jni_str!("getSystemService"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
        &[(&service_str).into()],
    )?
    .l()
}

// Note: tried and failed to use libc
pub fn local_ip() -> IpAddr {
    // Wi-Fi is no longer used: only the USB/NCM link is supported, so report its address.
    if USB_NETWORK_BOUND.load(Ordering::Relaxed)
        && let Some(ip) = usb_network_ip()
    {
        return ip;
    }

    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}

fn usb_network_ip() -> Option<IpAddr> {
    vm().attach_current_thread(|env| {
        let manager = get_system_service(env, "connectivity")?;
        let networks = env
            .call_method(
                &manager,
                jni_str!("getAllNetworks"),
                jni_sig!("()[Landroid/net/Network;"),
                &[],
            )?
            .l()?;
        let networks = env.cast_local::<JObjectArray<JObject>>(networks)?;
        for index in 0..networks.len(env)? {
            let network = networks.get_element(env, index)?;
            let capabilities = env
                .call_method(
                    &manager,
                    jni_str!("getNetworkCapabilities"),
                    jni_sig!("(Landroid/net/Network;)Landroid/net/NetworkCapabilities;"),
                    &[(&network).into()],
                )?
                .l()?;
            if capabilities.is_null()
                || !env
                    .call_method(
                        &capabilities,
                        jni_str!("hasTransport"),
                        jni_sig!("(I)Z"),
                        &[8.into()],
                    )?
                    .z()?
            {
                continue;
            }

            let properties = env
                .call_method(
                    &manager,
                    jni_str!("getLinkProperties"),
                    jni_sig!("(Landroid/net/Network;)Landroid/net/LinkProperties;"),
                    &[(&network).into()],
                )?
                .l()?;
            if properties.is_null() {
                continue;
            }
            let addresses = env
                .call_method(
                    properties,
                    jni_str!("getLinkAddresses"),
                    jni_sig!("()Ljava/util/List;"),
                    &[],
                )?
                .l()?;
            let count = env
                .call_method(&addresses, jni_str!("size"), jni_sig!("()I"), &[])?
                .i()?;
            let mut ipv4 = None;
            let mut link_local_ipv6 = None;
            let mut other_ipv6 = None;
            for address_index in 0..count {
                let link_address = env
                    .call_method(
                        &addresses,
                        jni_str!("get"),
                        jni_sig!("(I)Ljava/lang/Object;"),
                        &[address_index.into()],
                    )?
                    .l()?;
                let inet_address = env
                    .call_method(
                        link_address,
                        jni_str!("getAddress"),
                        jni_sig!("()Ljava/net/InetAddress;"),
                        &[],
                    )?
                    .l()?;
                let host_address = env
                    .call_method(
                        inet_address,
                        jni_str!("getHostAddress"),
                        jni_sig!("()Ljava/lang/String;"),
                        &[],
                    )?
                    .l()?;
                let host_address = env.cast_local::<JString>(host_address)?.to_string();
                let host_address = host_address.split('%').next().unwrap_or(&host_address);
                if let Ok(ip) = host_address.parse::<IpAddr>() {
                    match ip {
                        IpAddr::V4(_) => ipv4 = Some(ip),
                        IpAddr::V6(address) if address.is_unicast_link_local() => {
                            link_local_ipv6 = Some(ip)
                        }
                        IpAddr::V6(_) => other_ipv6 = Some(ip),
                    }
                }
            }
            // The USB link is IPv6 link-local only: the host's NCM adapter gets no address in the
            // headset's IPv4 subnet, so announcing that IPv4 makes the server send the connection
            // out of its internet adapter instead. Prefer the link-local address the host can
            // reach, and keep the IPv4 for links that actually share a subnet with the host.
            if let Some(ip) = link_local_ipv6.or(ipv4).or(other_ipv6) {
                return JniResult::Ok(Some(ip));
            }
        }

        JniResult::Ok(None)
    })
    .unwrap_or(None)
}


pub fn get_battery_status() -> (f32, bool) {
    vm().attach_current_thread(|env| {
        let intent_action_jstring = env.new_string("android.intent.action.BATTERY_CHANGED")?;
        let intent_filter = env.new_object(
            jni_str!("android/content/IntentFilter"),
            jni_sig!("(Ljava/lang/String;)V"),
            &[(&intent_action_jstring).into()],
        )?;
        let battery_intent = env
            .call_method(
                unsafe { JObject::global_kind_from_raw(context()) },
                jni_str!("registerReceiver"),
                jni_sig!(
                    "(Landroid/content/BroadcastReceiver;Landroid/content/IntentFilter;)Landroid/content/Intent;"
                ),
                &[(&JObject::null()).into(), (&intent_filter).into()],
            )?
            .l()?;

        fn get_battery_value<'a>(env: &mut Env<'a>, battery_intent: &JObject, key: &str) -> JniResult<i32> {
            let key_jstring = env.new_string(key)?;
            env.call_method(
                battery_intent,
                jni_str!("getIntExtra"),
                jni_sig!("(Ljava/lang/String;I)I"),
                &[(&key_jstring).into(), (-1).into()],
            )?
            .i()
        }

        let level = get_battery_value(env, &battery_intent, "level")?;
        let scale = get_battery_value(env, &battery_intent, "scale")?;
        let plugged = get_battery_value(env, &battery_intent, "plugged")?;

        JniResult::Ok((level as f32 / scale as f32, plugged > 0))
    })
    .unwrap()
}

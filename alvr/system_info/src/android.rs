use alvr_common::warn;
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
    sync::atomic::{AtomicBool, Ordering},
};

pub const MICROPHONE_PERMISSION: &str = "android.permission.RECORD_AUDIO";

static USB_NETWORK_REQUESTED: AtomicBool = AtomicBool::new(false);
static USB_NETWORK_BOUND: AtomicBool = AtomicBool::new(false);

/// Request Horizon OS's USB NCM network and bind ALVR's process to it when available.
/// This is intentionally best-effort: unsupported devices continue using Wi-Fi.
pub fn request_usb_network() -> bool {
    if USB_NETWORK_BOUND.load(Ordering::Relaxed) {
        return true;
    }

    if !USB_NETWORK_REQUESTED.load(Ordering::Relaxed) {
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
                    &[13.into()], // NET_CAPABILITY_TRUSTED
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

        if let Err(error) = result {
            warn!("Could not request Horizon USB networking: {error}");
            USB_NETWORK_REQUESTED.store(true, Ordering::Relaxed);
            return false;
        }
        USB_NETWORK_REQUESTED.store(true, Ordering::Relaxed);
    }

    // requestNetwork is asynchronous. The connection retry loop polls for the USB network, so
    // ALVR can continue advertising over Wi-Fi while the headset displays its consent prompt.
    bind_available_usb_network()
}

pub fn is_usb_network_bound() -> bool {
    USB_NETWORK_BOUND.load(Ordering::Relaxed)
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
            let is_usb = env
                .call_method(
                    capabilities,
                    jni_str!("hasTransport"),
                    jni_sig!("(I)Z"),
                    &[8.into()], // NetworkCapabilities.TRANSPORT_USB
                )?
                .z()?;
            if is_usb {
                let bound = env
                    .call_method(
                        &manager,
                        jni_str!("bindProcessToNetwork"),
                        jni_sig!("(Landroid/net/Network;)Z"),
                        &[(&network).into()],
                    )?
                    .z()?;
                if bound {
                    USB_NETWORK_BOUND.store(true, Ordering::Relaxed);
                    return JniResult::Ok(true);
                }
            }
        }

        JniResult::Ok(false)
    });

    match result {
        Ok(bound) => bound,
        Err(error) => {
            warn!("Could not bind ALVR to Horizon USB networking: {error}");
            false
        }
    }
}

pub fn vm() -> JavaVM {
    unsafe { JavaVM::from_raw(ndk_context::android_context().vm().cast()) }
}

pub fn context() -> jobject {
    ndk_context::android_context().context().cast()
}

fn get_api_level() -> i32 {
    vm().attach_current_thread(|env| {
        env.get_static_field(
            jni_str!("android/os/Build$VERSION"),
            jni_str!("SDK_INT"),
            jni_sig!("I"),
        )?
        .i()
    })
    .unwrap()
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
    if USB_NETWORK_BOUND.load(Ordering::Relaxed)
        && let Some(ip) = usb_network_ip()
    {
        return ip;
    }

    vm().attach_current_thread(|env| {
        let wifi_manager = get_system_service(env, "wifi")?;
        let wifi_info = env
            .call_method(
                wifi_manager,
                jni_str!("getConnectionInfo"),
                jni_sig!("()Landroid/net/wifi/WifiInfo;"),
                &[],
            )?
            .l()?;
        let ip_i32 = env
            .call_method(wifi_info, jni_str!("getIpAddress"), jni_sig!("()I"), &[])?
            .i()?;

        let ip_arr = ip_i32.to_le_bytes();

        JniResult::Ok(IpAddr::V4(Ipv4Addr::new(
            ip_arr[0], ip_arr[1], ip_arr[2], ip_arr[3],
        )))
    })
    .unwrap()
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
            let mut ipv6 = None;
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
                    if ip.is_ipv4() {
                        return JniResult::Ok(Some(ip));
                    }
                    ipv6 = Some(ip);
                }
            }
            if ipv6.is_some() {
                return JniResult::Ok(ipv6);
            }
        }

        JniResult::Ok(None)
    })
    .unwrap_or(None)
}

// This is needed to avoid wifi scans that disrupt streaming.
// Code inspired from https://github.com/Meumeu/WiVRn/blob/master/client/application.cpp
pub fn set_wifi_lock(enabled: bool) {
    vm().attach_current_thread(|env| {
        let wifi_manager = get_system_service(env, "wifi")?;

        fn set_lock<'a>(env: &mut Env<'a>, lock: &JObject, enabled: bool) -> JniResult<()> {
            env.call_method(
                lock,
                jni_str!("setReferenceCounted"),
                jni_sig!("(Z)V"),
                &[false.into()],
            )?;
            env.call_method(
                &lock,
                if enabled {
                    jni_str!("acquire")
                } else {
                    jni_str!("release")
                },
                jni_sig!("()V"),
                &[],
            )?;

            let lock_is_aquired = env
                .call_method(lock, jni_str!("isHeld"), jni_sig!("()Z"), &[])?
                .z()?;

            if lock_is_aquired != enabled {
                warn!("Failed to set wifi lock: expected {enabled}, got {lock_is_aquired}");
            }

            JniResult::Ok(())
        }

        let wifi_lock_jstring = env.new_string("alvr_wifi_lock")?;
        let wifi_lock = env
            .call_method(
                &wifi_manager,
                jni_str!("createWifiLock"),
                jni_sig!("(ILjava/lang/String;)Landroid/net/wifi/WifiManager$WifiLock;"),
                &[
                    if get_api_level() >= 29 {
                        // Recommended for virtual reality since it disables WIFI scans
                        4 // WIFI_MODE_FULL_LOW_LATENCY
                    } else {
                        3 // WIFI_MODE_FULL_HIGH_PERF
                    }
                    .into(),
                    (&wifi_lock_jstring).into(),
                ],
            )?
            .l()?;
        set_lock(env, &wifi_lock, enabled)?;

        let multicast_lock_jstring = env.new_string("alvr_multicast_lock")?;
        let multicast_lock = env
            .call_method(
                wifi_manager,
                jni_str!("createMulticastLock"),
                jni_sig!("(Ljava/lang/String;)Landroid/net/wifi/WifiManager$MulticastLock;"),
                &[(&multicast_lock_jstring).into()],
            )?
            .l()?;
        set_lock(env, &multicast_lock, enabled)?;

        JniResult::Ok(())
    })
    .unwrap();
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

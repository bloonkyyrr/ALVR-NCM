## ALVR wired mode over USB (Meta Quest)

ALVR connects to Quest headsets over the cable using the USB network (USB NCM) that Horizon OS
exposes to apps. This works without ADB, without Developer Mode and without USB debugging, and it
needs no extra software on the PC.

### Requirements

- A Quest headset running Horizon OS 2.5 or newer.
- A data-capable USB-C cable. USB 3.0 or faster is recommended for bandwidth and stability.

### Setup

1. In the headset, enable **Settings > Link > USB connection for apps**.
2. Connect the USB cable between the headset and the PC.
3. Open the ALVR client in the headset and accept the USB network request if it is shown. Without
   this consent Horizon OS does not grant the client access to the USB network.
4. On the dashboard's **Devices** screen, enable the **Wired Connection** toggle. The headset is
   discovered over the USB network and appears as the wired device.
5. Start streaming. The client and server use the TCP protocol automatically for wired sessions.

Keep the client running while connecting. ALVR keeps waiting for the USB network and will connect
as soon as it appears; it does not fall back to ADB.

### Notes

- Wired mode relies on the headset's own USB networking. Other Android-based headsets that do not
  expose a USB network to apps (for example Pico) should use a Wi-Fi connection instead.
- Wired mode does not launch the client for you. Open the ALVR client in the headset yourself.
- USB networking is local-only: the link carries ALVR's traffic and no internet routing. ALVR
  discovers the headset over mDNS on that link and keeps its IPv6 link-local scope, so the stream
  uses the same interface.

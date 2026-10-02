just disables ADB and replaces with the new one

|     PC OS      |                                    Support                                    |
| :------------: | :---------------------------------------------------------------------------: |
| Windows 10/11  | :heavy_check_mark: ([store link](https://store.steampowered.com/app/3312710)) |
| Windows XP/7/8 |                                      :x:                                      |
|     Linux      |                             :heavy_check_mark:****                            |
|     macOS      |                                      :x:                                      |

\**** Please check the wiki for detailed compatibility information.

### Requirements

-   A supported standalone VR headset (see compatibility table above).
-   SteamVR.
-   A high-end gaming PC:
    -   See the OS compatibility table above.
    -   NVIDIA GPU with NVENC support (GTX 1000 series or newer), an AMD GPU with AMF VCE support, or an INTEL GPU with VPL support (Arc, Tiger Lake or newer), with the latest drivers.
    -   On laptops with both an integrated GPU (Intel HD, AMD iGPU) and a dedicated GPU (NVIDIA GTX/RTX, AMD HD/R5/R7), make sure to assign the dedicated GPU (or "high performance graphics adapter") to ALVR and SteamVR for the best performance and compatibility.  
        (NVIDIA: Nvidia Control Panel → 3D Settings → Application Settings; AMD: similar method)

-   Network:
    -   802.11ac 5 GHz Wi-Fi for the headset, and wired Ethernet for the PC is recommended.
    -   The PC and the headset must be connected to the same router (or use a routed connection as described [here](https://github.com/alvr-org/ALVR/wiki/ALVR-v14-and-Above)).

## Installation

Follow the [installation guide](https://github.com/alvr-org/ALVR/wiki/Installation-guide).

## Troubleshooting

-   See the [Troubleshooting](https://github.com/alvr-org/ALVR/wiki/Troubleshooting) page, and [Linux Troubleshooting](https://github.com/alvr-org/ALVR/wiki/Linux-Troubleshooting) if applicable.
-   Configuration recommendations and additional information can be found [here](https://github.com/alvr-org/ALVR/wiki/Information-and-Recommendations).

## Uninstallation

Open `ALVR Dashboard.exe`, go to the `Installation` tab, then press `Remove firewall rules`.  
Close the ALVR window and delete the ALVR folder.

## Build from Source

Follow the [build guide](https://github.com/alvr-org/ALVR/wiki/Building-From-Source).

## License

ALVR is licensed under the [MIT License](LICENSE).

## Privacy Policy

ALVR apps do not directly collect any personal data.

## Donate

If you would like to support this project, you can donate through our [Open Source Collective account](https://opencollective.com/alvr).

[badge-discord]: https://img.shields.io/discord/720612397580025886?style=for-the-badge&logo=discord&color=5865F2 "Join us on Discord"
[link-discord]: https://discord.gg/ALVR
[badge-matrix]: https://img.shields.io/static/v1?label=chat&message=%23alvr&style=for-the-badge&logo=matrix&color=blueviolet "Join us on Matrix"
[link-matrix]: https://matrix.to/#/#alvr:ckie.dev?via=ckie.dev
[badge-opencollective]: https://img.shields.io/opencollective/all/alvr?style=for-the-badge&logo=opencollective&color=79a3e6 "Donate"
[link-opencollective]: https://opencollective.com/alvr

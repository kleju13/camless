<img src="gui/icon.svg" width="120" height="120" alt="">

# camless

A light replacement for NZXT CAM. It handles fan and pump curves, lighting and the Kraken LCD in two small exes, about 4 MB together, where CAM installs about 500 MB of Electron.

![The Cooling page: liquid, CPU and GPU temperatures, and each fan on its curve](docs/screenshots/cooling.png)

**[Download camless-setup.exe](https://github.com/kleju13/camless/releases/latest/download/camless-setup.exe)** · Windows 10 and 11, x64

- **Curves as in FanControl.** A curve is a graph, a fixed duty, a mix of other curves or a trigger. It follows CPU, GPU or liquid temperature, load or power, with hysteresis and separate rise and fall times. Each fan adds its own offset and limits.
- **Curves stored on the Kraken.** When the Kraken can run a curve by itself, camless stores it there, so the pump keeps its curve if camless stops.
- **Lighting.** CAM's effects on the Kraken ring and on fans behind a Control Hub.
- **The Kraken LCD.** Liquid temperature, frames from your own program, or a web page made for CAM, such as [KrakenDial](https://github.com/kleju13/krakendial).
- **History.** Temperatures, loads, power and fan speeds over the last 10 minutes up to 30 days.
- **Games and FPS.** Every game session with its play time, average frame rate, 1 % low and temperatures. Steam, Epic and Xbox games are found on their own.
- **Profiles.** Each one is a whole setup. *Import from NZXT CAM* makes one from what CAM is set to.
- **A tray app.** It updates itself from GitHub releases. Closing it never stops fan control.

| Monitoring | Kraken curves |
|---|---|
| ![CPU, GPU, memory and Kraken gauges, and temperatures over ten minutes](docs/screenshots/monitoring.png) | ![The pump on a CPU curve, and the radiator fans on a liquid curve stored on the cooler, with the safety floor hatched](docs/screenshots/curves.png) |
| **Lighting** | **LCD** |
| ![The Tai Chi effect on Control Hub fans](docs/screenshots/lighting.png) | ![The Kraken LCD showing a web page](docs/screenshots/lcd.png) |

> [!WARNING]
> camless sets your pump and fan speeds, and a bad curve can overheat your hardware. It comes with no warranty (sections 15 and 16 of the [license](LICENSE)). Two safeguards hold whatever the config says. While camless runs, liquid at 55 °C or above sends every fan and pump it manages to 100 %. The curve stored on a Kraken reaches 100 % at 55 °C liquid even when camless is not running. [How it works](docs/how-it-works.md#fan-control) has the details.

camless is not affiliated with or endorsed by NZXT. NZXT, CAM and Kraken are trademarks of NZXT, Inc.

## Devices

| Device | USB id | Tested on the maintainer's hardware |
|---|---|---|
| Kraken Elite 2024 (Kraken Elite V2) | `1e71:3012` | pump and fan curves, lighting, LCD |
| Control Hub (H9 Flow RGB+ and other recent cases) | `1e71:2022` | fan duties, lighting |

The app lists other NZXT devices as "no driver yet". If you have one, *Settings → Devices → Create report* opens a GitHub issue with what a driver needs filled in, and shows you all of it first. A driver is one Rust file, and [CONTRIBUTING.md](CONTRIBUTING.md) shows how to write one.

## Install

1. Quit NZXT CAM, and turn it off in *Task Manager → Startup apps* so it does not come back at the next logon. While CAM's app runs, it holds the devices and camless cannot open them. CAM Service can keep running.
2. Run [camless-setup.exe](https://github.com/kleju13/camless/releases/latest/download/camless-setup.exe). It needs no administrator rights. It is not code-signed, so SmartScreen may stop it: *More info → Run anyway*. Or install from PowerShell, which also checks the download's SHA-256:
   ```
   irm https://raw.githubusercontent.com/kleju13/camless/main/scripts/install.ps1 | iex
   ```
3. The setup adds camless to the Start menu and opens it. The app starts the service and asks Windows for administrator rights. Answer Yes, so the service can read CPU temperature. To be asked only once, turn on *Settings → Service → Start as administrator without asking*.
4. To bring over your CAM setup, use *Profiles → Import from NZXT CAM*.

The app checks for a new release each time it starts and shows *Update available* under the service in the sidebar. *Update* there, or in *Settings → About*, installs it. Uninstall camless from *Settings → Apps* in Windows. It keeps your settings in `%APPDATA%\camless` unless you choose to delete them.

CPU temperature on AMD Zen also needs [PawnIO](https://pawnio.eu) and its signed `AMDFamily17.bin` in `%LOCALAPPDATA%\Programs\camless`. Without them, curves on CPU temperature hold the failsafe duty. The Kraken's liquid temperature needs none of this, so new curves use it by default.

## Documentation

- [Configuration](docs/config.md): `camless.toml`, profiles, curve kinds and the command line.
- [How it works](docs/how-it-works.md): the service and the app, fan control and safeguards, history and games, web pages on the LCD, the local API, and updates.
- [Protocol notes](docs/protocol.md) for each device.
- [Contributing](CONTRIBUTING.md): building, testing and adding a device.

## Not done yet

- Drivers for other NZXT devices.

## Credits

Most of what camless knows about NZXT devices comes from [liquidctl](https://github.com/liquidctl/liquidctl): its drivers, issues and pull requests. The Linux `nzxt-kraken3` driver helped too. Frames for the Kraken LCD are [Q565](https://github.com/seritools/q565), by seritools. How CAM streams them to the Kraken Elite was worked out from captures of CAM. The [protocol notes](docs/protocol.md) list the sources for each device. The history, the game sessions and the FPS counter were inspired by [Nexus](https://github.com/hello-nexus/nexus), which counts frames the same way.

## License

Copyright (C) 2026 kleju13 and camless contributors.

[GNU AGPL v3](LICENSE). Use it, change it and share it freely, at work too. If you pass on a changed version, or run one for other people over a network, you must publish its source under the same license.

The Montserrat font in `gui/ui` is under the [SIL Open Font License](gui/ui/OFL.txt).

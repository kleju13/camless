# Contributing

The maintainer can only test a Kraken Elite 2024 and a Control Hub. Every other NZXT device needs someone who owns it. A driver is one Rust file. The service, the config and the app pick a new device up from what the driver describes, so nothing else changes.

The project is under the [GNU AGPL v3](LICENSE). By opening a pull request you agree that your contribution is licensed under it too.

## Layout

| Path | What |
|---|---|
| `src/` | the `camless` crate: the service, drivers and CLI |
| `src/drivers/<model>.rs` | one device: its protocol and a `Driver` |
| `src/drivers/mod.rs` | the `Driver` and `Lcd` traits, `MODELS`, bus scanning |
| `src/drivers/hid.rs`, `hue2.rs` | shared HID I/O and the Hue 2 lighting protocol |
| `src/daemon.rs` | the service loop: curves, duties, reloads, failsafes |
| `src/web.rs`, `src/cdp.rs` | web pages on an LCD: a headless browser driven over the DevTools protocol |
| `core/` | config and profiles, curve engine, CAM import, status types; no hardware code |
| `gui/` | the app: tray icon, WebView2 window, the JSON API its page uses, and updates from GitHub releases |
| `gui/ui/` | the app's page: HTML, CSS and plain JavaScript, built into `camless-gui.exe` |
| `installer/camless.iss` | the Inno Setup script for `camless-setup.exe`, which installs, updates and uninstalls both exes |
| `scripts/` | `package.ps1` builds the setup; `install.ps1` installs the latest release |
| `.github/workflows/` | `ci.yml` tests and packages every pull request; `release.yml` publishes a release for a `v*` tag |
| `tools/capture/` | Frida scripts that log what CAM sends to a device |
| `tools/icon/` | draws the logo into `gui/camless.ico`, the icon in both exes, the tray and the window, and into `gui/icon.svg` |
| `docs/protocol.md` | protocol notes per device |

## Adding a device

1. **Find what CAM sends.** Start with [liquidctl](https://github.com/liquidctl/liquidctl): many NZXT devices are documented there, and its sources are the best reference. If it is not there, capture CAM (below).
2. **Write `src/drivers/<model>.rs`.** Use `control_hub.rs` (fans and lights only) or `kraken_elite_v2.rs` (pump, stored curves, LCD) as a template:
   - a `pub const MODEL: Model` with the USB vendor and product id, the HID interface if the device has several, a slug for the config (`kraken-x3`, `rgb-fan-controller`), and the name people know it by;
   - `describe()`: its fan channels (`pump: true` for pumps, `min_duty`, `device_curves` if it can store curves), light channels with LED counts, and the LCD;
   - `sensors()` and `poll()`: readings by the driver's own ids, such as `liquid`;
   - `set_duties()`, `set_lighting()`, and `store_curve()` / `lcd()` where the device supports them;
   - pure functions that build and parse reports, with unit tests against captured bytes.
3. **List it** in `MODELS` in `src/drivers/mod.rs`.
4. **Check it** with `camless status`. It opens the device once and prints every channel and sensor. Then use `camless run --verbose` with a small config.
5. **Set `verified: true`** in `describe()` only for what you ran on the hardware. The app shows unverified devices as such. Say in the module comment and the pull request what you tested.
6. **Add a section** to `docs/protocol.md`.

Rules for drivers:
- **Never write flash.** No firmware updates, no saving images or settings to flash. On the 2023/2024 Kraken Elite, flash uploads have left coolers stuck in the bootloader (liquidctl #774, #907, #927). Use streaming and runtime commands only.
- **Fail safe.** If the device can run a curve by itself, store a curve that still speeds up when it gets hot. The Kraken driver adds a floor from 45 to 55 °C liquid.
- **Be gentle with lighting.** Hue 2 devices drop packets sent faster than about four per half second. Use `hue2::write`, which spaces them out.
- **No new dependencies** without a reason. The service is meant to stay small.

## Capturing CAM

`tools/capture/` logs the HID reports and WinUSB transfers that CAM sends and receives, without changing them. It needs Python with `frida`:

```
python -m venv frida-venv
frida-venv\Scripts\pip install frida
frida-venv\Scripts\python tools\capture\cam_usb_capture.py tools\capture\cam_usb_agent.js cam.jsonl 120
```

Start it, then start CAM, and use the feature you want to capture within those 120 seconds. Each line of `cam.jsonl` is one transfer, tagged with its device. Bulk LCD frames are saved next to it as `.bin` files. `q565_to_png.py` turns a captured Q565 frame into a PNG.

Attaching Frida to CAM needs the same user, and administrator rights if CAM runs elevated. Before you share a capture, remove anything personal from it: serial numbers, file paths, and any images you showed on a screen.

## Building and testing

```
cargo build --release --workspace                   # camless.exe and camless-gui.exe in target/release
cargo test --workspace --release
.\scripts\package.ps1                               # both, packed into dist\camless-setup.exe; needs Inno Setup 6
camless run --demo demo.toml                        # simulated devices for working on the app
camless-gui --serve 127.0.0.1:30652 --config demo.toml --ui-dir gui/ui
```

`demo.toml` can be a copy of `gui/tests/data/demo.toml`. `--serve` shows the app's page at http://127.0.0.1:30652/ in any browser and edits only the file given with `--config`. With `--ui-dir`, the page comes from the source folder, so a reload picks up changes to `gui/ui` without a rebuild.

Run the tests before opening a pull request. A new driver needs tests for its report builders and parsers, preferably against bytes captured from CAM.

CI builds the setup for every pull request, as the run's *camless-setup* artifact. A tag `vX.Y.Z` publishes it as a release, and fails unless all three `Cargo.toml` files are at that version: the app compares its own version with the latest release.

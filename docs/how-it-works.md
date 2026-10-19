# How camless works

camless is two programs:
- `camless.exe` is the service. It finds NZXT devices, runs the curves, sets lighting and drives the LCD.
- `camless-gui.exe` is the app. It edits `camless.toml`, and the service applies every change within a second. Closing its window keeps it in the tray, and *Exit* in the tray menu quits it. Closing the app never stops fan control.

The app starts the service if it is not running. It asks Windows for administrator rights (UAC), so CPU temperature works. If the answer is no, the service runs without them and the app says so. The bottom of the sidebar always shows the service's state with a start or restart button, and *Settings → Service* starts, restarts and stops it.

## Fan control

- **Curves a device can run by itself are stored on it.** This means a fixed duty, or a graph on the device's own sensor (a Kraken's liquid temperature). The Kraken keeps running them if camless stops. The fan's offset and limits are folded in. Smoothing and hysteresis are host-only.
- **Everything else is host-driven.** camless reads sensors every `interval_ms`, runs the curve engine, and sends duties that changed. Unchanged duties are re-sent once a minute.
- **Safety floor.** Every table stored on a Kraken, including the flat ones for host-driven duties, is raised to at least a ramp from 0 % at 45 °C liquid to 100 % at 55 °C. If camless hangs, the cooler still speeds up on its own.
- **Liquid at 55 °C or above** on any device sends every managed fan and pump to 100 %.
- **Missing sensor:** the fans on that curve hold `failsafe_duty`, and the app marks them.
- **Config changes** apply within one interval. Only what changed is sent again: lighting effects do not restart when a curve moves. An invalid file is reported in the app, and the previous config keeps running.
- **On exit:** host-driven fans go to `failsafe_duty` or their fixed duty, and LCDs go back to their own screen.
- **Lost devices** (sleep, resume, replug) are reopened every 5 s.

## Web pages on the LCD

`mode = "web"` shows a page the way CAM's Web Integration does, so pages made for CAM, such as [KrakenDial](https://github.com/kleju13/krakendial), work unchanged:
- a headless Microsoft Edge (or Chrome, if there is no Edge) renders it at the screen's size, 640×640 on a Kraken Elite;
- the URL gets `?kraken=1`, which tells a page it is on the cooler;
- `window.nzxt.v1` has `width`, `height`, `shape` and `targetFps`, and `window.nzxt.v1.onMonitoringDataUpdate` gets CAM's monitoring object every second;
- what the page draws goes to the screen at up to `fps` frames per second, and a page that stops changing is sent again every second.

The browser runs as the signed-in user even when camless runs as administrator, with its own profile in `%LOCALAPPDATA%\camless\browser`, and it ends with camless. If the page does not load, the screen shows liquid temperature and camless tries again every 10 s. `camless lcd page URL` renders a page once, without a device, to check it.

A browser is the heavy part of camless: KrakenDial with a video background takes Edge about 170 MB of memory and 40 % of one CPU core. Edge runs without a GPU, as CAM's Web Integration does, which saves it about 150 MB. The other LCD modes need no browser.

Most of that CPU goes to redrawing and capturing the whole page for every frame of a moving background. A page that knows camless can hand its background over instead. It bakes the background into a loop of frames once, and `window.nzxt.v1.camless.background`, which exists only under camless, takes it:
- `begin(fps, frames)` starts a loop of 1 to 600 frames, played at 1 to 60 fps;
- `frame(index, png)` gives frame `index` as a PNG data URL at the screen's size;
- `end()` starts the loop once every frame is in. From then on the browser draws the page on a transparent background, and camless lays it over the loop, so the page should leave its own background out;
- `speed(rate)` plays the loop 0 to 8 times as fast;
- `clear()` gives the page the whole screen again, as does loading another page.

Each call returns a promise that resolves once camless has taken the message, or rejects with the reason it did not. A loop can take up to 64 MB in camless. KrakenDial's camless mode uses this. With a video background at 10 fps, Edge then takes about 140 MB and 3 % of a core, and camless 5 % playing a loop of 300 frames, against 30 to 40 % without it.

## Local API

| | |
|---|---|
| `GET /v1/status` | devices, channels, sensors, curves and duties, as `camless_core::status::Status`; the app reads this |
| `GET /v1/monitoring` | CAM's Web Integration monitoring object (`cpus`, `gpus`, `kraken`, `ram`); pages in web mode get the same through `window.nzxt.v1` |
| `POST /v1/stop` | stops the service the way Ctrl+C does: host-driven fans to `failsafe_duty` or their fixed duty, the LCD back to its own screen; `Content-Type: application/json` only, so web pages cannot send it |
| `POST /v1/lcd/frame` | one frame for the LCD in `frames` mode at its size (640×640 on the Kraken Elite), raw RGBA or RGB565 big-endian, `Content-Type: application/octet-stream`; 202, 409 without such an LCD, 429 when two frames are already waiting |

It listens on `127.0.0.1:30641` by default and binds to loopback only. It rejects foreign `Host` headers, so web pages cannot reach it through DNS rebinding.

## Installs and updates

`camless-setup.exe` (Inno Setup, `installer/camless.iss`) installs both exes for the current user in `%LOCALAPPDATA%\Programs\camless`. Run over an installed camless, it closes the app, stops the service through `POST /v1/stop`, replaces both exes and opens the app again, which starts the service.

The app asks GitHub for the latest release once each time it starts. *Update* downloads `camless-setup.exe`, checks it against the release's SHA-256, quits the app and runs the setup, so the app and the service always update together. Uninstalling keeps `%APPDATA%\camless` unless you choose to delete it.

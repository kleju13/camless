# How camless works

camless is two programs, `camless` and `camless-gui` on [Linux](#linux):
- `camless.exe` is the service. It finds NZXT devices, runs the curves, sets lighting and drives the LCD.
- `camless-gui.exe` is the app. It edits `camless.toml`, and the service applies every change within a second. Closing its window keeps it in the tray, and *Exit* in the tray menu quits it. Closing the app never stops fan control.

The app starts the service if it is not running. On Windows, it asks for administrator rights (UAC), so CPU temperature works. If the answer is no, the service runs without them and the app says so. The bottom of the sidebar always shows the service's state with a start or restart button, and *Settings → Service* starts, restarts and stops it.

*Settings → Service → Start as administrator without asking* stops the prompts. It is off until you turn it on, and Windows asks once then. The app sets up a scheduled task, `camless (<your user name>)`, that runs `camless-gui.exe --start-service` with your full rights, and from then on starts the service through it, also at logon with *Start with Windows*. The task has no trigger of its own. You may run and delete it without administrator rights, so turning the switch off and uninstalling need none. The trade-off: camless sits in a folder you can write to, so any program you run could replace it and use the task to get administrator rights without a prompt. On a standard account the switch refuses: a task that runs as you could not give the service administrator rights.

## Fan control

- **Curves a device can run by itself are stored on it.** This means a fixed duty, or a graph on the device's own sensor (a Kraken's liquid temperature). The Kraken keeps running them if camless stops. The fan's offset and limits are folded in. Smoothing and hysteresis are host-only.
- **Everything else is host-driven.** camless reads sensors every `interval_ms`, runs the curve engine, and sends duties that changed. Unchanged duties are re-sent once a minute.
- **Safety floor.** Every table stored on a Kraken, including the flat ones for host-driven duties, is raised to at least a ramp from 0 % at 45 °C liquid to 100 % at 55 °C. If camless hangs, the cooler still speeds up on its own.
- **Liquid at 55 °C or above** on any device sends every managed fan and pump to 100 %.
- **Missing sensor:** the fans on that curve hold `failsafe_duty`, and the app marks them.
- **Config changes** apply within one interval. Only what changed is sent again: lighting effects do not restart when a curve moves. An invalid file is reported in the app, and the previous config keeps running.
- **On exit:** host-driven fans go to `failsafe_duty` or their fixed duty, and LCDs go back to their own screen.
- **Lost devices** (sleep, resume, replug) are reopened every 5 s.

## Other brands

`[devices] other_brands`, off unless you turn it on, lets camless drive other brands' fan hubs and LED controllers it has a driver for: Lian Li's Uni Hubs, Uni Fan TL controller and Strimer Plus V2, Asiahorse's Umbra V2, Nollie's controllers, Corsair's iCUE LINK System Hub, HYTE's Smart Hub and NP50, and iBUYPOWER's MiniHub. None of them has run with camless yet. Their own apps drive them too, so:
- camless opens only a HID device's vendor-defined collection (usage page `FF00` and up) of a model it has a driver for. Keyboards, mice and the other input collections of the same brand never match, and camless lists no other device of that brand, not even in `camless report`. The HYTE and iBUYPOWER hubs are USB serial ports, which camless opens only for those USB ids.
- While the brand's app runs, camless leaves the hub to it, by process name, as Nexus does: L-Connect for Lian Li, Sync_light for Asiahorse, iCUE for Corsair, HYTE Nexus for HYTE and iBUYPOWER. Nollie has no app camless waits for. camless checks every 5 s. When the app starts, camless hands back what it took, as below, and closes the hub; once the app quits, camless opens the hub again. A Corsair hub that answers commands camless did not send is open in another program too: camless closes it at once, without handing it back, and takes it again only after a minute without such replies.
- A fan stays as the hub runs it until a curve takes it: on the motherboard's PWM, the hub's own curve or the duty saved in it. The Corsair hub is the exception: camless puts it in software mode when it opens it, where it has no fallback, so camless holds the channels no curve sets at 50 % for fans and 70 % for pumps, and never runs a pump below 50 % (70 % on the H100i to H170i coolers).
- Some hubs take several fans' duties in one command: then the ones no curve sets run at 70 % (the 4-pin header on the NP50, the other port on the MiniHub), and the NP50's FP12 fans at 50 %. The Umbra runs all its fans at one duty.
- The HYTE Smart Hub and the NP50 take their fans and LEDs back 5 s after the last command, so camless sends them with every poll, and an `interval_ms` above about 4000 loses them. The Lian Li Uni Hubs and TL controller, the Umbra and the Corsair hub get their duties again with every poll too.
- Lighting is off or one colour per light. camless changes a light only once the config sets it, and leaves the others as they are, except on the HYTE and iBUYPOWER hubs, which take their lights together: there the lights the config does not set go dark once it sets one. When camless lets go, the HYTE hubs go back to their own animation after 5 s and the MiniHub to the motherboard's ARGB; the others keep the last colour.
- Letting go hands the fans back: the Lian Li hubs and the Umbra to the motherboard's PWM, the TL fans to the motherboard's sync, the Corsair hub to its hardware mode, the NP50 to the mode saved in it, and the MiniHub to the motherboard. The Smart Hub's watchdog takes its fans back on its own.
- Turning the setting off lets go of the hubs within one `interval_ms`, like any config change.
- When the service stops, camless hands the hubs back as when it lets go, after it sets their fans to `failsafe_duty`, which they keep if the hand-back fails. If camless crashes, the HYTE Smart Hub and NP50 take their fans back 5 s later; the others keep their last duty until camless starts again or they lose power.
- camless sends nothing that writes a hub's flash or EEPROM: no firmware, saved modes, LED counts or settings. [Protocol notes](protocol.md) list the commands it never sends, per hub.

## History

The service records every sensor, and the speed and duty of every fan, while it runs:
- the last hour at one reading a second, in memory;
- each minute's average and highest value, in `history/` next to `camless.toml`, one CSV file per UTC day. Files older than 30 days are deleted. A day takes about 300 KB with a Kraken and a Control Hub.

*Monitoring → History* shows them for the last 10 minutes, hour, 24 hours, 7 days or 30 days, a chart for each kind: liquid temperature, CPU and GPU temperature, load, power, fan and pump speeds, and the frame rate while Games is on. Liquid has a chart of its own, scaled close around its readings, so a change of a degree shows; the line at 55 °C, where every fan goes to 100 %, shows once the readings come near it. Each series gives its reading now, its average and its highest over the range. Pointing at one chart shows that moment in all of them. Over 24 hours and more, a point stands for several minutes: the line is their average, and a shade above it shows how high they went. `camless run --demo` keeps its simulated readings in memory only.

## Games

Games is an experimental feature, off until you turn it on in *Settings → Experimental*. Not everyone wants a program that watches which game they play. Until then the service does not look at the program in front, and *Games* is not in the sidebar. Once it is on, the service follows the game in front:
- A program is a game when it runs from a Steam library (`steamapps\common`), an Xbox games folder (`XboxGames`) or an Epic Games install. `[games]` in `camless.toml` adds others and leaves some out (see [Configuration](config.md#games)).
- A session starts when a game comes to the front. It ends when the game closes, when another game comes to the front, or after 3 minutes in the background. Only time in front counts.
- Each session keeps its play time, its average frame rate, its 1 % and 0.1 % lows, the average and highest value of each temperature, and the Steam app id of a Steam game. Sessions shorter than a minute are dropped. The others go to `history/sessions.jsonl`, one JSON object a line, and stay there.
- The frame rate of the game in front also goes to the history, as the series `fps`.

Frame rates come from the presents that the graphics kernel reports through ETW: one event of `Microsoft-Windows-DxgKrnl`, Present_Info (184), of which camless reads only the process and the time. Nexus counts frames the same way, and PresentMon reads the same provider. The ETW session runs only while a game is played, needs administrator rights, and writes nothing to disk. Without administrator rights, a session still has its play time and temperatures.

ETW hands events over in batches, up to a second late, so frames are counted 2 s after they are shown. The 1 % low is the frame rate of the frame time that 1 % of frames took at least as long as: with 1 000 frames, the 10th slowest. Frame times are counted in buckets 1 % wide, so a low is within 1 %. A gap of more than a second between two frames is a pause, such as a loading screen, not a frame.

*Games*, at the bottom of the sidebar, shows the game being played, a card for each game with its average frame rate, 1 % low and play time, and the sessions by day. Picking a card shows only that game's sessions. A session opens on its own: what it came to, how its frame rate compares with the game's other sessions, and charts over it from the history: frame rate, CPU and GPU temperature, liquid temperature and load. The history keeps 30 days, so older sessions keep their numbers but lose their charts. The game being played opens the same way, and its charts go on until it closes. *Monitoring → History* charts the frame rate too.

Covers come from the Steam client's own cache on this PC, `appcache\librarycache` in Steam's folder, which holds the pictures of the games in a Steam library. camless downloads none. Games from other stores, and Steam games without a picture there, get their initials on a colour instead. `camless fps` prints the frame rate of the program in front once a second, to check that frames are counted. `camless run --demo` makes up a game in front, once Games is on, and three past sessions.

## AI assistants (MCP)

`camless mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server on stdin and stdout. An assistant such as Claude can use it to read the sensors and set fan curves. It asks the running service through the local API. It changes the config the service runs, or the selected profile if there is one. Like the app, it rewrites the whole file, so comments in it are lost.

| Tool | |
|---|---|
| `get_cooling_state` | the sensors; each fan and pump channel with its speed, duty and curve; the curves; `failsafe_duty` |
| `get_sensor_history` | up to 8 series over 10 minutes to 30 days: average, lowest, highest, last, and up to 30 points |
| `get_game_sessions` | the latest game sessions, with frame rates and temperatures |
| `set_fan_curve` | adds or replaces a graph curve on a temperature sensor and puts fans on it |
| `assign_curve` | puts fans on a curve the config already has, such as the one they had before |

A curve from `set_fan_curve` must speed the fans up as it gets hot:
- It has 2 to 16 points, and the temperature rises from each point to the next, up to 90 °C.
- The duty never falls as the temperature rises.
- The last point is at `failsafe_duty` or more.
- It takes at most 10 s to follow a rise and 60 s to follow a fall, with at most 5 °C of hysteresis.

The safeguards under [Fan control](#fan-control) still apply on top. The server cannot change fan limits, lighting, the LCD or any other setting. After a change, it waits for the service to read the config, then reports any error and the curve's duty.

To add it to Claude Code:

```
claude mcp add camless -- C:\Users\<you>\AppData\Local\Programs\camless\camless.exe mcp
```

On Linux, the command is `~/.local/bin/camless mcp`.

Or to Claude Desktop, in `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "camless": {
      "command": "C:\\Users\\<you>\\AppData\\Local\\Programs\\camless\\camless.exe",
      "args": ["mcp"]
    }
  }
}
```

Nexus has an MCP server for its cooling too, and this one was inspired by it.

## Web pages on the LCD

`mode = "web"` shows a page the way CAM's Web Integration does, so pages made for CAM, such as [KrakenDial](https://github.com/kleju13/krakendial), work unchanged:
- a headless Microsoft Edge (or Chrome, if there is no Edge) renders it at the screen's size, 640×640 on a Kraken Elite. On Linux it is the first of Chrome, Chromium, Edge and Brave that is installed, or Ubuntu's Chromium snap, which has not been tried yet;
- the URL gets `?kraken=1`, which tells a page it is on the cooler;
- `window.nzxt.v1` has `width`, `height`, `shape` and `targetFps`, and `window.nzxt.v1.onMonitoringDataUpdate` gets CAM's monitoring object every second;
- what the page draws goes to the screen at up to `fps` frames per second, and a page that stops changing is sent again every second.

The browser runs as the signed-in user even when camless runs as administrator, with its own profile in `%LOCALAPPDATA%\camless\browser` (on Linux `~/.local/share/camless/browser`), and it ends with camless. On Linux camless drives it through two pipes (`--remote-debugging-pipe`) rather than a debugging port on 127.0.0.1, which other users of the computer could reach too. If the page does not load, the screen shows liquid temperature and camless tries again every 10 s. `camless lcd page URL` renders a page once, without a device, to check it.

A browser is the heavy part of camless: KrakenDial with a video background takes Edge about 170 MB of memory and 40 % of one CPU core. Edge runs without a GPU, as CAM's Web Integration does, which saves it about 150 MB. The other LCD modes need no browser.

Most of that CPU goes to redrawing and capturing the whole page for every frame of a moving background. A page that knows camless can hand its background over instead. It bakes the background into a loop of frames once, and `window.nzxt.v1.camless.background`, which exists only under camless and only in the page itself, not in frames inside it, takes it:
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
| `GET /v1/history?span=<seconds>&ids=<id>,<id>` | the readings over the last `span` seconds (600 by default, up to 30 days), or from `from` to `to` in Unix seconds in place of `span` (`to` is now when left out), at most 721 points a series: `from` and `step` in seconds, `count`, and for each series id its `avg` and `max` at every point, `null` where there was no reading. Ids are sensor ids, and `<device>/<channel>.rpm` or `.duty` for fans; without `ids`, every series |
| `GET /v1/games` | the game sessions kept, newest first, as `{"sessions": [...]}`. Each has `game`, `exe`, `start` and `end` in Unix milliseconds, `played` in seconds, `frames`, `fps`, `low1` and `low01` (`null` without frame rates), `temps`: `avg` and `max` by sensor id, and `steam`, the Steam app id, for a Steam game. The game being played is `game` in `/v1/status` |
| `GET /v1/monitoring` | CAM's Web Integration monitoring object (`cpus`, `gpus`, `kraken`, `ram`); pages in web mode get the same through `window.nzxt.v1` |
| `POST /v1/stop` | stops the service the way Ctrl+C does: host-driven fans to `failsafe_duty` or their fixed duty, the LCD back to its own screen; `Content-Type: application/json` only, so web pages cannot send it |
| `POST /v1/lcd/frame` | one frame for the LCD in `frames` mode at its size (640×640 on the Kraken Elite), raw RGBA or RGB565 big-endian, `Content-Type: application/octet-stream`; 202, 409 without such an LCD, 429 when two frames are already waiting |

It listens on `127.0.0.1:30641` by default and binds to loopback only. It rejects foreign `Host` headers, so web pages cannot reach it through DNS rebinding.

## Installs and updates

`camless-setup.exe` (Inno Setup, `installer/camless.iss`) installs both exes for the current user in `%LOCALAPPDATA%\Programs\camless`. Run over an installed camless, it closes the app, stops the service through `POST /v1/stop`, replaces both exes and opens the app again, which starts the service.

The app asks GitHub for the latest release once each time it starts. *Update* downloads `camless-setup.exe`, checks it against the release's SHA-256, quits the app and runs the setup, so the app and the service always update together. Uninstalling keeps `%APPDATA%\camless` unless you choose to delete it.

*Settings → About → Update channel* picks what the app offers:
- **Stable**, the default, offers releases.
- **Beta** offers betas too: GitHub pre-releases such as 0.5.0-beta.1, then beta.2 and so on. Once they have been tried, the version itself comes out as the release, 0.5.0.

Back on Stable, camless keeps the version it has until a newer release comes out. `install.ps1` always installs the latest release, never a beta. In `camless.toml` the channel is `updates = "beta"`. On Linux, `install.sh` installs and updates camless; [Linux](#linux) has the details.

## Linux

The same service and app, built for Linux. They have not run on NZXT hardware yet. What was tested: the build and the tests on Ubuntu 22.04, and on 24.04 `install.sh`, the systemd service, updates over it, `uninstall.sh` and the app with `camless run --demo`. What differs from Windows:

- **The service** runs as you, as the systemd user service `camless.service`, which `install.sh` sets up to start when you log in. Without a systemd user session, the app starts it, as on Windows. *Settings → Service* starts, restarts and stops it either way. `systemctl --user status camless` and `journalctl --user -u camless` show how it runs.
- **Devices.** camless opens their hidraw nodes, and usbfs for the LCD and the Asetek-made Krakens. The device files belong to root. `71-camless.rules` tags the models camless has a driver for, and no others, with `uaccess`, which gives them to the user logged in at the computer, as liquidctl's `71-liquidctl.rules` does. A test checks that the rules list exactly the drivers' models. The service drives the devices while you are logged in at the computer, not at the login screen or over SSH alone; a Kraken runs its stored curves meanwhile. CAM does not run on Linux, so nothing holds the devices. A kernel hwmon driver such as `nzxt-kraken3` can stay bound, as hidraw reaches the device beside it.
- **Sensors**, read as Nexus reads them on Linux. CPU temperature comes from hwmon: Tctl from k10temp or zenpower on AMD, the package from coretemp on Intel. Load comes from `/proc/stat` and memory from `/proc/meminfo`. CPU power comes from RAPL's energy counter, which only root can read since the PLATYPUS side channel, so the service usually has none. NVIDIA GPUs are read through NVML (`libnvidia-ml.so.1`, from the driver), AMD GPUs through amdgpu's files in sysfs.
- **Other brands.** A Lian Li hub's feature reports go out as they are, 7 bytes, where Windows pads them to the hub's report length, and its speeds come back through `HIDIOCGINPUT`, as in Nexus's `LinuxHidDevice`. The HYTE and iBUYPOWER hubs are `ttyACM` ports, found in `/sys/class/tty` by USB id and opened exclusively. The udev rules also tell ModemManager to leave them alone, as it would probe them for a modem.
- **Games** come from Steam libraries (`steamapps/common`): native games, and Windows games under Proton or Wine, whose path camless reads from the Windows path Wine runs them by, where `Z:\` is `/`. Proton and the Steam Linux Runtime are not games. Epic and Xbox games are not found; `[games] add` adds others. camless does not ask which window is in front (Wayland does not say), so a game counts as played while it runs, and its session ends when it closes. There are no frame rates, which come from ETW on Windows, and no `camless fps`. Covers come from Steam's cache in `~/.steam/steam`, `~/.local/share/Steam` or the Flatpak's `~/.var/app/com.valvesoftware.Steam/.local/share/Steam`, where Nexus looks for Steam.
- **The app** is a GTK window with the page in WebKitGTK, and a tray icon through AppIndicator. The icon shows only on a desktop that shows such icons (it asks for `org.kde.StatusNotifierWatcher` on the session bus): KDE Plasma, or GNOME with the AppIndicator extension. Without one, the window is the app, and closing it quits the app while the service keeps running. A second launch brings the open window to the front. *Start at login* writes `~/.config/autostart/camless.desktop`, which opens the app in the tray. WebKit draws the page on the CPU, which takes its web process about 200 MB instead of 430 MB (measured on Xvfb): the app sets `WEBKIT_DISABLE_COMPOSITING_MODE=1` unless it is set already. If the window stays blank on NVIDIA's driver, start the app with `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
- **Updates.** The app downloads `camless-linux-x86_64.tar.gz`, checks it against the release's SHA-256, unpacks it in `~/.cache/camless/update`, quits and runs its `install.sh --update`. That replaces both programs, restarts the systemd service (without one, it restarts the service the app started, from the new program, with the same config), installs changed udev rules if the desktop can ask for your password (pkexec), and opens the app again. It logs to `~/.local/state/camless/update.log`. Releases without the Linux tarball are not offered. The app updates itself only if it can write the folder it runs from.

| Linux | |
|---|---|
| `~/.local/bin` | `camless` and `camless-gui` (`CAMLESS_BIN_DIR` for `install.sh` puts them elsewhere) |
| `~/.config/camless` | `camless.toml`, the history and the game sessions (`$XDG_CONFIG_HOME/camless`) |
| `~/.config/systemd/user/camless.service` | the service, when there is a systemd user session |
| `/etc/udev/rules.d/71-camless.rules` | access to the devices camless drives |
| `~/.local/share/camless` | the app's WebKit data, the LCD browser's profile and `uninstall.sh` |
| `~/.cache/camless/update`, `~/.local/state/camless/update.log` | the last update and its log |

`~/.local/share/camless/uninstall.sh` removes all of it except `~/.config/camless`.

# How camless works

camless is two programs:
- `camless.exe` is the service. It finds NZXT devices, runs the curves, sets lighting and drives the LCD.
- `camless-gui.exe` is the app. It edits `camless.toml`, and the service applies every change within a second. Closing its window keeps it in the tray, and *Exit* in the tray menu quits it. Closing the app never stops fan control.

The app starts the service if it is not running. It asks Windows for administrator rights (UAC), so CPU temperature works. If the answer is no, the service runs without them and the app says so. The bottom of the sidebar always shows the service's state with a start or restart button, and *Settings → Service* starts, restarts and stops it.

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
- a headless Microsoft Edge (or Chrome, if there is no Edge) renders it at the screen's size, 640×640 on a Kraken Elite;
- the URL gets `?kraken=1`, which tells a page it is on the cooler;
- `window.nzxt.v1` has `width`, `height`, `shape` and `targetFps`, and `window.nzxt.v1.onMonitoringDataUpdate` gets CAM's monitoring object every second;
- what the page draws goes to the screen at up to `fps` frames per second, and a page that stops changing is sent again every second.

The browser runs as the signed-in user even when camless runs as administrator, with its own profile in `%LOCALAPPDATA%\camless\browser`, and it ends with camless. If the page does not load, the screen shows liquid temperature and camless tries again every 10 s. `camless lcd page URL` renders a page once, without a device, to check it.

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
- **Beta** offers betas too. A version that changes only the last number, such as 0.4.1, comes out first as a GitHub pre-release. It becomes a release once it has been tried.

Back on Stable, camless keeps the version it has until a newer release comes out. `install.ps1` always installs the latest release, never a beta. In `camless.toml` the channel is `updates = "beta"`.

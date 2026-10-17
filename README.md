# camless

A small replacement for NZXT CAM for one setup: a Kraken Elite V2 (2024, `1e71:3012`) and an NZXT Control Hub (`1e71:2022`). It runs pump and fan curves, sets lighting, and keeps the LCD either on the cooler's own liquid screen or on frames posted over a local API.

One 1.2 MB executable. Measured while idle on the API: 0.2 % of one core and 30 MB of RAM. CAM 4.76.6 on the same machine uses about 1.2 GB of RAM, and its Web Integration LCD path alone takes about 45 % of one core.

> Status: built and unit-tested (27 tests). The host side (sensors, API, CAM import) has been run on the target machine. **Nothing has been sent to the devices yet:** that needs CAM stopped.

## Commands

```
camless list                 NZXT HID devices that are connected
camless watch [seconds]      listen to status reports, sending nothing
camless sensors              read CPU, GPU and memory sensors once
camless import-cam [dir]     print a camless.toml built from CAM's active profiles (-o FILE to write it)
camless check [config]       validate a config and show what run would send, without opening any device
camless status               initialize both devices and print their state
camless run [config]         apply the config and keep fans on their curves (--verbose prints every interval)
camless lcd liquid           put the Kraken's own liquid temperature screen back
camless lcd test [seconds]   stream red, green, blue and white quarters with a moving bar (default 20 s)
```

The default config is `camless.toml` next to `camless.exe`.

## Config

```toml
interval_ms = 1000
failsafe_duty = 70            # host-driven fans whose sensor is missing, and every host-driven fan on exit

[api]                         # omit to turn the API off; loopback only
listen = "127.0.0.1:30641"

[pump]
source = "cpu"                # liquid | cpu | gpu | fixed
curve = [[20, 61], [65, 61], [70, 71], [80, 82]]

[radiator]                    # the Kraken's fan header
source = "liquid"
curve = [[30, 20], [40, 41], [50, 75], [55, 100]]

[[hub_fan]]
port = 1
name = "bottom"
source = "gpu"
curve = [[45, 25], [80, 80]]

[[hub_fan]]
port = 4
source = "fixed"
duty = 100

[ring]                        # off | fixed | fading | spectrum-wave | covering-marquee | pulse | breathing
mode = "tai-chi"              # candle | rainbow-flow | super-rainbow | rainbow-pulse | tai-chi | water-cooler
colors = ["3232de", "ffffff"]
speed = "normal"              # slowest | slower | normal | faster | fastest

[[hub_led]]
port = 3
mode = "fixed"
colors = ["aa00ff"]

[lcd]
mode = "liquid"               # liquid: the cooler draws it; api: frames from POST /v1/lcd/frame
brightness = 100
orientation = 0
```

`camless import-cam` writes this from CAM's own files in `%APPDATA%\NZXT CAM\DataStorage\latest\user`: the active master profile, its cooling curves, fan nicknames, per-channel lighting and the LCD mode. It drops curve points that lie on a straight line between their neighbours (within 0.5 %).

## How the control works

- **Liquid-sourced and fixed Kraken channels are stored on the cooler** as its 40-point table (20 to 59 °C liquid). The cooler runs them itself, so they keep working if camless stops.
- **CPU- and GPU-sourced channels and every hub fan are host-driven.** The Control Hub only takes fixed duties. camless reads sensors every `interval_ms`, smooths CPU and GPU temperature (rises within a few seconds, settles over about half a minute) and writes a duty only when it changes.
- **Safety floor.** Every table camless stores on the Kraken, including the flat ones it writes for host-driven pump and fan duties, is raised to at least a ramp from 0 % at 45 °C liquid to 100 % at 55 °C. If camless hangs or is killed, the cooler still speeds up on its own as the liquid warms.
- **Liquid at 55 °C or above** sends every channel to 100 %.
- **Missing sensor:** the channel holds `failsafe_duty`.
- **On exit** (Ctrl+C, console close, logoff, shutdown): host-driven channels go to `failsafe_duty`, and the LCD goes back to the liquid screen.
- **Lost devices** (sleep, resume, replug) are reopened every 5 s, and everything is applied again.

## Sensors

- GPU: NVML (`nvml.dll` from the NVIDIA driver): temperature, load, power, name.
- CPU load and memory: `GetSystemTimes`, `GlobalMemoryStatusEx`.
- CPU temperature and package power on AMD Zen: [PawnIO](https://pawnio.eu) with the signed `AMDFamily17.bin` module from PawnIO.Modules. It reads the SMN register `0x59800` and the RAPL MSRs `0xC0010299` and `0xC001029B`, and serializes on `Global\Access_PCI` with HWiNFO and LibreHardwareMonitor. It needs administrator rights. Without them, CPU-sourced channels hold `failsafe_duty`.

## Local API

| | |
|---|---|
| `GET /v1/monitoring` | CAM's Web Integration monitoring object (`cpus`, `gpus`, `kraken.liquidTemperature`, `ram.inUse`/`totalSize` in MB) |
| `GET /v1/status` | sensors, Kraken and hub readings, current target duties |
| `POST /v1/lcd/frame` | one 640×640 frame, raw RGBA (1 638 400 B) or RGB565 big-endian (819 200 B), `Content-Type: application/octet-stream`; camless encodes it to Q565. 202, or 429 when two frames are already waiting |

The API binds to loopback only and rejects foreign `Host` headers (421), so web pages cannot reach it by DNS rebinding. Browsers cannot send `application/octet-stream` cross-site without a preflight, which camless never approves. If no frame arrives for 5 s, the LCD returns to the liquid screen.

## Protocol notes

Sources:
- liquidctl's `kraken3.py` and PRs #882 (lighting, 2024 Elite), #927 and #932;
- the Linux `nzxt-kraken3` driver;
- strings and tables in CAM 4.76.6's `@nzxt/cam-core` 71.9.3 (`index.node`, a napi-rs Rust addon), `service.exe` and the Electron bundles, inspected statically with [rea](https://github.com/morluto/rea) and plain string extraction;
- CAM's own profile files.

**HID framing**
- Reports are numbered, and the report ID is the first command byte. Payloads go out as they are, padded to 64 bytes.
- CAM's service opens both devices exclusively. Another process can open them but gets `Access is denied` on the first read.

**Kraken Elite V2**
- Init: `70 02 01 B8 01`, then `70 01`.
- Firmware: `10 01` → `11 01`, version in bytes 17..19.
- Status: `74 01` → `75 01`. Periodic `75 02` reports also arrive. Liquid temperature = `[15] + [16]/10`; `ff ff` means faulty. Pump rpm is LE at `[17]`, pump duty at `[19]`; fan rpm is LE at `[23]`, fan duty at `[25]`.
- Curves: `72 01 01 00` (pump) or `72 02 01 01` (fan), followed by 40 duties for 20..59 °C liquid. The pump minimum is 20 %.
- Lighting:
  - Static colour (matches a CAM capture): `26 04 <ch> 00` + 24 × GRB, then `26 06 <ch> 00 01 00 00 18 00 00 80 00 32 00 00 01`.
  - Effects use `2A 04 <ch> <ch> <mode> <speed:2>` + 16 × GRB + `<dir> <count> <mode-flag> 18 03`.
  - Ring = `0b001`, RGB header = `0b010`.
  - More than about 4 packets per 0.5 s are dropped, and so are packets sent during LCD transfers.
- LCD:
  - Info: `30 01` → `31 01`, brightness at `[0x18]`, orientation at `[0x1A]`. Settings: `30 02 01 <brightness> 00 00 01 <quarter turns>`.
  - Liquid screen: `38 01 02 00`.
  - Streaming, per frame, as captured from CAM 4.76.6 on firmware 1.2.0:
    1. `36 01 00 01 08`;
    2. on interface 0 (WinUSB), endpoint `0x02`, two bulk transfers: first the 20-byte header `12 FA 01 E8 AB CD EF 98 76 54 32 10` + `08 00 00 00` + the u32 LE length, then the frame;
    3. `36 02`.
  - Frames are 640×640 [Q565](https://github.com/seritools/q565) (QOI adapted to RGB565, red in the top bits), format `0x08`. camless's encoder gives the same bytes as CAM's for every captured frame.
  - The cooler acknowledges raw RGB565 (format `0x06`) the same way but never shows it.
  - No display mode command is needed: the screen switches to the stream when a frame arrives. CAM goes straight from `38 01 02 00` to streaming. Each frame takes about 40 ms.
  - `36 03` cancels a transfer. Replies are `[cmd+1, sub]` with a result in byte 14 (`01` ok).
- CAM's image formats in cam-core: GifImage 0, GifAnimation 1, Rgba8888Be 2, CompressedRaw1 3, CompressedRaw2 4, Rgb888Be 5, Rgb565Be 6, Rgb565Le 7, Q565 8, Rgb888Le 9.
- CAM's display modes: Off, BootUpAnimation, FactoryBackupGraphic (the liquid screen), FlashSavedGraphic, MemoryBucket, StreamingBuffer.
- camless uses only streaming. Memory-bucket uploads write flash and have left 2023/2024 Elites stuck in the bootloader (liquidctl #774, #907, #927).

**Control Hub**
- Init: `60 02 01 E8 01 01 E8 01`, then `66 01`.
- Firmware: `10 02` → `11 02`, version at `[0x11..0x13]`.
- Periodic `67 02` reports: fan mode at `[16+i]`, rpm LE at `[24+2i]`, duty at `[40+i]`, noise at `[56]`.
- Duties: `62 01 <mask> d1..d5`.
- LED ports 1..5 are channel bytes `02 01 04 08 10`.

**Why CAM is heavy here**
- Web Integration renders the page in an offscreen Electron window, in software (SwiftShader), because hardware acceleration is off.
- Every paint at 40 fps is resized, copied over IPC, base64-encoded in the renderer, then converted and sent.
- camless leaves rendering to whoever posts frames.

## Not done yet

- Something that renders KrakenDial without CAM and posts frames here.
- Running at logon with administrator rights (a scheduled task).

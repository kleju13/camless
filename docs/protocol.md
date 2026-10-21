# Protocol notes

Sources:
- liquidctl's `kraken3.py` and PRs #882 (lighting, 2024 Elite), #927 and #932;
- the Linux `nzxt-kraken3` driver;
- liquidctl's `kraken2.py` and `asetek.py` with their guides, and the Linux `nzxt-kraken2` driver, for the Kraken X2, the M22 and the Asetek-made Krakens, which Nexus leaves out;
- [Nexus](https://github.com/hello-nexus/nexus) (`KrakenModel.cs`, `KrakenProtocol.cs`, `KrakenHub.cs`), for the Krakens camless has not run on. Nexus says only its Elite 2024 rows were run on hardware;
- Nexus's `Peripherals/LianLi`, `Cooling/LianLiCoolingProvider.cs` and `Conflicts/ConflictAppDefinitions.cs`, which follow FanControl.LianLi and L-Connect 3, for the Lian Li Uni Hubs;
- Nexus's `Peripherals` and `Lighting` handlers for the other brands' hubs: `LianLiTl`, `Strimer`, `AsiahorseUmbra`, `Nollie`, `CorsairLink` and `Hyte`. Each section below names its files;
- strings and tables in CAM 4.76.6's `@nzxt/cam-core` 71.9.3 (`index.node`, a napi-rs Rust addon), `service.exe` and the Electron bundles, inspected statically with [rea](https://github.com/morluto/rea) and plain string extraction;
- CAM's own profile files.

**HID framing**
- Reports are numbered, and the report ID is the first command byte. Payloads go out as they are, padded to at least 64 bytes. Some are longer: the Control Hub's static colour is 76 bytes.
- The NZXT CAM app opens both devices exclusively. Another process can open them but gets `Access is denied` on the first read. CAM Service (`service.exe`) does not hold them: camless drives both devices while it runs.

**On Linux** (not yet run on hardware)
- The same bytes go out. HID goes through hidapi's hidraw backend (hidapi 0.15, built in). A hidraw node is a whole USB interface, and hidapi lists each top-level collection in its report descriptor, so camless matches a usage page as on Windows, then opens the interface's node. The LCD's bulk interface and the Asetek-made Krakens go through usbfs, where Windows needs WinUSB.
- The device files belong to root. `linux/71-camless.rules` tags the hidraw and USB nodes of the drivers' models, and no others, with `uaccess`, as liquidctl's `71-liquidctl.rules` does: systemd then gives them to the user at the seat. liquidctl's file notes that a hidraw node can keep its old rights until the device is plugged in again or the PC restarts, so `install.sh` says so.
- `camless report` reads the configuration descriptor from sysfs (`descriptors`) and the report descriptor from `/sys/class/hidraw/<node>/device/report_descriptor`, which need no access to the device.
- Kernel drivers: `nzxt-kraken3` and `nzxt-kraken2` bind to the Krakens as hwmon drivers and start them with `HID_CONNECT_HIDRAW`, so the hidraw node stays, and camless reaches the device beside them.
- The HYTE and iBUYPOWER hubs are USB serial ports (`cdc_acm`, `/dev/ttyACM*`). camless finds them under `/sys/class/tty` by the USB ids above each port's `device`, as Nexus's `LinuxSerialDiscovery` does, and opens them exclusively (`TIOCEXCL`) in raw mode. The udev rule for them also sets `ID_MM_DEVICE_IGNORE`, so ModemManager does not probe them with modem commands.

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

**Kraken Plus 2024** (`1e71:3014`, not verified)
- The Elite V2 driver, on Nexus's word alone. Nexus lists it as a second Elite V2 with the 640×640 Q565 screen, and says CAM's driver treats the two as one device. liquidctl calls it the Kraken 2024 Plus with a 240×240 screen, but gives it no lighting channels and streams no pictures to it.
- camless asks: `30 01` → `31 01` has the width and height, u16 LE, at `[20]` and `[22]` (Nexus `DecodeLcdInfo`, from its Elite V2). If the cooler does not answer, 240×240.
- LED channels: as many as `21 03` reports at `[14]`, at most two, as ring (`0b001`) and RGB header (`0b010`). None if it reports none. The table can come back empty right after reporting starts, so camless asks up to three times, as Nexus does. Each channel goes by the name of its first accessory, as in Nexus.

**Kraken X3, Z3, 2023 and Elite 2023** (`1e71:2007`, `2014`, `3008`, `300e`, `300c`; not verified)
- Init, firmware, status layout and the 40-point curves are the Elite V2's. The same safety floor goes under every table.
- Status: the Z3 and the 2023 models answer `74 01`. The X3 does not: the Linux driver never sends it one, and reads the `75` reports it sends every 0.5 s.
- Curves: the X3 and Z3 take `72 01 00 00` (pump) and `72 02 00 00` (fan, Z3 only). The Kraken 2023 and Elite 2023 take the Elite V2's `72 01 01 00` and `72 02 01 01`.
- Lighting, only on the X3 and Z3: RGB header `0b001`, and on the X3 the pump ring `0b010` and logo `0b100`. Effects as on the Elite V2, with liquidctl's per-channel value in the footer where CAM puts the LED count: 40, 8 and 1. A static colour is `2A 04` too, liquidctl's `fixed` mode: mode `00`, speed `32 00`, one GRB slot, footer `00 01 00 <value> 03`. Off is the same in black. The 2023 models have no lighting.
- LCD: settings `30 02 01 <brightness> 00 00 01 <quarter turns>` and the liquid screen `38 01 02 00`, as on the Elite V2.
  - Kraken 2023, 240×240, on firmware 2 (liquidctl checks for 2 exactly): `36 01 00 01 06`, the header with format `06`, raw RGB565 high byte first, then `36 02` (liquidctl `_send_2023_data_fw2`). liquidctl sends each picture twice, as the cooler needs that once after init; camless sends the first frame of a stream twice.
  - Elite 2023, 640×640, from firmware 2: the same with format `09`, three bytes a pixel in B, G, R order (Nexus `PushDirectFrame`). This rests on Nexus alone. liquidctl lists the cooler as "NZXT Kraken 2023 Elite (broken)" and sends it pictures only through the buckets, and Nexus's own model table still lists it as Q565. CAM's format table names format 9 `Rgb888Le`, which fits B, G, R in byte order.
  - Both sources turn the picture before sending it, as the screen's rotation applies only to its own display, so camless does too.
  - The Z3, and the 2023 models on firmware 1, take pictures only through memory buckets, which are flash. camless offers only their own display there.
- Where the sources disagree:
  - X3 and Z3 curves: liquidctl and Linux send `01 00 00` and `02 00 00`; Nexus sends the Elite V2's tuples to every model. camless follows liquidctl and Linux, which have driven these coolers for years.
  - Kraken 2023 and Elite 2023 curves: liquidctl and Linux send the newer tuples on any firmware. Nexus says they moved at firmware 2.1.1 and sends `01 00 00` and `02 00 00` before it. camless follows liquidctl and Linux. On firmware older than 2.1.1, if a curve does not take, Nexus's tuples are the first thing to try.
  - RGB565 byte order on the Kraken 2023: liquidctl sends the high byte first, Nexus the low byte. CAM's own format list names format 6 `Rgb565Be`, so camless follows liquidctl.
  - Before streaming to the Elite 2023, Nexus deletes all 16 memory buckets. camless leaves them alone, since that writes flash. If the cooler refuses frames while a stored picture is up, that is the likely reason.
  - Replies: liquidctl reads one report after `36 01` and after `36 02` and does not look at it. Nexus waits up to 150 ms and needs none. It says the Elite 2023 on firmware 2 answers successful commands with `05` in byte 14, where the Elite 2024 sends `01`, and that a refusal comes as report `ff` with the command in bytes 14 and 15. camless waits up to 200 ms, takes any `37` or `39` reply as success, and fails only on that refusal. On the Elite 2024, `ff` reports after lighting commands are acknowledgements (see below), so camless reads `ff` as a refusal only for LCD commands on these models.

**Kraken X2 and M22** (`1e71:170e`, `1715`; experimental)
- An older command set than the X3's: no `10 01` firmware request, no `74 01` status, no `72` curves. Nexus leaves the X2 out for that reason. Everything here follows liquidctl's `kraken2.py`.
- Writes are padded to 65 bytes, report id `02` first. The cooler keeps every setting while it has power, even in S5 (liquidctl's guide), so none of them is flash.
- Status: report `04`, sent twice a second without being asked (the Linux driver). Liquid = `[1] + [2]/10`, fan rpm BE at `[3]`, pump rpm BE at `[5]`, firmware at `[0xb..0xf]`. No duties. CAM's short version is `fw0.fw3` from firmware 5 on, `fw0.fw2.fw3` before. The M22 sends the report too, as liquidctl reads its firmware from it, but has no sensor or speeds.
- Curves, from firmware 3 (liquidctl's check; its guide says 4): 36 points `02 4d <base + i> <temp> <duty>`, base `80` for the fan and `c0` for the pump, at 20..49 °C, then 50, 52, 54, 56, 58 and 60. Both channels must use the same temperatures, a firmware limit liquidctl names, so on opening camless stores liquidctl's reset curves on both, as liquidctl's initialization does. The safety floor of the later Krakens goes under every point, which reaches 100 % at 55 °C.
- A fixed duty is a flat curve, as liquidctl sets one, so the floor still holds if camless stops. camless sends it only when the duty changes, as it is 36 writes. On firmware before 3, `02 4d <base & 70> 00 <duty>`, which liquidctl says does not persist.
- Minimum duties from liquidctl's guide: 25 % fan, 50 % pump. The code accepts 0..100; camless follows the guide.
- Lighting: `02 4c <dir | ch> <mode> <speed | step << 5>`, then the logo as G, R, B and eight ring LEDs as R, G, B. Channels: logo `01`, ring `02`. A backward direction adds `10`. One report per color step, each setting all nine LEDs to its color. Modes camless uses: fixed and off `00`, fading `01`, spectrum wave `02`, covering marquee `04`, breathing `06`, pulse `07`, tai chi `08`, water cooler `09`; the last three and covering marquee run on the ring only. The X2 has none of the Hue 2 effects that came later (candle, the rainbows), so its lights list the modes they take and the app offers only those.
- Nexus says lighting writes are limited to one per 500 ms, without a source; liquidctl sends the steps back to back. camless sends lighting only when it changes, as liquidctl does.

**Kraken X31, X41, X61, X40 and X60** (`2433:b200`; experimental)
- Asetek 690LC pumps behind a Silicon Labs USBXpress bridge: vendor class, not HID. Bulk out `02`, bulk in `82` with 32-byte replies. On Windows the cooler needs WinUSB bound to it (Zadig), as liquidctl's guide says; camless says so when it finds another driver there. EVGA's CLC coolers share the id with newer 690LC firmware; camless sends only the legacy commands, liquidctl's `Legacy690Lc`.
- Vendor request `02` to the device: value `02` (clear to send) once on opening, value `01` (flush) before every transaction. A transaction is the writes, then one 32-byte read.
- Setup and lighting: `10` + color 1, color 2, the alert color (R, G, B) + alert temperature, interval 1, interval 2, on, fading, blinking, alert, `00 01`. camless keeps the lights on and the alert at liquidctl's 45 °C in red: liquidctl's blackout would also hide the alert. Off is a fixed black. Fading takes 1 to 10 s per color from the speed, liquidctl's default being 5.
- Duties: `13 <duty>` for the pump (50..100 %) and `12 <duty>` for the fan (0..100 %). They store no curves, so camless drives both from the host and sends both with every poll, as liquidctl does with every status read. Until the first duty, 100 %.
- Reply: fan rpm BE at `[0]`, pump rpm BE at `[8]`, liquid = `[10] + [14]/10`, firmware `[0x17..0x1b]`.
- Command `21` stores the settings in flash on the newer 690LC. camless never sends it.

**Control Hub**
- Init: `60 02 01 E8 01 01 E8 01`, then `66 01`.
- Firmware: `10 02` → `11 02`, version at `[0x11..0x13]`.
- Periodic `67 02` reports: fan mode at `[16+i]`, rpm LE at `[24+2i]`, duty at `[40+i]`, noise at `[56]`.
- Duties: `62 01 <mask> d1..d5`.
- LED ports 1..5 are channel bytes `02 01 04 08 10`.

**Smart Device (V1) and Grid+ V3** (`1e71:1714`, `1711`; experimental)
- The Kraken X2's generation, before Hue 2: none of the Control Hub's commands. Nexus leaves both out. Everything here follows liquidctl's `smart_device.py` (`SmartDevice`), its tests and guide, and the liquidtux `nzxt-grid3` hwmon driver. The Smart Device has three fan ports and one LED channel, the Grid+ V3 six fan ports and no LEDs.
- Writes are padded to 65 bytes, as liquidctl does; the report descriptor (liquidtux `Documentation/internal/1e71:1714.rdesc`) has output reports `01` of 1 byte and `02`, `03` of 64. The device keeps every setting while it has power, even in S5 (liquidctl's guide), so none of them is flash.
- Start: `01 5c` detects the fans and LED accessories, then `01 5d` starts the status reports. Before that the device sends no reports, runs every fan at 40 % and honours duties only sometimes (the Linux driver). camless sends both on every open, as liquidctl's `initialize` does. liquidctl skips them while the liquidtux `nzxt-grid3` driver is bound, which sends them when it loads and then sets every fan to 40 %; that driver is not in the mainline kernel, and camless does not check for it.
- Status: report `04`, 21 bytes, five times a second, one fan per report in turn. Fan = `[15] >> 4` (0-based), mode = `[15] & 3` (1 DC, 2 PWM, 0 not detected), rpm BE at `[3]`, volts = `[7] + [8]/100`, amps = `[9] + [10]/100`, noise at `[1]`, firmware at `[0xb..0xf]`, shown as `fw0.fw3`. On the Smart Device, `[0x11]` is the number of LED accessories and `[0x10] >> 3` their type: 0 HUE+ strips, 10 LEDs each, 1 Aer RGB fans, 8 each. A port the device did not detect is still measured: liquidctl's sample has 1390 rpm on one, so camless counts a port with a speed as connected. The device does not report duties.
- Duty: `02 4d <port> 00 <duty>`, 0..100 %. No curves on the device: camless drives the fans from the host, and a fan keeps its last duty when camless stops.
- Lighting: `02 4b <mode> <dir> <speed | step << 5>` with the first 57 bytes of 40 LEDs as G, R, B, then `03` with the other 63. One pair per color step, each setting all 40 LEDs to its color. Backward adds `10` to the direction byte. Modes camless uses: fixed and off `00`, fading `01`, spectrum wave `02`, covering marquee `04`, pulse `06`, breathing `07`, candle `09`. The device has none of the rainbows, tai chi or water cooler, so its light lists the modes it takes.

**Lian Li Uni Hubs** (`0cf2:7750`, `a100` to `a106`; experimental, with `[devices] other_brands` on)
- Four fan ports. camless opens only the vendor collection on interface 1: usage page `FF72`, usage `A1`. Every command is a feature report, id `E0`, 7 bytes, which Windows pads to the hub's feature report length. Nexus calls its Lian Li support experimental and ran the SL-Infinity (`a102`) and SL (`a100`) hubs only; the other rows come from its table.
- Per hub: SL (`7750`, `a100`, the Redragon OEM `a106`) mode register `31`, speeds from byte 1; AL (`a101`) register `42`, speeds from byte 1; SL-Infinity (`a102`) register `62`, speeds from byte 1; SL v2 (`a103`, `a105`) and AL v2 (`a104`) register `62`, speeds from byte 2. The SL-Infinity and v2 hubs send duty 0 as 1 and 1..9 as 10, below which their fans stall; the others take 0..100.
- Manual mode: `E0 10 <register> <10 << port>`. Back to the motherboard's PWM input: `E0 10 <register> <11 << port>`. Duty: `E0 <20 | port> 00 <duty>`. The firmware drops a duty that comes within 200 ms of manual mode, and entering manual mode again resets the fan to its default, so camless enters it once per port and then sends only duties. Nexus sends the duties again with every poll, and so does camless.
- Speeds: feature report `E0 50 00`, then input report `E0` read with `HidD_GetInputReport` (65 bytes): one BE speed per port from byte 1 or 2. Above 6000 is no speed, and camless keeps the last one.
- Firmware: `E0 50 01`, then the input report: `E0 50` again at `[1]`, the family id at `[4]`, the version at `[5]`, in a different code per family (`firmware_number` in `lianli.rs`, after Nexus). A hub that just attached can answer it empty; camless asks three times when it opens the hub.
- After sleep the hub is out of manual mode (Nexus re-enters it on resume). camless takes a gap of over 30 s between polls for a wake-up and enters manual mode again.
- L-Connect: Nexus leaves the hubs alone while `L-Connect 3`, `L-Connect`, `LConnect3` or `LConnect` runs, by process name. camless does the same, checks every 5 s, and sends the ports it drives back to the motherboard before it lets go, and when the service stops, which Nexus does not. Nexus does not count L-Connect's service (`LConnectService`) as the app running, and neither does camless; nobody has checked whether it drives the fans with the app closed.
- Lighting (Nexus `LianLiLightingFrameWriter.cs` and `LianLiUniHubLightingProfile`), off or one colour per port:
  - Colours go in a 353-byte output report, `E0 <30 | channel>` and R, B, G per LED, over the interrupt pipe on the SL hubs (`7750` is not one: it takes `HidD_SetOutputReport`, as the SL-Infinity and v2 hubs do). A colour whose R + G + B passes 460 is scaled down to that, Nexus's limit on the hub's current.
  - Then `E0 <10 | channel> 01 00 00 <brightness> 00` as a feature report: the static effect, brightness `00` for full and `08` for off. After every port, `E0 60 00 <latch> 00 00 00`: latch `01`, `04` on the SL v2 hubs (`a103`, `a105`).
  - Each port's fan count goes first, in the quantity register: `E0 10 <register> <port << 4 | fans> 00 00 00` packed on the SL and SL v2 hubs, `E0 10 <register> <port + 1> <fans> 00 00` on the others. camless does not know the fans on a port and sends the most the hub takes, 4 or 6, which is the same for one colour.
  - The SL-Infinity hubs have two channels per port, the inner ring (8 LEDs a fan, channel 2p) and the outer (12, channel 2p + 1), and take the fan count again before each frame. The SL hubs take 16 LEDs a fan on one channel.
  - The ARGB merge goes off first (`E0 10 34 00 00 00 00`, then 20 ms): once on the SL hubs, before every frame on the SL-Infinity (`a102`).
  - The hub keeps its colours when camless lets go. Nexus also sends a sync with the motherboard's ARGB (`E0 10 <argb register> <on>`) when the user asks for it; camless never does.
- Not done: the L-Wireless controllers (`0416:8040`). Nexus sends a fan's duty in its RF bind frames, through a pairing state machine that addresses the master and each fan by MAC over WinUSB. Its `RfSaveCfg` (`15`) stores the setup in the fans' flash, and the fans need a clock-sync heartbeat. camless leaves them out until someone with the hardware can test it.
- hidapi's native Windows backend has no `get_input_report`, and it trims output reports to the feature report length. `winhid.rs` makes these calls itself, and gives a write to the interrupt pipe 1 s, as Nexus's overlapped writes, so that a hub that stops taking reports cannot hold the service. On Linux, `hidraw.rs` sends the 7-byte feature report as it is (`HIDIOCSFEATURE`) and reads the speeds with `HIDIOCGINPUT`, as Nexus's `LinuxHidDevice.cs` does.

**Lian Li Uni Fan TL controller** (`0416:7372`; experimental, with `[devices] other_brands` on)
- Nexus `Peripherals/LianLiTl`, `Peripherals/LianLiCp` and `Lighting/TlLightingFrameWriter.cs`, from L-Connect and FanControl.LianLi. Nexus calls it untested, pending verification.
- No source names the HID collection: Nexus picks the interface with 64-byte output and input reports, and so does camless, on a vendor page only (`Usage::Vendor`).
- Every command is a 64-byte report, `01 <command> 00 00 00 <payload length> <payload>`, and every one is answered. camless drops what is waiting (1 ms reads) before each one.
- Handshake `A1`: one record per fan from byte 6, three bytes: `<detected 80 | port << 4 | fan>` and the rpm, BE. camless takes the fans found at open; above 6000 rpm is no reading. A reply whose command byte is not `A1` is a late one to another command, and lists no fans (Nexus does not check).
- Duty: `AA <port << 4 | fan> <duty>`, 12..100, with 0 sent as 1. Off the motherboard's sync: `B1 <address>`; back on it: `B1 <address | 80>`. camless takes a fan off the sync when a curve first takes it, and puts it back when camless lets go or the service stops. Nexus never puts a fan back on the sync.
- Lighting, per fan: `A3` with 20 bytes: `<port << 4> <address> <mode> <brightness> <speed>` and the colour from byte 5. Static: mode `03`, brightness 4, speed 2, a colour count of 1 at byte 19. Off: mode `00` and byte 18 set.

**Lian Li Strimer Plus V2** (`0cf2:a200`; experimental, with `[devices] other_brands` on)
- Nexus `Peripherals/Strimer` and `Lighting/StrimerLightingFrameWriter.cs`, from OpenRGB's `LianLiStrimerLConnectController`. Nexus calls it untested, pending verification.
- The Uni Hub's collection (`FF72`, `A1`), 255-byte reports with id `E0` on the interrupt pipe. The 24-pin cable is zones 0..5 of 20 LEDs, the 8-pin GPU cable zones 6..11 of 27 (the dual 8-pin cable has the first four).
- A colour: `E0 <30 | zone>` and R, B, G per LED, then `E0 <10 | zone> 01 00 01 00 00` (direct mode, Nexus's middle speed, forward, full brightness). Off: the same commit with mode `00`. Then a latch over every zone: `E0 2C 0F FF 00 00 00 00`, a bit per zone, the GPU zones from bit 6. Nexus latches `03 FF` for the dual 8-pin cable when its setting says so; camless has no such setting and latches all twelve, Nexus's default.

**Asiahorse Umbra V2** (`1a86:fe04`; experimental, with `[devices] other_brands` on)
- Nexus `Peripherals/AsiahorseUmbra`, `Lighting/AsiahorseUmbraLightingFrameWriter.cs` and `Cooling/AsiahorseUmbraCoolingProvider.cs`, from the vendor app and captures of it. camless leaves the hub alone while `Sync_light` runs.
- No source names the HID collection: 64-byte output and input reports on a vendor page, as for the TL.
- Frames: `52 42 <length> 00 <command> <arguments> <sum>`, 64 bytes after a zero report id. The length counts the header, the arguments and the sum; the sum adds every byte before it. A reply carries the same command. Windows leaves the zero report id in front of replies.
- Info `00`: the model as BE u32 at byte 5, which must be 5 for this id, and the firmware at 17..19.
- Fans: they all run at one duty and report no speed. `08 01` takes them off the motherboard's PWM, `08 00` gives them back, when camless lets go and when the service stops, as Nexus does; `FB <duty>` sets the duty, which Nexus sends again with every info poll, and so does camless. An info request without a reply is a slow hub, not a lost one, as in Nexus: the duty still goes out.
- Lighting: `FD 01` first (the hub drives the LEDs, not its ARGB input). Then the static effect on every port, `03 FF FF 00 03 01 FF` and colour 1 in all four colour slots of the 52-byte block; off is `06`.
- camless never sends `02` (a port's LED count, kept in flash), `0B`, `0C`, `F9`, `FA`, `FC` or `FF`.

**Nollie ARGB controllers** (`3061:4714`, `4716`; `16d2:1f01`, `1f11`, `1616`, `1617`, `1618`; `16d5:4714`, `4716`, `1f01`, `1f11`, `2a32`, `2a16`, `2a08`, `2c08`, `2a01`; experimental, with `[devices] other_brands` on)
- Nexus `Peripherals/Nollie` and `Lighting/NollieLightingFrameWriter.cs`, ported from OpenRGB's `NollieController`. Nexus has no Nollie app to leave the controllers to, so neither has camless.
- Lighting only, and the controllers answer nothing. They do not know their LED counts, so camless sends each channel the most LEDs it takes (Nexus's table), in one colour or black. The firmware keeps the last colours.
- Collection: the composite OS2.1 controllers (`2a32` and `2a16` on interface 0, `2a08`, `2c08` and `2a01` on interface 2) at usage page `FF00`, usage `01`, as Nexus picks them. For the others Nexus matches the output report length: camless opens a vendor-defined collection whose output reports are exactly that long, and does not read it first, as the controllers send nothing.
- Wide transport (the 16- and 32-channel ones): a 1025-byte report per channel, `00 <wire channel> <flag> <count BE>` and G, R, B per LED from byte 5, shown on receipt. Wire channels 15 and 31 carry flag 1 and 2, go in every frame, empty if nothing is set on them, and get 8 ms after them. Channels go in ascending wire order.
- Chunked transport (the others): 65-byte reports of 21 LEDs, `00 <chunk + wire channel × stride>` and the LEDs from byte 2, G, R, B on `1f01`, `1f11`, `2a08`, `2a01` and `2c08`, R, G, B on the 28 series. A latch `00 FF` after the frame.
- Each card channel goes to a wire channel through a map per controller (`Map32`, `Map16Legacy`, `Map16Os2`, or none).
- camless never sends the standalone settings (`80`, `FE 02`), the release (`FF`, `FE 01`) or the LED-count handshake (`FE 03`).

**Corsair iCUE LINK System Hub** (`1b1c:0c3f`; experimental, with `[devices] other_brands` on)
- Nexus `Peripherals/CorsairLink`, `Cooling/CorsairLinkCoolingProvider.cs` and `Lighting/CorsairLinkLightingFrameWriter.cs`, checked against OpenLinkHub and OpenRGB, and run by Nexus on firmware 3.2.571 and 4.1.656. camless leaves the hub alone while iCUE runs (`iCUE`, its launcher, plugin host and services, and Corsair Link 4).
- Usage page `FF42`, usage `01`. A command goes in a 513-byte write, `00 00 01 <command> <payload>`, and the hub answers each with a 512-byte report: the command at byte 2, its status at 3 (0 for taken).
- Firmware `02 13`: the version at 4, 5 and 6..7 LE. Software mode `01 03 00 02`, then 500 ms before anything else, and every channel's duty right after the colour endpoint opens, as Nexus does; hardware mode, the hub's own curve, `01 03 00 01`, when camless lets go and when the service stops.
- Data sits behind endpoints. A read: close `05 01 01 <endpoint>`, open `0D 01 <endpoint>`, read `08 01 <endpoint>`, close. The reply has the endpoint's data type at 4..5; another type is a late reply, and up to five more reads (50 ms each) find the right one.
  - Devices `36`, type `21 00`: the channel count at 6, then per channel an 8-byte record (type at 2, model at 3, serial length at 7) and the serial. Length 0 is an empty channel. Nexus's model table gives names, LED counts, which have a speed and which a probe.
  - LED counts `20` (no type check): per channel from 1 at 7 + 4 × channel, connected when the first u16 is 2, then the count, capped at 50. For the devices whose count varies.
  - Speeds `17`, type `25 00`, and temperatures `21`, type `10 00`: the count at 6, then per channel from 0 a status (0 for a reading) and a LE value; temperatures in tenths.
- Duties: one write for every speed channel on endpoint `18`, `06 01` and `<length LE> 00 00 07 00 <count>`, then `<channel> 00 <duty> 00` each. The length counts from the data type on. The reply proves nothing.
- Software mode has no fallback: the hub holds the duties it got. Nexus sends them all every 2 s, holding the channels nothing set at 50 % (fans) and 70 % (pumps and coolers), and so does camless, with every poll. A duty set by hand goes no lower than 50 % on a pump; a curve's no lower than 70 % on a cooler (the H100i to H170i and the TITAN II), 31 % on a TITAN (type 17) and 30 % on another pump. camless takes the higher of the two: 70 % on those coolers, 50 % on the TITAN and the other pumps.
- Lighting: close `05 01 00`, open `0D 00 22` once, then 40 ms (QX and RX chains drop QX lighting without it). Each frame: `06 00` and `<length LE> 00 00 12 00` and R, G, B for every device's LEDs in chain order, with an LCD cap's after its cooler's, in 508-byte pieces, the rest with `07 00`. A 0 leaves an LED as it was, so Nexus sends every byte as at least 1; camless sends 0 for the devices the config does not set. A refused frame opens the endpoint again first.
- Each of the two ports takes so many lit LEDs: past 238, 340 and 442 on one, every colour is dimmed to 66 %, 33 % and 10 % (OpenLinkHub). The port is channel 13 on (7 on firmware 1). With a pump LCD on the chain, the cooler's inner ring (LEDs 16..19) goes dark.
- Another program: every open handle gets every reply, and the hub sends nothing unasked. camless opens the hub without the read other HID devices get, so that a queued reply is heard, listens 2.5 s before it takes the hub, and lets it go at the first reply to a command it did not send, without hardware mode. After one, it takes the hub again only once a minute has passed without one, longer than iCUE's keep-alive (Nexus's 60 s).
- camless never sends `1A 01`, which maps the chain again and powers it off and on, or anything to an LCD.

**HYTE and iBUYPOWER hubs: the serial port**
- USB serial (CDC ACM), 115200 baud, 8N1, DTR and RTS on (Nexus `Np50SerialTransport`). On Windows camless finds the port through SetupAPI (the Ports class, the hardware id, the `PortName` value) as Nexus's `WindowsSmartHubPortDiscovery` does, and opens it exclusively.
- A request drops the input left over, then writes, then reads the reply's length or until its timeout (Nexus `SendRequest`). Fan writes and LED frames get no reply.
- camless leaves the hubs alone while HYTE Nexus runs (`HYTE Nexus`, `HYTE.Nexus.Service`, and Nexus 3's `Nexus.exe`, `Nexus` on Linux).

**HYTE Smart Hub** (`3402:0904`; experimental, with `[devices] other_brands` on)
- Nexus `Peripherals/Hyte/SmartHub`, from HYTE's own service. Four PWM fan ports and four ARGB ports.
- Info `FF CC 01 00` → 20 bytes, `FF CC` first: the tach of port n at 3 + 2n and 4 + 2n, rpm = 60000 / (H + L / 100) / 4, 0 when both are 0. The enabled flags after them are set whatever is plugged in. Firmware `FF DD 02` → `FF DD 02 a b c d`.
- Duty: `FF CC 02 <port 1..4> <duty> 01`. The hub's watchdog puts every fan back on the duty saved in its flash 5 s after the last duty, so camless sends the duties it set with every poll and leaves the other ports alone. A poll interval above about 4 s loses the fans.
- LEDs: `FF EE 01 <port> 00 00 00` and 200 LEDs in G, R, B, 607 bytes. Any frame keeps the hub's own animation off on all four ports (`ARGB_NO_Activity`), and it comes back 5 s after the last. Nexus writes every port each time; once the config sets a light, camless sends every port's frame with every poll, the ports it does not set off.
- camless never sends `FF CC 07` (the animation's on/off, a flash write; Nexus sends it once when it reads it off) or `FF CC 0C` (the settings saved in flash).

**HYTE Nexus Portal NP50** (`3402:0901`; experimental, with `[devices] other_brands` on)
- Nexus `Peripherals/Hyte/Np50`, `Cooling/Np50CoolingProvider.cs` and `Lighting/Np50LightingFrameWriter.cs`, from HYTE's own service and protocol documents.
- Info `FF CC 01 00` → 20 bytes: the 4-pin tach at 9 and 10, rpm = 60000 / ((H × 100 + L) / 10 × 4); the mode at 12 (`01` software, `02` motherboard, `03` static); the firmware animation at 15..19, which an older firmware leaves out (camless then keeps the last one, as Nexus does). It is also the heartbeat: the hub leaves software mode 5 s after the last one. camless sends one with every poll.
- Port info `FF CC 01 <port 1..3>` → up to 240 bytes of 12-byte slots, until one has no module type at 3: the index at 2 (0 for its place), the type (`01` LS10, `02` LS30, `03` FP12, `06` LN60, `07` LN70), the LEDs at 5, the tach at 8 and 9 as on the Smart Hub. camless reads every port with every poll, and opens the hub again when the modules change. The hub sometimes sends less than 240 bytes (Nexus `PollPort`), so the read ends at the first empty slot rather than at its 400 ms timeout.
- Cooling mode, 15 bytes: `FF CC 02 00 <mode> <static duty> 00 00 00 01 <animation> <R> <G> <B> <brightness>`, the animation as the hub reported it (brightness 100 for 0). Nexus sends it again at most every 30 s while the hub reports another mode, as it parses on the thread the LEDs stream on; so does camless. camless sends software mode only once a curve takes a fan.
- Duties: the 4-pin header `FF CC 02 00 00 <duty>` and 6 zeros; a port's fans `FF CC 02 <port>` and 18 slots of `<slot> 00 <duty>` and 6 zeros. Nexus sends 0 for the slots nothing set; camless holds the FP12s at 50 % and the 4-pin header at 70 %.
- Letting go: Nexus reads the saved mode, `FF CC 04 00` → 17 bytes, mode at 4 (`01` motherboard) and its fixed duty at 5, and sets that live, as does camless.
- LEDs: a cycle of four frames, `FF EE 01 <port> 01 68 00` and G, R, B per LED, at least 90 bytes, the fourth empty, port 1 starting with the 6-LED logo. Without the fourth the firmware leaves the strips dark. The firmware's animation comes back 5 s after the last cycle.
- camless never sends `FF CC 03` (the saved mode), `05`, `07` or `0C`, which write the hub's EEPROM.

**iBUYPOWER MiniHub** (`3402:0900`; experimental, with `[devices] other_brands` on)
- Nexus `Peripherals/Hyte/MiniHub` and `Cooling/MiniHubCoolingProvider.cs`, from HYTE's own service. Two PWM fan ports, the second for up to three fans, and four ARGB ports.
- Firmware `FF DD 02` → 7 bytes. Fan mode `FF DD 05 <00 software | 01 motherboard>`, LED mode `FF DD 03 <00 | 01>`: motherboard after a power cycle, so not flash.
- Duties: `FF DD 04 00 <port 1> 00 <port 2>`, 10..100 (the firmware runs a lower duty at 0). Both ports go in one command: Nexus sends 10 % for the port nothing set, camless 70 %.
- Speeds `FF DD 06` → `FF DD 06 01 _ <period> 02 _ <period>`, rpm = 60000 / (period × 0.4). The tach reads right about half the time, and only while frames stream. Nexus shows a speed only once 5 of the last 20 readings from 200 to 4000 rpm agree within 2 %, their median, and holds it while one near it remains; so does camless, and it sends the frames it set with every poll.
- LEDs: `FF EE 03 <port> 01 68 00` and G, R, B, padded to 50 LEDs (157 bytes), 100 on port 4 (307 bytes). Shorter frames leave LEDs lit. The LED mode is the whole hub's, so once the config sets a light camless sends every port's frame with every poll, the ports it does not set off, as Nexus writes every port.
- No watchdog (Nexus `MiniHubHeartbeatWorker`): the hub keeps a duty until it loses power. camless hands the fans and the LEDs back to the motherboard when it lets go and when the service stops (Nexus's `ReleaseAll` does it for the fans). If camless crashes, the fans keep their last duty.

**Hue 2 accessories** (the Krakens since the X3, and the Control Hub)
- `20 03` → `21 03`: the channel count at `[14]`, then six accessory ids per channel from `[15]`; `00` ends a channel.
- Names follow liquidctl's `Hue2Accessory`. LED counts known from CAM captures: `0x19` 8, `0x1d` (F360 RGB Core) 24, `0x1e` (Kraken Elite ring) 24, `0x1f` 24. CAM sends 24 for channels it does not tell apart.
- The LED count goes into byte 7 of `26 06` and the effect footer, as CAM sends it.

**What did not work**

What was tried on the Kraken Elite V2 and the Control Hub and failed, so the next device does not repeat it. Nothing of the other brands' hubs has run on hardware yet.

- LCD:
  - Raw RGB565 frames (format `0x06`): the cooler acknowledged every test frame and showed none. It shows only Q565 (`0x08`), the format CAM sends.
  - A `38 01 05 00` mode switch before streaming was a guess. The cooler never answered it, and none is needed: the first frame switches the screen.
  - The 20-byte header and the frame in one bulk transfer: the transfer timed out, and the cooler did not answer `36 02`. They have to be two transfers, header first.
  - LCD I/O in the same loop as fan control: one LCD error held the fans for up to 12 s, because each command waited 6 s for a reply. LCD commands now wait at most 500 ms, each bulk transfer gives up after 1 s, and the loop gets frames already encoded.
- Replies: `ff 01` reports after lighting commands looked like errors. They are acknowledgements that echo the command in bytes 14–15, and CAM gets the same ones.
- HID writes capped at 64 bytes: the Control Hub's 76-byte static colour failed with "packet too large". Its output reports are longer, as its descriptor says.
- Capturing CAM:
  - Hooking `WriteFile` in CAM caught none of the LCD commands. cam-core is Rust, and Rust's standard library calls `NtWriteFile` and `NtReadFile` in ntdll directly. `tools/capture/` hooks those and `WinUsb_WritePipe`.
  - Static analysis of the whole CAM folder with rea also parses the 19,245 files in `node_modules`, about 50 minutes for nothing. Point it at CAM's own bundles and `index.node`. Their strings gave the image formats and display modes. The Q565 wire format came only from a live capture.
- Flash: memory-bucket uploads were never tried. liquidctl #927 reports that repeated uploads on `1e71:3012` left Krakens in the bootloader until standby power was cut.

These notes cover what camless's drivers send. Each driver module in `src/drivers/` names its sources at the top. A new driver should add its own section here, with what did not work.

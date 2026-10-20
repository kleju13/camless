# Protocol notes

Sources:
- liquidctl's `kraken3.py` and PRs #882 (lighting, 2024 Elite), #927 and #932;
- the Linux `nzxt-kraken3` driver;
- [Nexus](https://github.com/hello-nexus/nexus) (`KrakenModel.cs`, `KrakenProtocol.cs`, `KrakenHub.cs`), for the Krakens camless has not run on. Nexus says only its Elite 2024 rows were run on hardware;
- strings and tables in CAM 4.76.6's `@nzxt/cam-core` 71.9.3 (`index.node`, a napi-rs Rust addon), `service.exe` and the Electron bundles, inspected statically with [rea](https://github.com/morluto/rea) and plain string extraction;
- CAM's own profile files.

**HID framing**
- Reports are numbered, and the report ID is the first command byte. Payloads go out as they are, padded to at least 64 bytes. Some are longer: the Control Hub's static colour is 76 bytes.
- The NZXT CAM app opens both devices exclusively. Another process can open them but gets `Access is denied` on the first read. CAM Service (`service.exe`) does not hold them: camless drives both devices while it runs.

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

**Control Hub**
- Init: `60 02 01 E8 01 01 E8 01`, then `66 01`.
- Firmware: `10 02` → `11 02`, version at `[0x11..0x13]`.
- Periodic `67 02` reports: fan mode at `[16+i]`, rpm LE at `[24+2i]`, duty at `[40+i]`, noise at `[56]`.
- Duties: `62 01 <mask> d1..d5`.
- LED ports 1..5 are channel bytes `02 01 04 08 10`.

**Hue 2 accessories** (the Krakens since the X3, and the Control Hub)
- `20 03` → `21 03`: the channel count at `[14]`, then six accessory ids per channel from `[15]`; `00` ends a channel.
- Names follow liquidctl's `Hue2Accessory`. LED counts known from CAM captures: `0x19` 8, `0x1d` (F360 RGB Core) 24, `0x1e` (Kraken Elite ring) 24, `0x1f` 24. CAM sends 24 for channels it does not tell apart.
- The LED count goes into byte 7 of `26 06` and the effect footer, as CAM sends it.

**What did not work**

What was tried on the Kraken Elite V2 and the Control Hub and failed, so the next device does not repeat it.

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

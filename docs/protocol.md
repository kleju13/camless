# Protocol notes

Sources:
- liquidctl's `kraken3.py` and PRs #882 (lighting, 2024 Elite), #927 and #932;
- the Linux `nzxt-kraken3` driver;
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

**Control Hub**
- Init: `60 02 01 E8 01 01 E8 01`, then `66 01`.
- Firmware: `10 02` → `11 02`, version at `[0x11..0x13]`.
- Periodic `67 02` reports: fan mode at `[16+i]`, rpm LE at `[24+2i]`, duty at `[40+i]`, noise at `[56]`.
- Duties: `62 01 <mask> d1..d5`.
- LED ports 1..5 are channel bytes `02 01 04 08 10`.

**Hue 2 accessories** (Kraken 2023/2024 and Control Hub)
- `20 03` → `21 03`: the channel count at `[14]`, then six accessory ids per channel from `[15]`; `00` ends a channel.
- Names follow liquidctl's `Hue2Accessory`. LED counts known from CAM captures: `0x19` 8, `0x1d` (F360 RGB Core) 24, `0x1e` (Kraken Elite ring) 24, `0x1f` 24. CAM sends 24 for channels it does not tell apart.
- The LED count goes into byte 7 of `26 06` and the effect footer, as CAM sends it.

**What did not work**

What was tried on the Kraken Elite V2 and the Control Hub and failed, so the next device does not repeat it.

- LCD:
  - Raw RGB565 frames (format `0x06`): the cooler acknowledged every test frame and showed none. It shows only Q565 (`0x08`), the format CAM sends.
  - A `38 01 05 00` mode switch before streaming was a guess. The cooler never answered it, and none is needed: the first frame switches the screen.
  - The 20-byte header and the frame in one bulk transfer: the transfer timed out, and the cooler did not answer `36 02`. They have to be two transfers, header first.
  - LCD I/O in the same loop as fan control: one LCD error held the fans for up to 12 s, because each command waited 6 s for a reply. LCD commands now wait at most 500 ms, and the loop gets frames already encoded.
- Replies: `ff 01` reports after lighting commands looked like errors. They are acknowledgements that echo the command in bytes 14–15, and CAM gets the same ones.
- HID writes capped at 64 bytes: the Control Hub's 76-byte static colour failed with "packet too large". Its output reports are longer, as its descriptor says.
- Capturing CAM:
  - Hooking `WriteFile` in CAM caught none of the LCD commands. cam-core is Rust, and Rust's standard library calls `NtWriteFile` and `NtReadFile` in ntdll directly. `tools/capture/` hooks those and `WinUsb_WritePipe`.
  - Static analysis of the whole CAM folder with rea also parses the 19,245 files in `node_modules`, about 50 minutes for nothing. Point it at CAM's own bundles and `index.node`. Their strings gave the image formats and display modes. The Q565 wire format came only from a live capture.
- Flash: memory-bucket uploads were never tried. liquidctl #927 reports that repeated uploads on `1e71:3012` left Krakens in the bootloader until standby power was cut.

These notes cover what camless's drivers send. Each driver module in `src/drivers/` names its sources at the top. A new driver should add its own section here, with what did not work.

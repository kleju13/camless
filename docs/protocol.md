# Protocol notes

Sources:
- liquidctl's `kraken3.py` and PRs #882 (lighting, 2024 Elite), #927 and #932;
- the Linux `nzxt-kraken3` driver;
- liquidctl's `kraken2.py` and `asetek.py` with their guides, and the Linux `nzxt-kraken2` driver, for the Kraken X2, the M22 and the Asetek-made Krakens, which Nexus leaves out;
- [Nexus](https://github.com/hello-nexus/nexus) (`KrakenModel.cs`, `KrakenProtocol.cs`, `KrakenHub.cs`), for the Krakens camless has not run on. Nexus says only its Elite 2024 rows were run on hardware;
- Nexus's `Peripherals/LianLi`, `Cooling/LianLiCoolingProvider.cs` and `Conflicts/ConflictAppDefinitions.cs`, which follow FanControl.LianLi and L-Connect 3, for the Lian Li Uni Hubs;
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

**Lian Li Uni Hubs** (`0cf2:7750`, `a100` to `a106`; experimental, with `[devices] other_brands` on)
- Four fan ports. camless opens only the vendor collection on interface 1: usage page `FF72`, usage `A1`. Every command is a feature report, id `E0`, 7 bytes, which Windows pads to the hub's feature report length. Nexus calls its Lian Li support experimental and ran the SL-Infinity (`a102`) and SL (`a100`) hubs only; the other rows come from its table.
- Per hub: SL (`7750`, `a100`, the Redragon OEM `a106`) mode register `31`, speeds from byte 1; AL (`a101`) register `42`, speeds from byte 1; SL-Infinity (`a102`) register `62`, speeds from byte 1; SL v2 (`a103`, `a105`) and AL v2 (`a104`) register `62`, speeds from byte 2. The SL-Infinity and v2 hubs send duty 0 as 1 and 1..9 as 10, below which their fans stall; the others take 0..100.
- Manual mode: `E0 10 <register> <10 << port>`. Back to the motherboard's PWM input: `E0 10 <register> <11 << port>`. Duty: `E0 <20 | port> 00 <duty>`. The firmware drops a duty that comes within 200 ms of manual mode, and entering manual mode again resets the fan to its default, so camless enters it once per port and then sends only duties. Nexus sends the duties again with every poll, and so does camless.
- Speeds: feature report `E0 50 00`, then input report `E0` read with `HidD_GetInputReport` (65 bytes): one BE speed per port from byte 1 or 2. Above 6000 is no speed, and camless keeps the last one.
- Firmware: `E0 50 01`, then the input report: `E0 50` again at `[1]`, the family id at `[4]`, the version at `[5]`, in a different code per family (`firmware_number` in `lianli.rs`, after Nexus). A hub that just attached can answer it empty; camless asks three times when it opens the hub.
- After sleep the hub is out of manual mode (Nexus re-enters it on resume). camless takes a gap of over 30 s between polls for a wake-up and enters manual mode again.
- L-Connect: Nexus leaves the hubs alone while `L-Connect 3`, `L-Connect`, `LConnect3` or `LConnect` runs, by process name. camless does the same, checks every 5 s, and sends the ports it drives back to the motherboard before it lets go. Nexus does not count L-Connect's service (`LConnectService`) as the app running, and neither does camless; nobody has checked whether it drives the fans with the app closed.
- Not done: lighting, which goes in 353-byte output reports, over `HidD_SetOutputReport` on the SL-Infinity and the interrupt pipe on the SL; the TL, Strimer and wireless controllers, which Nexus drives with other handlers.
- hidapi's native Windows backend has no `get_input_report`, and it trims output reports to the feature report length. `winhid.rs` makes these calls itself.

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

# Notes for coding agents

camless was built with a coding agent, and new devices will mostly come the same way: someone who owns the hardware runs an agent on this repo. These notes are for that agent.

## Read first

- [CONTRIBUTING.md](CONTRIBUTING.md): the layout, how to add a device, how to capture what CAM sends, and the rules for drivers.
- [docs/protocol.md](docs/protocol.md): what is known for each device, with sources, and what did not work. Check it before you try something on a device.
- [docs/how-it-works.md](docs/how-it-works.md): the service, the app, fan control and the safeguards.

## Working on a device

- **Start from the report.** For a device from an issue labeled `hardware`, start from the `camless report` output in that issue: it has the USB interfaces, endpoints and HID report ids. If the issue only has Hardware Ids, ask for that output.
- **You can't see the hardware.** Ask the person you work with what the device actually does after each step: the screen, the pump, the fan speeds, the lights. A reply without an error is not proof that something worked.
- **Look before you write.** Quit the NZXT CAM app first: it opens the devices exclusively, and it starts again at logon unless it is turned off in Task Manager's startup apps. CAM Service does not hold them. Then run `camless status`: it lists every NZXT device and opens only the ones with a driver.
- **Starting CAM from your shell**, for a capture: if it exits at once with code 0 or 9 and writes no log, your shell has `ELECTRON_RUN_AS_NODE=1` set, as some agent harnesses do. CAM's exe then runs as plain Node. Remove the variable for that process first.
- **Send only bytes you have a source for:** liquidctl, the Linux drivers, or a capture of CAM made with `tools/capture/`. Don't guess commands on someone's cooler.
- **Never write flash.** Flash uploads have left Kraken Elites stuck in the bootloader (liquidctl #774, #907, #927). No firmware updates, no images or settings saved to flash: streaming and runtime commands only.
- **Fail safe.** A curve stored on a device must still speed up when it gets hot.
- **Mark only what you ran.** Set `verified: true` only for what was tested on the hardware, and say what was tested in the module comment and the pull request.

## Write down what you learned

Add the device's section to `docs/protocol.md` when you add or fix a driver. List what worked, with sources. Also list what you tried that did not work, with its symptom and the reason if you found it. The next agent, on the next device, starts from your notes.

## Before a pull request

```
cargo test --workspace --release
```

A driver needs tests for its report builders and parsers, preferably against bytes captured from CAM or the device.

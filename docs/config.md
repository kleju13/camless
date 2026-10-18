# Configuration

The app writes the config for you. This page is for reading or editing it by hand.

The config is `camless.toml` next to `camless.exe` if there is one, else `%APPDATA%\camless\camless.toml`. Device ids and channel names come from `camless status`.

## Profiles

A profile is a whole setup: curves, fans, lighting and the LCD. Each one is `profiles/<name>.toml` next to `camless.toml`, which then holds only the service settings and the profile it runs:

```toml
interval_ms = 1000
failsafe_duty = 70
profile = "NZXT CAM"          # profiles/NZXT CAM.toml
```

The app switches, creates, copies, renames and deletes profiles, and *Import from NZXT CAM* makes one from what CAM is set to, so there is always a way back. A setup can also live in `camless.toml` itself, without a profile, as in the example below. The app offers to save it as a profile before switching away from it. Fans that a profile leaves out keep the duty they were last given.

## Curves

Curves work as in FanControl:
- curves are named objects: a graph on any sensor, a fixed duty, a mix of other curves (max, min, average, sum), or a trigger with separate load and idle thresholds;
- each graph has hysteresis and separate rise and fall times;
- each fan picks a curve and adds its own offset, minimum, maximum and ramp limits.

## Example

```toml
interval_ms = 1000
failsafe_duty = 70            # fans whose sensor has no reading, and host-driven fans when camless stops

[[curve]]
kind = "graph"
name = "Liquid"
sensor = "kraken-elite-v2.liquid"
points = [[25, 25], [35, 40], [45, 80], [50, 100]]

[[curve]]
kind = "graph"
name = "GPU"
sensor = "gpu.temp"           # also cpu.temp, cpu.load, cpu.power, gpu.load, gpu.power, ram.load
points = [[45, 25], [80, 80]]
hysteresis = 2                # the sensor must move this far before the duty changes
response_up = 2               # seconds; how fast the curve follows a rise
response_down = 10            # and a fall

[[curve]]
kind = "mix"
name = "Case"
curves = ["Liquid", "GPU"]
function = "max"              # max | min | average | sum

[[curve]]
kind = "trigger"
name = "Boost"
sensor = "gpu.temp"
idle_duty = 30
load_duty = 70
load_at = 65                  # load_duty from here
idle_at = 55                  # back to idle_duty only below this

[[curve]]
kind = "fixed"
name = "Full"
duty = 100

[[fan]]
channel = "kraken-elite-v2/pump"
curve = "Liquid"
min = 40

[[fan]]
channel = "control-hub/fan1"
name = "Bottom"
curve = "Case"
offset = 5
step_down = 5                 # % per second at most

[[light]]
channel = "kraken-elite-v2/ring"
mode = "tai-chi"              # off | fixed | fading | spectrum-wave | covering-marquee | pulse | breathing
colors = ["3232de", "ffffff"] # candle | rainbow-flow | super-rainbow | rainbow-pulse | tai-chi | water-cooler
speed = "normal"              # slowest | slower | normal | faster | fastest

[[lcd]]
device = "kraken-elite-v2"
mode = "web"                  # liquid | frames (POST /v1/lcd/frame) | web
url = "http://127.0.0.1:30640/"  # web: shown with ?kraken=1, as CAM does
fps = 10                      # web: at most this many frames per second
brightness = 100
orientation = 0
```

`camless import-cam` builds this from CAM's files in `%APPDATA%\NZXT CAM\DataStorage\latest\user`: the active profile's curves (identical ones shared), fan nicknames, lighting and the LCD mode.

## Commands

`camless.exe` is also a command-line tool:

```
camless status               open every supported device and print its channels, sensors and readings
camless list                 NZXT HID devices that are connected
camless watch [seconds]      listen to input reports without sending anything (--raw for hex)
camless sensors              read CPU, GPU and memory sensors once
camless import-cam [dir]     print a camless.toml built from CAM's active profiles (-o FILE to write it)
camless check [config]       validate a config and show what each fan follows, without opening a device
camless run [config]         apply the config and keep fans on their curves (--verbose, --demo, --for SECONDS)
camless lcd liquid           put the LCD's own liquid temperature screen back
camless lcd test [seconds]   stream a test pattern to the LCD (default 20 s)
camless lcd page URL         render a web page as the LCD would show it and save it as a PNG (-o FILE, --for SECONDS)
```

`camless run --demo` uses a simulated cooler and hub and touches no USB device, to try the app without hardware.

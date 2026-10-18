# camless interface concepts

Clickable mockups of a camless interface, as a replacement for NZXT CAM. They are design studies, not part of the app: nothing here talks to hardware.

The three `cam-*` versions look and work like CAM 4, so people can move over without relearning: CAM's dark and light palettes, its 180 px left bar, and its names for everything (AIO Pump, Fan Connector, Temperatures source, Fixed, Silent, Performance, Custom). They differ only in layout. An earlier round of ten bolder concepts was dropped.

Open `index.html` in a browser; `file://` works, no build step or server. Every layout runs on the same simulation in `shared/camless-sim.js`, which follows camless's real rules: liquid and fixed curves stored on the cooler, CPU and GPU curves set by camless every second, the 45–55 °C safety floor and the 70 % failsafe.

The floating bar on each page drives the demo: load (idle, game, render), a missing CPU sensor, hot liquid, and moving between layouts. On screens wider than 1180 px camless opens as a 1024 × 640 window, as the app will; the bar switches it to 1280 × 800, 800 × 560 or the whole browser, and the window's corner resizes it. The bar is not part of any design.

| Folder | Layout |
|---|---|
| `cam-1-classic` | CAM's own structure: Monitoring, Cooling, Lighting (with the LCD), Settings |
| `cam-2-devices` | The same look, one page per device with Cooling, Lighting and LCD Display tabs |
| `cam-3-simple` | One page: every fan and light as a row, curves and the lighting editor in a dialog |

The CAM versions share `shared/cam-kit.css` and `shared/cam-kit.js`. Colors come from CAM 4.76's theme; the font is Montserrat, standing in for CAM's Gotham. Logos, icons and hardware pictures are drawn for camless; none of CAM's assets are used. CAM has a preset per channel but camless stores only a source and a curve, so the kit keeps the preset and each source's custom curve in the page.

camless is a small app, so the CAM versions follow the size of their own window, not the screen. Below 1000 px wide the left bar shows icons only; below 1000 × 720 panels, curves and gauges get denser; cooling cards stay two across while each still fits the four presets. Below 600 px the bar becomes a row of names, for phones.

Each fan channel shows its fans: a picture on the cooling card and a Fans row under Lighting, like CAM's step for telling it which NZXT fans are plugged in: F RGB Core (LEDs in the hub), F RGB Duo (LED rings on both sides), F RGB (a ring on the front) or a fan without RGB, in 120 or 140 mm and black or white. The preview, cooling card names and effects follow the choice. The LED counts in the kit only spread effects across the preview; they are not NZXT's specifications. The choice is kept in the browser with the other app settings, not in `camless.toml`.

`shots/` holds the gallery thumbnails (1440 × 900).

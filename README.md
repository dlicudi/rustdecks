# rustdecks

A lean [Loupedeck Live](https://loupedeck.com/) controller for [X-Plane 12](https://www.x-plane.com/),
in Rust. A deliberately small alternative to [cockpitdecks](https://github.com/devleaks/cockpitdecks):
Loupedeck Live only, no web decks, no async runtime — ~1,900 lines and a handful
of dependencies.

## Requirements

- A Loupedeck Live (USB)
- X-Plane **12.1.4+** with the Web API enabled (Settings → Network → *Accept connections… / Web API*)
- Rust (stable)

## Build & run

```sh
cargo build --release

# Run a profile (this is the normal mode)
./target/release/rustdecks examples/cessna.yaml

# Component smoke tests
./target/release/rustdecks probe                 # Loupedeck only: light LEDs, draw, print input
./target/release/rustdecks simprobe [dataref]    # X-Plane only: subscribe to a dataref, print values
```

The simulator host is discovered via the X-Plane UDP beacon; if no beacon is
seen it falls back to `127.0.0.1`. Set `sim.host` in the profile to override.

## Profile format

A profile is one YAML file describing pages. Each page has up to 12 screen
**keys** (`0..11`), 6 **encoders** (`e0..e5`, which draw to the side strips),
and 8 round LED **buttons** (`b0..b7`). Every input may carry an `Action`; every
surface may carry a `Draw`.

```yaml
device: loupedeck-live
brightness: 0.7            # 0.0 .. 1.0
sim:
  host: auto              # "auto" = beacon discovery, or an IP/hostname
  port: 8086

home: main                # page shown at startup

pages:
  main:
    keys:
      0:
        draw:                                       # what's shown
          text: GEN                                 # static label
          value: sim/cockpit2/electrical/generator_on[0]   # live dataref ([i] indexes arrays)
          format: "{:.0}"                           # see "Formatting" below
        press: { command: sim/electrical/generator_1_toggle }   # what a tap does
      3:
        draw: { text: "RADIO →" }
        press: { page: radio }                      # switch pages

    encoders:
      e0:
        draw: { text: THR, value: sim/flightmodel/engine/ENGN_thro[0], scale: 100.0, format: "{:.0}%" }
        turn_cw:  { command: sim/engines/throttle_up }
        turn_ccw: { command: sim/engines/throttle_down }
        press:    { command: sim/engines/throttle_full }

    leds:
      b0:
        color: "#00ff00"                            # #rrggbb, #rgb, or a name
        press: { set-dataref: sim/cockpit2/switches/landing_lights_on, value: 1.0 }

  radio:
    keys:
      0:
        draw: { text: "← BACK" }
        press: { page: main }
```

### Draw fields

| field | meaning |
|---|---|
| `text` | static label (top of the cell) |
| `value` | dataref path to display; `name[i]` indexes an array element |
| `scale`, `offset` | `shown = value * scale + offset` (defaults 1, 0) |
| `format` | format string applied to the number (default `"{}"`) |
| `text_color`, `bg_color` | `#rrggbb`, `#rgb`, or a named color |
| `lit_color` | makes the key an annunciator: a small LED bar glows this color when `value` ≥ 0.5 (dim when off), label below |
| `icon` | FontAwesome glyph name (`engine`, `radio`, …) or hex codepoint; renders as an icon key |
| `accent` | color of a thin bar across the top of the key (`#rrggbb`, `#rgb`, or a name) |
| `font` | `seven-seg` renders the value in a 7-segment avionics face; otherwise the default monospace |

### Actions

- `{ command: <name> }` — fire an X-Plane command (momentary press+release)
- `{ page: <name> }` — switch the deck to another page
- `{ set-dataref: <name>, value: <num> }` — write a dataref

Keys take `press` / `long_press`; encoders take `turn_cw` / `turn_ccw` / `press`;
LEDs take `color` / `press`.

### Shared defaults

Top-level `encoders` and `leds` are inherited by every page (a page may override
an individual id). This is how the SR22 profile puts the FCU knobs and the 8-button
pager on every page without repetition.

### Formatting

A single `{...}` placeholder with optional precision, plus surrounding text:
`"{}"`, `"{:.0}"`, `"{:.1}"`, `"{:.0} ft"`, `"{:.0}%"`. Anything without a
placeholder is printed verbatim (handy for fixed labels).

## How it works

Five modules, ~1,900 lines, no async:

- **config** — the profile schema (serde) and validation
- **device** — Loupedeck Live serial driver (WebSocket-framed binary over USB CDC)
- **render** — an in-house RGB canvas + `fontdue`, packing to the device's RGB565
- **sim** — X-Plane 12 Web API client (beacon discovery, REST id resolution, WebSocket values/commands)
- **app** — the wiring loop

Three threads — device read, sim WebSocket, main — communicate over channels.
The main thread coalesces update bursts and repaints a surface only when its
displayed *text* actually changes.

## Not included (by design)

Serial pinning, command hold (begin/end), dataref-driven LED colors, side-strip
touch, multi-value keys, an equality test for annunciators (`light when value ==
N`, needed for mode selectors), and any formula/expression engine. These were cut
to stay lean; they can be added when a real profile needs them.

## Credits & license

rustdecks stands entirely on the work of **Pierre M. ([devleaks](https://github.com/devleaks))**:

- [**python-loupedeck-live**](https://github.com/devleaks/python-loupedeck-live) —
  the reverse-engineered Loupedeck Live serial protocol that rustdecks' device
  layer is a Rust port of. Without it there'd be nothing to talk to the hardware.
- [**cockpitdecks**](https://github.com/devleaks/cockpitdecks) — the X-Plane
  controller that defined this whole idea: the page/encoder/LED model, the dataref
  and command vocabulary, the icon and font choices, and the SR22 profile rustdecks
  ships ([`examples/cirrus-sr22.yaml`](examples/cirrus-sr22.yaml)) is ported from its
  config. rustdecks is a deliberately lean reimplementation of a slice of cockpitdecks;
  for the full-featured, multi-deck, web-capable original, use cockpitdecks.

Code is MIT (see [LICENSE](LICENSE)). Bundled fonts, each under the SIL Open Font
License with its license alongside in [assets/fonts/](assets/fonts/):

- [B612](https://github.com/polarsys/b612) — labels/values, designed for cockpit displays.
- [Segment7](https://fontlibrary.org/en/font/segment7) by Cedric Knight — the 7-segment avionics readout face.
- [Font Awesome 6 Free (Solid)](https://fontawesome.com/) — nav/menu icon glyphs.

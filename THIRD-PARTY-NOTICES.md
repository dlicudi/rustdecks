# Third-party notices

rustdecks is MIT licensed (see [LICENSE](LICENSE)). It incorporates substantial
portions derived from the following MIT-licensed works by Pierre Mareschal
([devleaks](https://github.com/devleaks)), whose copyright and permission
notices are reproduced below as the MIT License requires.

---

## python-loupedeck-live

The Loupedeck Live serial protocol in `src/device.rs` is a Rust port of
<https://github.com/devleaks/python-loupedeck-live>.

```
MIT License

Copyright (c) 2022 Pierre Mareschal

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## cockpitdecks

The deck model (pages, encoders, LEDs), dataref/command vocabulary, and icon
choices follow <https://github.com/devleaks/cockpitdecks>.

```
MIT License

Copyright (c) 2019-2024 Pierre Mareschal
```

(Full MIT permission text as above.)

The SR22 profile (`examples/cirrus-sr22.yaml`) is ported from the author's own
cockpitdecks deck config at <https://github.com/dlicudi/cockpitdecks-configs>,
so it carries no third-party notice.

---

## Bundled fonts

Each ships under the SIL Open Font License 1.1, with its license file alongside
the font in [`assets/fonts/`](assets/fonts/):

- **B612 / B612 Mono** — labels and values. License: `assets/fonts/OFL.txt`.
- **Segment7** (Cedric Knight) — 7-segment readouts. License:
  `assets/fonts/Segment7Standard-LICENSE.txt`.
- **Font Awesome 6 Free (Solid)** — icon glyphs. License:
  `assets/fonts/FontAwesome-LICENSE.txt`.

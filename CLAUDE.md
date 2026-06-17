# rustdecks

A lean Loupedeck Live controller for X-Plane 12, in Rust. Deliberately small:
Loupedeck Live only, no web decks, no async runtime, a handful of dependencies.
Keep it that way — build in small reviewable increments, no speculative features.

## Layout

- `src/app.rs` — the wiring loop: device + sim threads feed one channel; the main
  thread coalesces bursts and redraws only surfaces whose text changed.
- `src/config.rs` — profile model (`Profile`/`Page`/`Key`/`Draw`/`Encoder`/`Led`/`Action`).
- `src/device.rs` — Loupedeck Live serial protocol. `src/render.rs` — tile rendering.
- `src/sim.rs` — X-Plane Web API (REST + WebSocket). `src/tui.rs` — terminal dashboard.
- `examples/*.yaml` — profiles. `examples/preview.rs` — render tiles to PNG.

Run `rustdecks check <profile.yaml>` to validate a profile without hardware or sim.

## Upstream reference for ports

The profiles are ported from cockpitdecks. When porting or extending a profile,
consult these (local clones):

- `~/GitHub/cockpitdecks` — devleaks' original Python engine. Reference for
  dataref/command names, FontAwesome icon codepoints, and color names.
- `~/GitHub/cockpitdecks-configs` — the author's own deck configs (dlicudi, MIT),
  the source the profiles are ported from. The SR22 lives at
  `decks/cirrus-sr22/deckconfig/loupedecklive1/<page>.yaml`; each file maps 1:1 to a
  rustdecks page (`pfi`, `engine`, `ap`, `radio_com1`, `transponder`, `fms`, …).

### Subset constraints when porting

cockpitdecks is far richer than the rustdecks `Draw`. A faithful port must collapse
to what `Draw` supports — **one value per key**, numbers only:

- Multi-part annunciators (EGT/FF, OIL temp+press) → pick the primary reading.
- Warning lamps → a rustdecks annunciator (`lit_color` + the warning dataref; lit at value ≥ 0.5).
- Formulas (`${...} 2.72 /`) → fold into `scale`/`offset` where it's a single mul+add.
- **String datarefs** (e.g. `gps_nav_id`) are unsupported — substitute a numeric proxy and note it.

Document any such compromise in a comment on the page, as `pfi` does.

For visual fidelity, `Draw` also has `font: seven-seg` (a 7-segment avionics face
for numeric readouts — frequencies, squawk, PFI values) and `accent` (a colored
top bar, e.g. to colour-match an index icon to its nav-button LED). Annunciators
render as a glowing LED bar on a dark tile, not a full-cell flood.

# Architecture

rustdecks is a single-process controller that sits between a **Loupedeck Live**
(USB serial) and **X-Plane 12** (Web API). It translates deck input into sim
commands, and sim value changes into deck redraws. No async runtime: the
concurrency is three OS threads joined by one channel.

## Data flow

```mermaid
graph LR
  subgraph Threads
    DEV[device reader thread<br/>serial -> Event]
    WS[sim WebSocket thread<br/>dataref values -> Update]
    TUI[TUI keyboard thread<br/>optional, injected Event]
  end

  DEV -->|Input| CH(((unified mpsc channel<br/>AppEvent)))
  WS  -->|Data|  CH
  TUI -->|Input| CH

  CH --> MAIN[main thread: block, drain burst,<br/>redraw only changed surfaces]

  MAIN -->|command / set-dataref<br/>REST| XP[(X-Plane Web API)]
  MAIN -->|tiles + LEDs<br/>serial| DECK[(Loupedeck Live)]
  MAIN -.->|snapshot| MIRROR[SharedDeck -> TUI]

  XP -.->|value updates| WS
```

## Modules

| File | Responsibility |
|------|----------------|
| [`src/main.rs`](src/main.rs) | CLI dispatch: run a profile, or the `probe` / `simprobe` / `check` / `tui` subcommands. |
| [`src/config.rs`](src/config.rs) | Profile model (`Profile`/`Page`/`Key`/`Draw`/`Encoder`/`Led`/`Action`), parsed and validated from one YAML file. |
| [`src/app.rs`](src/app.rs) | The wiring loop (`run_inner`). Spawns the threads, owns the unified channel, maps input `Event`s to `Action`s and `Update`s to a values map, and drives redraws. |
| [`src/device.rs`](src/device.rs) | Loupedeck Live serial protocol: a reader thread decodes input `Event`s (keys, encoder turns/pushes, buttons, touch); the write side draws tiles, sets LEDs and brightness. |
| [`src/sim.rs`](src/sim.rs) | X-Plane Web API client: UDP beacon discovery, REST to resolve dataref/command ids, a WebSocket that streams value `Update`s. |
| [`src/render.rs`](src/render.rs) | Tile composition: key images and side strips, annunciators, the seven-segment face, icons, accent bars, colour parsing. |
| [`src/tui.rs`](src/tui.rs) / [`src/mirror.rs`](src/mirror.rs) | Terminal dashboard. `app::run_mirrored` publishes a `DeckState` snapshot and accepts injected keyboard input, so the deck can run as a virtual deck with no hardware. |

## The main loop

The device-read thread and the sim WebSocket thread each forward into one
`mpsc` channel of `AppEvent` (`Input` or `Data`). The main thread:

1. **blocks** on the channel, then **drains** the rest of the burst with
   `try_recv` so a flurry of dataref updates collapses into one redraw pass;
2. applies each event — input becomes an `Action` (`command` / `page` /
   `set-dataref`), data updates the values map keyed by the REST-resolved id;
3. either loads a new page (if an action requested one) or **redraws only the
   surfaces whose displayed text actually changed** (tracked per key and per
   side-strip row);
4. publishes a snapshot for the optional TUI mirror.

## Design constraints

Both the sim and the deck are **optional at startup**: without the deck the TUI
can drive a virtual one; without the sim the deck still renders icons, labels,
nav and LEDs, and live values simply stay blank. Dataref and command ids
resolved over REST are cached so each name is looked up once. The project is
deliberately small — Loupedeck Live only, no web decks, no async runtime — and
new capability is added in small reviewable increments rather than speculatively.

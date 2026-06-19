//! Terminal dashboard: mirrors the deck (keys, side strips, LEDs, live values,
//! input log, connection status, throughput) and drives the whole deck from the
//! keyboard or mouse, so it works as a monitor and as a full virtual deck with
//! no hardware.
//!
//! Input map:
//!   screen keys 0..11  -> 1 2 3 4 / q w e r / a s d f   (or click a tile)
//!   round buttons b0..7 -> F1..F8                        (or click an LED)
//!   encoders e0..5     -> Up/Down focus, Left/Right turn, Enter/Space push
//!                          (or scroll/click an encoder cell)
//!   quit               -> Esc

use std::collections::VecDeque;
use std::io::stdout;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CtEvent, KeyCode, KeyEventKind,
    MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Sparkline};
use ratatui::{DefaultTerminal, Frame};

use crate::app;
use crate::config::Profile;
use crate::device::Event as DeckEvent;
use crate::mirror::{Cell, DeckState, KeyKind, KeyView, LedView, SharedDeck};

/// Keyboard chars for screen keys 0..11, in deck grid order.
const KEY_CHARS: [char; 12] = ['1', '2', '3', '4', 'q', 'w', 'e', 'r', 'a', 's', 'd', 'f'];
/// Samples kept for the throughput sparklines.
const HIST: usize = 120;

/// TUI-local state not carried by the mirror snapshot.
struct Ui {
    focus: usize, // focused encoder e0..e5 (keyboard turn/push target)
    data_hist: VecDeque<u64>,
    redraw_hist: VecDeque<u64>,
}

impl Default for Ui {
    fn default() -> Self {
        Ui { focus: 0, data_hist: VecDeque::new(), redraw_hist: VecDeque::new() }
    }
}

pub fn run(profile: Profile) -> Result<(), String> {
    let mirror: SharedDeck = Arc::new(Mutex::new(DeckState::default()));
    let (inject_tx, inject_rx) = mpsc::channel::<DeckEvent>();
    {
        let mirror = mirror.clone();
        thread::spawn(move || {
            if let Err(e) = app::run_mirrored(profile, mirror, inject_rx) {
                eprintln!("app error: {e}");
            }
        });
    }
    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableMouseCapture);
    let result = event_loop(&mut terminal, &mirror, &inject_tx);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result.map_err(|e| e.to_string())
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    mirror: &SharedDeck,
    inject: &Sender<DeckEvent>,
) -> std::io::Result<()> {
    let mut ui = Ui::default();
    loop {
        let snap = mirror.lock().map(|g| g.clone()).unwrap_or_default();
        push(&mut ui.data_hist, snap.rate as u64);
        push(&mut ui.redraw_hist, snap.redraw_rate as u64);
        terminal.draw(|f| render(f, &snap, &ui))?;

        // Drain every input queued this frame before the next redraw, so bursts
        // of wheel/arrow events don't trickle in one-per-redraw (the "lag").
        if event::poll(Duration::from_millis(100))? {
            loop {
                if process_event(event::read()?, terminal, &mut ui, inject)? {
                    return Ok(());
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    }
}

/// Handle one input event. Returns `Ok(true)` when the user asked to quit.
fn process_event(
    ev: CtEvent,
    terminal: &DefaultTerminal,
    ui: &mut Ui,
    inject: &Sender<DeckEvent>,
) -> std::io::Result<bool> {
    match ev {
        CtEvent::Key(key) if key.kind == KeyEventKind::Press => {
            if matches!(key.code, KeyCode::Esc) {
                return Ok(true);
            }
            handle_key(key.code, ui, inject);
        }
        CtEvent::Mouse(m) => {
            let size = terminal.size()?;
            let l = layout(Rect::new(0, 0, size.width, size.height));
            handle_mouse(m.kind, m.column, m.row, &l, ui, inject);
        }
        _ => {}
    }
    Ok(false)
}

fn push(q: &mut VecDeque<u64>, v: u64) {
    q.push_back(v);
    while q.len() > HIST {
        q.pop_front();
    }
}

// --- input ---------------------------------------------------------------

fn press_key(inject: &Sender<DeckEvent>, index: u8) {
    let _ = inject.send(DeckEvent::Key { index, pressed: true });
    let _ = inject.send(DeckEvent::Key { index, pressed: false });
}
fn press_button(inject: &Sender<DeckEvent>, index: u8) {
    let _ = inject.send(DeckEvent::Button { index, pressed: true });
    let _ = inject.send(DeckEvent::Button { index, pressed: false });
}
fn push_encoder(inject: &Sender<DeckEvent>, index: u8) {
    let _ = inject.send(DeckEvent::EncoderPress { index, pressed: true });
    let _ = inject.send(DeckEvent::EncoderPress { index, pressed: false });
}
fn turn_encoder(inject: &Sender<DeckEvent>, index: u8, clockwise: bool) {
    let _ = inject.send(DeckEvent::EncoderTurn { index, clockwise });
}

fn handle_key(code: KeyCode, ui: &mut Ui, inject: &Sender<DeckEvent>) {
    match code {
        KeyCode::Char(c) => {
            if let Some(i) = KEY_CHARS.iter().position(|&k| k == c.to_ascii_lowercase()) {
                press_key(inject, i as u8);
            } else if c == ' ' {
                push_encoder(inject, ui.focus as u8);
            }
        }
        KeyCode::F(n) if (1..=8).contains(&n) => press_button(inject, n - 1),
        KeyCode::Up => ui.focus = (ui.focus + 5) % 6,
        KeyCode::Down => ui.focus = (ui.focus + 1) % 6,
        KeyCode::Left => turn_encoder(inject, ui.focus as u8, false),
        KeyCode::Right => turn_encoder(inject, ui.focus as u8, true),
        KeyCode::Enter => push_encoder(inject, ui.focus as u8),
        _ => {}
    }
}

fn handle_mouse(kind: MouseEventKind, x: u16, y: u16, l: &LayoutRects, ui: &mut Ui, inject: &Sender<DeckEvent>) {
    let cell = (0..6).find(|&i| hit(l.encoders[i], x, y));
    // The wheel is forgiving: anywhere in the encoders panel turns an encoder —
    // the one under the pointer if any, else the focused one.
    let in_panel = hit(l.enc_outer, x, y);
    let wheel_target = cell.or(if in_panel { Some(ui.focus) } else { None });
    match kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(i) = (0..12).find(|&i| hit(l.keys[i], x, y)) {
                press_key(inject, i as u8);
            } else if let Some(i) = (0..8).find(|&i| hit(l.leds[i], x, y)) {
                press_button(inject, i as u8);
            } else if let Some(i) = cell.or(if in_panel { Some(ui.focus) } else { None }) {
                ui.focus = i;
                push_encoder(inject, i as u8);
            }
        }
        MouseEventKind::ScrollUp => {
            if let Some(i) = wheel_target {
                ui.focus = i;
                turn_encoder(inject, i as u8, true);
            }
        }
        MouseEventKind::ScrollDown => {
            if let Some(i) = wheel_target {
                ui.focus = i;
                turn_encoder(inject, i as u8, false);
            }
        }
        _ => {}
    }
}

fn hit(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

// --- layout (shared by render and mouse hit-testing) ---------------------

struct LayoutRects {
    header: Rect,
    keys_outer: Rect,
    keys: [Rect; 12],
    enc_outer: Rect,
    encoders: [Rect; 6],
    leds_outer: Rect,
    leds: [Rect; 8],
    datarefs: Rect,
    events: Rect,
    data_spark: Rect,
    redraw_spark: Rect,
    footer: Rect,
}

fn bordered_inner(r: Rect) -> Rect {
    Block::default().borders(Borders::ALL).inner(r)
}

/// Split `area` into `rows` x `cols` equal cells, row-major.
fn grid(area: Rect, rows: u16, cols: u16) -> Vec<Rect> {
    let row_c = vec![Constraint::Ratio(1, rows as u32); rows as usize];
    let col_c = vec![Constraint::Ratio(1, cols as u32); cols as usize];
    let mut cells = Vec::with_capacity((rows * cols) as usize);
    for rr in Layout::vertical(row_c).split(area).iter() {
        for cc in Layout::horizontal(col_c.clone()).split(*rr).iter() {
            cells.push(*cc);
        }
    }
    cells
}

fn layout(area: Rect) -> LayoutRects {
    let rows = Layout::vertical([
        Constraint::Length(1),  // header
        Constraint::Min(8),     // keys | encoders+leds
        Constraint::Length(11), // datarefs | events
        Constraint::Length(5),  // throughput sparklines
        Constraint::Length(1),  // footer
    ])
    .split(area);

    let top = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).split(rows[1]);
    let right = Layout::vertical([Constraint::Length(5), Constraint::Min(4)]).split(top[1]);

    let keys_inner = bordered_inner(top[0]);
    let keys: [Rect; 12] = grid(keys_inner, 3, 4).try_into().unwrap();

    // Encoders: left column e0..e2, right column e3..e5.
    let enc_inner = bordered_inner(right[0]);
    let enc_cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(enc_inner);
    let mut enc = Vec::with_capacity(6);
    for col in 0..2 {
        for cell in Layout::vertical([Constraint::Ratio(1, 3); 3]).split(enc_cols[col]).iter() {
            enc.push(*cell);
        }
    }
    let encoders: [Rect; 6] = enc.try_into().unwrap();

    let led_inner = bordered_inner(right[1]);
    let led_cells = Layout::vertical([Constraint::Ratio(1, 8); 8]).split(led_inner);
    let leds: [Rect; 8] = (0..8).map(|i| led_cells[i]).collect::<Vec<_>>().try_into().unwrap();

    let mid = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[2]);
    let spark = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[3]);

    LayoutRects {
        header: rows[0],
        keys_outer: top[0],
        keys,
        enc_outer: right[0],
        encoders,
        leds_outer: right[1],
        leds,
        datarefs: mid[0],
        events: mid[1],
        data_spark: spark[0],
        redraw_spark: spark[1],
        footer: rows[4],
    }
}

// --- render --------------------------------------------------------------

fn render(f: &mut Frame, s: &DeckState, ui: &Ui) {
    let l = layout(f.area());
    render_header(f, l.header, s);
    render_keys(f, &l, s);
    render_encoders(f, &l, s, ui.focus);
    render_leds(f, &l, s);
    render_datarefs(f, l.datarefs, s);
    render_events(f, l.events, s);
    render_spark(f, l.data_spark, "data/s", &ui.data_hist, Color::Yellow);
    render_spark(f, l.redraw_spark, "redraw/s", &ui.redraw_hist, Color::Magenta);
    f.render_widget(
        Paragraph::new(Span::styled(
            " keys 1234/qwer/asdf · buttons F1-8 · enc \u{2191}\u{2193} focus \u{2190}\u{2192} turn \u{21b5} push · mouse click/scroll · Esc quit ",
            Style::default().fg(Color::DarkGray),
        )),
        l.footer,
    );
}

fn ok_bad(text: &str) -> Style {
    if text.starts_with("connected") {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Red)
    }
}

fn render_header(f: &mut Frame, area: Rect, s: &DeckState) {
    let line = Line::from(vec![
        Span::styled(" rustdecks ", Style::default().fg(Color::Black).bg(Color::Cyan)),
        Span::raw("  deck: "),
        Span::styled(s.device.clone(), ok_bad(&s.device)),
        Span::raw("   sim: "),
        Span::styled(s.sim.clone(), ok_bad(&s.sim)),
        Span::raw("   data: "),
        Span::styled(format!("{:.0}/s", s.rate), Style::default().fg(Color::Yellow)),
        Span::raw("   redraw: "),
        Span::styled(format!("{:.0}/s", s.redraw_rate), Style::default().fg(Color::Magenta)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn rgb(c: [u8; 3]) -> Color {
    Color::Rgb(c[0], c[1], c[2])
}

fn render_keys(f: &mut Frame, l: &LayoutRects, s: &DeckState) {
    f.render_widget(
        Block::default().borders(Borders::ALL).title(format!(" keys — {} ", s.page)),
        l.keys_outer,
    );
    for (i, &cell) in l.keys.iter().enumerate() {
        render_key_cell(f, cell, i, &s.keys[i]);
    }
}

fn render_key_cell(f: &mut Frame, area: Rect, idx: usize, k: &KeyView) {
    let (text, style) = match k.kind {
        KeyKind::Empty => (String::new(), Style::default().fg(Color::DarkGray)),
        KeyKind::Icon => (format!("[{}]", k.label), Style::default().fg(Color::Cyan)),
        KeyKind::Annunciator { lit: true } => (
            k.label.clone(),
            Style::default().fg(Color::Black).bg(rgb(k.lit_rgb)).add_modifier(Modifier::BOLD),
        ),
        KeyKind::Annunciator { lit: false } => {
            (k.label.clone(), Style::default().fg(rgb(dim(k.lit_rgb))))
        }
        KeyKind::Text if k.value.is_empty() => (k.label.clone(), Style::default().fg(Color::White)),
        KeyKind::Text => (format!("{}\n{}", k.label, k.value), Style::default().fg(Color::White)),
    };
    let header = Line::from(Span::styled(
        format!("{idx:>2}"),
        match k.accent {
            Some(c) => Style::default().fg(Color::Black).bg(rgb(c)),
            None => Style::default().fg(Color::DarkGray),
        },
    ));
    let mut lines = vec![header];
    for t in text.split('\n') {
        lines.push(Line::from(Span::styled(t.to_string(), style)));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// Roughly half-bright, for an unlit annunciator.
fn dim(c: [u8; 3]) -> [u8; 3] {
    [c[0] / 2, c[1] / 2, c[2] / 2]
}

fn render_encoders(f: &mut Frame, l: &LayoutRects, s: &DeckState, focus: usize) {
    f.render_widget(Block::default().borders(Borders::ALL).title(" encoders "), l.enc_outer);
    let cells = [&s.left[0], &s.left[1], &s.left[2], &s.right[0], &s.right[1], &s.right[2]];
    for (i, &cell) in l.encoders.iter().enumerate() {
        let c: &Cell = cells[i];
        let body = if c.label.is_empty() {
            format!("e{i} —")
        } else {
            format!("e{i} {} {}", c.label, c.value)
        };
        let style = if i == focus {
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        f.render_widget(Paragraph::new(Line::from(Span::styled(body, style))), cell);
    }
}

fn render_leds(f: &mut Frame, l: &LayoutRects, s: &DeckState) {
    f.render_widget(Block::default().borders(Borders::ALL).title(" LEDs "), l.leds_outer);
    for (i, &cell) in l.leds.iter().enumerate() {
        let led: &LedView = &s.leds[i];
        let dot = if led.on {
            Style::default().fg(rgb(led.rgb))
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let line = Line::from(vec![
            Span::styled("\u{25cf} ", dot),
            Span::raw(format!("b{i}  {}", led.target)),
        ]);
        f.render_widget(Paragraph::new(line), cell);
    }
}

fn render_datarefs(f: &mut Frame, area: Rect, s: &DeckState) {
    let lines: Vec<Line> = if s.datarefs.is_empty() {
        vec![Line::from(Span::styled(
            "(no live data — start X-Plane)",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        s.datarefs
            .iter()
            .map(|(name, value)| Line::from(format!("{value:>10}  {}", short(name))))
            .collect()
    };
    let block = Block::default().borders(Borders::ALL).title(" datarefs ");
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Trim a dataref path to its last two segments for display.
fn short(name: &str) -> String {
    let parts: Vec<&str> = name.rsplit('/').take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

fn render_events(f: &mut Frame, area: Rect, s: &DeckState) {
    let lines: Vec<Line> = s.events.iter().map(|e| Line::from(e.clone())).collect();
    let block = Block::default().borders(Borders::ALL).title(" input ");
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_spark(f: &mut Frame, area: Rect, title: &str, hist: &VecDeque<u64>, color: Color) {
    let data: Vec<u64> = hist.iter().copied().collect();
    let spark = Sparkline::default()
        .block(Block::default().borders(Borders::ALL).title(format!(" {title} ")))
        .data(&data)
        .style(Style::default().fg(color));
    f.render_widget(spark, area);
}

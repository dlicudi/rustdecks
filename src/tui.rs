//! Terminal dashboard: mirrors the deck (keys, side strips, LEDs, live values,
//! input log, connection status) and drives page navigation from the keyboard,
//! so it works as a monitor and as a virtual deck with no hardware.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event as CtEvent, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::app;
use crate::config::Profile;
use crate::device::Event as DeckEvent;
use crate::mirror::{DeckState, KeyKind, KeyView, SharedDeck};

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
    let result = event_loop(&mut terminal, &mirror, &inject_tx);
    ratatui::restore();
    result.map_err(|e| e.to_string())
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    mirror: &SharedDeck,
    inject: &Sender<DeckEvent>,
) -> std::io::Result<()> {
    loop {
        let snap = mirror.lock().map(|g| g.clone()).unwrap_or_default();
        terminal.draw(|f| render(f, &snap))?;

        if event::poll(Duration::from_millis(100))? {
            if let CtEvent::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    // Number keys press the matching round button (b0..b7).
                    KeyCode::Char(c @ '0'..='7') => {
                        let b = c as u8 - b'0';
                        let _ = inject.send(DeckEvent::Button { index: b, pressed: true });
                        let _ = inject.send(DeckEvent::Button { index: b, pressed: false });
                    }
                    _ => {}
                }
            }
        }
    }
}

fn render(f: &mut Frame, s: &DeckState) {
    let rows = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(8),    // keys | strips+leds
        Constraint::Length(11), // datarefs | events
        Constraint::Length(1), // footer
    ])
    .split(f.area());

    render_header(f, rows[0], s);

    let top = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).split(rows[1]);
    render_keys(f, top[0], s);
    let right = Layout::vertical([Constraint::Length(6), Constraint::Min(4)]).split(top[1]);
    render_strips(f, right[0], s);
    render_leds(f, right[1], s);

    let bottom = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[2]);
    render_datarefs(f, bottom[0], s);
    render_events(f, bottom[1], s);

    f.render_widget(
        Paragraph::new(Span::styled(
            " [0-7] navigate   [q] quit ",
            Style::default().fg(Color::DarkGray),
        )),
        rows[3],
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
        Span::styled(
            format!("{:.0}/s", s.rate),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw("   redraw: "),
        Span::styled(
            format!("{:.0}/s", s.redraw_rate),
            Style::default().fg(Color::Magenta),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_keys(f: &mut Frame, area: Rect, s: &DeckState) {
    let lines: Vec<Line> = (0..3)
        .map(|r| Line::from((0..4).map(|c| cell_span(r * 4 + c, &s.keys[r * 4 + c])).collect::<Vec<_>>()))
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" keys — {} ", s.page));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn cell_span(idx: usize, k: &KeyView) -> Span<'static> {
    let text = match k.kind {
        KeyKind::Empty => String::new(),
        KeyKind::Icon => format!("{idx:>2} [{}]", k.label),
        KeyKind::Annunciator { .. } => format!("{idx:>2} {}", k.label),
        KeyKind::Text if k.value.is_empty() => format!("{idx:>2} {}", k.label),
        KeyKind::Text => format!("{idx:>2} {} {}", k.label, k.value),
    };
    let style = match k.kind {
        KeyKind::Empty => Style::default().fg(Color::DarkGray),
        KeyKind::Icon => Style::default().fg(Color::Cyan),
        KeyKind::Annunciator { lit: true } => {
            Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
        }
        KeyKind::Annunciator { lit: false } => Style::default().fg(Color::DarkGray),
        KeyKind::Text => Style::default().fg(Color::White),
    };
    Span::styled(format!("{text:<17.17}"), style)
}

fn render_strips(f: &mut Frame, area: Rect, s: &DeckState) {
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<14}{:<14}", "LEFT", "RIGHT"),
        Style::default().fg(Color::DarkGray),
    ))];
    for i in 0..3 {
        let l = fmt_cell(&s.left[i]);
        let r = fmt_cell(&s.right[i]);
        lines.push(Line::from(format!("{l:<14}{r:<14}")));
    }
    let block = Block::default().borders(Borders::ALL).title(" encoders ");
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn fmt_cell(c: &crate::mirror::Cell) -> String {
    if c.label.is_empty() {
        String::new()
    } else {
        format!("{} {}", c.label, c.value)
    }
}

fn render_leds(f: &mut Frame, area: Rect, s: &DeckState) {
    let lines: Vec<Line> = s
        .leds
        .iter()
        .enumerate()
        .map(|(i, led)| {
            let dot = if led.on {
                Style::default().fg(Color::Rgb(led.rgb[0], led.rgb[1], led.rgb[2]))
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(vec![
                Span::styled("● ", dot),
                Span::raw(format!("b{i}  {}", led.target)),
            ])
        })
        .collect();
    let block = Block::default().borders(Borders::ALL).title(" LEDs ");
    f.render_widget(Paragraph::new(lines).block(block), area);
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

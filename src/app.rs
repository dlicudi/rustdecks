//! The wiring loop: connect to the deck and X-Plane, then translate device
//! input into sim commands and sim value updates into redraws.
//!
//! Threading: the device-read thread and the sim WebSocket thread each forward
//! into one unified channel; the main thread blocks on it, coalesces bursts,
//! and redraws only surfaces whose displayed text actually changed.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use crate::config::{Action, Device, Draw, Encoder, Led, Profile};
use crate::device::{Event, LoupedeckLive};
use crate::render::{self, Renderer, Style};
use crate::sim::{self, DataValue, Sim, Update};

enum AppEvent {
    Input(Event),
    Data(Update),
}

pub fn run(profile: Profile) -> Result<(), String> {
    // Only the Loupedeck Live is supported; this match forces a decision here
    // when another device variant is added.
    match profile.device {
        Device::LoupedeckLive => {}
    }

    // --- Device ---
    let port = LoupedeckLive::find_port()
        .ok_or("no Loupedeck found (VID 0x2EC2); is it plugged in?")?;
    let mut device = LoupedeckLive::connect(&port).map_err(|e| format!("device: {e}"))?;
    println!("device: connected at {port}");
    device
        .set_brightness(profile.brightness)
        .map_err(|e| e.to_string())?;

    // --- Simulator ---
    let host = if profile.sim.host == "auto" {
        match sim::discover(Duration::from_secs(5)) {
            Some(a) => {
                println!("sim: X-Plane {} at {}", a.xplane_version, a.host);
                a.host
            }
            None => {
                println!("sim: no beacon; trying 127.0.0.1");
                "127.0.0.1".to_string()
            }
        }
    } else {
        profile.sim.host.clone()
    };
    let (sim, updates) = Sim::connect(&host, profile.sim.port)?;
    println!("sim: Web API connected");

    let renderer = Renderer::new()?;

    // --- Unified event channel ---
    let (tx, rx) = mpsc::channel::<AppEvent>();
    let reader = device.reader().map_err(|e| e.to_string())?;
    let (dev_tx, dev_rx) = mpsc::channel();
    thread::spawn(move || reader.run(dev_tx));
    forward(dev_rx, tx.clone(), AppEvent::Input);
    forward(updates, tx, AppEvent::Data);

    let mut app = App {
        profile,
        device,
        sim,
        renderer,
        values: HashMap::new(),
        dr_ids: HashMap::new(),
        cmd_ids: HashMap::new(),
        current_page: String::new(),
        pending_page: None,
        eff_encoders: HashMap::new(),
        eff_leds: HashMap::new(),
        key_pressed: [false; 12],
        enc_pressed: [false; 6],
        btn_pressed: [false; 8],
        last: HashMap::new(),
    };

    let home = app.profile.home.clone();
    app.load_page(&home);
    println!("running page `{home}`; Ctrl-C to exit");

    // --- Main loop: block, drain the burst, then redraw what changed ---
    while let Ok(ev) = rx.recv() {
        app.apply(ev);
        while let Ok(ev) = rx.try_recv() {
            app.apply(ev);
        }
        match app.pending_page.take() {
            Some(page) => app.load_page(&page),
            None => app.redraw_changed(),
        }
    }
    Ok(())
}

/// Spawn a thread forwarding every item of `rx` into `tx`, wrapped by `wrap`.
fn forward<T: Send + 'static>(
    rx: Receiver<T>,
    tx: mpsc::Sender<AppEvent>,
    wrap: fn(T) -> AppEvent,
) {
    thread::spawn(move || {
        for item in rx {
            if tx.send(wrap(item)).is_err() {
                break;
            }
        }
    });
}

/// A surface that can show text, for change-tracking. Side strips track per row.
#[derive(PartialEq, Eq, Hash, Clone, Copy)]
enum Surface {
    Key(u8),
    Left,
    Right,
}

struct App {
    profile: Profile,
    device: LoupedeckLive,
    sim: Sim,
    renderer: Renderer,
    values: HashMap<i64, DataValue>,
    dr_ids: HashMap<String, Option<i64>>,
    cmd_ids: HashMap<String, Option<i64>>,
    current_page: String,
    pending_page: Option<String>,
    /// Current page's encoders/leds, profile defaults merged with page overrides.
    eff_encoders: HashMap<String, Encoder>,
    eff_leds: HashMap<String, Led>,
    key_pressed: [bool; 12],
    enc_pressed: [bool; 6],
    btn_pressed: [bool; 8],
    /// Last rendered text content per surface, to suppress redundant redraws.
    last: HashMap<Surface, String>,
}

impl App {
    fn apply(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Input(e) => self.handle_input(e),
            AppEvent::Data(u) => {
                self.values.insert(u.id, u.value);
            }
        }
    }

    fn handle_input(&mut self, e: Event) {
        match e {
            Event::Key { index, pressed } => {
                if self.edge(index as usize, pressed, Input::Key) {
                    if let Some(a) = self.key_action(index) {
                        self.execute(a);
                    }
                }
            }
            Event::EncoderTurn { index, clockwise } => {
                if let Some(a) = self.encoder_turn_action(index, clockwise) {
                    self.execute(a);
                }
            }
            Event::EncoderPress { index, pressed } => {
                if self.edge(index as usize, pressed, Input::Enc) {
                    if let Some(a) = self.encoder_press_action(index) {
                        self.execute(a);
                    }
                }
            }
            Event::Button { index, pressed } => {
                if self.edge(index as usize, pressed, Input::Btn) {
                    if let Some(a) = self.led_action(index) {
                        self.execute(a);
                    }
                }
            }
        }
    }

    /// Track press state; return true only on a not-pressed -> pressed transition.
    fn edge(&mut self, i: usize, pressed: bool, kind: Input) -> bool {
        let state = match kind {
            Input::Key => &mut self.key_pressed[i],
            Input::Enc => &mut self.enc_pressed[i],
            Input::Btn => &mut self.btn_pressed[i],
        };
        let rising = pressed && !*state;
        *state = pressed;
        rising
    }

    // --- Action lookup on the current page (cloned out to free the borrow) ---

    fn key_action(&self, index: u8) -> Option<Action> {
        self.page().keys.get(&index)?.press.clone()
    }
    fn encoder_turn_action(&self, index: u8, cw: bool) -> Option<Action> {
        let e = self.eff_encoders.get(&format!("e{index}"))?;
        if cw { e.turn_cw.clone() } else { e.turn_ccw.clone() }
    }
    fn encoder_press_action(&self, index: u8) -> Option<Action> {
        self.eff_encoders.get(&format!("e{index}"))?.press.clone()
    }
    fn led_action(&self, index: u8) -> Option<Action> {
        self.eff_leds.get(&format!("b{index}"))?.press.clone()
    }

    fn execute(&mut self, action: Action) {
        match action {
            Action::Command { command } => {
                if let Some(id) = self.command_id(&command) {
                    self.sim.run_command(id);
                }
            }
            Action::SetDataref { dataref, value } => {
                let (name, _) = sim::split_ref(&dataref);
                if let Some(id) = self.dataref_id(name) {
                    self.sim.set_dataref(id, value);
                }
            }
            Action::Page { page } => self.pending_page = Some(page),
        }
    }

    // --- Page loading: resolve ids, subscribe, set LEDs, full redraw ---

    fn load_page(&mut self, name: &str) {
        if !self.profile.pages.contains_key(name) {
            eprintln!("page `{name}` not found; ignoring");
            return;
        }
        self.current_page = name.to_string();
        self.last.clear();

        // Merge inherited defaults with this page's encoders/leds (page wins per id).
        let page = &self.profile.pages[name];
        self.eff_encoders = merge(&self.profile.encoders, &page.encoders);
        self.eff_leds = merge(&self.profile.leds, &page.leds);

        // Resolve and subscribe every dataref the page displays.
        let refs = self.page_value_refs();
        let mut ids = Vec::new();
        for r in refs {
            let (n, _) = sim::split_ref(&r);
            if let Some(id) = self.dataref_id(n) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        self.sim.subscribe(&ids);

        // Static LED colors.
        for b in 0..8u8 {
            if let Some(led) = self.eff_leds.get(&format!("b{b}")) {
                if let Some(rgb) = led.color.as_deref().and_then(render::parse_color) {
                    let _ = self.device.set_button_color(b, rgb);
                }
            }
        }

        // Full redraw.
        for k in 0..12u8 {
            self.render_key(k);
        }
        self.render_strip(Surface::Left);
        self.render_strip(Surface::Right);
    }

    /// Recompute and redraw only the surfaces whose text changed.
    fn redraw_changed(&mut self) {
        for k in 0..12u8 {
            self.render_key(k);
        }
        self.render_strip(Surface::Left);
        self.render_strip(Surface::Right);
    }

    fn render_key(&mut self, index: u8) {
        let draw = self.page().keys.get(&index).and_then(|k| k.draw.as_ref());
        let Some(draw) = draw else { return };

        // Icon: an icon glyph (optionally with a label) — for nav/menu keys.
        if let Some(glyph) = draw.icon.as_deref().and_then(render::icon_glyph) {
            let label = draw.text.clone();
            let content = format!("I\u{0}{glyph}\u{0}{}", label.as_deref().unwrap_or(""));
            if self.last.get(&Surface::Key(index)) == Some(&content) {
                return;
            }
            let style = style_for(draw);
            let buf = self.renderer.icon_key(glyph, label.as_deref(), &style);
            if self.device.draw_key(index, &buf).is_ok() {
                self.last.insert(Surface::Key(index), content);
            }
            return;
        }

        // Annunciator: lit_color present -> on/off rendering driven by the value.
        if let Some(lit_color) = draw.lit_color.as_deref().and_then(render::parse_color) {
            let lit = self.value_number(draw).map(|v| v >= 0.5).unwrap_or(false);
            let label = draw.text.clone().unwrap_or_default();
            let content = format!("A\u{0}{lit}\u{0}{label}");
            if self.last.get(&Surface::Key(index)) == Some(&content) {
                return;
            }
            let buf = self.renderer.annunciator(&label, lit, lit_color, &Style::default());
            if self.device.draw_key(index, &buf).is_ok() {
                self.last.insert(Surface::Key(index), content);
            }
            return;
        }

        let label = draw.text.clone();
        let value = self.value_text(draw);
        let style = style_for(draw);
        let content = format!("{}\u{0}{}", label.as_deref().unwrap_or(""), value.as_deref().unwrap_or(""));
        if self.last.get(&Surface::Key(index)) == Some(&content) {
            return;
        }
        let buf = self.renderer.key(label.as_deref(), value.as_deref(), &style);
        if self.device.draw_key(index, &buf).is_ok() {
            self.last.insert(Surface::Key(index), content);
        }
    }

    fn render_strip(&mut self, side: Surface) {
        let base = if side == Surface::Left { 0 } else { 3 };
        let cells: [Option<(String, String)>; 3] =
            [self.enc_cell(base), self.enc_cell(base + 1), self.enc_cell(base + 2)];
        let content = format!("{cells:?}");
        if self.last.get(&side) == Some(&content) {
            return;
        }
        let buf = self.renderer.side_strip(&cells, &Style::default());
        let ok = match side {
            Surface::Left => self.device.draw_left(&buf),
            _ => self.device.draw_right(&buf),
        };
        if ok.is_ok() {
            self.last.insert(side, content);
        }
    }

    fn enc_cell(&self, index: u8) -> Option<(String, String)> {
        let draw = self.eff_encoders.get(&format!("e{index}"))?.draw.as_ref()?;
        let label = draw.text.clone().unwrap_or_default();
        let value = self.value_text(draw).unwrap_or_default();
        Some((label, value))
    }

    /// The raw `value` dataref reading, scaled and offset, if data exists.
    fn value_number(&self, draw: &Draw) -> Option<f64> {
        let vref = draw.value.as_ref()?;
        let (name, index) = sim::split_ref(vref);
        let id = (*self.dr_ids.get(name)?)?;
        let raw = self.values.get(&id)?.scalar(index)?;
        Some(raw * draw.scale.unwrap_or(1.0) + draw.offset.unwrap_or(0.0))
    }

    /// The formatted display string for a draw's `value`, if data exists.
    fn value_text(&self, draw: &Draw) -> Option<String> {
        let v = self.value_number(draw)?;
        Some(format_value(draw.format.as_deref().unwrap_or("{}"), v))
    }

    // --- Helpers ---

    fn page(&self) -> &crate::config::Page {
        &self.profile.pages[&self.current_page]
    }

    /// Every dataref reference shown on the current page (keys + encoders).
    fn page_value_refs(&self) -> Vec<String> {
        let keys = self.page().keys.values().filter_map(|k| k.draw.as_ref());
        let encs = self.eff_encoders.values().filter_map(|e| e.draw.as_ref());
        keys.chain(encs).filter_map(|d| d.value.clone()).collect()
    }

    fn dataref_id(&mut self, name: &str) -> Option<i64> {
        if let Some(cached) = self.dr_ids.get(name) {
            return *cached;
        }
        let id = self.sim.dataref(name).map(|m| m.id);
        self.dr_ids.insert(name.to_string(), id);
        id
    }

    fn command_id(&mut self, name: &str) -> Option<i64> {
        if let Some(cached) = self.cmd_ids.get(name) {
            return *cached;
        }
        let id = self.sim.command(name);
        self.cmd_ids.insert(name.to_string(), id);
        id
    }
}

enum Input {
    Key,
    Enc,
    Btn,
}

/// Merge inherited defaults with page overrides (page wins per id).
fn merge<T: Clone>(
    defaults: &std::collections::BTreeMap<String, T>,
    page: &std::collections::BTreeMap<String, T>,
) -> HashMap<String, T> {
    let mut m: HashMap<String, T> = defaults.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (k, v) in page {
        m.insert(k.clone(), v.clone());
    }
    m
}

fn style_for(draw: &Draw) -> Style {
    let mut s = Style::default();
    if let Some(rgb) = draw.text_color.as_deref().and_then(render::parse_color) {
        s.label_color = rgb;
        s.value_color = rgb;
    }
    if let Some(rgb) = draw.bg_color.as_deref().and_then(render::parse_color) {
        s.bg_color = rgb;
    }
    s
}

/// Apply a minimal format string to a number. Supports a single `{...}`
/// placeholder with optional precision (`{:.0}`, `{:.1}`), plus literals
/// around it (`"{:.0} ft"`, `"{:.0}%"`). Anything else prints the bare number.
fn format_value(fmt: &str, v: f64) -> String {
    let (Some(a), Some(b)) = (fmt.find('{'), fmt.find('}')) else {
        return fmt.to_string();
    };
    let spec = &fmt[a + 1..b];
    let num = if let Some(dot) = spec.find('.') {
        match spec[dot + 1..].trim_end_matches('f').parse::<usize>() {
            Ok(p) => format!("{v:.p$}"),
            Err(_) => bare(v),
        }
    } else {
        bare(v)
    };
    format!("{}{num}{}", &fmt[..a], &fmt[b + 1..])
}

/// A whole number prints without a decimal point; otherwise default float.
fn bare(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(format_value("{:.0}", 95.4), "95");
        assert_eq!(format_value("{:.1}", 95.46), "95.5");
        assert_eq!(format_value("{:.0} ft", 5500.0), "5500 ft");
        assert_eq!(format_value("{:.0}%", 80.2), "80%");
        assert_eq!(format_value("{}", 24.0), "24");
        assert_eq!(format_value("{}", 24.5), "24.5");
        assert_eq!(format_value("ON", 1.0), "ON");
    }
}

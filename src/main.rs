//! rustdecks — a lean Loupedeck Live controller for X-Plane 12.
//!
//! Build order (modules added as implemented):
//!   config  ✓  profile schema + validation
//!   sim        X-Plane 12 Web API client (beacon, REST ids, WS values)
//!   device     Loupedeck Live serial driver
//!   render     key/side-strip image composition
//!   app        wiring: events -> actions, dataref updates -> redraws

use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rustdecks::config::Profile;
use rustdecks::device::LoupedeckLive;
use rustdecks::{app, render, sim};

fn main() -> ExitCode {
    let arg = match std::env::args().nth(1) {
        Some(a) => a,
        None => {
            eprintln!("usage: rustdecks <profile.yaml> | rustdecks probe");
            return ExitCode::FAILURE;
        }
    };

    if arg == "probe" {
        return probe();
    }
    if arg == "simprobe" {
        return simprobe(std::env::args().nth(2));
    }
    if arg == "check" {
        return check(std::env::args().nth(2));
    }
    let path = arg;

    let yaml = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let profile = match Profile::parse(&yaml) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    match app::run(profile) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Parse and validate a profile without touching hardware or the sim.
fn check(path: Option<String>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("usage: rustdecks check <profile.yaml>");
        return ExitCode::FAILURE;
    };
    let yaml = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    match Profile::parse(&yaml) {
        Ok(p) => {
            println!("ok: {} pages, home `{}`", p.pages.len(), p.home);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{path}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Hardware smoke test: connect, light up, and print input events for 15s.
fn probe() -> ExitCode {
    let port = match LoupedeckLive::find_port() {
        Some(p) => p,
        None => {
            eprintln!("no Loupedeck found (VID 0x2EC2). Is it plugged in?");
            return ExitCode::FAILURE;
        }
    };
    println!("found Loupedeck at {port}");

    let mut dev = match LoupedeckLive::connect(&port) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("connect failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("connected; handshake ok");

    let reader = match dev.reader() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("reader clone failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || reader.run(tx));

    // Output smoke test: brightness up, light b0..b7, render labeled keys + a strip.
    let _ = dev.set_brightness(0.7);
    for i in 0..8u8 {
        let _ = dev.set_button_color(i, [0, 80, 0]);
    }
    let r = render::Renderer::new().expect("font load");
    let style = render::Style::default();
    let demo = [(0u8, "GEN", "ON"), (1, "BAT", "24.8"), (5, "ALT", "5500")];
    for (idx, label, value) in demo {
        let _ = dev.draw_key(idx, &r.key(Some(label), Some(value), &style));
    }
    let strip = [
        Some(("THR".to_string(), "80%".to_string())),
        Some(("PRP".to_string(), "2400".to_string())),
        Some(("MIX".to_string(), "RICH".to_string())),
    ];
    let _ = dev.draw_left(&r.side_strip(&strip, &style));
    println!("rendered keys + left strip; now listening for input (15s)...");

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(ev) => println!("  {ev:?}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                eprintln!("reader thread ended");
                break;
            }
        }
    }
    println!("probe done");
    ExitCode::SUCCESS
}

/// X-Plane smoke test: discover (or use 127.0.0.1), subscribe to a dataref, and
/// print live values for 10s.
fn simprobe(dataref: Option<String>) -> ExitCode {
    let dataref = dataref
        .unwrap_or_else(|| "sim/flightmodel/position/indicated_airspeed".to_string());

    let host = match sim::discover(Duration::from_secs(5)) {
        Some(addr) => {
            println!(
                "beacon: X-Plane {} at {}",
                addr.xplane_version, addr.host
            );
            addr.host
        }
        None => {
            println!("no beacon; trying 127.0.0.1");
            "127.0.0.1".to_string()
        }
    };

    let (sim, updates) = match sim::Sim::connect(&host, 8086) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("connect failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("connected to Web API");

    let (name, index) = sim::split_ref(&dataref);
    let meta = match sim.dataref(name) {
        Some(m) => m,
        None => {
            eprintln!("dataref not found: {name}");
            return ExitCode::FAILURE;
        }
    };
    println!("resolved {name} -> id {} (writable: {})", meta.id, meta.writable);
    sim.subscribe(&[meta.id]);
    println!("subscribed; printing {dataref} for 10s...");

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match updates.recv_timeout(Duration::from_millis(200)) {
            Ok(up) if up.id == meta.id => {
                println!("  {dataref} = {:?}", up.value.scalar(index));
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    println!("simprobe done");
    ExitCode::SUCCESS
}

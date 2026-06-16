//! Loupedeck Live serial driver.
//!
//! Ported from dlicudi/python-loupedeck-live. The device speaks a WebSocket-ish
//! binary protocol over a USB CDC serial port: an HTTP upgrade handshake, then
//! masked binary frames (mask key is always zero). Multi-byte fields are big-endian.
//!
//! Hardware: 480x270 screen as one framebuffer addressed by x-offset —
//! left strip 0..60, center 60..420 (4x3 grid of 90x90 keys), right strip 420..480.
//! Six rotary knobs (e0..e5, push + rotate) and eight round RGB buttons (b0..b7).

use std::io::{self, ErrorKind, Read, Write};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use serialport::{SerialPort, SerialPortType};

const BAUD_RATE: u32 = 460_800;
const READ_TIMEOUT: Duration = Duration::from_millis(1000);
const LOUPEDECK_VID: u16 = 0x2EC2;

/// Display framebuffer id used by every region (the whole screen is one display).
const DISPLAY_ID: [u8; 2] = [0x00, 0x4D]; // "\x00M"

// Outbound action headers.
const SET_BRIGHTNESS: u16 = 0x0409;
const SET_COLOR: u16 = 0x0702;
const WRITE_FRAMEBUFF: u16 = 0xFF10;
const DRAW: u16 = 0x050F;

// Inbound message headers.
const BUTTON_PRESS: u16 = 0x0500;
const KNOB_ROTATE: u16 = 0x0501;
const TOUCH: u16 = 0x094D;
const TOUCH_END: u16 = 0x096D;

// Geometry.
pub const KEY_SIZE: u16 = 90;
pub const SIDE_W: u16 = 60;
pub const SIDE_H: u16 = 270;
const CENTER_OFFSET: u16 = 60;
const RIGHT_OFFSET: u16 = 420;

/// The serial-WebSocket upgrade handshake the firmware expects. Note the request
/// uses bare `\n` line endings (the firmware is lenient); the reply is normal CRLF.
const WS_UPGRADE_HEADER: &[u8] =
    b"GET /index.html\nHTTP/1.1\nConnection: Upgrade\nUpgrade: websocket\nSec-WebSocket-Key: 123abc\n\n";

/// An input event from the device, in profile vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Screen key 0..=11 touched/released (Loupedeck has no physical key press).
    Key { index: u8, pressed: bool },
    /// Encoder e0..=e5 rotated.
    EncoderTurn { index: u8, clockwise: bool },
    /// Encoder e0..=e5 pushed/released.
    EncoderPress { index: u8, pressed: bool },
    /// Round RGB button b0..=b7 pushed/released.
    Button { index: u8, pressed: bool },
}

pub struct LoupedeckLive {
    port: Box<dyn SerialPort>,
    txn: u8,
}

impl LoupedeckLive {
    /// Find the first Loupedeck-VID serial port, if any.
    pub fn find_port() -> Option<String> {
        serialport::available_ports().ok()?.into_iter().find_map(|p| {
            match p.port_type {
                SerialPortType::UsbPort(info) if info.vid == LOUPEDECK_VID => Some(p.port_name),
                _ => None,
            }
        })
    }

    /// Open a port and perform the upgrade handshake.
    pub fn connect(path: &str) -> io::Result<LoupedeckLive> {
        let mut port = serialport::new(path, BAUD_RATE)
            .timeout(READ_TIMEOUT)
            .open()
            .map_err(to_io)?;
        port.write_all(WS_UPGRADE_HEADER)?;
        port.flush()?;
        if !await_handshake(&mut *port)? {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "Loupedeck handshake failed (no 101 Switching Protocols)",
            ));
        }
        Ok(LoupedeckLive { port, txn: 0 })
    }

    /// A cloned read handle to drive the inbound event loop on its own thread.
    pub fn reader(&self) -> io::Result<Reader> {
        Ok(Reader {
            port: self.port.try_clone().map_err(to_io)?,
            buffer: Vec::with_capacity(256),
        })
    }

    /// Screen/LED brightness, 0.0..=1.0 (device resolves to 0..=10 steps).
    pub fn set_brightness(&mut self, level: f32) -> io::Result<()> {
        let step = (level.clamp(0.0, 1.0) * 10.0).round() as u8;
        self.do_action(SET_BRIGHTNESS, &[step])
    }

    /// Set round button b0..=b7 LED color.
    pub fn set_button_color(&mut self, index: u8, rgb: [u8; 3]) -> io::Result<()> {
        let Some(key) = led_key(index) else {
            return Err(io::Error::new(ErrorKind::InvalidInput, "led index 0..=7"));
        };
        self.do_action(SET_COLOR, &[key, rgb[0], rgb[1], rgb[2]])
    }

    /// Draw a 90x90 RGB565-LE buffer to center key 0..=11.
    pub fn draw_key(&mut self, index: u8, rgb565: &[u8]) -> io::Result<()> {
        let x = (index as u16 % 4) * KEY_SIZE + CENTER_OFFSET;
        let y = (index as u16 / 4) * KEY_SIZE;
        self.write_framebuffer(x, y, KEY_SIZE, KEY_SIZE, rgb565)?;
        self.refresh()
    }

    /// Draw a 60x270 RGB565-LE buffer to the left side strip.
    pub fn draw_left(&mut self, rgb565: &[u8]) -> io::Result<()> {
        self.write_framebuffer(0, 0, SIDE_W, SIDE_H, rgb565)?;
        self.refresh()
    }

    /// Draw a 60x270 RGB565-LE buffer to the right side strip.
    pub fn draw_right(&mut self, rgb565: &[u8]) -> io::Result<()> {
        self.write_framebuffer(RIGHT_OFFSET, 0, SIDE_W, SIDE_H, rgb565)?;
        self.refresh()
    }

    fn write_framebuffer(
        &mut self,
        x: u16,
        y: u16,
        w: u16,
        h: u16,
        buf: &[u8],
    ) -> io::Result<()> {
        let expected = w as usize * h as usize * 2;
        if buf.len() != expected {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("framebuffer {} bytes, expected {expected}", buf.len()),
            ));
        }
        let mut payload = Vec::with_capacity(2 + 8 + buf.len());
        payload.extend_from_slice(&DISPLAY_ID);
        payload.extend_from_slice(&x.to_be_bytes());
        payload.extend_from_slice(&y.to_be_bytes());
        payload.extend_from_slice(&w.to_be_bytes());
        payload.extend_from_slice(&h.to_be_bytes());
        payload.extend_from_slice(buf);
        self.do_action(WRITE_FRAMEBUFF, &payload)
    }

    fn refresh(&mut self) -> io::Result<()> {
        self.do_action(DRAW, &DISPLAY_ID)
    }

    /// Build `header(2) + txn(1) + data`, frame it, and write it.
    fn do_action(&mut self, action: u16, data: &[u8]) -> io::Result<()> {
        self.txn = self.txn.wrapping_add(1);
        if self.txn == 0 {
            self.txn = 1; // firmware ignores transaction id 0
        }
        let mut payload = Vec::with_capacity(3 + data.len());
        payload.extend_from_slice(&action.to_be_bytes());
        payload.push(self.txn);
        payload.extend_from_slice(data);
        self.port.write_all(&ws_frame(&payload))?;
        self.port.flush()
    }
}

/// The inbound side of the connection; run on its own thread.
pub struct Reader {
    port: Box<dyn SerialPort>,
    buffer: Vec<u8>,
}

impl Reader {
    /// Blocking read loop: decode frames and forward events until the channel
    /// closes or the port errors.
    pub fn run(mut self, tx: Sender<Event>) {
        let mut chunk = [0u8; 1024];
        loop {
            match self.port.read(&mut chunk) {
                Ok(0) => {}
                Ok(n) => {
                    self.buffer.extend_from_slice(&chunk[..n]);
                    for msg in parse_frames(&mut self.buffer) {
                        if let Some(ev) = decode_event(&msg) {
                            if tx.send(ev).is_err() {
                                return;
                            }
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::TimedOut => {}
                Err(_) => return,
            }
        }
    }
}

// ---- Pure framing/decoding helpers (unit-tested without hardware) ----

/// Wrap a payload in a zero-masked WebSocket binary frame.
fn ws_frame(payload: &[u8]) -> Vec<u8> {
    let len = payload.len();
    let mut out = Vec::with_capacity(len + 14);
    if len > 0x80 {
        out.extend_from_slice(&[0x82, 0xFF, 0, 0, 0, 0]);
        out.extend_from_slice(&(len as u32).to_be_bytes()); // length at bytes 6..10
        out.extend_from_slice(&[0, 0, 0, 0]); // mask key
    } else {
        out.push(0x82);
        out.push(0x80 + len as u8);
        out.extend_from_slice(&[0, 0, 0, 0]); // mask key
    }
    out.extend_from_slice(payload);
    out
}

/// Scan the buffer for complete inbound frames (`0x82, len, <len bytes>`),
/// returning the message bodies and consuming them from the buffer.
fn parse_frames(buffer: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut msgs = Vec::new();
    loop {
        let Some(pos) = buffer.iter().position(|&b| b == 0x82) else {
            buffer.clear(); // no frame start in sight; drop noise
            break;
        };
        if buffer.len() < pos + 2 {
            buffer.drain(..pos); // keep the partial frame start, drop leading noise
            break;
        }
        let len = buffer[pos + 1] as usize;
        let end = pos + 2 + len;
        if buffer.len() < end {
            buffer.drain(..pos);
            break;
        }
        msgs.push(buffer[pos + 2..end].to_vec());
        buffer.drain(..end);
    }
    msgs
}

/// Decode an inbound message body (`header(2) + txn(1) + data`) into an Event.
fn decode_event(msg: &[u8]) -> Option<Event> {
    if msg.len() < 3 {
        return None;
    }
    let header = u16::from_be_bytes([msg[0], msg[1]]);
    let data = &msg[3..];
    match header {
        BUTTON_PRESS => button_event(*data.first()?, *data.get(1)? == 0x00),
        KNOB_ROTATE => Some(Event::EncoderTurn {
            index: knob_index(*data.first()?)?,
            clockwise: *data.get(1)? == 0x01,
        }),
        TOUCH => decode_touch(data, false),
        TOUCH_END => decode_touch(data, true),
        _ => None,
    }
}

/// Map a BUTTON_PRESS id: 0x01..=0x06 are knob pushes, 0x07 circle, 0x08..=0x0E numbered.
fn button_event(id: u8, pressed: bool) -> Option<Event> {
    match id {
        0x01..=0x06 => Some(Event::EncoderPress { index: id - 1, pressed }),
        0x07 => Some(Event::Button { index: 0, pressed }), // circle -> b0
        0x08..=0x0E => Some(Event::Button { index: id - 7, pressed }), // b1..b7
        _ => None,
    }
}

fn knob_index(id: u8) -> Option<u8> {
    (0x01..=0x06).contains(&id).then_some(id - 1)
}

/// Touch payload: data[1..3]=x, data[3..5]=y (big-endian). Only center touches
/// map to a screen key; side-strip touches are ignored for now.
fn decode_touch(data: &[u8], end: bool) -> Option<Event> {
    if data.len() < 6 {
        return None;
    }
    let x = u16::from_be_bytes([data[1], data[2]]);
    let y = u16::from_be_bytes([data[3], data[4]]);
    if !(CENTER_OFFSET..RIGHT_OFFSET).contains(&x) {
        return None;
    }
    let col = ((x - CENTER_OFFSET) / KEY_SIZE) as u8;
    let row = (y / KEY_SIZE) as u8;
    if col > 3 || row > 2 {
        return None;
    }
    Some(Event::Key {
        index: row * 4 + col,
        pressed: !end,
    })
}

fn led_key(index: u8) -> Option<u8> {
    match index {
        0 => Some(0x07),        // circle
        1..=7 => Some(0x07 + index),
        _ => None,
    }
}

/// Read until the `101 Switching Protocols` line appears, or ~2s elapse.
fn await_handshake(port: &mut dyn SerialPort) -> io::Result<bool> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut acc = Vec::new();
    let mut chunk = [0u8; 256];
    while Instant::now() < deadline {
        match port.read(&mut chunk) {
            Ok(0) => {}
            Ok(n) => {
                acc.extend_from_slice(&chunk[..n]);
                if acc.windows(23).any(|w| w == b"101 Switching Protocols") {
                    return Ok(true);
                }
            }
            Err(e) if e.kind() == ErrorKind::TimedOut => {}
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

fn to_io(e: serialport::Error) -> io::Error {
    io::Error::new(ErrorKind::Other, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_frame_layout() {
        let f = ws_frame(&[0x04, 0x09, 0x01, 0x05]);
        assert_eq!(&f[..6], &[0x82, 0x84, 0, 0, 0, 0]); // 0x80 | len(4)
        assert_eq!(&f[6..], &[0x04, 0x09, 0x01, 0x05]);
    }

    #[test]
    fn long_frame_layout() {
        let payload = vec![0xAB; 200];
        let f = ws_frame(&payload);
        assert_eq!(&f[..2], &[0x82, 0xFF]);
        assert_eq!(&f[6..10], &200u32.to_be_bytes()); // length at bytes 6..10
        assert_eq!(f.len(), 14 + 200);
    }

    #[test]
    fn parse_one_and_partial() {
        // noise, one full frame (len 5: header 0x0500, txn 1, id 0x08, down), then a partial
        let mut buf = vec![0xAA, 0x82, 0x05, 0x05, 0x00, 0x01, 0x08, 0x00, 0x82, 0x02];
        let msgs = parse_frames(&mut buf);
        assert_eq!(msgs, vec![vec![0x05, 0x00, 0x01, 0x08, 0x00]]);
        assert_eq!(buf, vec![0x82, 0x02]); // partial frame retained
    }

    #[test]
    fn decode_button_knob_touch() {
        // round button "1" (0x08) down -> b1
        assert_eq!(
            decode_event(&[0x05, 0x00, 0x01, 0x08, 0x00]),
            Some(Event::Button { index: 1, pressed: true })
        );
        // circle (0x07) up -> b0 release
        assert_eq!(
            decode_event(&[0x05, 0x00, 0x01, 0x07, 0x01]),
            Some(Event::Button { index: 0, pressed: false })
        );
        // knob push 0x01 -> e0
        assert_eq!(
            decode_event(&[0x05, 0x00, 0x01, 0x01, 0x00]),
            Some(Event::EncoderPress { index: 0, pressed: true })
        );
        // knob rotate 0x04 clockwise -> e3 CW
        assert_eq!(
            decode_event(&[0x05, 0x01, 0x01, 0x04, 0x01]),
            Some(Event::EncoderTurn { index: 3, clockwise: true })
        );
        // touch at x=150 (col 1), y=95 (row 1) -> key 5
        let touch = [0x00, 0x00, 150, 0x00, 95, 0x01];
        assert_eq!(
            decode_event(&[&[0x09, 0x4D, 0x01][..], &touch].concat()),
            Some(Event::Key { index: 5, pressed: true })
        );
    }

    #[test]
    fn led_and_knob_mapping() {
        assert_eq!(led_key(0), Some(0x07));
        assert_eq!(led_key(7), Some(0x0E));
        assert_eq!(led_key(8), None);
        assert_eq!(knob_index(0x06), Some(5));
        assert_eq!(knob_index(0x07), None);
    }
}

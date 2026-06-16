//! X-Plane 12 Web API client.
//!
//! Discovery via the UDP beacon, REST (port 8086) to resolve dataref/command
//! names to numeric ids, and a WebSocket for live value subscriptions and
//! command/dataref writes. One thread owns the socket; callers talk to it
//! through channels, so no async runtime is needed.

use std::io::ErrorKind;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use serde_json::{json, Value as Json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::Message;

const BEACON_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 1, 1);
const BEACON_PORT: u16 = 49707;
const WS_READ_TIMEOUT: Duration = Duration::from_millis(50);

/// Where X-Plane was found.
#[derive(Debug, Clone)]
pub struct SimAddr {
    pub host: String,
    pub xplane_version: i32,
}

/// A live value update for a subscribed dataref id.
#[derive(Debug, Clone)]
pub struct Update {
    pub id: i64,
    pub value: DataValue,
}

#[derive(Debug, Clone)]
pub enum DataValue {
    Num(f64),
    Array(Vec<f64>),
}

impl DataValue {
    /// Resolve to a scalar, indexing if this is an array reference.
    pub fn scalar(&self, index: Option<usize>) -> Option<f64> {
        match (self, index) {
            (DataValue::Num(n), _) => Some(*n),
            (DataValue::Array(a), Some(i)) => a.get(i).copied(),
            (DataValue::Array(a), None) => a.first().copied(),
        }
    }
}

/// Metadata for a resolved dataref.
#[derive(Debug, Clone)]
pub struct DatarefMeta {
    pub id: i64,
    pub writable: bool,
}

/// Listen for the X-Plane multicast beacon. `host` is taken from the packet
/// source; the REST/WS API is always on port 8086.
pub fn discover(timeout: Duration) -> Option<SimAddr> {
    let sock = UdpSocket::bind(("0.0.0.0", BEACON_PORT)).ok()?;
    sock.join_multicast_v4(&BEACON_GROUP, &Ipv4Addr::UNSPECIFIED).ok()?;
    sock.set_read_timeout(Some(timeout)).ok()?;
    let mut buf = [0u8; 1024];
    let (n, src) = sock.recv_from(&mut buf).ok()?;
    parse_beacon(&buf[..n]).map(|version| SimAddr {
        host: src.ip().to_string(),
        xplane_version: version,
    })
}

/// Parse a beacon packet, returning the X-Plane version number, or None if the
/// packet isn't a valid `BECN` master-X-Plane beacon.
fn parse_beacon(pkt: &[u8]) -> Option<i32> {
    if pkt.len() < 21 || &pkt[0..5] != b"BECN\x00" {
        return None;
    }
    // <BBiiIH : major, minor, host_id(i32), version(i32), role(u32), port(u16)
    let host_id = i32::from_le_bytes(pkt[7..11].try_into().ok()?);
    let version = i32::from_le_bytes(pkt[11..15].try_into().ok()?);
    if host_id != 1 {
        return None; // 1 = X-Plane (2 = PlaneMaker)
    }
    Some(version)
}

/// A connected X-Plane session: REST resolution + a channel to the WS thread.
pub struct Sim {
    rest_base: String,
    agent: ureq::Agent,
    out: Sender<String>,
    req: AtomicU64,
}

impl Sim {
    /// Connect to the Web API at `host`, pick the API version, and spawn the
    /// WebSocket thread. Returns the handle and the stream of value updates.
    pub fn connect(host: &str, port: u16) -> Result<(Sim, Receiver<Update>), String> {
        let agent = ureq::builder()
            .timeout(Duration::from_secs(5))
            .build();
        let version = detect_version(&agent, host, port);
        let rest_base = format!("http://{host}:{port}/api/{version}");
        let ws_url = format!("ws://{host}:{port}/api/{version}");

        let (mut socket, _) =
            tungstenite::connect(&ws_url).map_err(|e| format!("ws connect {ws_url}: {e}"))?;
        if let MaybeTlsStream::Plain(s) = socket.get_mut() {
            s.set_read_timeout(Some(WS_READ_TIMEOUT))
                .map_err(|e| e.to_string())?;
        }

        let (out_tx, out_rx) = mpsc::channel::<String>();
        let (up_tx, up_rx) = mpsc::channel::<Update>();
        std::thread::spawn(move || ws_loop(socket, out_rx, up_tx));

        Ok((
            Sim {
                rest_base,
                agent,
                out: out_tx,
                req: AtomicU64::new(1),
            },
            up_rx,
        ))
    }

    fn next_req(&self) -> u64 {
        self.req.fetch_add(1, Ordering::Relaxed)
    }

    /// Resolve a dataref name (without any `[index]` suffix) to its id.
    pub fn dataref(&self, name: &str) -> Option<DatarefMeta> {
        let url = format!("{}/datarefs?filter[name]={}", self.rest_base, name);
        let body: Json = self.agent.get(&url).call().ok()?.into_json().ok()?;
        let item = body.get("data")?.as_array()?.first()?;
        Some(DatarefMeta {
            id: item.get("id")?.as_i64()?,
            writable: item.get("is_writable").and_then(Json::as_bool).unwrap_or(false),
        })
    }

    /// Resolve a command name to its id.
    pub fn command(&self, name: &str) -> Option<i64> {
        let url = format!("{}/commands?filter[name]={}", self.rest_base, name);
        let body: Json = self.agent.get(&url).call().ok()?.into_json().ok()?;
        body.get("data")?.as_array()?.first()?.get("id")?.as_i64()
    }

    /// Subscribe to live values for the given dataref ids (whole datarefs).
    pub fn subscribe(&self, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let datarefs: Vec<Json> = ids.iter().map(|id| json!({ "id": id })).collect();
        self.send(json!({
            "type": "dataref_subscribe_values",
            "req_id": self.next_req(),
            "params": { "datarefs": datarefs },
        }));
    }

    /// Write a value to a dataref id.
    pub fn set_dataref(&self, id: i64, value: f64) {
        self.send(json!({
            "type": "dataref_set_values",
            "req_id": self.next_req(),
            "params": { "datarefs": [ { "id": id, "value": value } ] },
        }));
    }

    /// Fire a command as a momentary tap (press then release).
    pub fn run_command(&self, id: i64) {
        self.send(json!({
            "type": "command_set_is_active",
            "req_id": self.next_req(),
            "params": { "commands": [ { "id": id, "is_active": true, "duration": 0.0 } ] },
        }));
        self.send(json!({
            "type": "command_set_is_active",
            "req_id": self.next_req(),
            "params": { "commands": [ { "id": id, "is_active": false } ] },
        }));
    }

    fn send(&self, msg: Json) {
        let _ = self.out.send(msg.to_string());
    }
}

/// Ask `/api/capabilities` for the newest supported API version (falls back to v2).
fn detect_version(agent: &ureq::Agent, host: &str, port: u16) -> String {
    let url = format!("http://{host}:{port}/api/capabilities");
    let fallback = "v2".to_string();
    let Ok(resp) = agent.get(&url).call() else {
        return fallback;
    };
    let Ok(body): Result<Json, _> = resp.into_json() else {
        return fallback;
    };
    body.get("api")
        .and_then(|a| a.get("versions"))
        .and_then(Json::as_array)
        .and_then(|v| v.last())
        .and_then(Json::as_str)
        .map(String::from)
        .unwrap_or(fallback)
}

/// The WebSocket thread: drain outgoing requests, then read inbound updates.
fn ws_loop(
    mut socket: tungstenite::WebSocket<MaybeTlsStream<std::net::TcpStream>>,
    out_rx: Receiver<String>,
    up_tx: Sender<Update>,
) {
    loop {
        // Flush any pending outgoing requests first (bounded latency = read timeout).
        loop {
            match out_rx.try_recv() {
                Ok(text) => {
                    if socket.send(Message::Text(text)).is_err() {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        match socket.read() {
            Ok(Message::Text(text)) => {
                for up in parse_updates(&text) {
                    if up_tx.send(up).is_err() {
                        return;
                    }
                }
            }
            Ok(Message::Ping(p)) => {
                let _ = socket.send(Message::Pong(p));
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
    }
}

/// Parse a `dataref_update_values` message into per-id updates. Other message
/// types (result, command updates) are ignored.
fn parse_updates(text: &str) -> Vec<Update> {
    let Ok(msg): Result<Json, _> = serde_json::from_str(text) else {
        return Vec::new();
    };
    if msg.get("type").and_then(Json::as_str) != Some("dataref_update_values") {
        return Vec::new();
    }
    let Some(data) = msg.get("data").and_then(Json::as_object) else {
        return Vec::new();
    };
    data.iter()
        .filter_map(|(k, v)| {
            let id = k.parse::<i64>().ok()?;
            let value = json_to_value(v)?;
            Some(Update { id, value })
        })
        .collect()
}

fn json_to_value(v: &Json) -> Option<DataValue> {
    if let Some(n) = v.as_f64() {
        return Some(DataValue::Num(n));
    }
    if let Some(b) = v.as_bool() {
        return Some(DataValue::Num(if b { 1.0 } else { 0.0 }));
    }
    if let Some(arr) = v.as_array() {
        let nums = arr.iter().filter_map(Json::as_f64).collect();
        return Some(DataValue::Array(nums));
    }
    None // string/data-type datarefs not handled yet
}

/// Split a dataref reference into its name and optional `[index]`.
pub fn split_ref(s: &str) -> (&str, Option<usize>) {
    if let Some(open) = s.rfind('[') {
        if s.ends_with(']') {
            if let Ok(i) = s[open + 1..s.len() - 1].parse::<usize>() {
                return (&s[..open], Some(i));
            }
        }
    }
    (s, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beacon_parsing() {
        let mut pkt = Vec::new();
        pkt.extend_from_slice(b"BECN\x00");
        pkt.push(1); // major
        pkt.push(1); // minor
        pkt.extend_from_slice(&1i32.to_le_bytes()); // host_id = X-Plane
        pkt.extend_from_slice(&121400i32.to_le_bytes()); // version
        pkt.extend_from_slice(&1u32.to_le_bytes()); // role
        pkt.extend_from_slice(&49000u16.to_le_bytes()); // udp port
        assert_eq!(parse_beacon(&pkt), Some(121400));

        // PlaneMaker (host_id 2) is rejected
        pkt[7] = 2;
        assert_eq!(parse_beacon(&pkt), None);

        assert_eq!(parse_beacon(b"junk"), None);
    }

    #[test]
    fn parse_update_message() {
        let text = r#"{"type":"dataref_update_values","data":{"42":3.5,"7":[1.0,0.0,2.0],"9":true}}"#;
        let mut ups = parse_updates(text);
        ups.sort_by_key(|u| u.id);
        assert_eq!(ups.len(), 3);
        assert!(matches!(ups[0], Update { id: 7, value: DataValue::Array(_) }));
        assert_eq!(ups[1].value.scalar(None), Some(1.0)); // id 9 = true
        assert_eq!(ups[2].value.scalar(None), Some(3.5)); // id 42
    }

    #[test]
    fn ignores_non_updates() {
        assert!(parse_updates(r#"{"type":"result","req_id":1,"success":true}"#).is_empty());
    }

    #[test]
    fn array_indexing() {
        let v = DataValue::Array(vec![10.0, 20.0, 30.0]);
        assert_eq!(v.scalar(Some(1)), Some(20.0));
        assert_eq!(v.scalar(Some(9)), None);
        assert_eq!(DataValue::Num(5.0).scalar(Some(2)), Some(5.0));
    }

    #[test]
    fn ref_splitting() {
        assert_eq!(split_ref("sim/foo/bar"), ("sim/foo/bar", None));
        assert_eq!(split_ref("sim/foo/bar[3]"), ("sim/foo/bar", Some(3)));
        assert_eq!(split_ref("sim/foo[x]"), ("sim/foo[x]", None));
    }
}

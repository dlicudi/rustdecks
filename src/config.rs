//! Profile schema: a single YAML file describing one deck's pages, what each
//! input does (`Action`) and what each surface shows (`Draw`).
//!
//! Hardware vocabulary (Loupedeck Live):
//!   - keys      `0..=11`  the 4x3 touch grid (90x90 each)
//!   - encoders  `e0..=e5` six rotary knobs; `e0..e2` draw to the left
//!                         side strip, `e3..e5` to the right (each cell 60x90)
//!   - leds      `b0..=b7` eight round RGB buttons below the screen

use std::collections::BTreeMap;

use serde::Deserialize;

/// Top-level profile, one per YAML file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(default)]
    pub device: Device,
    /// Screen/LED brightness, 0.0..=1.0.
    #[serde(default = "default_brightness")]
    pub brightness: f32,
    #[serde(default)]
    pub sim: Sim,
    /// Name of the page shown at startup.
    pub home: String,
    pub pages: BTreeMap<String, Page>,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Device {
    #[default]
    LoupedeckLive,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sim {
    /// `"auto"` to discover via the X-Plane UDP beacon, or a host/IP.
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    #[serde(default)]
    pub keys: BTreeMap<u8, Key>,
    #[serde(default)]
    pub encoders: BTreeMap<String, Encoder>,
    #[serde(default)]
    pub leds: BTreeMap<String, Led>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    pub draw: Option<Draw>,
    pub press: Option<Action>,
    pub long_press: Option<Action>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Encoder {
    /// Rendered into this encoder's side-strip cell.
    pub draw: Option<Draw>,
    pub turn_cw: Option<Action>,
    pub turn_ccw: Option<Action>,
    pub press: Option<Action>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Led {
    /// `#rrggbb` or a named color; may be driven by a dataref later.
    pub color: Option<String>,
    pub press: Option<Action>,
}

/// What a surface shows. A static `text` label and/or a live `value` (a
/// dataref) transformed by `scale`/`offset` and rendered with `format`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draw {
    pub text: Option<String>,
    /// Dataref path whose value is displayed.
    pub value: Option<String>,
    /// Rust format string applied to the value, e.g. `"{:.0}"`. Default `"{}"`.
    pub format: Option<String>,
    pub scale: Option<f64>,
    pub offset: Option<f64>,
    pub text_color: Option<String>,
    pub bg_color: Option<String>,
}

/// What an input does. Untagged: the present field selects the variant, so the
/// YAML reads naturally (`press: { command: ... }`, `press: { page: ... }`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Action {
    Command {
        command: String,
    },
    Page {
        page: String,
    },
    SetDataref {
        #[serde(rename = "set-dataref")]
        dataref: String,
        value: f64,
    },
}

fn default_brightness() -> f32 {
    0.7
}
fn default_host() -> String {
    "auto".to_string()
}
fn default_port() -> u16 {
    8086
}

impl Default for Sim {
    fn default() -> Self {
        Sim {
            host: default_host(),
            port: default_port(),
        }
    }
}

impl Profile {
    /// Parse and validate a profile from YAML text.
    pub fn parse(yaml: &str) -> Result<Profile, String> {
        let profile: Profile =
            serde_yaml_ng::from_str(yaml).map_err(|e| format!("invalid YAML: {e}"))?;
        profile.validate()?;
        Ok(profile)
    }

    /// Check hardware bounds and cross-references that serde can't express.
    fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.brightness) {
            return Err(format!(
                "brightness must be 0.0..=1.0, got {}",
                self.brightness
            ));
        }
        if !self.pages.contains_key(&self.home) {
            return Err(format!("home page `{}` is not defined", self.home));
        }
        for (name, page) in &self.pages {
            for &idx in page.keys.keys() {
                if idx > 11 {
                    return Err(format!("page `{name}`: key {idx} out of range (0..=11)"));
                }
            }
            check_ids(name, "encoder", page.encoders.keys(), 'e', 5)?;
            check_ids(name, "led", page.leds.keys(), 'b', 7)?;

            // Every `page` action must target a defined page.
            for action in page.actions() {
                if let Action::Page { page: target } = action {
                    if !self.pages.contains_key(target) {
                        return Err(format!(
                            "page `{name}`: action targets undefined page `{target}`"
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Validate `eN` / `bN`-style ids against their max index.
fn check_ids<'a>(
    page: &str,
    kind: &str,
    ids: impl Iterator<Item = &'a String>,
    prefix: char,
    max: u8,
) -> Result<(), String> {
    for id in ids {
        let n = id
            .strip_prefix(prefix)
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|&n| n <= max);
        if n.is_none() {
            return Err(format!(
                "page `{page}`: invalid {kind} id `{id}` (expected {prefix}0..={prefix}{max})"
            ));
        }
    }
    Ok(())
}

impl Page {
    /// All actions defined on this page, for cross-reference checks.
    fn actions(&self) -> impl Iterator<Item = &Action> {
        let keys = self
            .keys
            .values()
            .flat_map(|k| [k.press.as_ref(), k.long_press.as_ref()]);
        let encoders = self.encoders.values().flat_map(|e| {
            [
                e.press.as_ref(),
                e.turn_cw.as_ref(),
                e.turn_ccw.as_ref(),
            ]
        });
        let leds = self.leds.values().map(|l| l.press.as_ref());
        keys.chain(encoders).chain(leds).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
home: main
pages:
  main:
    keys:
      0:
        draw: { text: "GEN", value: "sim/cockpit2/electrical/generator_on[0]", format: "{:.0}" }
        press: { command: "sim/electrical/generator_1_toggle" }
      1:
        draw: { text: "RADIO" }
        press: { page: radio }
    encoders:
      e0:
        draw: { text: "THR", value: "sim/flightmodel/engine/ENGN_thro[0]", scale: 100.0, format: "{:.0}%" }
        turn_cw: { command: "sim/engines/throttle_up" }
        turn_ccw: { command: "sim/engines/throttle_down" }
    leds:
      b0:
        color: "#00ff00"
  radio:
    keys:
      0:
        draw: { text: "BACK" }
        press: { page: main }
"##;

    #[test]
    fn parses_and_validates_sample() {
        let p = Profile::parse(SAMPLE).expect("should parse");
        assert_eq!(p.device, Device::LoupedeckLive);
        assert_eq!(p.brightness, 0.7);
        assert_eq!(p.sim.port, 8086);
        assert_eq!(p.pages.len(), 2);
    }

    #[test]
    fn rejects_undefined_home() {
        let err = Profile::parse("home: nope\npages:\n  main: {}\n").unwrap_err();
        assert!(err.contains("home page"), "{err}");
    }

    #[test]
    fn rejects_out_of_range_key() {
        let yaml = "home: main\npages:\n  main:\n    keys:\n      12: { draw: { text: x } }\n";
        let err = Profile::parse(yaml).unwrap_err();
        assert!(err.contains("out of range"), "{err}");
    }

    #[test]
    fn rejects_bad_encoder_id() {
        let yaml = "home: main\npages:\n  main:\n    encoders:\n      e9: {}\n";
        let err = Profile::parse(yaml).unwrap_err();
        assert!(err.contains("invalid encoder id"), "{err}");
    }

    #[test]
    fn rejects_page_action_to_missing_page() {
        let yaml = "home: main\npages:\n  main:\n    keys:\n      0: { press: { page: ghost } }\n";
        let err = Profile::parse(yaml).unwrap_err();
        assert!(err.contains("undefined page"), "{err}");
    }
}

//! A snapshot of the deck's current visual state, published by the running app
//! and read by the TUI. Lets the terminal mirror what's on (or would be on) the
//! physical deck, with or without hardware.

use std::sync::{Arc, Mutex};

pub type SharedDeck = Arc<Mutex<DeckState>>;

#[derive(Debug, Clone, Default)]
pub struct DeckState {
    pub device: String,
    pub sim: String,
    pub page: String,
    pub keys: [KeyView; 12],
    pub left: [Cell; 3],
    pub right: [Cell; 3],
    pub leds: [LedView; 8],
    /// Resolved live datarefs (name, formatted value).
    pub datarefs: Vec<(String, String)>,
    /// Most-recent input events, newest last.
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct KeyView {
    pub kind: KeyKind,
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum KeyKind {
    #[default]
    Empty,
    Text,
    Icon,
    Annunciator {
        lit: bool,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Cell {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Default)]
pub struct LedView {
    pub on: bool,
    pub rgb: [u8; 3],
    /// Page this LED navigates to, if any.
    pub target: String,
}

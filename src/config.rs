//! Settings kept between runs in `%APPDATA%\MonitorAligner\config.json`.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::layout::Anchor;

#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    /// Panel sizes entered by hand, in native (unrotated) orientation, keyed by `Display::key`.
    #[serde(default)]
    pub sizes: HashMap<String, [f32; 2]>,
    #[serde(default)]
    pub anchor: Anchor,
    #[serde(default)]
    pub live: bool,
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("MonitorAligner").join("config.json"))
}

impl Config {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }
}

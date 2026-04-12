use anyhow::Result;
use serde::{Deserialize, Serialize};
use terminal_core::SplitDirection;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize)]
pub struct LayoutFile {
    pub name: String,
    pub description: Option<String>,
    pub root: SerializedNode,
    #[serde(default)]
    pub panes: std::collections::HashMap<String, PaneDef>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SerializedNode {
    Leaf { pane: String },
    Split {
        direction: SerializedDirection,
        ratio: f32,
        first: Box<SerializedNode>,
        second: Box<SerializedNode>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SerializedDirection {
    Vertical,
    Horizontal,
}

impl From<&SerializedDirection> for SplitDirection {
    fn from(d: &SerializedDirection) -> Self {
        match d {
            SerializedDirection::Vertical => SplitDirection::Vertical,
            SerializedDirection::Horizontal => SplitDirection::Horizontal,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PaneDef {
    pub spawn: String,
    #[serde(default)]
    pub command: Vec<String>,
    pub cwd: Option<String>,
}

pub fn load_layout(path: &Path) -> Result<LayoutFile> {
    let text = std::fs::read_to_string(path)?;
    let layout: LayoutFile = toml::from_str(&text)?;
    Ok(layout)
}

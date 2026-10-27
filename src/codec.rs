//! The generic save explorer. Every game's save is turned into a tree of
//! labelled values, shown the same way for every game. What differs between
//! games is only how the save is decoded and encoded, which each game
//! describes through [`Codec`].

use serde_json::Value;

/// One row of the explorer: a label, a short value, and the rows below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
  pub label: String,
  pub value: String,
  pub children: Vec<Node>,
}

impl Node {
  /// Builds the tree for a JSON value. Objects and arrays get one child per
  /// key or element; scalars become leaves.
  pub fn from_value(label: impl Into<String>, value: &Value) -> Node {
    let label = label.into();
    match value {
      Value::Object(map) => Node {
        label,
        value: format!("{} fields", map.len()),
        children: map.iter().map(|(key, value)| Node::from_value(key.as_str(), value)).collect(),
      },
      Value::Array(items) => Node {
        label,
        value: format!("{} items", items.len()),
        children: items.iter().enumerate().map(|(index, value)| Node::from_value(format!("[{index}]"), value)).collect(),
      },
      Value::String(text) => Node { label, value: text.clone(), children: Vec::new() },
      other => Node { label, value: other.to_string(), children: Vec::new() },
    }
  }
}

/// How one game's save file is decoded, encoded and backed up. Shown in the
/// explorer so the difference between games is visible in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codec {
  pub game: &'static str,
  pub decode: &'static str,
  pub encode: &'static str,
  pub backup: &'static str,
}

/// Rise: a DSSS/Citrus container around an RSZ class stream.
pub const RISE_CODEC: Codec = Codec {
  game: "MH Rise",
  decode: "Steam DSSS/Citrus container, then the RSZ class stream is parsed into classes and fields.",
  encode: "The class stream is packed back into a DSSS container with a fresh MurmurHash3 checksum.",
  backup: "The whole save folder is copied to Documents\\MHR-Save-Backups before any write.",
};

/// World (Iceborne): a Blowfish-encrypted save with checksummed regions.
pub const WORLD_CODEC: Codec = Codec {
  game: "MH World",
  decode: "Blowfish-ECB with the constant key, 4-byte word byteswap, then the character slots are read at fixed offsets.",
  encode: "Slots are written back, then the four region checksums and the outer SHA-1 are rewritten and the file is re-encrypted.",
  backup: "The whole save folder is copied to Documents\\MHR-Save-Backups before any write.",
};

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  #[test]
  fn builds_labelled_rows_for_objects_and_arrays() {
    let value = json!({ "name": "Aang", "items": [1, 2] });
    let root = Node::from_value("save", &value);
    assert_eq!(root.value, "2 fields");
    let name = root.children.iter().find(|node| node.label == "name").expect("name row");
    assert_eq!(name.value, "Aang");
    let items = root.children.iter().find(|node| node.label == "items").expect("items row");
    assert_eq!(items.value, "2 items");
    assert_eq!(items.children[1].label, "[1]");
    assert_eq!(items.children[1].value, "2");
  }
}

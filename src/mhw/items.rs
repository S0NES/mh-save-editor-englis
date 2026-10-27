//! Item id → English name for Monster Hunter: World (Iceborne).
//!
//! `assets/mhw/item_names_en.json` is extracted from the game's
//! `itemData.itm` and `item_eng.gmd`: the name of item `id` is string `id * 2`.
//! Parsed once on first use.

use std::{collections::BTreeMap, sync::OnceLock};

const ITEM_NAMES_EN_JSON: &str = include_str!("../../assets/mhw/item_names_en.json");

fn table() -> &'static BTreeMap<u32, &'static str> {
  static TABLE: OnceLock<BTreeMap<u32, &'static str>> = OnceLock::new();
  TABLE.get_or_init(|| {
    let parsed: BTreeMap<String, String> =
      serde_json::from_str(ITEM_NAMES_EN_JSON).expect("embedded World item names");
    parsed
      .into_iter()
      .map(|(key, name)| {
        let id: u32 = key.parse().expect("decimal item id");
        let name: &'static str = Box::leak(name.into_boxed_str());
        (id, name)
      })
      .collect()
  })
}

/// The English name of a World item, if the table has one.
pub fn item_name(id: u32) -> Option<&'static str> {
  table().get(&id).copied()
}


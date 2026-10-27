//! Item icons for Monster Hunter Rise, from the Monster Hunter Wiki's MHRS item
//! icon set. `assets/rise/items.png` packs the icons 16 per row in 64-pixel
//! cells; `assets/rise/item_icons.json` maps a Rise item id to its cell.

use std::{collections::BTreeMap, sync::OnceLock};

use image::RgbaImage;

const ATLAS_PNG: &[u8] = include_bytes!("../assets/rise/items.png");
const CELLS_JSON: &str = include_str!("../assets/rise/item_icons.json");

/// Edge length of one icon cell, in pixels.
pub const ICON_SIZE: u32 = 64;
/// Icon cells per atlas row.
const ATLAS_COLUMNS: u32 = 16;

struct Assets {
  atlas: RgbaImage,
  /// Rise item id -> cell index in the atlas.
  cells: BTreeMap<u32, u32>,
}

fn assets() -> &'static Assets {
  static ASSETS: OnceLock<Assets> = OnceLock::new();
  ASSETS.get_or_init(|| {
    let atlas = image::load_from_memory(ATLAS_PNG).expect("embedded Rise item atlas").to_rgba8();
    let parsed: BTreeMap<String, u32> = serde_json::from_str(CELLS_JSON).expect("embedded Rise item icon index");
    let cells = parsed.into_iter().map(|(key, cell)| (key.parse().expect("decimal item id"), cell)).collect();
    Assets { atlas, cells }
  })
}

/// Decodes the atlas now, so the first Rise item view does not pay for it.
pub fn warm() {
  let _ = assets();
}

/// RGBA pixels (straight alpha, `ICON_SIZE` squared) for a Rise item, or `None`
/// when the wiki has no icon for it.
pub fn item_icon(item_id: u32) -> Option<Vec<u8>> {
  let assets = assets();
  let &cell = assets.cells.get(&item_id)?;
  let left = (cell % ATLAS_COLUMNS) * ICON_SIZE;
  let top = (cell / ATLAS_COLUMNS) * ICON_SIZE;
  let mut pixels = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
  for y in 0..ICON_SIZE {
    for x in 0..ICON_SIZE {
      pixels.extend_from_slice(&assets.atlas.get_pixel(left + x, top + y).0);
    }
  }
  Some(pixels)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn potion_has_a_rise_icon() {
    let pixels = item_icon(0x0410_0006).expect("Rise Potion has an icon");
    assert_eq!(pixels.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
    assert!(item_icon(u32::MAX).is_none());
  }
}

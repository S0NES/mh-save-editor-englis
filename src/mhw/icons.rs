//! World item icons from the game's atlas (`items.png` and `items_mask.png`),
//! drawn the way the reference editor draws them: the atlas is masked, then
//! multiplied with the item's rarity colour from `ui_colortable.uct`.

use std::{collections::BTreeMap, sync::OnceLock};

use image::RgbaImage;

const ATLAS_PNG: &[u8] = include_bytes!("../../assets/mhw/items.png");
const MASK_PNG: &[u8] = include_bytes!("../../assets/mhw/items_mask.png");
const PALETTE_UCT: &[u8] = include_bytes!("../../assets/mhw/ui_colortable.uct");
const ICONS_JSON: &str = include_str!("../../assets/mhw/item_icons.json");

/// Edge length of one icon in the atlas, in pixels.
pub const ICON_SIZE: u32 = 64;
/// Icons per atlas row.
const ATLAS_COLUMNS: u32 = 16;
/// Bytes of the `.uct` file header before the item palette.
const UCT_HEADER: usize = 12;
/// One palette entry: a u32, RGBA bytes, then five f32s.
const UCT_ENTRY: usize = 28;

struct Assets {
  atlas: RgbaImage,
  mask: RgbaImage,
  /// Item palette: RGBA per colour index.
  palette: Vec<[u8; 4]>,
  /// Item id -> (icon id in the atlas, colour index in the palette).
  icons: BTreeMap<u32, (u32, u32)>,
}

fn assets() -> &'static Assets {
  static ASSETS: OnceLock<Assets> = OnceLock::new();
  ASSETS.get_or_init(|| {
    let atlas = image::load_from_memory(ATLAS_PNG).expect("embedded item atlas").to_rgba8();
    let mask = image::load_from_memory(MASK_PNG).expect("embedded item mask").to_rgba8();
    let count = u32::from_le_bytes(PALETTE_UCT[UCT_HEADER..UCT_HEADER + 4].try_into().expect("four bytes"));
    let palette = (0..count as usize)
      .map(|index| {
        let at = UCT_HEADER + 4 + index * UCT_ENTRY + 4;
        [PALETTE_UCT[at], PALETTE_UCT[at + 1], PALETTE_UCT[at + 2], PALETTE_UCT[at + 3]]
      })
      .collect();
    let parsed: BTreeMap<String, (u32, u32)> = serde_json::from_str(ICONS_JSON).expect("embedded item icons");
    let icons = parsed.into_iter().map(|(key, value)| (key.parse().expect("decimal item id"), value)).collect();
    Assets { atlas, mask, palette, icons }
  })
}

/// Decodes the atlas and palette now, so the first icon does not pay for it.
pub fn warm() {
  let _ = assets();
}

/// RGBA pixels (straight alpha, `ICON_SIZE` squared) for a World item, or
/// `None` when the item has no icon.
pub fn item_icon(item_id: u32) -> Option<Vec<u8>> {
  let assets = assets();
  let &(icon_id, color_index) = assets.icons.get(&item_id)?;
  let tint = assets.palette.get(color_index as usize).copied().unwrap_or([255, 255, 255, 255]);
  let left = (icon_id % ATLAS_COLUMNS) * ICON_SIZE;
  let top = (icon_id / ATLAS_COLUMNS) * ICON_SIZE;

  let mut pixels = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
  for y in 0..ICON_SIZE {
    for x in 0..ICON_SIZE {
      let source = assets.atlas.get_pixel(left + x, top + y).0.map(|byte| byte as f32 / 255.0);
      let mask_alpha = assets.mask.get_pixel(left + x, top + y).0[3] as f32 / 255.0;
      // Destination: the tint, with the mask cutting its alpha out.
      let dest_alpha = tint[3] as f32 / 255.0 * (1.0 - mask_alpha);
      let dest = tint.map(|byte| byte as f32 / 255.0);
      // Multiply the atlas over it (premultiplied, Qt's Multiply mode).
      let (src_alpha, src_rgb) = (source[3], [source[0], source[1], source[2]]);
      let alpha = src_alpha + dest_alpha - src_alpha * dest_alpha;
      let mut out = [0u8; 4];
      for channel in 0..3 {
        let src_pre = src_rgb[channel] * src_alpha;
        let dst_pre = dest[channel] * dest_alpha;
        let premul = src_pre * (1.0 - dest_alpha) + dst_pre * (1.0 - src_alpha) + src_pre * dst_pre;
        out[channel] = if alpha > 0.0 { (premul / alpha * 255.0).round().clamp(0.0, 255.0) as u8 } else { 0 };
      }
      out[3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
      pixels.extend_from_slice(&out);
    }
  }
  Some(pixels)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn known_items_have_icons_of_the_right_size() {
    let pixels = item_icon(1).expect("Potion has an icon");
    assert_eq!(pixels.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
    assert!(item_icon(u32::MAX).is_none());
  }

  /// Writes a few icons to `MHW_ICON_DUMP` (a directory) for a visual check.
  #[test]
  fn dump_icons_for_inspection() {
    let Some(dir) = std::env::var_os("MHW_ICON_DUMP") else {
      return;
    };
    let ids = [1u32, 2, 5, 32, 33, 34, 42, 1000];
    let mut sheet = RgbaImage::new(ICON_SIZE * ids.len() as u32, ICON_SIZE);
    for (column, id) in ids.iter().enumerate() {
      let pixels = item_icon(*id).expect("icon");
      let icon = RgbaImage::from_raw(ICON_SIZE, ICON_SIZE, pixels).expect("sized");
      image::imageops::replace(&mut sheet, &icon, (column as u32 * ICON_SIZE) as i64, 0);
    }
    sheet.save(std::path::Path::new(&dir).join("sheet.png")).unwrap();
  }
}

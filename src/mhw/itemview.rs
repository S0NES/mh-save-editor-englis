//! Connects a World character slot to the shared item view: its six item areas
//! become tile boxes, and edits go back through [`MhwSave::set_item`].

use crate::{
  i18n::{fill, tr},
  itemtiles::{Area, Item, ItemModel},
  language::Language,
};

use super::{
  icons,
  items,
  save::{ItemArea, ItemSlot, MAX_ITEM_AMOUNT, MhwSave},
};

/// One character slot of an open World save, as an item model.
pub struct WorldSlotItems<'a> {
  pub save: &'a mut MhwSave,
  pub slot: usize,
  pub language: Language,
}

fn area_label(language: Language, area: ItemArea) -> &'static str {
  match area {
    ItemArea::PouchItems => tr(language, "Item Pouch"),
    ItemArea::PouchAmmo => tr(language, "Ammo Pouch"),
    ItemArea::StorageItems => tr(language, "Item Box"),
    ItemArea::StorageAmmo => tr(language, "Ammo Box"),
    ItemArea::StorageMaterials => tr(language, "Material Box"),
    ItemArea::StorageDecorations => tr(language, "Deco Box"),
  }
}

impl ItemModel for WorldSlotItems<'_> {
  fn game(&self) -> &'static str {
    "mhw"
  }

  fn areas(&self) -> Vec<Area> {
    ItemArea::ALL
      .iter()
      .map(|&area| Area { label: area_label(self.language, area), max_amount: MAX_ITEM_AMOUNT })
      .collect()
  }

  fn entries(&self, area: usize) -> Vec<Item> {
    self
      .save
      .items(self.slot, ItemArea::ALL[area])
      .unwrap_or_default()
      .into_iter()
      .map(|entry| Item { id: entry.id, amount: entry.amount })
      .collect()
  }

  fn set_entry(&mut self, area: usize, index: usize, item: Item) -> Result<(), String> {
    let entry = ItemSlot { id: item.id, amount: item.amount };
    self.save.set_item(self.slot, ItemArea::ALL[area], index, entry).map_err(|error| format!("{error:?}"))
  }

  fn name(&self, id: u32) -> String {
    match items::item_name(id) {
      Some(name) => name.to_owned(),
      None => fill(tr(self.language, "Unknown item {}"), &[&id]),
    }
  }

  fn icon(&self, id: u32) -> Option<Vec<u8>> {
    icons::item_icon(id)
  }
}

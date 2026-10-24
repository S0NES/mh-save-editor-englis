//! Bulk operations on an item box layout: a `Vec<(item id, count)>` in slot
//! order. Everything here is pure list manipulation; `edit::write_item_box`
//! puts the result back into the save.

use crate::{edit::ITEM_ID_NONE, items};

/// The game's empty-slot form.
pub const EMPTY: (u32, u32) = (ITEM_ID_NONE, 0);

/// Largest quantity the bulk "set quantity" action will write.
pub const MAX_QUANTITY: u32 = 9999;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
  Name,
  Category,
  Id,
  QuantityDesc,
}

impl SortKey {
  pub const ALL: [SortKey; 4] =
    [SortKey::Name, SortKey::Category, SortKey::Id, SortKey::QuantityDesc];

  pub fn label(self) -> &'static str {
    match self {
      SortKey::Name => "Name",
      SortKey::Category => "Category",
      SortKey::Id => "ID",
      SortKey::QuantityDesc => "Quantity (high to low)",
    }
  }
}

fn is_empty(slot: (u32, u32)) -> bool {
  slot.1 == 0
}

/// Empties the selected slots. Returns how many held an item.
pub fn delete(layout: &mut [(u32, u32)], selected: &[usize]) -> usize {
  let mut removed = 0;
  for &index in selected {
    if let Some(slot) = layout.get_mut(index)
      && !is_empty(*slot)
    {
      *slot = EMPTY;
      removed += 1;
    }
  }
  removed
}

/// Sets the quantity of every selected, non-empty slot. A quantity of 0
/// empties the slot. Returns how many slots changed.
pub fn set_quantity(
  layout: &mut [(u32, u32)],
  selected: &[usize],
  quantity: u32,
) -> Result<usize, String> {
  if quantity > MAX_QUANTITY {
    return Err(format!("Quantity must be at most {MAX_QUANTITY}"));
  }
  let mut changed = 0;
  for &index in selected {
    let Some(slot) = layout.get_mut(index) else {
      continue;
    };
    if is_empty(*slot) || slot.1 == quantity {
      continue;
    }
    *slot = if quantity == 0 { EMPTY } else { (slot.0, quantity) };
    changed += 1;
  }
  Ok(changed)
}

/// Moves the selected slots so they sit side by side starting at `target`
/// (0-based), keeping their relative order. The slots they land on swap into
/// the places they vacated, so nothing is lost or duplicated. Returns the
/// new positions of the moved slots.
pub fn move_to(
  layout: &mut [(u32, u32)],
  selected: &[usize],
  target: usize,
) -> Result<Vec<usize>, String> {
  let mut sources: Vec<usize> = selected.to_vec();
  sources.sort_unstable();
  sources.dedup();
  if sources.is_empty() {
    return Err("Nothing is selected".to_owned());
  }
  if sources.iter().any(|&index| index >= layout.len()) {
    return Err("Selection is outside the item box".to_owned());
  }
  let count = sources.len();
  if target + count > layout.len() {
    return Err(format!(
      "Not enough room: {count} selected slots do not fit from slot {} to the end of the box",
      target + 1
    ));
  }
  let targets: Vec<usize> = (target..target + count).collect();
  let moved: Vec<(u32, u32)> = sources.iter().map(|&index| layout[index]).collect();
  let displaced: Vec<(u32, u32)> =
    targets.iter().filter(|index| !sources.contains(index)).map(|&index| layout[index]).collect();
  let vacated: Vec<usize> = sources.iter().copied().filter(|index| !targets.contains(index)).collect();
  debug_assert_eq!(displaced.len(), vacated.len());
  for (slot, item) in targets.iter().zip(moved) {
    layout[*slot] = item;
  }
  for (slot, item) in vacated.iter().zip(displaced) {
    layout[*slot] = item;
  }
  Ok(targets)
}

/// Sorts the whole box. Items come first in the chosen order; every empty
/// slot moves to the end, so the box is compacted at the same time.
pub fn sort(layout: &mut Vec<(u32, u32)>, key: SortKey) {
  let (mut items_part, empties): (Vec<_>, Vec<_>) =
    layout.iter().copied().partition(|&slot| !is_empty(slot));
  match key {
    SortKey::Name => items_part.sort_by_cached_key(|&(id, _)| {
      (items::item_display_name(id).to_lowercase(), id)
    }),
    SortKey::Category => items_part.sort_by_cached_key(|&(id, _)| {
      (
        items::item_category(id).unwrap_or("~").to_owned(),
        items::item_display_name(id).to_lowercase(),
        id,
      )
    }),
    SortKey::Id => items_part.sort_by_key(|&(id, _)| id),
    SortKey::QuantityDesc => items_part.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0))),
  }
  let empty_count = empties.len();
  layout.clear();
  layout.extend(items_part);
  layout.extend(std::iter::repeat_n(EMPTY, empty_count));
}

#[cfg(test)]
mod tests {
  use super::*;

  fn sample() -> Vec<(u32, u32)> {
    vec![(0x0410_0006, 10), EMPTY, (0x0410_0007, 20), (0x0410_0009, 5), EMPTY]
  }

  #[test]
  fn delete_empties_only_item_slots() {
    let mut layout = sample();
    assert_eq!(delete(&mut layout, &[0, 1, 3]), 2);
    assert_eq!(layout, vec![EMPTY, EMPTY, (0x0410_0007, 20), EMPTY, EMPTY]);
  }

  #[test]
  fn set_quantity_skips_empty_slots_and_zero_deletes() {
    let mut layout = sample();
    assert_eq!(set_quantity(&mut layout, &[0, 1, 2], 99), Ok(2));
    assert_eq!(layout[0], (0x0410_0006, 99));
    assert_eq!(layout[1], EMPTY);
    assert_eq!(set_quantity(&mut layout, &[3], 0), Ok(1));
    assert_eq!(layout[3], EMPTY);
    assert!(set_quantity(&mut layout, &[0], MAX_QUANTITY + 1).is_err());
  }

  #[test]
  fn move_swaps_displaced_slots_into_vacated_ones() {
    let mut layout = sample();
    let landed = move_to(&mut layout, &[0, 2], 3).expect("move");
    assert_eq!(landed, vec![3, 4]);
    assert_eq!(layout[3], (0x0410_0006, 10));
    assert_eq!(layout[4], (0x0410_0007, 20));
    // The slot at 3 (item 9) took the place of the first vacated slot.
    assert_eq!(layout[0], (0x0410_0009, 5));
    assert_eq!(layout[2], EMPTY);
    let mut counts: Vec<_> = layout.iter().copied().filter(|s| s.1 > 0).collect();
    counts.sort();
    assert_eq!(counts.len(), 3);
  }

  #[test]
  fn move_handles_overlapping_ranges_and_bounds() {
    let mut layout = sample();
    let landed = move_to(&mut layout, &[2, 3], 3).expect("overlapping move");
    assert_eq!(landed, vec![3, 4]);
    assert_eq!(layout[3], (0x0410_0007, 20));
    assert_eq!(layout[4], (0x0410_0009, 5));
    assert_eq!(layout[2], EMPTY);
    assert!(move_to(&mut sample(), &[0, 2], 4).is_err());
    assert!(move_to(&mut sample(), &[], 0).is_err());
  }

  #[test]
  fn sort_by_quantity_compacts_the_box() {
    let mut layout = sample();
    sort(&mut layout, SortKey::QuantityDesc);
    assert_eq!(
      layout,
      vec![(0x0410_0007, 20), (0x0410_0006, 10), (0x0410_0009, 5), EMPTY, EMPTY]
    );
  }

  #[test]
  fn sort_by_name_orders_alphabetically() {
    // 0x04100006 is Potion, 0x04100007 is Mega Potion.
    let mut layout = vec![(0x0410_0006, 1), (0x0410_0007, 1)];
    sort(&mut layout, SortKey::Name);
    assert_eq!(layout, vec![(0x0410_0007, 1), (0x0410_0006, 1)]);
  }
}

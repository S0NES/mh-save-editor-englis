//! Bulk operations on an item box layout: a `Vec<(item id, count)>` in slot
//! order. Everything here is pure list manipulation; `edit::write_item_box`
//! puts the result back into the save.

use crate::{edit::ITEM_ID_NONE, items};

/// The game's empty-slot form.
pub const EMPTY: (u32, u32) = (ITEM_ID_NONE, 0);

/// Largest quantity a quantity edit will write.
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

/// A quantity edit typed by the user: `5` sets, `+5` adds, `-5` subtracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuantityOp {
  Set(u32),
  Add(u32),
  Subtract(u32),
}

impl QuantityOp {
  /// Parses `5`, `+5` or `-5` (a space after the sign is allowed). A plain
  /// set above [`MAX_QUANTITY`] is rejected; add/subtract amounts are
  /// clamped when applied.
  pub fn parse(text: &str) -> Result<Self, String> {
    let text = text.trim();
    let (sign, digits) = match text.chars().next() {
      Some('+') => ('+', text[1..].trim_start()),
      Some('-') => ('-', text[1..].trim_start()),
      _ => (' ', text),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
      return Err("Quantity must be a number: 5, +5 or -5".to_owned());
    }
    // Digits that overflow u32 are far above any cap, so saturate.
    let amount = digits.parse::<u32>().unwrap_or(u32::MAX);
    match sign {
      '+' => Ok(QuantityOp::Add(amount)),
      '-' => Ok(QuantityOp::Subtract(amount)),
      _ if amount > MAX_QUANTITY => Err(format!("Quantity must be at most {MAX_QUANTITY}")),
      _ => Ok(QuantityOp::Set(amount)),
    }
  }

  /// The new quantity for a slot currently holding `current`, kept within
  /// `0..=MAX_QUANTITY`.
  pub fn apply(self, current: u32) -> u32 {
    match self {
      QuantityOp::Set(value) => value.min(MAX_QUANTITY),
      QuantityOp::Add(value) => current.saturating_add(value).min(MAX_QUANTITY),
      QuantityOp::Subtract(value) => current.saturating_sub(value),
    }
  }
}

/// Applies `op` to every selected, non-empty slot. A result of 0 empties the
/// slot. Returns how many slots changed.
pub fn apply_quantity(layout: &mut [(u32, u32)], selected: &[usize], op: QuantityOp) -> usize {
  let mut changed = 0;
  for &index in selected {
    let Some(slot) = layout.get_mut(index) else {
      continue;
    };
    if is_empty(*slot) {
      continue;
    }
    let quantity = op.apply(slot.1);
    if quantity == slot.1 {
      continue;
    }
    *slot = if quantity == 0 { EMPTY } else { (slot.0, quantity) };
    changed += 1;
  }
  changed
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
  fn parse_accepts_set_add_and_subtract() {
    assert_eq!(QuantityOp::parse("5"), Ok(QuantityOp::Set(5)));
    assert_eq!(QuantityOp::parse(" 0 "), Ok(QuantityOp::Set(0)));
    assert_eq!(QuantityOp::parse("+5"), Ok(QuantityOp::Add(5)));
    assert_eq!(QuantityOp::parse("+ 5"), Ok(QuantityOp::Add(5)));
    assert_eq!(QuantityOp::parse("-12"), Ok(QuantityOp::Subtract(12)));
    assert_eq!(QuantityOp::parse("- 12"), Ok(QuantityOp::Subtract(12)));
    assert_eq!(QuantityOp::parse("9999"), Ok(QuantityOp::Set(9999)));
  }

  #[test]
  fn parse_rejects_bad_input() {
    for text in ["", "  ", "+", "-", "abc", "5x", "1.5", "+-5", "--5", "10000", "99999999999"] {
      assert!(QuantityOp::parse(text).is_err(), "{text:?} should be rejected");
    }
    assert_eq!(QuantityOp::parse("+99999999999"), Ok(QuantityOp::Add(u32::MAX)));
  }

  #[test]
  fn apply_clamps_to_the_valid_range() {
    assert_eq!(QuantityOp::Set(7).apply(100), 7);
    assert_eq!(QuantityOp::Add(5).apply(10), 15);
    assert_eq!(QuantityOp::Add(5).apply(9998), MAX_QUANTITY);
    assert_eq!(QuantityOp::Add(u32::MAX).apply(1), MAX_QUANTITY);
    assert_eq!(QuantityOp::Subtract(4).apply(10), 6);
    assert_eq!(QuantityOp::Subtract(50).apply(10), 0);
  }

  #[test]
  fn apply_quantity_skips_empty_slots_and_zero_deletes() {
    let mut layout = sample();
    assert_eq!(apply_quantity(&mut layout, &[0, 1, 2], QuantityOp::Set(99)), 2);
    assert_eq!(layout[0], (0x0410_0006, 99));
    assert_eq!(layout[1], EMPTY);
    assert_eq!(apply_quantity(&mut layout, &[3], QuantityOp::Set(0)), 1);
    assert_eq!(layout[3], EMPTY);
    // Unchanged slots are not counted.
    assert_eq!(apply_quantity(&mut layout, &[0], QuantityOp::Set(99)), 0);
  }

  #[test]
  fn apply_quantity_adds_and_subtracts() {
    let mut layout = sample();
    assert_eq!(apply_quantity(&mut layout, &[0, 2, 3], QuantityOp::Add(5)), 3);
    assert_eq!(layout[0].1, 15);
    assert_eq!(layout[2].1, 25);
    assert_eq!(apply_quantity(&mut layout, &[0, 3], QuantityOp::Subtract(10)), 2);
    assert_eq!(layout[0].1, 5);
    assert_eq!(layout[3], EMPTY);
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

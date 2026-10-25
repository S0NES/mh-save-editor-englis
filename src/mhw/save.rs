//! Iceborne character saves: decrypt, read and edit the three character slots,
//! and encrypt back to the exact container the game expects.
//!
//! Layout facts come from the MHWISaveEditor `SAVEDATA1000.bt` template and
//! `types/mhw_save.h`, and were checked against a real Steam save. After
//! decryption the file is a flat struct, not an RSZ class stream, so every
//! field lives at a fixed offset.

use std::{fs, path::Path};

use anyhow::{Context, Result, bail, ensure};

use super::crypto::{decrypt_save, encrypt_save};

/// Size of an Iceborne `SAVEDATA1000` file. Base-game and PS4 saves differ.
pub const IB_SAVE_LEN: usize = 11_284_704;
/// Character slots the game keeps in one file.
pub const SLOT_COUNT: usize = 3;

/// Absolute offset of slot 0's data. Each slot's data is one region long and
/// is followed by its 0x200-byte checksum, so slots are spaced by `SLOT_STRIDE`.
const SLOT_OFFSET: usize = 0x30_10D8;
const SLOT_LEN: usize = 0x20_98C0;
const SLOT_STRIDE: usize = SLOT_LEN + 0x200;

/// `steam_id` in the outer header (u64 at offset 40).
const HEADER_STEAM_ID: usize = 40;
/// `data_size` in the outer header (u64 at offset 32): file size minus 64.
const HEADER_DATA_SIZE: usize = 32;
const HEADER_LEN: usize = 64;

/// Offsets inside a slot. The slot starts with a u32 the editor does not
/// understand, followed by the `mhw_hunter` record at `HUNTER_OFFSET`.
const HUNTER_OFFSET: usize = 4;
const NAME_LEN: usize = 64;
const HUNTER_RANK: usize = HUNTER_OFFSET + NAME_LEN;
const MASTER_RANK: usize = HUNTER_RANK + 4;
const ZENI: usize = MASTER_RANK + 4;
const RESEARCH_POINTS: usize = ZENI + 4;
const HUNTER_RANK_XP: usize = RESEARCH_POINTS + 4;
const MASTER_RANK_XP: usize = HUNTER_RANK_XP + 4;
const PLAYTIME: usize = MASTER_RANK_XP + 4;

/// Highest zenny and research points the game shows.
pub const MAX_ZENI: u32 = 999_999_999;

/// Editable fields of one character slot, as read from the save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunter {
  pub name: String,
  pub hunter_rank: u32,
  pub master_rank: u32,
  pub zeni: u32,
  pub research_points: u32,
  pub hunter_rank_xp: u32,
  pub master_rank_xp: u32,
  /// Seconds played on this slot.
  pub playtime: u32,
}

/// A decrypted Iceborne save held in memory.
#[derive(Debug, Clone)]
pub struct MhwSave {
  data: Vec<u8>,
}

impl MhwSave {
  /// Opens and decrypts an Iceborne save file. No SteamID is needed: every
  /// Iceborne save uses the same key.
  pub fn open_path(path: impl AsRef<Path>) -> Result<Self> {
    let path = path.as_ref();
    let data = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    Self::open_bytes(&data).with_context(|| format!("could not open {}", path.display()))
  }

  pub fn open_bytes(encrypted: &[u8]) -> Result<Self> {
    ensure!(
      encrypted.len() == IB_SAVE_LEN,
      "expected an Iceborne save of {IB_SAVE_LEN} bytes, found {} (base game and PS4 saves are not supported)",
      encrypted.len()
    );
    let mut data = encrypted.to_vec();
    decrypt_save(&mut data)?;

    let save = Self { data };
    ensure!(
      save.read_u64(HEADER_DATA_SIZE) == (IB_SAVE_LEN - HEADER_LEN) as u64,
      "decrypted header does not declare an Iceborne save size"
    );
    Ok(save)
  }

  /// The SteamID64 recorded in the save header, `0` when it is not set.
  pub fn steam_id(&self) -> u64 {
    self.read_u64(HEADER_STEAM_ID)
  }

  pub fn hunter(&self, slot: usize) -> Result<Hunter> {
    let base = self.slot_base(slot)?;
    Ok(Hunter {
      name: self.read_name(base + HUNTER_OFFSET),
      hunter_rank: self.read_u32(base + HUNTER_RANK),
      master_rank: self.read_u32(base + MASTER_RANK),
      zeni: self.read_u32(base + ZENI),
      research_points: self.read_u32(base + RESEARCH_POINTS),
      hunter_rank_xp: self.read_u32(base + HUNTER_RANK_XP),
      master_rank_xp: self.read_u32(base + MASTER_RANK_XP),
      playtime: self.read_u32(base + PLAYTIME),
    })
  }

  pub fn set_zeni(&mut self, slot: usize, zeni: u32) -> Result<()> {
    ensure!(zeni <= MAX_ZENI, "zenny is capped at {MAX_ZENI}");
    self.write_hunter_u32(slot, ZENI, zeni)
  }

  pub fn set_research_points(&mut self, slot: usize, points: u32) -> Result<()> {
    ensure!(points <= MAX_ZENI, "research points are capped at {MAX_ZENI}");
    self.write_hunter_u32(slot, RESEARCH_POINTS, points)
  }

  /// Sets the Hunter Rank experience and the rank it corresponds to.
  pub fn set_hunter_rank_xp(&mut self, slot: usize, xp: u32, rank: u32) -> Result<()> {
    self.write_hunter_u32(slot, HUNTER_RANK_XP, xp)?;
    self.write_hunter_u32(slot, HUNTER_RANK, rank)
  }

  /// Sets the Master Rank experience and the rank it corresponds to.
  pub fn set_master_rank_xp(&mut self, slot: usize, xp: u32, rank: u32) -> Result<()> {
    self.write_hunter_u32(slot, MASTER_RANK_XP, xp)?;
    self.write_hunter_u32(slot, MASTER_RANK, rank)
  }

  /// Replaces the hunter name. The game stores at most 63 bytes plus a NUL.
  pub fn set_name(&mut self, slot: usize, name: &str) -> Result<()> {
    ensure!(name.len() < NAME_LEN, "hunter names are limited to {} bytes", NAME_LEN - 1);
    let base = self.slot_base(slot)? + HUNTER_OFFSET;
    let field = &mut self.data[base..base + NAME_LEN];
    field.fill(0);
    field[..name.len()].copy_from_slice(name.as_bytes());
    Ok(())
  }

  /// Re-encrypts the save into the bytes the game reads.
  pub fn to_bytes(&self) -> Result<Vec<u8>> {
    let mut data = self.data.clone();
    encrypt_save(&mut data)?;
    Ok(data)
  }

  fn slot_base(&self, slot: usize) -> Result<usize> {
    if slot >= SLOT_COUNT {
      bail!("character slot {} does not exist; the game keeps {SLOT_COUNT}", slot + 1);
    }
    Ok(SLOT_OFFSET + slot * SLOT_STRIDE)
  }

  fn write_hunter_u32(&mut self, slot: usize, field: usize, value: u32) -> Result<()> {
    let at = self.slot_base(slot)? + field;
    self.data[at..at + 4].copy_from_slice(&value.to_le_bytes());
    Ok(())
  }

  fn read_name(&self, at: usize) -> String {
    let field = &self.data[at..at + NAME_LEN];
    let end = field.iter().position(|&byte| byte == 0).unwrap_or(NAME_LEN);
    String::from_utf8_lossy(&field[..end]).into_owned()
  }

  fn read_u32(&self, at: usize) -> u32 {
    u32::from_le_bytes(self.data[at..at + 4].try_into().expect("four bytes"))
  }

  fn read_u64(&self, at: usize) -> u64 {
    u64::from_le_bytes(self.data[at..at + 8].try_into().expect("eight bytes"))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn slot_layout_matches_the_region_table() {
    // The third slot's region starts where the template says it does.
    assert_eq!(SLOT_OFFSET + 2 * SLOT_STRIDE, 0x71_4658);
    assert_eq!(SLOT_OFFSET + SLOT_STRIDE, 0x50_AB98);
  }

  /// Reads a real Iceborne save when `MHW_SAVE_FIXTURE` points at one, and
  /// checks a read-modify-write round trip on a copy of it.
  #[test]
  fn real_save_edits_round_trip() {
    let Some(path) = std::env::var_os("MHW_SAVE_FIXTURE") else {
      return;
    };
    let encrypted = fs::read(path).unwrap();
    let mut save = MhwSave::open_bytes(&encrypted).unwrap();
    for slot in 0..SLOT_COUNT {
      let hunter = save.hunter(slot).unwrap();
      println!("slot {slot}: {hunter:?}");
    }

    let before = save.hunter(0).unwrap();
    save.set_zeni(0, 123_456_789).unwrap();
    save.set_research_points(0, 4242).unwrap();
    save.set_name(0, "Edited").unwrap();

    let reopened = MhwSave::open_bytes(&save.to_bytes().unwrap()).unwrap();
    let after = reopened.hunter(0).unwrap();
    assert_eq!(after.zeni, 123_456_789);
    assert_eq!(after.research_points, 4242);
    assert_eq!(after.name, "Edited");
    assert_eq!(after.hunter_rank, before.hunter_rank);
    assert_eq!(after.hunter_rank_xp, before.hunter_rank_xp);
    assert_eq!(reopened.hunter(1).unwrap(), save.hunter(1).unwrap());
  }

  #[test]
  fn rejects_slots_and_values_out_of_range() {
    let mut save = MhwSave { data: vec![0; IB_SAVE_LEN] };
    assert!(save.hunter(SLOT_COUNT).is_err());
    assert!(save.set_zeni(0, MAX_ZENI + 1).is_err());
    assert!(save.set_name(0, &"x".repeat(NAME_LEN)).is_err());
  }
}

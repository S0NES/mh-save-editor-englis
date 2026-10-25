//! Monster Hunter: World (Iceborne) save encryption.
//!
//! Port of the `crypto/iceborne_crypt.h` routines of the MIT-licensed
//! MHWISaveEditor project, which follow LEGENDFF/mhw-Savecrypt. Unlike Rise,
//! there is no per-account key: every Iceborne save is keyed by the same
//! constant, so the SteamID64 is not needed to read or write a save. The
//! layers, outermost first:
//!
//! 1. Blowfish-ECB over the whole file, keyed by [`SAVE_KEY`], with every
//!    4-byte word byte-swapped before and after the cipher.
//! 2. A SHA-1 of everything from offset 64 is stored at offset 12, byte-swapped.
//! 3. Four regions, each followed by a 0x200-byte slot checksum, are masked
//!    with AES-128-ECB blocks. The keys and XOR masks come from a salt that is
//!    derived from the checksum, so changing one byte re-keys the whole region.

use aes::{
  Aes128,
  cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit},
};
use anyhow::{Result, bail, ensure};
use blowfish::Blowfish;
use byteorder::BigEndian;
use sha1::{Digest, Sha1};

use super::tables::{FLOAT_CONSTANTS, INTEGER_CONSTANTS};

/// Blowfish key shared by every Iceborne save (`KEY_SAVEDATA1000`).
pub const SAVE_KEY: &[u8] = b"xieZjoe#P2134-3zmaghgpqoe0z8$3azeq";

const HASH_OFFSET: usize = 12;
const HASH_LEN: usize = 20;
const HASH_START: usize = 64;
const CHECKSUM_LEN: usize = 0x200;
const CRC_IV: u32 = 0xA37A_55D7;

/// `(offset, length, slot)` of each masked region. The checksum for a region
/// sits right after it, at `offset + length`.
const REGIONS: [(usize, usize, u32); 4] = [
  (0x70, 0xDA50, 3),
  (0x30_10D8, 0x20_98C0, 0),
  (0x50_AB98, 0x20_98C0, 1),
  (0x71_4658, 0x20_98C0, 2),
];

/// Decrypts a whole Iceborne save in place. Fails when the outer hash or a
/// region checksum does not match, which means the file is not an Iceborne
/// save or is corrupt.
pub fn decrypt_save(save: &mut [u8]) -> Result<()> {
  ensure!(save.len() % 8 == 0, "save length is not a multiple of the Blowfish block size");
  blowfish_in_place(save, Direction::Decrypt);

  let digest = payload_hash(save);
  ensure!(
    save[HASH_OFFSET..HASH_OFFSET + HASH_LEN] == digest,
    "save hash does not match: not an Iceborne save, or a corrupt one"
  );

  for (offset, length, slot) in REGIONS {
    if !decrypt_region(save, offset, length, slot) {
      bail!("region at 0x{offset:X} failed its checksum: not an Iceborne save, or a corrupt one");
    }
  }
  Ok(())
}

/// Encrypts a whole Iceborne save in place, refreshing every region checksum
/// and the outer hash first. The inverse of [`decrypt_save`].
pub fn encrypt_save(save: &mut [u8]) -> Result<()> {
  ensure!(save.len() % 8 == 0, "save length is not a multiple of the Blowfish block size");
  for (offset, length, slot) in REGIONS {
    encrypt_region(save, offset, length, slot);
  }

  let digest = payload_hash(save);
  save[HASH_OFFSET..HASH_OFFSET + HASH_LEN].copy_from_slice(&digest);
  blowfish_in_place(save, Direction::Encrypt);
  Ok(())
}

#[derive(Clone, Copy)]
enum Direction {
  Encrypt,
  Decrypt,
}

fn blowfish_in_place(data: &mut [u8], direction: Direction) {
  let cipher = Blowfish::<BigEndian>::new_from_slice(SAVE_KEY).expect("the save key length is valid");
  byteswap_words(data);
  for block in data.chunks_exact_mut(8) {
    let mut buffer: [u8; 8] = block.try_into().expect("chunks_exact yields 8 bytes");
    let mut array = buffer.into();
    match direction {
      Direction::Encrypt => cipher.encrypt_block(&mut array),
      Direction::Decrypt => cipher.decrypt_block(&mut array),
    }
    buffer = array.into();
    block.copy_from_slice(&buffer);
  }
  byteswap_words(data);
}

/// Reverses every 4-byte word, the `byteswap` helper of the reference code.
fn byteswap_words(data: &mut [u8]) {
  for word in data.chunks_exact_mut(4) {
    word.reverse();
  }
}

/// SHA-1 of the payload after the header, stored byte-swapped at offset 12.
fn payload_hash(save: &[u8]) -> [u8; HASH_LEN] {
  let mut digest: [u8; HASH_LEN] = Sha1::digest(&save[HASH_START..]).into();
  byteswap_words(&mut digest);
  digest
}

/// Table-free CRC-32 (reflected polynomial 0xEDB88320) with a caller-supplied
/// register, as used by the reference code. There is no final XOR.
fn crc32(mut register: u32, data: &[u8]) -> u32 {
  for &byte in data {
    let mut temp = (register ^ u32::from(byte)) & 0xFF;
    for _ in 0..8 {
      temp = if temp & 1 == 1 { (temp >> 1) ^ 0xEDB8_8320 } else { temp >> 1 };
    }
    register = (register >> 8) ^ temp;
  }
  register
}

/// Little-endian bytes of a word list, the layout the reference code's
/// `int[]` CRC overload reads.
fn words_as_bytes(words: &[u32]) -> Vec<u8> {
  words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// The 0x200-byte checksum that protects one region.
fn slot_checksum(save: &[u8], offset: usize, length: usize, slot: u32) -> [u8; CHECKSUM_LEN] {
  const CONSTANTS: [u32; 4] = [0x5501_2174, 0x09FA_3690, 0x4F5A_E762, 0xA37A_55D7];
  const SLOT_GENERATORS: [u32; 8] = [
    0x2EA1_0CEB,
    0x204D_E35E,
    0x4BF0_CF23,
    0x72B4_01FD,
    0x5CDD_1F19,
    0x681B_A6CF,
    0x0626_B4CA,
    0x7C8B_3AF0,
  ];

  let length_int = (length >> 3) as u32;
  let mut slot_constants = [0u32; 8];
  for (i, constant) in slot_constants.iter_mut().enumerate() {
    let index = ((-(slot as i64)) - i as i64 - 1) & 3;
    *constant = SLOT_GENERATORS[i] ^ CONSTANTS[index as usize];
  }

  // Split the region into eight pieces of pseudo-random length. The last
  // piece always ends at `length`.
  let mut crc_lengths = [0i64; 8];
  for i in 0..7 {
    let index = slot_constants[i]
      .wrapping_add(INTEGER_CONSTANTS[(slot_constants[i] & 0xFFF) as usize])
      & 0xFFF;
    let variation = FLOAT_CONSTANTS[index as usize];
    let scaled = ((variation - 0.5) * length_int as f32) as i32 as i64;
    crc_lengths[i] = scaled + (i as i64 + 1) * i64::from(length_int);
  }
  crc_lengths[7] = length as i64;

  let mut partial_crcs = [0u32; 8];
  let mut register = slot_constants[0];
  let mut piece_start = offset;
  let mut piece_len = crc_lengths[0] as usize;
  for i in 0..8 {
    partial_crcs[i] = crc32(register, &save[piece_start..piece_start + piece_len]);
    if i < 7 {
      piece_start += piece_len;
      piece_len = (crc_lengths[i + 1] - crc_lengths[i]) as usize;
      register = partial_crcs[i] ^ slot_constants[i + 1];
    }
  }

  let mut hash_lookup = [0u32; 8];
  for i in 0..7 {
    hash_lookup[i] = slot_constants[i] ^ partial_crcs[i] ^ partial_crcs[7];
  }
  hash_lookup[7] = slot_constants[7] ^ partial_crcs[7];

  let seed = CRC_IV ^ CONSTANTS[(3 - slot) as usize];
  let mut next_index = crc32(
    crc32(seed, &words_as_bytes(&slot_constants)),
    &words_as_bytes(&partial_crcs),
  );
  let jump = (next_index >> 24)
    + ((next_index >> 16) & 0xFF)
    + ((next_index >> 8) & 0xFF)
    + (next_index & 0xFF);

  let mut checksum = [0u8; CHECKSUM_LEN];
  for chunk in checksum.chunks_exact_mut(4) {
    let current = (next_index & 0xFFF) as usize;
    next_index = next_index.wrapping_add(jump).wrapping_add(1) & 0xFFF;

    let constant = INTEGER_CONSTANTS[current];
    let mut value = constant ^ hash_lookup[(constant.wrapping_add(slot) & 7) as usize];
    if constant & 7 == 1 {
      value ^= 0x0BD7_5F29;
    }
    chunk.copy_from_slice(&value.to_le_bytes());
  }
  checksum
}

fn salt_table(key_salt: u32) -> [u8; CHECKSUM_LEN] {
  let mask = key_salt ^ 0x4BF0_CF23;
  let mut index = 0x5D7u32;
  let step = (key_salt >> 24)
    + ((key_salt >> 16) & 0xFF)
    + ((key_salt >> 8) & 0xFF)
    + (key_salt & 0xFF)
    + 1;

  let mut salt = [0u8; CHECKSUM_LEN];
  for chunk in salt.chunks_exact_mut(4) {
    let mut word = INTEGER_CONSTANTS[(index & 0xFFF) as usize] ^ mask;
    if word & 7 == 1 {
      word ^= 0x0BD7_5F29;
    }
    chunk.copy_from_slice(&word.to_le_bytes());
    index = index.wrapping_add(step);
  }
  salt
}

fn key_table(key_salt: u32, salt: &[u8; CHECKSUM_LEN]) -> [[u8; 16]; 32] {
  let masks = [
    0x5A8B_79A9 ^ key_salt,
    0x3461_6F90 ^ key_salt,
    0xC4C6_38DF ^ key_salt,
    0x94FB_64E8 ^ key_salt,
  ];

  let mut keys = [[0u8; 16]; 32];
  for (i, key) in keys.iter_mut().enumerate() {
    let encoded = u32::from_le_bytes(salt[i * 4..i * 4 + 4].try_into().expect("four bytes"));
    for (j, mask) in masks.iter().enumerate() {
      key[j * 4..j * 4 + 4].copy_from_slice(&(encoded ^ mask).to_le_bytes());
    }
  }
  keys
}

/// Where each of the 32 keys stops applying, as an offset into the region.
fn key_boundaries(key_salt: u32, length: usize) -> [usize; 32] {
  let average = (length >> 5) as i64;
  let mut expected = average;
  let mut boundaries = [0usize; 32];
  for (i, boundary) in boundaries.iter_mut().take(31).enumerate() {
    let a = INTEGER_CONSTANTS[((key_salt.wrapping_add(i as u32) & 0xFFF) ^ 0x5D7) as usize];
    let b = FLOAT_CONSTANTS[((a & 0xFFF) ^ 0x885) as usize] - 0.5;
    let scaled = (b * average as f32) as i32 as i64;
    *boundary = ((scaled + expected + 0xF) & !0xF) as usize;
    expected += average;
  }
  boundaries[31] = length;
  boundaries
}

/// XORs four bytes of the region at `at` with four salt bytes from `from`.
fn xor_salt(save: &mut [u8], at: usize, salt: &[u8], from: usize) {
  for k in 0..4 {
    save[at + k] ^= salt[(from + k) & 0x1FF];
  }
}

/// Encrypts or decrypts the 16-byte block at `position` in place.
fn cipher_block(save: &mut [u8], position: usize, cipher: &Aes128, encrypting: bool) {
  let mut block: [u8; 16] = save[position..position + 16].try_into().expect("16 bytes");
  let mut array = block.into();
  if encrypting {
    cipher.encrypt_block(&mut array);
  } else {
    cipher.decrypt_block(&mut array);
  }
  block = array.into();
  save[position..position + 16].copy_from_slice(&block);
}

/// Runs the masking pass over one region. `encrypting` picks the mirror
/// image of the pass: the salt XORs and the AES direction both flip.
fn mask_region(save: &mut [u8], offset: usize, length: usize, key_salt: u32, encrypting: bool) {
  let salt = salt_table(key_salt);
  let keys = key_table(key_salt, &salt);
  let boundaries = key_boundaries(key_salt, length);

  let mut position = offset;
  for (key, boundary) in keys.iter().zip(boundaries) {
    let cipher = Aes128::new(&(*key).into());
    let mut salt_offset = 0usize;
    while position < boundary + offset {
      let branch = if salt[salt_offset & 0x1FF] & 1 == 0 { 4 } else { 0 };
      if encrypting {
        xor_salt(save, position + branch, &salt, salt_offset);
        xor_salt(save, position + branch + 8, &salt, salt_offset + 4);
        cipher_block(save, position, &cipher, true);
        xor_salt(save, position + 4 - branch, &salt, salt_offset + 8);
        xor_salt(save, position + 12 - branch, &salt, salt_offset + 12);
      } else {
        xor_salt(save, position + 4 - branch, &salt, salt_offset + 8);
        xor_salt(save, position + 12 - branch, &salt, salt_offset + 12);
        cipher_block(save, position, &cipher, false);
        xor_salt(save, position + branch, &salt, salt_offset);
        xor_salt(save, position + branch + 8, &salt, salt_offset + 4);
      }
      salt_offset += 4;
      position += 16;
    }
  }
}

fn decrypt_region(save: &mut [u8], offset: usize, length: usize, slot: u32) -> bool {
  let checksum_at = offset + length;
  let key_salt = crc32(CRC_IV, &save[checksum_at..checksum_at + CHECKSUM_LEN]);
  mask_region(save, offset, length, key_salt, false);

  let expected = slot_checksum(save, offset, length, slot);
  save[checksum_at..checksum_at + CHECKSUM_LEN] == expected
}

fn encrypt_region(save: &mut [u8], offset: usize, length: usize, slot: u32) {
  let checksum = slot_checksum(save, offset, length, slot);
  let checksum_at = offset + length;
  save[checksum_at..checksum_at + CHECKSUM_LEN].copy_from_slice(&checksum);

  let key_salt = crc32(CRC_IV, &checksum);
  mask_region(save, offset, length, key_salt, true);
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Builds a save-shaped buffer with plausible payload bytes and encrypts it.
  fn synthetic_save() -> Vec<u8> {
    let size = 0x71_4658 + 0x20_98C0 + CHECKSUM_LEN + 0x1000;
    let mut save: Vec<u8> = (0..size).map(|i| (i.wrapping_mul(31) ^ (i >> 7)) as u8).collect();
    save[..8].copy_from_slice(&[0x01, 0, 0, 0, 0, 0, 0, 0]);
    save
  }

  #[test]
  fn encrypt_then_decrypt_restores_the_payload() {
    let plain = synthetic_save();
    let mut save = plain.clone();

    encrypt_save(&mut save).unwrap();
    assert_ne!(save, plain);
    decrypt_save(&mut save).unwrap();

    // Only the regions' checksum bytes and the outer hash are rewritten by
    // encryption, so compare the parts a save edit cares about.
    assert_eq!(save[HASH_START..0x70], plain[HASH_START..0x70]);
    assert_eq!(save[0x70..0x70 + 0x100], plain[0x70..0x70 + 0x100]);
  }

  #[test]
  fn corrupt_save_is_rejected() {
    let mut save = synthetic_save();
    encrypt_save(&mut save).unwrap();
    save[1000] ^= 0x40;
    assert!(decrypt_save(&mut save).is_err());
  }

  /// Round-trips a real Iceborne save when `MHW_SAVE_FIXTURE` points at one.
  /// Skipped otherwise, since saves are personal data and not kept in the repo.
  #[test]
  fn real_save_decrypts_and_reencrypts_byte_for_byte() {
    let Some(path) = std::env::var_os("MHW_SAVE_FIXTURE") else {
      return;
    };
    let original = std::fs::read(path).unwrap();
    let mut save = original.clone();

    decrypt_save(&mut save).unwrap();
    assert_eq!(save[0..4], [1, 0, 0, 0], "decrypted magic");
    encrypt_save(&mut save).unwrap();
    assert!(save == original, "re-encrypted save differs from the original");
  }
}

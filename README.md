# MH Save Editor: Monster Hunter Rise & World Save Editor

A free, portable save editor for the Steam versions of **Monster Hunter Rise: Sunbreak** and **Monster Hunter: World (Iceborne)**. Open one of your character saves, change Zenny, research points, hunter and master ranks, the Item Box, loadouts and more, then write it back. No hex editing, no digging through folders: it finds your Rise saves on its own, works out your SteamID64 from the save path, and has an English and Chinese interface.

Keywords: Monster Hunter Rise save editor, Monster Hunter World save editor, MH World Iceborne save editor, Monster Hunter Sunbreak save editor, MHR save editor, Zenny editor, Steam save editor, Monster Hunter save editing tool.

## Where this comes from

This project is a fork of [ChevalGrand/mhrise-save-editor](https://github.com/ChevalGrand/mhrise-save-editor), and all the hard work of cracking the save format is theirs. Huge thanks to ChevalGrand. On top of that base I translated the whole UI and item names to English, added a game selector with logos, light and dark themes, bulk Item Box editing (with a `+5` / `-5` quantity calculator) and the loadout tools. The container and crypto code goes further back to [jinghaihan/mhrise-save-converter](https://github.com/jinghaihan/mhrise-save-converter), whose research builds on [kvasszn/ree-save-editor](https://github.com/kvasszn/ree-save-editor).

## Download

Grab the latest zip from the [Releases page](https://github.com/S0NES/mh-save-editor-englis/releases), unzip it anywhere and run `mhrise-save-editor-gui.exe`. It is a single portable program, nothing to install.

Monster Hunter Rise and Monster Hunter: World (Iceborne, Steam) can be edited. Wilds shows up in the selector but its saves are not supported yet. World support covers the three character slots: name, zenny, research points, and HR / MR rank and experience. The logos are Capcom trademarks, used only to label the games.

> [!WARNING]
> Back up your saves before editing (the Backup tab does it in one click, the CLI has a `backup` command) and keep the game closed. Every field mapping was checked against real saves (Zenny, points, Item Box, equipment, ranks, loadouts), but extreme edits can still go wrong. Try things on a throwaway slot first.

## How it works

### Save container: DSSS v2 + Citrus encryption
- The file starts with a `DSSS` v2 container; Steam saves carry the CITRUS flag (0x04).
- Citrus: **the SteamID64 derives an ECC key, which decrypts the data blocks with AES-128-CBC**. Each block has a SHA3-256 hash, blocks are 0x40000 bytes, and there is a trailing 0x1000 block (may be zero).
- The curve index is found by brute force over 128 preset curves, checking the decrypted block hash, so **you only need the SteamID64** (17 digits = 76561197960265728 + the 32-bit account ID; the account ID is the `userdata\<account ID>\` folder name, and the GUI works it out for you).
- The outer checksum is MurmurHash3 (seed 0xffffffff) with a declared-length prefix.
- The container and crypto code comes from the MIT-licensed [jinghaihan/mhrise-save-converter](https://github.com/jinghaihan/mhrise-save-converter), via ChevalGrand's editor.

### Class stream: RSZ structures and field hashes
The decrypted data is an RE Engine RSZ class stream (`native_hash` + `class{hash, fields[]}`). **Key finding of this project: field/class hash = murmur3_32(field name, seed 0xffffffff)**, which matches the community RSZ dictionary (`assets/mhrise/rszmhrise.json`, 250,000 entries) with 99.98% coverage. Every edit locates its target by class hash AND field hash together, so a same-named field elsewhere is never touched.

Main verified structures (class → key fields):

| Class | Hash | Contents |
|---|---|---|
| `snow.data.HandMoney` | `079713ac` | `_Value` current Zenny, `_TotalAddedValue` lifetime total |
| `snow.data.VillagePoint` | `306735ee` | `_Point` village points |
| `snow.data.ItemBox` | `67696161` | `_InventoryList`: 1800 × (ItemId, Num); empty slot = `0x04000000` |
| `snow.data.EquipBox` | `f0509899` | `_WeaponArmorInventoryList`: 5000 × EquipmentInventoryData (weapons / armor / talismans) |
| `snow.data.EquipDataManager.SaveData1` | `960baed3` | `_PlEquipPack` worn gear, `_PlEquipMySetList` (loadouts × 224), `_HunterMySetList` (hunter sets × 224), `_PlArmorColorMySetData` |
| `snow.equip.PlEquipMySetData` | `d897238a` | A loadout: `_Name`, `_IsUsing`, `_InventoryIndexList` (Equipment Box index table) |
| `snow.data.HunterMySetData` | `af140835` | A hunter set: `_Name` + `_ItemMysetIndex` / `_EquipMysetIndex` / `_OverwearMysetIndex` |
| `snow.data.ItemMySet` | `8923cd68` | Item pouch loadouts × 40 |
| `snow.progress.ProgressSaveData` | `948b7b9d` | `_HunterRank` / `_MasterRank` / `_MysteryResearchLevel` + their points (the authoritative rank data) |
| `snow.data.GuildCardData` | `44541321` | Own guild card: hunter name, display copies of ranks (HunterRank / `_MasterRank` / LaboLv), unlock booleans (isMRRelease etc.). Buddy cards live in an array and are never touched. |

Item names: `assets/mhrise/item_names_cn.json` (1771 entries) comes from the community 16.0.0 item table and is aligned by numeric ID with the game's internal enum (`item_names.json`). `assets/mhrise/item_names_en.json` is a best-effort English translation of the Chinese names; a few monster-part names may differ from the official English text.

## Features

### GUI (`mhrise-save-editor-gui`)
- **Game selector with logos:** clickable logo cards for Rise, World and Wilds sit at the top (the selected card gets an accent outline). The logos are Capcom trademarks, used only to label the games.
- **File selector:** choose a save from the auto-detected list or browse for a file. Only Monster Hunter Rise is editable today; World and Wilds are listed as not supported yet.
- **Dark and light themes** (toggle in the top right).
- **Language selector** (top right): switches the interface and item names between English (default) and Chinese (中文). Item names also show the other language on hover, and search matches both.
- **Auto-detect saves:** reads the registry and the Steam library config to find every `userdata\<account>\1446780\remote\win64_save`; Quick select fills in the SteamID64.
- **General:** hunter name, ranks (HR / MR / Anomaly Research), Zenny and points, container info.
- **Player:** edit Zenny, lifetime Zenny, points and the three ranks (applied in memory, written on save).
- **Item Box:** all 1800 slots with English names (the Chinese name shows on hover and is searchable), category filter, search by name / category / ID, and per-row quantity edits. Bulk tools:
  - tick rows with the checkboxes (shift-click selects a range, "Select all shown" ticks every item in the current filter);
  - **Quantity calculator** on the selection: `5` sets the quantity, `+5` adds (max 9999), `-5` subtracts (stops at 0, which empties the slot). The per-row "New qty" boxes take the same syntax;
  - **Delete selected**;
  - **Move selection to slot N**, which keeps the selection's order and swaps the slots it lands on into the places it left, so nothing is lost;
  - **Sort the whole box** by name, category, ID or quantity; empty slots move to the end.
- **Transfer** (between slots of the same Steam account; applied in memory and written together):
  - Item Box merged by category (consumables / materials / ammo and bottles / sell items and antiques / other); unticked categories stay as in the target;
  - equipment transfer by component (Equipment Box / worn gear and loadout registry / item pouch loadouts);
  - rank progress transfer (three ranks + points).
- **Loadouts:** shows the 224 loadout slots (name / in use) and transfers loadouts from a source save (to a chosen slot or the first free one, optionally moving the Equipment Box pieces they reference, overwriting the same slots), or single pieces by Equipment Box slot number.
- **Progress:** edit rank points and the guild card unlock flags (display copies), with an explanation of the unlock clamp.
- **Backup:** one-click backup of the whole save folder to `Documents\MHR-Save-Backups\<account>\<timestamp>` (verified byte by byte, outside Steam Cloud sync scope); restore needs a second confirming click and reloads the save.
- **Write to save (backs up first):** backs up the whole folder before writing, so you do not have to swap `.edited.bin` files by hand. "Save as new file" is still there for manual swapping.
- All I/O runs on a background thread so the UI never freezes, and every write has a backup behind it.

### CLI (`mhrise-save-editor`)
`inspect` (container info + ranks), `dump-json` / `apply-json` (lossless JSON export / import), `repack` / `roundtrip` (round-trip check), `backup` / `restore`, `diff` (structural diff of two saves), `set-money` / `set-points`, `transfer-items` / `transfer-equipment`.

## Known limits
- **Rank unlock clamp:** on a save that has not finished the unlock quest chain, the game pushes ranks back to the current cap after loading (and shows an achievement popup). Unlock state lives in a quest-clear bitmap (a game-specific packed structure; mapping quests to bits needs the game data tables), which this tool does not rewrite yet. Unlock in game, or use Transfer to bring progress over.
- **Quest flags:** same as above; the bitmap mapping is not reverse-engineered, so they are not edited automatically.
- **Chinese input:** the winit window library has a known compatibility problem with TSF-mode input methods. Microsoft Pinyin works with "Use previous version of Microsoft IME" turned on (see the Help tab).
- Weapon and armor names are not stored in the save and need the game data tables, so equipment is shown by ID.
- The English item names are a best-effort translation, not the game's own text.

## Development
```bash
cargo test          # unit and integration tests
cargo run --release --bin mhrise-save-editor-gui
scripts/package.py  # bundle the release exes + README + LICENSE into dist/
```

Code layout: `src/container.rs` (DSSS/Citrus container), `src/crypto/` (AES/ECC/SHA3), `src/payload.rs` (RSZ class stream parsing), `src/edit.rs` (field lookup and edits, all verified hash constants), `src/items.rs` (item name tables), `src/itembox.rs` (bulk Item Box operations), `src/game.rs` (game profiles), `src/theme.rs` (dark and light themes), `src/archive.rs` (backups), `src/discover.rs` (save discovery), `src/gui.rs` (eframe UI), `src/main.rs` (CLI). Rust 2024 edition, `unsafe_code = forbid`.

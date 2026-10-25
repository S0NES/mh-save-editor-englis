//! eframe/egui front end for the save editor.
//!
//! Model: open a save once (background thread — decryption takes seconds),
//! edit values in memory (instant), then write the result to a NEW file
//! (`Save as new file`). The selected save itself is never modified; the user
//! swaps the output file in with the game closed.

use std::{
  collections::BTreeSet,
  fs,
  path::{Path, PathBuf},
  sync::mpsc::{self, Receiver, TryRecvError},
  sync::OnceLock,
  thread,
  time::Duration,
};

use eframe::egui;
use egui::Grid;

use crate::{
  container::SteamSave,
  edit,
  format::parse_header,
  game::Game,
  i18n::{fill, tr},
  mhw::save::{Hunter, MhwSave, SLOT_COUNT},
  itembox::{self, QuantityOp, SortKey},
  language::Language,
  theme::Theme,
};

const TAB_INFO: usize = 0;
const TAB_PLAYER: usize = 1;
const TAB_ITEMBOX: usize = 2;
const TAB_EQUIP: usize = 3;
const TAB_TRANSFER: usize = 4;
const TAB_PROGRESS: usize = 5;
const TAB_BACKUP: usize = 6;
const TAB_HELP: usize = 7;

/// Loads a single-face CJK .ttf font. egui's text rasterizer (ab_glyph) does
/// NOT support .ttc collections, which is why msyh.ttc/simsun.ttc cannot be
/// used directly. Returns a status line for the on-screen log.
fn install_cjk_font(ctx: &egui::Context) -> String {
  for (path, label) in [
    ("C:/Windows/Fonts/msyh.ttc", "Microsoft YaHei (msyh.ttc)"),
    ("C:/Windows/Fonts/msjh.ttc", "Microsoft JhengHei (msjh.ttc)"),
    ("C:/Windows/Fonts/simsun.ttc", "SimSun (simsun.ttc)"),
    ("C:/Windows/Fonts/Deng.ttf", "DengXian (Deng.ttf)"),
    ("C:/Windows/Fonts/simhei.ttf", "SimHei (simhei.ttf)"),
    ("C:/Windows/Fonts/Dengb.ttf", "DengXian Bold (Dengb.ttf)"),
    ("C:/Windows/Fonts/STXIHEI.TTF", "STXihei (STXIHEI.TTF)"),
  ] {
    let Ok(bytes) = fs::read(path) else {
      continue;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("cjk".to_owned(), egui::FontData::from_owned(bytes).into());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
      fonts.families.entry(family).or_default().insert(0, "cjk".to_owned());
    }
    ctx.set_fonts(fonts);
    return format!("Font: loaded {label}");
  }
  "Font: no system CJK font found; Chinese text will show as boxes".to_owned()
}

// ---------------------------------------------------------------------------
// Opened document
// ---------------------------------------------------------------------------

struct OpenedSave {
  path: PathBuf,
  document: SteamSave,
  hunter_name: String,
  box_items: Vec<(u32, u32)>,
}

/// A Monster Hunter: World save open in the World panel. Edits change the
/// decrypted copy in memory; "Write to save" backs up the folder first.
struct MhwOpened {
  path: PathBuf,
  save: MhwSave,
  slots: Vec<MhwSlotInputs>,
  dirty: bool,
}

/// Text shown for one World character slot, so values being typed are kept
/// until "Apply" checks them.
#[derive(Clone, Default)]
struct MhwSlotInputs {
  name: String,
  hunter_rank: String,
  hunter_rank_xp: String,
  master_rank: String,
  master_rank_xp: String,
  zeni: String,
  research_points: String,
}

impl MhwSlotInputs {
  fn from_hunter(hunter: &Hunter) -> Self {
    Self {
      name: hunter.name.clone(),
      hunter_rank: hunter.hunter_rank.to_string(),
      hunter_rank_xp: hunter.hunter_rank_xp.to_string(),
      master_rank: hunter.master_rank.to_string(),
      master_rank_xp: hunter.master_rank_xp.to_string(),
      zeni: hunter.zeni.to_string(),
      research_points: hunter.research_points.to_string(),
    }
  }
}

fn apply_mhw_inputs(save: &mut MhwSave, slot: usize, inputs: &MhwSlotInputs) -> anyhow::Result<()> {
  let number = |text: &str, label: &str| -> anyhow::Result<u32> {
    text.trim().parse::<u32>().map_err(|_| anyhow::anyhow!("{label} must be a whole number"))
  };
  let zeni = number(&inputs.zeni, "Zenny")?;
  let research_points = number(&inputs.research_points, "Research points")?;
  let hunter_rank = number(&inputs.hunter_rank, "HR")?;
  let hunter_rank_xp = number(&inputs.hunter_rank_xp, "HR experience")?;
  let master_rank = number(&inputs.master_rank, "MR")?;
  let master_rank_xp = number(&inputs.master_rank_xp, "MR experience")?;

  save.set_name(slot, inputs.name.trim())?;
  save.set_zeni(slot, zeni)?;
  save.set_research_points(slot, research_points)?;
  save.set_hunter_rank_xp(slot, hunter_rank_xp, hunter_rank)?;
  save.set_master_rank_xp(slot, master_rank_xp, master_rank)
}

/// Backs up the save folder, then writes the re-encrypted save over the file.
/// Returns the backup folder.
fn write_mhw_in_place(path: &Path, save: &MhwSave) -> anyhow::Result<PathBuf> {
  let dir = path.parent().ok_or_else(|| anyhow::anyhow!("the save path has no folder"))?;
  let backup = crate::archive::backup_dir(dir, None)?.destination;
  std::fs::write(path, save.to_bytes()?)?;
  Ok(backup)
}

fn mhw_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
  ui.label(label);
  ui.add(egui::TextEdit::singleline(value).desired_width(200.0));
  ui.end_row();
}

impl GuiApp {
  /// Opens the World save at the target path. Unsaved World edits need a
  /// second click, as Rise saves do.
  fn request_mhw_open(&mut self) {
    let unsaved = self.mhw.as_ref().is_some_and(|opened| opened.dirty);
    if unsaved && !self.reopen_clicked_once {
      self.reopen_clicked_once = true;
      self.log_line(tr(self.language, "! Unsaved changes: click Open save again to discard them and reopen"));
      return;
    }
    self.reopen_clicked_once = false;

    let path = PathBuf::from(self.target_path.trim());
    match MhwSave::open_path(&path) {
      Ok(save) => {
        let slots = (0..SLOT_COUNT)
          .filter_map(|slot| save.hunter(slot).ok())
          .map(|hunter| MhwSlotInputs::from_hunter(&hunter))
          .collect();
        self.status_error = None;
        self.log_line(&fill(tr(self.language, "✔ Opened {}"), &[&path.display()]));
        self.mhw = Some(MhwOpened { path, save, slots, dirty: false });
      }
      Err(error) => {
        let text = format!("{error:?}");
        self.log_line(&fill(tr(self.language, "✘ Open failed: {}"), &[&text]));
        self.status_error = Some(text);
      }
    }
  }

  /// Checks one slot's text and applies it to the in-memory save.
  fn apply_mhw_slot(&mut self, slot: usize) {
    let language = self.language;
    let Some(opened) = self.mhw.as_mut() else {
      return;
    };
    let inputs = opened.slots[slot].clone();
    let result = apply_mhw_inputs(&mut opened.save, slot, &inputs);
    if result.is_ok() {
      opened.dirty = true;
      if let Ok(hunter) = opened.save.hunter(slot) {
        opened.slots[slot] = MhwSlotInputs::from_hunter(&hunter);
      }
    }
    match result {
      Ok(()) => self.log_line(&fill(
        tr(language, "✔ Applied to character slot {} (not written yet)"),
        &[&(slot + 1)],
      )),
      Err(error) => self.log_line(&fill(tr(language, "✘ Apply failed: {}"), &[&format!("{error:?}")])),
    }
  }

  fn write_mhw_save(&mut self) {
    let language = self.language;
    let Some(opened) = self.mhw.as_mut() else {
      return;
    };
    let outcome = write_mhw_in_place(&opened.path, &opened.save);
    let path_text = opened.path.display().to_string();
    if outcome.is_ok() {
      opened.dirty = false;
    }
    match outcome {
      Ok(backup) => self.log_line(&fill(
        tr(language, "✔ Written: {} (backup first: {})"),
        &[&path_text, &backup.display()],
      )),
      Err(error) => self.log_line(&fill(tr(language, "✘ Write failed: {}"), &[&format!("{error:?}")])),
    }
  }

  fn show_mhw_panel(&mut self, ui: &mut egui::Ui) {
    let language = self.language;
    let Some(opened) = self.mhw.as_mut() else {
      ui.label(tr(
        language,
        "Browse to a SAVEDATA1000 file (Steam: userdata\\<account>\\582010\\remote) and click Open save.",
      ));
      return;
    };
    ui.label(fill(tr(language, "File: {}"), &[&opened.path.display()]));
    ui.label(fill(tr(language, "SteamID64 in save header: {}"), &[&opened.save.steam_id()]));
    if opened.dirty {
      ui.label(tr(language, "● Unsaved changes (in memory)"));
    }

    let mut apply = None;
    for (index, inputs) in opened.slots.iter_mut().enumerate() {
      ui.separator();
      ui.strong(fill(tr(language, "Character slot {}"), &[&(index + 1)]));
      Grid::new(("mhw-slot", index)).num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        mhw_field(ui, tr(language, "Hunter name"), &mut inputs.name);
        mhw_field(ui, tr(language, "Hunter rank (HR)"), &mut inputs.hunter_rank);
        mhw_field(ui, tr(language, "HR experience"), &mut inputs.hunter_rank_xp);
        mhw_field(ui, tr(language, "Master rank (MR)"), &mut inputs.master_rank);
        mhw_field(ui, tr(language, "MR experience"), &mut inputs.master_rank_xp);
        mhw_field(ui, tr(language, "Zenny"), &mut inputs.zeni);
        mhw_field(ui, tr(language, "Research points"), &mut inputs.research_points);
      });
      if ui.button(fill(tr(language, "Apply slot {}"), &[&(index + 1)])).clicked() {
        apply = Some(index);
      }
    }
    if let Some(slot) = apply {
      self.apply_mhw_slot(slot);
    }
  }

  fn show_mhw_save_bar(&mut self, ui: &mut egui::Ui) {
    let ready = !self.busy && self.mhw.is_some();
    ui.horizontal(|ui| {
      if ui.add_enabled(ready, egui::Button::new(tr(self.language, "Write to save (backs up first)"))).clicked() {
        self.write_mhw_save();
      }
      ui.label(tr(
        self.language,
        "Close the game before writing. The save folder is backed up next to it first.",
      ));
    });
  }
}

impl OpenedSave {
  fn open(path: PathBuf, steamid64: u64) -> anyhow::Result<Self> {
    let document = SteamSave::open_path(&path, steamid64)?;
    let hunter_name = edit::read_hunter_name(document.payload()).unwrap_or_default();
    let box_items = edit::read_item_box(document.payload())?
      .into_iter()
      .map(|entry| (entry.id, entry.num))
      .collect();
    Ok(Self { path, document, hunter_name, box_items })
  }

  fn summary(&self, language: Language) -> String {
    let hunter = if self.hunter_name.is_empty() { tr(language, "(unknown)") } else { &self.hunter_name };
    fill(tr(language, "{} · Curve {}"), &[&hunter, &self.document.curve_index()])
  }
}

enum WorkerEvent {
  Opened(Result<Box<OpenedSave>, String>, bool),
  /// Wrote the document; the second element is the automatic backup location
  /// for in-place saves.
  Saved(Result<(PathBuf, Option<PathBuf>), String>),
  /// A background operation that only produces a status line (backup/restore).
  Report(Result<String, String>),
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct GuiApp {
  target_path: String,
  source_path: String,
  steamid64: String,
  discovered: Vec<(PathBuf, String, Option<u32>)>,
  target: Option<Box<OpenedSave>>,
  source: Option<Box<OpenedSave>>,
  dirty: bool,
  reopen_clicked_once: bool,
  /// The Monster Hunter: World save open in the World panel, if any.
  mhw: Option<MhwOpened>,
  status_error: Option<String>,
  money_input: String,
  points_input: String,
  total_input: String,
  hunter_rank_input: String,
  master_rank_input: String,
  mystery_rank_input: String,
  box_filter: String,
  /// Index into [`ITEM_CATEGORY_GROUPS`] + 1; 0 = all categories.
  box_category: usize,
  box_show_empty: bool,
  /// Slot indices (0-based) ticked for bulk actions.
  box_selected: BTreeSet<usize>,
  /// Row position (in the visible list) of the last checkbox click, for
  /// shift-click range selection.
  box_last_clicked: Option<usize>,
  box_bulk_qty: String,
  box_move_slot: String,
  /// Index into [`SortKey::ALL`].
  box_sort: usize,
  box_edits: Vec<String>,
  game: Game,
  /// Game logo textures, uploaded on the first frame.
  game_logos: Option<[egui::TextureHandle; 3]>,
  theme: Theme,
  /// Language for item names in the Item Box and loadout views.
  language: Language,
  busy: bool,
  busy_text: String,
  log: String,
  receiver: Option<Receiver<WorkerEvent>>,
  tab: usize,
  /// Item-category toggles for the transfer filter, parallel to
  /// [`ITEM_CATEGORY_GROUPS`].
  xfer_cats: Vec<bool>,
  xfer_equip_box: bool,
  xfer_equip_manager: bool,
  xfer_item_my_set: bool,
  /// Backup awaiting a second confirming click on Restore.
  restore_confirm: Option<PathBuf>,
  /// Point-accumulator inputs on the Progress tab (HR/MR/anomaly research).
  hr_point_input: String,
  mr_point_input: String,
  mystery_point_input: String,
  /// Unlock-flag toggles on the Progress tab, parallel to
  /// [`edit::GUILD_FLAGS`].
  guild_flags: Vec<bool>,
  /// Transfer toggle: copy ProgressSaveData (ranks + points).
  xfer_progress: bool,
  /// Loadouts tab state: selected source register, target slot text
  /// (empty = append to the first free slot), and the single-piece inputs.
  equip_source_combo: usize,
  equip_target_slot_text: String,
  equip_with_pieces: bool,
  equip_piece_source_text: String,
  equip_piece_target_text: String,
}

/// (label, item-table categories) groups for the item-box transfer filter.
/// Ids missing from the name table follow the last ("Tools / Other") toggle.
const ITEM_CATEGORY_GROUPS: &[(&str, &[&str])] = &[
  ("Consumables", &["Consume"]),
  ("Materials", &["Material", "OffcutsMaterial"]),
  ("Ammo / Bottles", &["Bullet", "Bottle"]),
  ("Sell / Antiques", &["PayOff", "CarryPayOff", "Antique"]),
  ("Tools / Other", &["Tool"]),
];

/// Runs `add` inside a fixed-width, fixed-height cell so table columns line
/// up no matter how wide the content is.
fn cell<R>(
  ui: &mut egui::Ui,
  width: f32,
  layout: egui::Layout,
  add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
  let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 18.0), egui::Sense::hover());
  let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
  add(&mut child)
}

/// Which [`ITEM_CATEGORY_GROUPS`] entry an item id belongs to; ids missing
/// from the name table fall into the last group.
fn category_group(id: u32) -> usize {
  let last = ITEM_CATEGORY_GROUPS.len() - 1;
  crate::items::item_category(id)
    .and_then(|category| {
      ITEM_CATEGORY_GROUPS.iter().position(|(_, categories)| categories.contains(&category))
    })
    .unwrap_or(last)
}

impl GuiApp {
  pub fn new(creation_context: &eframe::CreationContext) -> Self {
    let font_status = install_cjk_font(&creation_context.egui_ctx);
    creation_context.egui_ctx.set_visuals(Theme::default().visuals());
    let mut app = Self {
      xfer_cats: vec![true; ITEM_CATEGORY_GROUPS.len()],
      xfer_equip_box: true,
      xfer_equip_manager: true,
      xfer_item_my_set: true,
      guild_flags: vec![false; edit::GUILD_FLAGS.len()],
      ..Self::default()
    };
    app.log_line(&font_status);
    app.discovered = discover_steam_slot_files();
    if let Some((_, _, Some(account))) = app.discovered.first() {
      app.steamid64 = steamid64_from_account(*account).to_string();
    }
    if app.discovered.is_empty() {
      app.log_line("Auto-detect: no MH Rise save folder found on this PC. Browse for a save file (see the Help tab).");
    } else {
      app.log_line(&format!(
        "Auto-detect: found {} character saves; pick one in Quick select",
        app.discovered.len()
      ));
    }
    app
  }

  /// Re-reads ranks/points/flag inputs from the opened document.
  fn refresh_progress_inputs(&mut self) {
    let Some(opened) = self.target.as_ref() else {
      return;
    };
    let payload = opened.document.payload();
    if let Some(ranks) = edit::read_ranks(payload) {
      self.hunter_rank_input = ranks.hunter.to_string();
      self.master_rank_input = ranks.master.to_string();
      self.mystery_rank_input = ranks.mystery_research.to_string();
    }
    let points = |hash: u32| -> String {
      edit::read_scalar(payload, edit::PROGRESS_SAVE_CLASS, hash)
        .map(|value| value.to_string())
        .unwrap_or_default()
    };
    self.hr_point_input = points(edit::HUNTER_RANK_POINT);
    self.mr_point_input = points(edit::MASTER_RANK_POINT);
    self.mystery_point_input = points(edit::MYSTERY_RESEARCH_POINT);
    if let Some(flags) = edit::read_guild_flags(payload) {
      self.guild_flags = flags.into_iter().map(|(_, value)| value).collect();
    }
  }

  fn log_line(&mut self, text: &str) {
    self.log.push_str(text);
    self.log.push('\n');
  }

  fn parsed_steamid64(&self) -> Result<u64, &'static str> {
    self.steamid64.trim().parse::<u64>().map_err(|_| tr(self.language, "SteamID64 must be digits only"))
  }

  fn spawn(&mut self, context: &egui::Context, task: WorkerTask) {
    let (sender, receiver) = mpsc::channel();
    self.receiver = Some(receiver);
    self.busy = true;
    let language = self.language;
    self.busy_text = task.describe(language);
    self.log_line(&format!("▶ {}", task.describe(language)));
    let context = context.clone();
    thread::spawn(move || {
      let event = task.run(language);
      let _ = sender.send(event);
      context.request_repaint();
    });
  }

  /// Applies the player-data edits (money / points / lifetime counter) that
  /// differ from the opened document. Runs on the UI thread: pure tree walks.
  fn apply_player_edits(&mut self) -> Result<(), String> {
    let Some(opened) = self.target.as_mut() else {
      return Err(tr(self.language, "No target save is open").to_owned());
    };
    let (current_money, current_total) = edit::read_money(opened.document.payload())
      .ok_or_else(|| tr(self.language, "Money field not found (schema mismatch)").to_owned())?;
    let current_points = edit::read_points(opened.document.payload())
      .ok_or_else(|| tr(self.language, "Points field not found (schema mismatch)").to_owned())?;

    let mut applied = 0usize;
    if let Ok(value) = self.money_input.trim().parse::<u32>()
      && value != current_money
    {
      edit::set_scalar(
        opened.document.payload_mut(),
        edit::HAND_MONEY_CLASS,
        edit::HAND_MONEY_VALUE,
        value,
        "_Value",
      )
      .map_err(|error| error.to_string())?;
      applied += 1;
    }
    if let Ok(value) = self.total_input.trim().parse::<u32>()
      && value != current_total
    {
      edit::set_scalar(
        opened.document.payload_mut(),
        edit::HAND_MONEY_CLASS,
        edit::HAND_MONEY_TOTAL_ADDED,
        value,
        "_TotalAddedValue",
      )
      .map_err(|error| error.to_string())?;
      applied += 1;
    }
    if let Ok(value) = self.points_input.trim().parse::<u32>()
      && value != current_points
    {
      edit::set_scalar(
        opened.document.payload_mut(),
        edit::VILLAGE_POINT_CLASS,
        edit::VILLAGE_POINT_VALUE,
        value,
        "_Point",
      )
      .map_err(|error| error.to_string())?;
      applied += 1;
    }

    // Ranks: only apply when the input parses and differs; the ranges are the
    // game's caps (HR/MR 999, anomaly research 300).
    let rank_parse = |text: &str| -> Result<Option<u32>, String> {
      let trimmed = text.trim();
      if trimmed.is_empty() {
        return Ok(None);
      }
      let value = trimmed
        .parse::<u32>()
        .map_err(|_| fill(tr(self.language, "Rank must be a number: {}"), &[&format!("{text:?}")]))?;
      Ok(Some(value))
    };
    let hunter_rank = rank_parse(&self.hunter_rank_input)?;
    let master_rank = rank_parse(&self.master_rank_input)?;
    let mystery_rank = rank_parse(&self.mystery_rank_input)?;
    for (name, value, cap) in [
      ("Hunter Rank", hunter_rank, 999),
      ("Master Rank", master_rank, 999),
      ("Anomaly Research Rank", mystery_rank, 300),
    ] {
      if let Some(value) = value
        && (value == 0 || value > cap)
      {
        return Err(fill(
          tr(self.language, "{} out of range (1-{}): {}"),
          &[&tr(self.language, name), &cap, &value],
        ));
      }
    }
    if hunter_rank.is_some() || master_rank.is_some() || mystery_rank.is_some() {
      let current = edit::read_ranks(opened.document.payload())
        .ok_or_else(|| tr(self.language, "Rank field not found (schema mismatch)").to_owned())?;
      let changed = |input: Option<u32>, current_value: u32| input.is_some_and(|v| v != current_value);
      if changed(hunter_rank, current.hunter)
        || changed(master_rank, current.master)
        || changed(mystery_rank, current.mystery_research)
      {
        let hunter = hunter_rank.unwrap_or(current.hunter);
        let master = master_rank.unwrap_or(current.master);
        let mystery = mystery_rank.unwrap_or(current.mystery_research);
        edit::set_ranks(opened.document.payload_mut(), hunter, master, mystery)
          .map_err(|error| error.to_string())?;
        applied += 1;
      }
    }

    if applied == 0 {
      return Err(tr(self.language, "Nothing to apply (values unchanged or empty)").to_owned());
    }
    // Refresh the input boxes from the document so they show the new values.
    let (money, total) = edit::read_money(opened.document.payload())
      .ok_or_else(|| tr(self.language, "Money field not found (schema mismatch)").to_owned())?;
    let points = edit::read_points(opened.document.payload())
      .ok_or_else(|| tr(self.language, "Points field not found (schema mismatch)").to_owned())?;
    self.money_input = money.to_string();
    self.total_input = total.to_string();
    self.points_input = points.to_string();
    if let Some(ranks) = edit::read_ranks(opened.document.payload()) {
      self.hunter_rank_input = ranks.hunter.to_string();
      self.master_rank_input = ranks.master.to_string();
      self.mystery_rank_input = ranks.mystery_research.to_string();
    }
    self.dirty = true;
    Ok(())
  }

  fn pending_item_changes(&self) -> Vec<(u32, u32)> {
    let Some(opened) = self.target.as_ref() else {
      return Vec::new();
    };
    let mut changes = Vec::new();
    if opened.box_items.is_empty() || opened.box_items.len() != self.box_edits.len() {
      return changes;
    }
    for (index, (id, num)) in opened.box_items.iter().enumerate() {
      let text = self.box_edits[index].trim();
      if text.is_empty() {
        continue;
      }
      if let Ok(op) = QuantityOp::parse(text) {
        let new_value = op.apply(*num);
        if new_value != *num {
          changes.push((*id, new_value));
        }
      }
    }
    changes
  }

  fn apply_box_edits(&mut self) -> Result<String, String> {
    let changes = self.pending_item_changes();
    if changes.is_empty() {
      return Err(tr(self.language, "Nothing to apply (quantities unchanged or empty)").to_owned());
    }
    let Some(opened) = self.target.as_mut() else {
      return Err(tr(self.language, "No target save is open").to_owned());
    };
    let report = edit::set_item_counts(opened.document.payload_mut(), &changes)
      .map_err(|error| error.to_string())?;
    opened.box_items = edit::read_item_box(opened.document.payload())
      .map_err(|error| error.to_string())?
      .into_iter()
      .map(|entry| (entry.id, entry.num))
      .collect();
    self.box_edits = vec![String::new(); opened.box_items.len()];
    self.dirty = true;
    Ok(fill(
      tr(self.language, "{} updated, {} new items placed in empty slots"),
      &[&report.applied, &report.filled],
    ))
  }

  /// Merge-transfer only the source item slots whose category group is
  /// enabled; other slots keep the target's contents.
  fn transfer_items_filtered(&mut self) -> Result<String, String> {
    let Some(source) = self.source.as_ref() else {
      return Err(tr(self.language, "No source save is open").to_owned());
    };
    let groups: Vec<&[&str]> = ITEM_CATEGORY_GROUPS
      .iter()
      .zip(&self.xfer_cats)
      .filter(|(_, on)| **on)
      .map(|((_, cats), _)| *cats)
      .collect();
    let other_allowed = *self.xfer_cats.last().unwrap_or(&false);
    let keep = move |id: u32| match crate::items::item_category(id) {
      Some(category) => groups.iter().any(|group| group.contains(&category)),
      // Ids missing from the name table follow the "Tools / Other" toggle.
      None => other_allowed,
    };
    let Some(target) = self.target.as_mut() else {
      return Err(tr(self.language, "No target save is open").to_owned());
    };
    let report = edit::transfer_item_box_filtered(target.document.payload_mut(), source.document.payload(), keep)
      .map_err(|error| error.to_string())?;
    self.refresh_target_items();
    Ok(fill(
      tr(self.language, "Item Box merged: of {} slots, {} items from source, {} kept from target (source has {} items)"),
      &[&report.slots, &report.transferred, &report.kept, &report.source_items],
    ))
  }

  /// Transfer only the ticked equipment components.
  fn transfer_equipment_selected(&mut self) -> Result<String, String> {
    let Some(source) = self.source.as_ref() else {
      return Err(tr(self.language, "No source save is open").to_owned());
    };
    let Some(target) = self.target.as_mut() else {
      return Err(tr(self.language, "No target save is open").to_owned());
    };
    let parts = edit::EquipParts {
      equip_box: self.xfer_equip_box,
      equip_manager: self.xfer_equip_manager,
      item_my_set: self.xfer_item_my_set,
    };
    let report =
      edit::transfer_equipment_parts(target.document.payload_mut(), source.document.payload(), parts)
        .map_err(|error| error.to_string())?;
    self.refresh_target_items();
    let partial = parts != edit::EquipParts::all();
    let classes = report
      .classes
      .iter()
      .map(|(label, fields)| fill(tr(self.language, "{}={} fields"), &[label, fields]))
      .collect::<Vec<_>>()
      .join(", ");
    let partial_note = if partial {
      tr(self.language, " (partial transfer: check worn gear and loadouts in game)")
    } else {
      ""
    };
    Ok(fill(
      tr(self.language, "Equipment transferred: {}; Equipment Box {} slots, {} items{}"),
      &[&classes, &report.box_slots, &report.box_used, &partial_note],
    ))
  }

  /// Re-reads the item box summary after an in-memory mutation.
  fn refresh_target_items(&mut self) {
    let read = self
      .target
      .as_ref()
      .map(|opened| edit::read_item_box(opened.document.payload()));
    let items: Vec<(u32, u32)> = match read {
      Some(Ok(entries)) => entries.into_iter().map(|entry| (entry.id, entry.num)).collect(),
      _ => return,
    };
    let len = items.len();
    if let Some(opened) = self.target.as_mut() {
      opened.box_items = items;
    }
    self.box_edits = vec![String::new(); len];
    self.dirty = true;
  }

  /// Snapshots the opened save and hands it to the worker to write as a new
  /// file next to the original.
  fn save_target_to_new_file(&mut self, ui: &egui::Ui) {
    let Some(opened) = self.target.as_ref() else {
      return;
    };
    let output = derived_output(&opened.path, "edited");
    if output == opened.path {
      self.log_line(tr(self.language, "✘ Output path equals the source file; the source save is never modified"));
      return;
    }
    let snapshot = Box::new(OpenedSave {
      path: opened.path.clone(),
      document: opened.document.clone(),
      hunter_name: opened.hunter_name.clone(),
      box_items: opened.box_items.clone(),
    });
    self.spawn(ui.ctx(), WorkerTask::Save { document: snapshot, output });
  }

  /// Writes the opened save back to its original path, taking a fresh
  /// directory backup first so the write is always reversible.
  fn save_target_in_place(&mut self, ui: &egui::Ui) {
    let Some(opened) = self.target.as_ref() else {
      return;
    };
    let Some(backup_dir) = opened.path.parent().map(Path::to_path_buf) else {
      return;
    };
    let snapshot = Box::new(OpenedSave {
      path: opened.path.clone(),
      document: opened.document.clone(),
      hunter_name: opened.hunter_name.clone(),
      box_items: opened.box_items.clone(),
    });
    self.spawn(ui.ctx(), WorkerTask::SaveInPlace { document: snapshot, backup_dir });
  }
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

enum WorkerTask {
  Open { path: PathBuf, steamid64: u64, is_source: bool },
  Save { document: Box<OpenedSave>, output: PathBuf },
  /// Writes over the original save path, after a fresh backup of its
  /// directory; the write is skipped when the backup fails.
  SaveInPlace { document: Box<OpenedSave>, backup_dir: PathBuf },
  Backup { dir: PathBuf },
  Restore { backup: PathBuf, target_dir: PathBuf },
}

impl WorkerTask {
  fn describe(&self, language: Language) -> String {
    match self {
      WorkerTask::Open { path, is_source, .. } => {
        if *is_source {
          fill(tr(language, "Open source save: {}"), &[&path.display()])
        } else {
          fill(tr(language, "Open save: {}"), &[&path.display()])
        }
      }
      WorkerTask::Save { output, .. } => fill(tr(language, "Save to: {}"), &[&output.display()]),
      WorkerTask::SaveInPlace { backup_dir, .. } => {
        fill(tr(language, "Write to original save (backup first: {})"), &[&backup_dir.display()])
      }
      WorkerTask::Backup { dir } => fill(tr(language, "Back up folder: {}"), &[&dir.display()]),
      WorkerTask::Restore { backup, target_dir } => {
        fill(tr(language, "Restore {} to {}"), &[&backup.display(), &target_dir.display()])
      }
    }
  }

  fn run(self, language: Language) -> WorkerEvent {
    match self {
      WorkerTask::Open { path, steamid64, is_source } => {
        let result =
          OpenedSave::open(path, steamid64).map(Box::new).map_err(|error| format!("{error:?}"));
        WorkerEvent::Opened(result, is_source)
      }
      WorkerTask::Save { document, output } => {
        let result =
          document.document.write_to(&output).map(|_| (output, None)).map_err(|error| format!("{error:?}"));
        WorkerEvent::Saved(result)
      }
      WorkerTask::SaveInPlace { document, backup_dir } => {
        let run = || -> anyhow::Result<(PathBuf, Option<PathBuf>)> {
          let report = crate::archive::backup_dir(&backup_dir, None)?;
          let backup = report.destination.clone();
          let output = document.path.clone();
          document.document.write_to(&output)?;
          Ok((output, Some(backup)))
        };
        let result = run().map_err(|error| format!("{error:?}"));
        WorkerEvent::Saved(result)
      }
      WorkerTask::Backup { dir } => {
        let result = crate::archive::backup_dir(&dir, None)
          .map(|report| {
            fill(
              tr(language, "Backed up {} files ({} bytes) to {}; every copy verified byte by byte"),
              &[&report.files.len(), &report.total_bytes(), &report.destination.display()],
            )
          })
          .map_err(|error| format!("{error:?}"));
        WorkerEvent::Report(result)
      }
      WorkerTask::Restore { backup, target_dir } => {
        let result = crate::archive::restore_dir(&backup, &target_dir, true)
          .map(|report| {
            fill(
              tr(language, "Restored {} files ({} bytes) to {}; reopen the save to see it"),
              &[&report.files.len(), &report.total_bytes(), &target_dir.display()],
            )
          })
          .map_err(|error| format!("{error:?}"));
        WorkerEvent::Report(result)
      }
    }
  }
}

// ---------------------------------------------------------------------------
// eframe app
// ---------------------------------------------------------------------------

impl GuiApp {
  /// Diagnostics for IME debugging: with `MHR_IME_DEBUG=1`, appends every
  /// keyboard/IME input event egui receives to `ime_debug.log` next to the
  /// executable. Cheap when the variable is unset.
  fn log_input_events_if_enabled(&mut self, ui: &egui::Ui) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("MHR_IME_DEBUG").is_some()) {
      return;
    }
    let events = ui.input(|input| input.events.clone());
    if events.is_empty() {
      return;
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open("ime_debug.log")
    else {
      return;
    };
    use std::io::Write as _;
    let stamp = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map(|d| d.as_millis())
      .unwrap_or(0);
    let _ = writeln!(file, "=== frame {stamp} ({} events)", events.len());
    for event in events {
      let _ = writeln!(file, "  {event:?}");
    }
  }
}

impl eframe::App for GuiApp {
  fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
    self.log_input_events_if_enabled(ui);
    if let Some(receiver) = &self.receiver {
      match receiver.try_recv() {
        Ok(WorkerEvent::Opened(result, is_source)) => {
          self.busy = false;
          self.receiver = None;
          if let Err(error) = &result {
            self.status_error = Some(error.clone());
            // A wrong SteamID64 is the common failure; the save path names the
            // 32-bit account id, so derive the correct value automatically.
            let failed_path = if is_source { &self.source_path } else { &self.target_path };
            if let Some(account) = account_id_from_path(Path::new(failed_path.trim()))
              && self.steamid64.trim() != steamid64_from_account(account).to_string()
            {
              self.steamid64 = steamid64_from_account(account).to_string();
              let note = tr(self.language, "! SteamID64 corrected to {} from the save path; try again");
              self.log_line(&fill(note, &[&self.steamid64]));
            }
          } else {
            self.status_error = None;
          }
          match result {
            Ok(opened) => {
              let (money, total) = edit::read_money(opened.document.payload()).unwrap_or((0, 0));
              let points = edit::read_points(opened.document.payload()).unwrap_or(0);
              self.money_input = money.to_string();
              self.total_input = total.to_string();
              self.points_input = points.to_string();
              let ranks = edit::read_ranks(opened.document.payload());
              self.hunter_rank_input = ranks.map(|r| r.hunter.to_string()).unwrap_or_default();
              self.master_rank_input = ranks.map(|r| r.master.to_string()).unwrap_or_default();
              self.mystery_rank_input =
                ranks.map(|r| r.mystery_research.to_string()).unwrap_or_default();
              self.box_edits = vec![String::new(); opened.box_items.len()];
              self.refresh_progress_inputs();
              let language = self.language;
              self.log_line(&fill(
                tr(language, "✔ Opened {}: {} ({})"),
                &[
                  &tr(language, if is_source { "source save" } else { "target save" }),
                  &opened.path.display(),
                  &opened.summary(language),
                ],
              ));
              if is_source {
                self.source = Some(opened);
              } else {
                self.target = Some(opened);
                self.dirty = false;
                self.reopen_clicked_once = false;
              }
            }
            Err(error) => self.log_line(&fill(tr(self.language, "✘ Open failed: {}"), &[&error])),
          }
        }
        Ok(WorkerEvent::Saved(result)) => {
          self.busy = false;
          self.receiver = None;
          match result {
            Ok((output, backup)) => {
              self.dirty = false;
              let language = self.language;
              self.log_line(&fill(
                tr(language, "✔ Written: {}{}"),
                &[
                  &output.display(),
                  &backup
                    .map(|backup| fill(tr(language, " (backed up to {})"), &[&backup.display()]))
                    .unwrap_or_default(),
                ],
              ));
            }
            Err(error) => self.log_line(&fill(tr(self.language, "✘ Save failed: {}"), &[&error])),
          }
        }
        Ok(WorkerEvent::Report(result)) => {
          self.busy = false;
          self.receiver = None;
          self.restore_confirm = None;
          let restored_ok = result.is_ok();
          match result {
            Ok(text) => self.log_line(&format!("✔ {text}")),
            Err(error) => self.log_line(&format!("✘ {error}")),
          }
          // A restore rewrote the save directory under us; reload the target
          // so the editor shows what is actually on disk now.
          if restored_ok
            && let (Ok(steamid64), Some(opened)) = (self.parsed_steamid64(), self.target.as_ref())
          {
            let task =
              WorkerTask::Open { path: opened.path.clone(), steamid64, is_source: false };
            self.spawn(ui.ctx(), task);
          }
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
          self.busy = false;
          self.receiver = None;
          self.log_line(tr(self.language, "✘ Worker thread exited unexpectedly"));
        }
      }
    }

    egui::CentralPanel::default().show(ui, |ui| {
      self.show_top_bar(ui);
      ui.add_space(4.0);
      if self.game != Game::World {
        self.show_tab_bar(ui);
      }
      ui.add_space(4.0);
      // Reserve room for the save bar and log below; without an explicit cap
      // the tab content claims every remaining pixel and pushes the save
      // button out of the visible area.
      let bottom_reserve = 180.0;
      let content_height = (ui.available_height() - bottom_reserve).max(120.0);
      egui::ScrollArea::vertical().auto_shrink(false).max_height(content_height).show(ui, |ui| {
        if self.game == Game::World {
          self.show_mhw_panel(ui);
          return;
        }
        match self.tab {
          TAB_INFO => self.show_info_tab(ui),
          TAB_PLAYER => self.show_player_tab(ui),
          TAB_ITEMBOX => self.show_itembox_tab(ui),
          TAB_EQUIP => self.show_equip_tab(ui),
          TAB_TRANSFER => self.show_transfer_tab(ui),
          TAB_PROGRESS => self.show_progress_tab(ui),
          TAB_BACKUP => self.show_backup_tab(ui),
          _ => self.show_help_tab(ui),
        }
      });

      ui.add_space(6.0);
      ui.separator();
      if self.game == Game::World {
        self.show_mhw_save_bar(ui);
      } else {
        self.show_save_bar(ui);
      }
      ui.label(tr(self.language, "Activity log"));
      egui::ScrollArea::vertical().stick_to_bottom(true).max_height(120.0).show(ui, |ui| {
        ui.monospace(if self.log.is_empty() { tr(self.language, "(ready)") } else { &self.log });
      });
    });

    // Keep the window's IME context associated at all times. winit dissociates
    // it at window creation, and egui disassociates it again whenever no
    // TextEdit has focus (Windows: ImmAssociateContextEx IACE_CHILDREN); on
    // some Windows builds the IME then never composes again — keys are
    // swallowed with no preedit or candidate window until a refocus. eframe
    // runs the viewport commands after egui's own IME handling, so re-allowing
    // IME on the frames where egui turned it off wins the race. Frames where a
    // TextEdit is focused (composition in progress) are left untouched. The
    // cost is that a stray composition can start while no text field is
    // focused; harmless here because nothing else consumes plain typing.
    let ime_allowed_by_text_edit = ui.ctx().output(|output| output.ime.is_some());
    if !ime_allowed_by_text_edit {
      ui.ctx().send_viewport_cmd(egui::ViewportCommand::IMEAllowed(true));
    }

    if self.busy {
      ui.ctx().request_repaint_after(Duration::from_millis(120));
    }
  }
}

// ---------------------------------------------------------------------------
// UI sections
// ---------------------------------------------------------------------------

/// Card size and corner radius of the game selector (design token `radius-lg`).
const GAME_CARD_SIZE: egui::Vec2 = egui::vec2(150.0, 84.0);
const GAME_CARD_RADIUS: u8 = 10;
/// Logo cards stay dark in both themes: the World and Wilds lettering is
/// white/silver and would vanish on the light theme.
const GAME_CARD_BG: egui::Color32 = egui::Color32::from_rgb(0x16, 0x14, 0x11);

fn load_game_logos(ctx: &egui::Context) -> [egui::TextureHandle; 3] {
  Game::ALL.map(|game| {
    let image = image::load_from_memory_with_format(game.logo_bytes(), image::ImageFormat::Png)
      .expect("embedded logo is a valid PNG")
      .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    ctx.load_texture(format!("logo-{}", game.name()), color, egui::TextureOptions::LINEAR)
  })
}

impl GuiApp {
  fn show_game_selector(&mut self, ui: &mut egui::Ui) {
    let logos = self.game_logos.get_or_insert_with(|| load_game_logos(ui.ctx())).clone();
    let palette = self.theme.palette();
    ui.horizontal(|ui| {
      for (game, texture) in Game::ALL.into_iter().zip(logos.iter()) {
        let (rect, response) = ui.allocate_exact_size(GAME_CARD_SIZE, egui::Sense::click());
        let selected = self.game == game;
        let radius = egui::CornerRadius::same(GAME_CARD_RADIUS);
        let painter = ui.painter();
        painter.rect_filled(rect, radius, GAME_CARD_BG);
        let opacity = if game.supported() { 1.0 } else { 0.45 };
        let tint = egui::Color32::WHITE.gamma_multiply(opacity);
        // Logo on top, status line below, both inside the card.
        let logo_area = egui::Rect::from_min_max(
          rect.min + egui::vec2(8.0, 6.0),
          egui::pos2(rect.max.x - 8.0, rect.max.y - 22.0),
        );
        let natural = texture.size_vec2();
        let scale = (logo_area.width() / natural.x).min(logo_area.height() / natural.y);
        let logo_rect = egui::Rect::from_center_size(logo_area.center(), natural * scale);
        painter.image(
          texture.id(),
          logo_rect,
          egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
          tint,
        );
        let status_color = if game.supported() { palette.success } else { egui::Color32::from_gray(0xb0) };
        painter.text(
          egui::pos2(rect.center().x, rect.max.y - 11.0),
          egui::Align2::CENTER_CENTER,
          tr(self.language, game.status()),
          egui::FontId::proportional(12.0),
          status_color,
        );
        if selected {
          painter.rect_stroke(rect, radius, egui::Stroke::new(2.0, palette.accent), egui::StrokeKind::Inside);
        } else if response.hovered() {
          painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0, palette.accent.gamma_multiply(0.5)),
            egui::StrokeKind::Inside,
          );
        }
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(game.name());
        if response.clicked() {
          self.game = game;
        }
      }
    });
  }

  fn show_top_bar(&mut self, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
      ui.heading(tr(self.language, "MH Save Editor"));
      ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        // Right-to-left layout: the first widget added sits at the far right.
        egui::ComboBox::from_id_salt("language")
          .selected_text(self.language.label())
          .show_ui(ui, |ui| {
            for language in Language::ALL {
              ui.selectable_value(&mut self.language, language, language.label());
            }
          });
        let theme_label = fill(tr(self.language, "Theme: {}"), &[&tr(self.language, self.theme.label())]);
        if ui.button(theme_label).clicked() {
          self.theme = self.theme.toggled();
          ui.ctx().set_visuals(self.theme.visuals());
        }
      });
    });
    self.show_game_selector(ui);
    if !self.game.supported() {
      ui.colored_label(
        self.theme.palette().danger,
        fill(
          tr(self.language, "✘ {} saves are not supported yet."),
          &[&self.game.name()],
        ),
      );
    }
    ui.add_space(4.0);
    Grid::new("top-bar").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label(tr(self.language, "Quick select"));
      let selected = self
        .discovered
        .iter()
        .find(|(path, _, _)| path.display().to_string() == self.target_path)
        .map(|(_, label, _)| label.clone())
        .unwrap_or_else(|| tr(self.language, "(no saves found)").to_owned());
      egui::ComboBox::from_id_salt("discovered-saves").selected_text(selected).show_ui(ui, |ui| {
        for (path, label, account) in &self.discovered {
          if ui.selectable_value(&mut self.target_path, path.display().to_string(), label).clicked()
            && let Some(account) = account
          {
            // The save folder name is the 32-bit account id; derive the
            // SteamID64 automatically so users never have to look it up.
            self.steamid64 = steamid64_from_account(*account).to_string();
            let note = format!(
              "{}\n",
              fill(tr(self.language, "SteamID64 filled in from the save path: {}"), &[&self.steamid64])
            );
            self.log.push_str(&note);
          }
        }
      });
      ui.label("SteamID64");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.steamid64));
      ui.end_row();

      ui.label(tr(self.language, "Target save"));
      ui.add_sized([420.0, 20.0], egui::TextEdit::singleline(&mut self.target_path));
      if ui.add_enabled(!self.busy, egui::Button::new(tr(self.language, "Browse…"))).clicked()
        && let Some(path) = rfd::FileDialog::new()
          .add_filter(tr(self.language, "MH Rise save"), &["bin"])
          .set_title(tr(self.language, "Choose target save"))
          .pick_file()
      {
        self.target_path = path.display().to_string();
      }
      let steamid = self.parsed_steamid64();
      let open_label = if self.dirty && !self.reopen_clicked_once {
        tr(self.language, "Open save (unsaved changes)")
      } else {
        tr(self.language, "Open save")
      };
      if ui
        .add_enabled(
          !self.busy
            && self.game.supported()
            && (self.game == Game::World || steamid.is_ok())
            && !self.target_path.trim().is_empty(),
          egui::Button::new(open_label),
        )
        .clicked()
      {
        if self.game == Game::World {
          self.request_mhw_open();
        } else if self.dirty && !self.reopen_clicked_once {
          self.reopen_clicked_once = true;
          self.log_line(tr(self.language, "! Unsaved changes: click Open save again to discard them and reopen"));
        } else {
          let task = WorkerTask::Open {
            path: PathBuf::from(self.target_path.trim()),
            steamid64: steamid.expect("checked"),
            is_source: false,
          };
          self.reopen_clicked_once = false;
          self.spawn(ui.ctx(), task);
        }
      }
      ui.end_row();
    });
    if let Some(error) = &self.status_error {
      let text = fill(tr(self.language, "✘ Last operation failed: {}"), &[&error]);
      ui.colored_label(self.theme.palette().danger, text);
    }
    let language = self.language;
    ui.label(match (&self.target, self.busy) {
      (_, true) => format!("⏳ {}…", self.busy_text),
      (Some(opened), _) => fill(
        tr(language, "Opened: {} ({}){}"),
        &[
          &opened.path.display(),
          &opened.summary(language),
          &if self.dirty { tr(language, " · ● unsaved changes") } else { "" },
        ],
      ),
      (None, _) => tr(language, "No save open").to_owned(),
    });
  }

  fn show_tab_bar(&mut self, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
      for (tab, label) in [
        (TAB_INFO, "General"),
        (TAB_PLAYER, "Player"),
        (TAB_ITEMBOX, "Item Box"),
        (TAB_EQUIP, "Loadouts"),
        (TAB_TRANSFER, "Transfer"),
        (TAB_PROGRESS, "Progress"),
        (TAB_BACKUP, "Backup"),
        (TAB_HELP, "Help"),
      ] {
        ui.selectable_value(&mut self.tab, tab, tr(self.language, label));
      }
    });
    ui.separator();
  }

  fn show_info_tab(&mut self, ui: &mut egui::Ui) {
    let Some(opened) = self.target.as_ref() else {
      ui.label(tr(self.language, "No save open. Choose one with Quick select or Browse above, then click Open save."));
      return;
    };
    let language = self.language;
    Grid::new("info").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label(tr(language, "Hunter name"));
      ui.label(if opened.hunter_name.is_empty() {
        tr(language, "(not found)").to_owned()
      } else {
        opened.hunter_name.clone()
      });
      ui.end_row();
      ui.label(tr(language, "Curve Index"));
      ui.label(opened.document.curve_index().to_string());
      ui.end_row();
      ui.label(tr(language, "Decrypted size"));
      ui.label(fill(tr(language, "{} bytes"), &[&opened.document.original_decrypted().len()]));
      ui.end_row();
      ui.label(tr(language, "Item Box"));
      ui.label(fill(
        tr(language, "{} slots, {} non-empty"),
        &[
          &opened.box_items.len(),
          &opened.box_items.iter().filter(|(_, num)| *num > 0).count(),
        ],
      ));
      ui.end_row();
      ui.label(tr(language, "Zenny / Points"));
      let (money, total) = edit::read_money(opened.document.payload()).unwrap_or((0, 0));
      let points = edit::read_points(opened.document.payload()).unwrap_or(0);
      ui.label(fill(tr(language, "{} / {} ({} earned in total)"), &[&money, &points, &total]));
      ui.end_row();
      ui.label(tr(language, "Rank (HR / MR / Anomaly)"));
      let ranks = edit::read_ranks(opened.document.payload());
      ui.label(match ranks {
        Some(ranks) => format!("{} / {} / {}", ranks.hunter, ranks.master, ranks.mystery_research),
        None => tr(language, "(not found)").to_owned(),
      });
      ui.end_row();
      ui.label(tr(language, "File"));
      ui.label(opened.path.display().to_string());
      ui.end_row();
    });
    if let Ok(data) = fs::read(&opened.path)
      && let Ok(header) = parse_header(&data)
    {
      ui.label(fill(
        tr(language, "Container: DSSS v{}, flags=0x{}, file {} bytes"),
        &[&header.version, &format!("{:08x}", header.raw_flags), &data.len()],
      ));
    }
  }

  fn show_player_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label(tr(self.language, "No save open."));
      return;
    }
    Grid::new("player").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label(tr(self.language, "Hunter name"));
      let name = self.target.as_ref().map(|opened| opened.hunter_name.clone()).unwrap_or_default();
      ui.label(if name.is_empty() { tr(self.language, "(not found)").to_owned() } else { name });
      ui.end_row();
      ui.label(tr(self.language, "Zenny"));
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.money_input));
      ui.end_row();
      ui.label(tr(self.language, "Total Zenny earned"));
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.total_input));
      ui.end_row();
      ui.label(tr(self.language, "Points"));
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.points_input));
      ui.end_row();
      ui.label(tr(self.language, "Hunter Rank (HR)"));
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.hunter_rank_input));
      ui.end_row();
      ui.label(tr(self.language, "Master Rank (MR)"));
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.master_rank_input));
      ui.end_row();
      ui.label(tr(self.language, "Anomaly Research Rank"));
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.mystery_rank_input));
      ui.end_row();
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
      let steamid = self.parsed_steamid64();
      let ready = !self.busy && steamid.is_ok() && self.target.is_some();
      if ui.add_enabled(ready, egui::Button::new(tr(self.language, "Apply changes"))).clicked() {
        match self.apply_player_edits() {
          Ok(()) => self.log_line(tr(self.language, "✔ Changes applied (in memory)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      if ui
        .add_enabled(!self.busy && self.target.is_some(), egui::Button::new(tr(self.language, "Write to save (backs up first)")))
        .clicked()
      {
        self.save_target_in_place(ui);
      }
      ui.label(tr(self.language, "(leave blank = no change; the name is read-only)"));
    });
    ui.colored_label(
      self.theme.palette().accent,
      tr(self.language, "! Ranks are limited by the unlock system: if the matching unlock quests (Hub / Master Rank / Anomaly Investigation urgent chains) are not complete, the game shows an achievement popup on load but still clamps the rank to the current cap. Unlock state is stored in a quest-clear bitmap that this tool cannot rewrite yet; finish the unlock quests in game, or use Transfer to move over a high-rank save."),
    );
  }

  fn show_itembox_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label(tr(self.language, "No save open."));
      return;
    }
    ui.horizontal(|ui| {
      ui.add_sized(
        [200.0, 20.0],
        egui::TextEdit::singleline(&mut self.box_filter).hint_text(tr(self.language, "Search name / ID")),
      );
      let current = if self.box_category == 0 {
        tr(self.language, "All categories")
      } else {
        tr(self.language, ITEM_CATEGORY_GROUPS[self.box_category - 1].0)
      };
      egui::ComboBox::from_id_salt("box-category").selected_text(current).show_ui(ui, |ui| {
        ui.selectable_value(&mut self.box_category, 0, tr(self.language, "All categories"));
        for (i, (label, _)) in ITEM_CATEGORY_GROUPS.iter().enumerate() {
          ui.selectable_value(&mut self.box_category, i + 1, tr(self.language, *label));
        }
      });
      ui.checkbox(&mut self.box_show_empty, tr(self.language, "Show empty slots"));
      let changes = self.pending_item_changes();
      let apply_text = fill(tr(self.language, "Apply item changes ({})"), &[&changes.len()]);
      if ui.add_enabled(!self.busy && !changes.is_empty(), egui::Button::new(apply_text)).clicked()
      {
        match self.apply_box_edits() {
          Ok(text) => {
            self.log_line(&fill(tr(self.language, "✔ Item changes applied: {} (in memory)"), &[&text]))
          }
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      if ui
        .add_enabled(!self.busy && self.target.is_some(), egui::Button::new(tr(self.language, "Write to save (backs up first)")))
        .clicked()
      {
        self.save_target_in_place(ui);
      }
      if self.dirty {
        ui.label(egui::RichText::new(tr(self.language, "● Unsaved changes")).color(self.theme.palette().accent));
      }
    });
    if self.box_edits.len() != self.box_items_len() {
      ui.label(tr(self.language, "(loading…)"));
      return;
    }
    let filter = self.box_filter.trim().to_lowercase();
    let rows = self.visible_box_rows(&filter);
    let muted = self.theme.palette().ink_muted;
    let danger = self.theme.palette().danger;
    let selected_count = self.box_selected.len();
    let actionable = !self.busy && selected_count > 0;
    ui.label(egui::RichText::new(tr(self.language, "Edit selected")).strong());
    egui::Frame::group(ui.style()).show(ui, |ui| {
      ui.horizontal(|ui| {
        ui.label(fill(tr(self.language, "{} selected"), &[&selected_count]));
        if ui.button(tr(self.language, "Select all shown")).clicked() {
          let with_items: Vec<usize> =
            rows.iter().copied().filter(|&index| self.box_slot_has_item(index)).collect();
          self.box_selected.extend(with_items);
        }
        if ui.add_enabled(selected_count > 0, egui::Button::new(tr(self.language, "Clear selection"))).clicked() {
          self.box_selected.clear();
          self.box_last_clicked = None;
        }
        ui.separator();
        ui.label(tr(self.language, "Quantity"));
        ui.add_sized(
          [80.0, 20.0],
          egui::TextEdit::singleline(&mut self.box_bulk_qty).hint_text(tr(self.language, "5, +5 or -5")),
        );
        if ui.add_enabled(actionable, egui::Button::new(tr(self.language, "Apply"))).clicked() {
          let result = self.box_set_quantity_selected();
          self.log_box_result(result);
        }
        let delete_label = egui::RichText::new(tr(self.language, "Delete selected")).color(danger);
        if ui.add_enabled(actionable, egui::Button::new(delete_label)).clicked() {
          let result = self.box_delete_selected();
          self.log_box_result(result);
        }
      });
      ui.horizontal(|ui| {
        ui.label(tr(self.language, "Move selection to slot"));
        ui.add_sized(
          [60.0, 20.0],
          egui::TextEdit::singleline(&mut self.box_move_slot).hint_text("1-1800"),
        );
        if ui.add_enabled(actionable, egui::Button::new(tr(self.language, "Move"))).clicked() {
          let result = self.box_move_selected();
          self.log_box_result(result);
        }
        ui.separator();
        ui.label(tr(self.language, "Sort box by"));
        let current = tr(self.language, SortKey::ALL[self.box_sort].label());
        egui::ComboBox::from_id_salt("box-sort").selected_text(current).show_ui(ui, |ui| {
          for (i, key) in SortKey::ALL.iter().enumerate() {
            ui.selectable_value(&mut self.box_sort, i, tr(self.language, key.label()));
          }
        });
        if ui.add_enabled(!self.busy, egui::Button::new(tr(self.language, "Sort (empty slots go last)"))).clicked() {
          let result = self.box_sort_all();
          self.log_box_result(result);
        }
      });
    });
    ui.horizontal(|ui| {
      let left = egui::Layout::left_to_right(egui::Align::Center);
      let right = egui::Layout::right_to_left(egui::Align::Center);
      let header = |text: &str| egui::RichText::new(text.to_owned()).small().strong().color(muted);
      let language = self.language;
      for (width, text) in [(24.0, ""), (52.0, "Slot"), (240.0, "Name"), (90.0, "Category"), (84.0, "ID")] {
        cell(ui, width, left, |ui| ui.label(header(tr(language, text))));
      }
      cell(ui, 72.0, right, |ui| ui.label(header(tr(language, "Qty"))));
      cell(ui, 90.0, left, |ui| ui.label(header(tr(language, "New qty"))));
    });
    egui::ScrollArea::vertical().max_height(300.0).auto_shrink(false).show_rows(
      ui,
      22.0,
      rows.len(),
      |ui, range| {
        for row in range {
          let index = rows[row];
          let Some(checked) = self.show_item_row(ui, index) else {
            continue;
          };
          let shift = ui.input(|input| input.modifiers.shift);
          let affected: Vec<usize> = match (shift, self.box_last_clicked) {
            (true, Some(last)) => {
              let (from, to) = (last.min(row), last.max(row));
              rows[from..=to].iter().copied().filter(|&i| self.box_slot_has_item(i)).collect()
            }
            _ => vec![index],
          };
          for slot in affected {
            if checked {
              self.box_selected.insert(slot);
            } else {
              self.box_selected.remove(&slot);
            }
          }
          self.box_last_clicked = Some(row);
        }
      },
    );
    ui.label(fill(
      tr(
        self.language,
        "Showing {} of {} slots. Tick rows (shift-click for a range) to edit many at once. Per row, New qty accepts 5 (set), +5 (add) or -5 (subtract); blank = no change, 0 = clear the item.",
      ),
      &[&rows.len(), &self.box_items_len()],
    ));
  }

  fn box_slot_has_item(&self, index: usize) -> bool {
    self.target.as_ref().is_some_and(|opened| opened.box_items.get(index).is_some_and(|slot| slot.1 > 0))
  }

  fn log_box_result(&mut self, result: Result<String, String>) {
    match result {
      Ok(text) => self.log_line(&fill(tr(self.language, "✔ {} (in memory)"), &[&text])),
      Err(error) => self.log_line(&format!("✘ {error}")),
    }
  }

  /// Rewrites the target's whole item box: `build` edits a copy of the layout
  /// and returns a status line, then the result is written into the save.
  fn rewrite_box(
    &mut self,
    build: impl FnOnce(&mut Vec<(u32, u32)>) -> Result<String, String>,
  ) -> Result<String, String> {
    let Some(opened) = self.target.as_mut() else {
      return Err(tr(self.language, "No target save is open").to_owned());
    };
    let mut layout = opened.box_items.clone();
    let message = build(&mut layout)?;
    edit::write_item_box(opened.document.payload_mut(), &layout)
      .map_err(|error| error.to_string())?;
    opened.box_items = edit::read_item_box(opened.document.payload())
      .map_err(|error| error.to_string())?
      .into_iter()
      .map(|entry| (entry.id, entry.num))
      .collect();
    self.box_edits = vec![String::new(); opened.box_items.len()];
    self.box_last_clicked = None;
    self.dirty = true;
    Ok(message)
  }

  fn box_delete_selected(&mut self) -> Result<String, String> {
    let selected: Vec<usize> = self.box_selected.iter().copied().collect();
    let language = self.language;
    let message = self.rewrite_box(|layout| {
      let removed = itembox::delete(layout, &selected);
      Ok(fill(tr(language, "Deleted {} slots"), &[&removed]))
    })?;
    self.box_selected.clear();
    Ok(message)
  }

  fn box_set_quantity_selected(&mut self) -> Result<String, String> {
    let op = QuantityOp::parse(&self.box_bulk_qty)?;
    let selected: Vec<usize> = self.box_selected.iter().copied().collect();
    let language = self.language;
    self.rewrite_box(|layout| {
      let changed = itembox::apply_quantity(layout, &selected, op);
      let what = match op {
        QuantityOp::Set(value) => fill(tr(language, "Set quantity to {}"), &[&value]),
        QuantityOp::Add(value) => fill(tr(language, "Added {} to the quantity"), &[&value]),
        QuantityOp::Subtract(value) => fill(tr(language, "Subtracted {} from the quantity"), &[&value]),
      };
      Ok(fill(tr(language, "{} on {} slots"), &[&what, &changed]))
    })
  }

  fn box_move_selected(&mut self) -> Result<String, String> {
    let slot = self
      .box_move_slot
      .trim()
      .parse::<usize>()
      .map_err(|_| tr(self.language, "Target slot must be a number").to_owned())?;
    if slot == 0 {
      return Err(tr(self.language, "Slots start at 1").to_owned());
    }
    let selected: Vec<usize> = self.box_selected.iter().copied().collect();
    let language = self.language;
    let mut landed = Vec::new();
    let message = self.rewrite_box(|layout| {
      landed = itembox::move_to(layout, &selected, slot - 1)?;
      Ok(fill(tr(language, "Moved {} slots to start at slot {}"), &[&landed.len(), &slot]))
    })?;
    self.box_selected = landed.into_iter().collect();
    Ok(message)
  }

  fn box_sort_all(&mut self) -> Result<String, String> {
    let key = SortKey::ALL[self.box_sort];
    let language = self.language;
    let message = self.rewrite_box(|layout| {
      itembox::sort(layout, key);
      Ok(fill(tr(language, "Item Box sorted by {}"), &[&tr(language, key.label())]))
    })?;
    self.box_selected.clear();
    Ok(message)
  }

  /// Draws one item box row. Returns `Some(checked)` when the row's checkbox
  /// was clicked this frame.
  fn show_item_row(&mut self, ui: &mut egui::Ui, index: usize) -> Option<bool> {
    let (id, num) = self.target.as_ref().map(|opened| opened.box_items[index])?;
    let muted = self.theme.palette().ink_muted;
    let mut selected = self.box_selected.contains(&index);
    let mut toggled = None;
    let fill = if selected { ui.visuals().selection.bg_fill } else { egui::Color32::TRANSPARENT };
    egui::Frame::new().fill(fill).corner_radius(egui::CornerRadius::same(3)).show(ui, |ui| {
      ui.horizontal(|ui| {
        let left = egui::Layout::left_to_right(egui::Align::Center);
        let right = egui::Layout::right_to_left(egui::Align::Center);
        if num > 0 {
          if cell(ui, 24.0, left, |ui| ui.checkbox(&mut selected, "")).changed() {
            toggled = Some(selected);
          }
        } else {
          cell(ui, 24.0, left, |_| {});
        }
        cell(ui, 52.0, left, |ui| {
          ui.label(egui::RichText::new(format!("{:04}", index + 1)).monospace().color(muted))
        });
        if num == 0 {
          cell(ui, 240.0, left, |ui| {
            ui.label(egui::RichText::new(tr(self.language, "(empty slot)")).italics().color(muted))
          });
        } else {
          let name = crate::items::item_display_name_in(id, self.language);
          let response = cell(ui, 240.0, left, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(name).strong()).truncate())
          });
          // Hover shows the other language's name, if the table has one.
          let other = match self.language {
            Language::English => crate::items::item_name_cn(id),
            Language::Chinese => crate::items::item_name_en(id),
          };
          if let Some(other) = other {
            response.on_hover_text(other);
          }
        }
        let category = crate::items::item_category(id).unwrap_or("-");
        cell(ui, 90.0, left, |ui| {
          ui.add(egui::Label::new(egui::RichText::new(category).color(muted)).truncate())
        });
        cell(ui, 84.0, left, |ui| {
          ui.label(egui::RichText::new(format!("{id:08x}")).monospace().color(muted))
        });
        cell(ui, 72.0, right, |ui| ui.label(egui::RichText::new(num.to_string()).monospace()));
        ui.add_sized([90.0, 18.0], egui::TextEdit::singleline(&mut self.box_edits[index]));
      });
    });
    toggled
  }

  fn show_equip_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label(tr(self.language, "Open the target save first (Quick select or Browse above)."));
      return;
    }
    let language = self.language;
    ui.columns(2, |columns| {
      let mut column_iter = columns.iter_mut();
      let left = column_iter.next().expect("two columns");
      let right = column_iter.next().expect("two columns");
      // Left: the target's own loadout registers.
      left.heading(tr(language, "Target loadouts (224 slots)"));
      let loadouts = self
        .target
        .as_ref()
        .map(|opened| edit::read_equip_loadouts(opened.document.payload()))
        .unwrap_or_default();
      let used = loadouts.iter().filter(|loadout| loadout.is_using).count();
      left.label(fill(
        tr(language, "{} registered; the Equipment Box slots a loadout references can be moved with it"),
        &[&used],
      ));
      egui::ScrollArea::vertical().max_height(360.0).auto_shrink(false).show(left, |ui| {
        for loadout in &loadouts {
          ui.horizontal(|ui| {
            ui.monospace(format!("{:3}", loadout.index));
            if loadout.is_using {
              ui.label(&loadout.name);
            } else {
              ui.label(egui::RichText::new(tr(language, "(empty)")).weak());
            }
          });
        }
      });

      // Right: transfer controls.
      right.heading(tr(language, "Transfer from source save"));
      let Some(source) = self.source.as_ref() else {
        right.label(tr(language, "Open a source save on the Transfer tab first to transfer loadouts or single pieces."));
        return;
      };
      let source_loadouts = edit::read_equip_loadouts(source.document.payload());
      let using: Vec<&edit::EquipLoadout> =
        source_loadouts.iter().filter(|loadout| loadout.is_using).collect();
      if using.is_empty() {
        right.label(tr(language, "The source save has no registered loadouts."));
        return;
      }
      let selected = self
        .equip_source_combo
        .min(using.len().saturating_sub(1));
      let selected_text = format!("[{}] {}", using[selected].index, using[selected].name);
      right.label(tr(language, "Source loadout:"));
      egui::ComboBox::from_id_salt("equip-source-loadout")
        .selected_text(selected_text)
        .width(260.0)
        .show_ui(right, |ui| {
          for (choice, loadout) in using.iter().enumerate() {
            ui.selectable_value(
              &mut self.equip_source_combo,
              choice,
              format!("[{}] {}", loadout.index, loadout.name),
            );
          }
        });
      right.horizontal(|ui| {
        ui.label(tr(language, "Referenced gear:"));
        ui.checkbox(
          &mut self.equip_with_pieces,
          tr(language, "Also move the gear the loadout references (overwrites the same slots)"),
        );
      });
      right.horizontal(|ui| {
        ui.label(tr(language, "Target slot (blank = first empty slot):"));
        ui.add_sized([80.0, 20.0], egui::TextEdit::singleline(&mut self.equip_target_slot_text));
      });
      let source_index = using[selected].index;
      if right
        .add_enabled(!self.busy, egui::Button::new(tr(language, "Transfer loadout")))
        .clicked()
      {
        match self.apply_equip_loadout_transfer(source_index) {
          Ok(text) => self.log_line(&fill(tr(self.language, "✔ {} (in memory)"), &[&text])),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }

      right.add_space(10.0);
      right.separator();
      right.heading(tr(language, "Single piece transfer"));
      right.horizontal(|ui| {
        ui.label(tr(language, "Source slot:"));
        ui.add_sized([70.0, 20.0], egui::TextEdit::singleline(&mut self.equip_piece_source_text));
        ui.label(tr(language, "Target slot:"));
        ui.add_sized([70.0, 20.0], egui::TextEdit::singleline(&mut self.equip_piece_target_text));
      });
      if right
        .add_enabled(!self.busy, egui::Button::new(tr(language, "Transfer piece (overwrites target slot)")))
        .clicked()
      {
        match self.apply_equip_piece_transfer() {
          Ok(text) => self.log_line(&fill(tr(language, "✔ {} (in memory)"), &[&text])),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      right.label(tr(
        language,
        "Gear is addressed by Equipment Box slot number; make sure the target slot contents are no longer needed.",
      ));
    });
    ui.label(tr(
      language,
      "Every transfer applies in memory at once. When it looks right, write to disk with the buttons below (writing to the original save backs up first).",
    ));
  }

  /// Applies the selected source loadout to the chosen target slot.
  fn apply_equip_loadout_transfer(&mut self, source_index: usize) -> Result<String, String> {
    let target_slot_text = self.equip_target_slot_text.trim().to_owned();
    let target_index = if target_slot_text.is_empty() {
      None
    } else {
      Some(target_slot_text.parse::<usize>().map_err(|_| tr(self.language, "Target slot must be a number"))?)
    };
    {
      let Some(source) = self.source.as_ref() else {
        return Err(tr(self.language, "No source save is open").to_owned());
      };
      let Some(target) = self.target.as_mut() else {
        return Err(tr(self.language, "No target save is open").to_owned());
      };
      let slot = edit::copy_equip_loadout(
        target.document.payload_mut(),
        source.document.payload(),
        source_index,
        target_index,
        self.equip_with_pieces,
      )
      .map_err(|error| error.to_string())?;
      self.dirty = true;
      let pieces = if self.equip_with_pieces {
        tr(self.language, ", referenced gear moved to the same slots")
      } else {
        ""
      };
      Ok(fill(tr(self.language, "Loadout transferred to slot {}{}"), &[&slot, &pieces]))
    }
  }

  /// Applies the single-piece box transfer.
  fn apply_equip_piece_transfer(&mut self) -> Result<String, String> {
    let source_index = self
      .equip_piece_source_text
      .trim()
      .parse::<usize>()
      .map_err(|_| tr(self.language, "Source slot must be a number"))?;
    let target_index = self
      .equip_piece_target_text
      .trim()
      .parse::<usize>()
      .map_err(|_| tr(self.language, "Target slot must be a number"))?;
    {
      let Some(source) = self.source.as_ref() else {
        return Err(tr(self.language, "No source save is open").to_owned());
      };
      let Some(target) = self.target.as_mut() else {
        return Err(tr(self.language, "No target save is open").to_owned());
      };
      edit::copy_equip_piece(target.document.payload_mut(), source.document.payload(), source_index, target_index)
        .map_err(|error| error.to_string())?;
      self.dirty = true;
      Ok(fill(
        tr(self.language, "Gear moved from source slot {} to target slot {}"),
        &[&source_index, &target_index],
      ))
    }
  }

  fn show_transfer_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label(tr(self.language, "Open the target save first (Quick select or Browse above, then Open save) before transferring."));
      return;
    }
    ui.label(tr(self.language, "Merge data from the source save into the target save (same Steam account):"));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
      ui.label(tr(self.language, "Source quick select"));
      let selected = self
        .discovered
        .iter()
        .find(|(path, _, _)| path.display().to_string() == self.source_path)
        .map(|(_, label, _)| label.clone())
        .unwrap_or_else(|| tr(self.language, "(none selected)").to_owned());
      egui::ComboBox::from_id_salt("discovered-sources").selected_text(selected).show_ui(ui, |ui| {
        for (path, label, account) in &self.discovered {
          if ui.selectable_value(&mut self.source_path, path.display().to_string(), label).clicked()
            && let Some(account) = account
          {
            // The save folder name is the 32-bit account id; derive the
            // SteamID64 automatically so users never have to look it up.
            self.steamid64 = steamid64_from_account(*account).to_string();
            let note = format!(
              "{}\n",
              fill(tr(self.language, "SteamID64 filled in from the save path: {}"), &[&self.steamid64])
            );
            self.log.push_str(&note);
          }
        }
      });
      ui.label(tr(self.language, "(no need to transfer if same as target)"));
    });
    Grid::new("transfer").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label(tr(self.language, "Source save"));
      ui.add_sized([420.0, 20.0], egui::TextEdit::singleline(&mut self.source_path));
      if ui.add_enabled(!self.busy, egui::Button::new(tr(self.language, "Browse…"))).clicked()
        && let Some(path) = rfd::FileDialog::new()
          .add_filter(tr(self.language, "MH Rise save"), &["bin"])
          .set_title(tr(self.language, "Choose source save"))
          .pick_file()
      {
        self.source_path = path.display().to_string();
      }
      ui.end_row();
    });
    ui.horizontal(|ui| {
      let steamid = self.parsed_steamid64();
      let open_ready = !self.busy
        && steamid.is_ok()
        && !self.source_path.trim().is_empty()
        && self.source.is_none();
      if ui.add_enabled(open_ready, egui::Button::new(tr(self.language, "Open source save"))).clicked() {
        let task = WorkerTask::Open {
          path: PathBuf::from(self.source_path.trim()),
          steamid64: steamid.expect("checked"),
          is_source: true,
        };
        self.spawn(ui.ctx(), task);
      }
      if self.source.is_some()
        && ui.add_enabled(!self.busy, egui::Button::new(tr(self.language, "Unload source save"))).clicked()
      {
        self.source = None;
        self.log_line(tr(self.language, "Source save unloaded"));
      }
      if let Some(source) = &self.source {
        let text = fill(
          tr(self.language, "Source: {} ({})"),
          &[&source.path.display(), &source.summary(self.language)],
        );
        ui.label(text);
      }
    });

    let language = self.language;
    ui.add_space(6.0);
    ui.separator();
    ui.heading(tr(language, "Item Box transfer"));
    ui.horizontal(|ui| {
      ui.label(tr(language, "Categories taken from source:"));
      for ((label, _), on) in ITEM_CATEGORY_GROUPS.iter().zip(&mut self.xfer_cats) {
        ui.checkbox(on, tr(language, *label));
      }
      if ui.small_button(tr(language, "Select all")).clicked() {
        self.xfer_cats.iter_mut().for_each(|on| *on = true);
      }
      if ui.small_button(tr(language, "Clear")).clicked() {
        self.xfer_cats.iter_mut().for_each(|on| *on = false);
      }
    });
    ui.horizontal(|ui| {
      let both_ready = !self.busy && self.source.is_some();
      let any_cat = self.xfer_cats.iter().any(|on| *on);
      if ui
        .add_enabled(both_ready && any_cat, egui::Button::new(tr(language, "Transfer Item Box (merge by category)")))
        .clicked()
      {
        match self.transfer_items_filtered() {
          Ok(text) => self.log_line(&fill(tr(language, "✔ {} (in memory)"), &[&text])),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label(tr(language, "Only slots in ticked categories are taken from the source; the rest stay as in the target. All ticked = full replacement"));
    });

    ui.add_space(6.0);
    ui.separator();
    ui.heading(tr(language, "Progress transfer"));
    ui.horizontal(|ui| {
      ui.checkbox(&mut self.xfer_progress, tr(language, "Rank progress (HR / MR / Anomaly Research rank + points)"));
    });
    ui.horizontal(|ui| {
      let both_ready = !self.busy && self.source.is_some() && self.xfer_progress;
      if ui.add_enabled(both_ready, egui::Button::new(tr(language, "Transfer rank progress"))).clicked() {
        match self.transfer_progress() {
          Ok(text) => self.log_line(&fill(tr(language, "✔ {} (in memory)"), &[&text])),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label(tr(language, "! If the target has not finished the unlock quests, the game still clamps the transferred ranks to the current cap"));
    });

    ui.add_space(6.0);
    ui.separator();
    ui.heading(tr(language, "Equipment transfer"));
    ui.horizontal(|ui| {
      ui.checkbox(&mut self.xfer_equip_box, tr(language, "Equipment Box (weapons / armor / talismans)"));
      ui.checkbox(&mut self.xfer_equip_manager, tr(language, "Worn gear and loadout registry"));
      ui.checkbox(&mut self.xfer_item_my_set, tr(language, "Item pouch loadouts"));
    });
    ui.horizontal(|ui| {
      let both_ready = !self.busy && self.source.is_some();
      let any_part = self.xfer_equip_box || self.xfer_equip_manager || self.xfer_item_my_set;
      if ui
        .add_enabled(both_ready && any_part, egui::Button::new(tr(language, "Transfer equipment (selected parts)")))
        .clicked()
      {
        match self.transfer_equipment_selected() {
          Ok(text) => self.log_line(&fill(tr(self.language, "✔ {} (in memory)"), &[&text])),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
    });
    if !(self.xfer_equip_box && self.xfer_equip_manager && self.xfer_item_my_set) {
      ui.colored_label(
        self.theme.palette().accent,
        tr(language, "! In a partial transfer, Equipment Box indexes referenced by loadouts or worn gear may point to different gear; check in game"),
      );
    }
    ui.label(tr(language, "Transfers apply in memory at once; when it looks right, write to disk with Save as new file below."));
  }

  fn show_progress_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label(tr(self.language, "Open a save first (Quick select or Browse above)."));
      return;
    }
    let language = self.language;
    ui.heading(tr(language, "Ranks and accumulated points"));
    ui.label(tr(language, "Points are the running progress toward the next rank; once the clamp is lifted the game advances the rank from them. Blank = no change."));
    ui.add_space(4.0);
    Grid::new("progress-points").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
      let row = |ui: &mut egui::Ui, label: &'static str, text: &mut String| {
        ui.label(tr(language, label));
        ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(text));
        ui.end_row();
      };
      row(ui, "Hunter Rank points (HR)", &mut self.hr_point_input);
      row(ui, "Master Rank points (MR)", &mut self.mr_point_input);
      row(ui, "Anomaly Research points", &mut self.mystery_point_input);
    });
    ui.horizontal(|ui| {
      if ui.add_enabled(!self.busy, egui::Button::new(tr(language, "Apply point changes"))).clicked() {
        match self.apply_point_edits() {
          Ok(text) => self.log_line(&fill(tr(self.language, "✔ {} (in memory)"), &[&text])),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label(tr(language, "(points are only accumulated progress and do not change the unlock clamp; 1 ≤ value ≤ 99999999)"));
    });

    ui.add_space(8.0);
    ui.separator();
    ui.heading(tr(language, "Guild card unlock flags (display copy)"));
    ui.horizontal_wrapped(|ui| {
      for ((_, label), on) in edit::GUILD_FLAGS.iter().zip(&mut self.guild_flags) {
        ui.checkbox(on, tr(language, label));
      }
    });
    ui.horizontal(|ui| {
      if ui.add_enabled(!self.busy, egui::Button::new(tr(language, "Apply card flags"))).clicked() {
        let values: Vec<(u32, bool)> = edit::GUILD_FLAGS
          .iter()
          .zip(&self.guild_flags)
          .map(|((hash, _), on)| (*hash, *on))
          .collect();
        match self.apply_guild_flags(&values) {
          Ok(text) => self.log_line(&fill(tr(self.language, "✔ {} (in memory)"), &[&text])),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label(tr(language, "These are display copies on the hunter card; in testing the unlock clamp is not fully decided by them"));
    });

    ui.add_space(8.0);
    ui.separator();
    ui.heading(tr(language, "Why does my rank change not stick?"));
    ui.label(tr(language, "· The game clamps ranks: until the matching unlock quest chain is done, loading shows an achievement popup but the displayed rank is pushed back to the current cap"));
    ui.label(tr(language, "· Unlock state lives in a packed quest-clear bitmap (mapping quests to bits needs the game data tables), which this tool does not rewrite yet"));
    ui.label(tr(language, "· To get a new save to a high rank: finish the unlock quests in game, or use Transfer to move gear / items / rank progress from a high-rank save (unlocks are still bound by quest progress)"));
  }

  /// Applies the three point-accumulator edits from the Progress tab.
  fn apply_point_edits(&mut self) -> Result<String, String> {
    let parse = |text: &str, name: &'static str| -> Result<Option<u32>, String> {
      let trimmed = text.trim();
      if trimmed.is_empty() {
        return Ok(None);
      }
      let value = trimmed.parse::<u32>().map_err(|_| {
        fill(tr(self.language, "{} must be a number: {}"), &[&tr(self.language, name), &format!("{text:?}")])
      })?;
      if value == 0 || value > 99_999_999 {
        return Err(fill(
          tr(self.language, "{} out of range (1-99999999): {}"),
          &[&tr(self.language, name), &value],
        ));
      }
      Ok(Some(value))
    };
    let hr = parse(&self.hr_point_input, "Hunter Rank points")?;
    let mr = parse(&self.mr_point_input, "Master Rank points")?;
    let mystery = parse(&self.mystery_point_input, "Anomaly Research points")?;
    let Some(opened) = self.target.as_mut() else {
      return Err(tr(self.language, "No target save is open").to_owned());
    };
    let mut applied = 0usize;
    for (value, hash, label) in [
      (hr, edit::HUNTER_RANK_POINT, "_HunterRankPoint"),
      (mr, edit::MASTER_RANK_POINT, "_MasterRankPoint"),
      (mystery, edit::MYSTERY_RESEARCH_POINT, "_MysteryResearchPoint"),
    ] {
      if let Some(value) = value
        && value != edit::read_scalar(opened.document.payload(), edit::PROGRESS_SAVE_CLASS, hash)
          .unwrap_or(0)
      {
        edit::set_scalar(opened.document.payload_mut(), edit::PROGRESS_SAVE_CLASS, hash, value, label)
          .map_err(|error| error.to_string())?;
        applied += 1;
      }
    }
    if applied == 0 {
      return Err(tr(self.language, "Nothing to apply").to_owned());
    }
    self.dirty = true;
    Ok(fill(tr(self.language, "Updated {} point values"), &[&applied]))
  }

  /// Copies ProgressSaveData (ranks + point accumulators) from the source.
  fn transfer_progress(&mut self) -> Result<String, String> {
    {
      let Some(source) = self.source.as_ref() else {
        return Err(tr(self.language, "No source save is open").to_owned());
      };
      let Some(target) = self.target.as_mut() else {
        return Err(tr(self.language, "No target save is open").to_owned());
      };
      edit::copy_class_fields(
        target.document.payload_mut(),
        source.document.payload(),
        edit::PROGRESS_SAVE_CLASS,
      )
      .map_err(|error| error.to_string())?;
    }
    self.refresh_target_items();
    self.refresh_progress_inputs();
    let ranks = self
      .target
      .as_ref()
      .and_then(|target| edit::read_ranks(target.document.payload()));
    Ok(fill(
      tr(self.language, "Rank progress transferred; target is now HR {} / MR {} / Anomaly {}"),
      &[
        &ranks.map(|r| r.hunter).unwrap_or(0),
        &ranks.map(|r| r.master).unwrap_or(0),
        &ranks.map(|r| r.mystery_research).unwrap_or(0),
      ],
    ))
  }

  /// Applies guild card unlock flags from the Progress tab.
  fn apply_guild_flags(&mut self, values: &[(u32, bool)]) -> Result<String, String> {
    let Some(opened) = self.target.as_mut() else {
      return Err(tr(self.language, "No target save is open").to_owned());
    };
    let updated = edit::set_guild_flags(opened.document.payload_mut(), values)
      .map_err(|error| error.to_string())?;
    self.dirty = true;
    Ok(fill(tr(self.language, "Updated {} card flags"), &[&updated]))
  }

  fn show_backup_tab(&mut self, ui: &mut egui::Ui) {
    let Some(target_dir) = self.target.as_ref().map(|opened| opened.path.parent().map(Path::to_path_buf)).flatten() else {
      ui.label(tr(self.language, "Open a save first (Quick select or Browse above) so its save folder can be located."));
      return;
    };
    let language = self.language;
    ui.label(fill(tr(language, "Current save folder: {}"), &[&target_dir.display()]));
    ui.label(tr(
      language,
      "A backup copies the whole save folder as-is to Documents\\MHR-Save-Backups\\<account>\\<timestamp>, verified byte by byte and outside Steam Cloud sync;",
    ));
    ui.label(tr(language, "Restore copies the chosen backup over the save folder. Close the game first and do not restore while Steam Cloud is syncing."));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
      if ui.add_enabled(!self.busy, egui::Button::new(tr(language, "Back up whole save folder"))).clicked() {
        self.spawn(ui.ctx(), WorkerTask::Backup { dir: target_dir.clone() });
      }
      let count = backups_for_target(&target_dir).map(|list| list.len()).unwrap_or(0);
      ui.label(fill(tr(language, "{} backups found"), &[&count]));
    });

    ui.add_space(6.0);
    ui.separator();
    ui.heading(tr(language, "Existing backups (newest first)"));
    let backups = backups_for_target(&target_dir).unwrap_or_default();
    if backups.is_empty() {
      ui.label(tr(language, "(no backups yet; back up once before your first edit)"));
      return;
    }
    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
      for backup in &backups {
        ui.horizontal(|ui| {
          ui.label(backup.file_name().and_then(|name| name.to_str()).unwrap_or("?").to_owned());
          let armed = self.restore_confirm.as_deref() == Some(backup.as_path());
          let label = if armed {
            tr(language, "! Click again to confirm restore")
          } else {
            tr(language, "Restore this backup")
          };
          if ui.add_enabled(!self.busy, egui::Button::new(label)).clicked() {
            if armed {
              self.restore_confirm = None;
              self.log_line(&fill(tr(language, "▶ Restoring {} …"), &[&backup.display()]));
              self.spawn(ui.ctx(), WorkerTask::Restore {
                backup: backup.clone(),
                target_dir: target_dir.clone(),
              });
            } else {
              self.restore_confirm = Some(backup.clone());
              self.log_line(tr(language, "! Restore overwrites every file in the save folder; click Restore this backup again to confirm"));
            }
          }
          if armed && ui.small_button(tr(language, "Cancel")).clicked() {
            self.restore_confirm = None;
          }
        });
      }
    });
  }

  fn show_help_tab(&mut self, ui: &mut egui::Ui) {
    let language = self.language;
    ui.label(tr(language, r"Save folder: <Steam library>\userdata\<32-bit account id>\1446780\remote\win64_save"));
    ui.label(tr(language, r"· Steam installs to C:\Program Files (x86)\Steam by default; custom libraries may be on other drives (for example D:\Steam). All Steam libraries are scanned at startup."));
    ui.label(tr(language, "· Character saves: data001Slot.bin ~ data003Slot.bin (up to 3 character slots in game)"));
    ui.label(tr(language, "· System save: data00-1.bin (global data, normally not edited); SS1_* / SS4_* / SS7_* are album data"));
    ui.label(tr(
      language,
      "· Recommended flow: back up → edit → write to save (backs up the whole folder first) → check in game. Save as new file writes <name>.edited.bin that you must swap in by hand",
    ));
    ui.label(tr(language, "· Steam Cloud: go offline in Steam while editing so an older cloud save cannot overwrite it"));
    ui.label(tr(language, "· The character save must already exist (the game will not recognise a file dropped into an empty slot)"));
    ui.add_space(8.0);
    ui.heading(tr(language, "Cannot type Chinese / no candidate window?"));
    ui.label(tr(language, "· This app uses the winit window library, which has a known compatibility problem with TSF-mode input methods (keys swallowed, no candidate list)"));
    ui.label(tr(language, "· Microsoft Pinyin: Settings → Time & language → Language & region → Chinese → Microsoft Pinyin → General → Compatibility → turn on 'Use previous version of Microsoft IME'"));
    ui.label(tr(language, "· Sogou and other third-party IMEs: if they fail too, it is the same compatibility problem; switch to Microsoft Pinyin in compatibility mode. This affects every winit app and has nothing to do with this tool"));
  }

  fn show_save_bar(&mut self, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
      let ready = !self.busy && self.target.is_some();
      if ui.add_enabled(ready, egui::Button::new(tr(self.language, "Write to save (backs up first)"))).clicked() {
        self.save_target_in_place(ui);
      }
      if ui.add_enabled(ready, egui::Button::new(tr(self.language, "Save as new file"))).clicked() {
        self.save_target_to_new_file(ui);
      }
      if self.dirty {
        ui.label(tr(self.language, "● Unsaved changes (in memory)"));
      }
      ui.label(tr(
        self.language,
        "Close the game before writing; Write to save backs up the whole save folder to Documents\\MHR-Save-Backups first",
      ));
    });
  }

  fn box_items_len(&self) -> usize {
    self.target.as_ref().map(|opened| opened.box_items.len()).unwrap_or(0)
  }

  fn visible_box_rows(&self, filter: &str) -> Vec<usize> {
    let Some(opened) = self.target.as_ref() else {
      return Vec::new();
    };
    (0..opened.box_items.len())
      .filter(|&index| {
        let (id, num) = opened.box_items[index];
        if !self.box_show_empty && num == 0 {
          return false;
        }
        if self.box_category != 0 && category_group(id) != self.box_category - 1 {
          return false;
        }
        if filter.is_empty() {
          return true;
        }
        let category = crate::items::item_category(id).unwrap_or("");
        crate::items::item_name_cn(id).unwrap_or("").contains(filter)
          || crate::items::item_name_en(id).unwrap_or("").to_lowercase().contains(filter)
          || category.to_lowercase().contains(filter)
          || crate::items::item_name(id).unwrap_or("").to_lowercase().contains(filter)
          || format!("{id:08x}").contains(filter)
          || format!("{id}").contains(filter)
      })
      .collect()
  }
}

// ---------------------------------------------------------------------------
// Steam save discovery
// ---------------------------------------------------------------------------

/// The backup folders recorded for the target's account, newest first.
/// Mirrors the layout `archive::documents_backup_destination` writes:
/// `Documents\MHR-Save-Backups\<account>\<timestamp>`.
fn backups_for_target(target_save: &Path) -> Option<Vec<PathBuf>> {
  let account = target_save.parent()?.parent()?.parent()?.file_name()?.to_str()?;
  let home = std::env::var_os("USERPROFILE")?;
  let root = PathBuf::from(home).join("Documents").join("MHR-Save-Backups").join(account);
  let mut backups: Vec<PathBuf> = fs::read_dir(&root)
    .ok()?
    .filter_map(|entry| entry.ok())
    .map(|entry| entry.path())
    .filter(|path| path.is_dir())
    .collect();
  backups.sort();
  backups.reverse();
  Some(backups)
}

fn path_key(path: &Path) -> String {
  let text = path.to_string_lossy().replace('/', "\\");
  text.trim_end_matches('\\').to_lowercase()
}

/// Scans every known Steam library for MHR save directories
/// (`userdata/<account>/1446780/remote/win64_save`).
fn discover_save_dirs() -> Vec<PathBuf> {
  let mut roots: Vec<PathBuf> = Vec::new();
  if let Ok(output) = std::process::Command::new("reg")
    .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
    .output()
  {
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
      if let Some(position) = line.find("REG_SZ") {
        let value = line[position + "REG_SZ".len()..].trim();
        if !value.is_empty() {
          roots.push(PathBuf::from(value));
        }
      }
    }
  }
  // Heuristic fallbacks for common install locations; the registry and the
  // library manifest above are the authoritative sources.
  roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
  roots.push(PathBuf::from(r"D:\Steam"));
  roots.push(PathBuf::from(r"E:\Steam"));

  let mut libraries = roots.clone();
  for root in &roots {
    let vdf = root.join("steamapps").join("libraryfolders.vdf");
    let Ok(text) = fs::read_to_string(&vdf) else {
      continue;
    };
    for line in text.lines() {
      let trimmed = line.trim();
      if !trimmed.starts_with("\"path\"") {
        continue;
      }
      let parts: Vec<&str> = trimmed.split('"').collect();
      if parts.len() >= 4 {
        let raw = parts[parts.len() - 2];
        if !raw.is_empty() {
          libraries.push(PathBuf::from(raw.replace("\\\\", "\\")));
        }
      }
    }
  }
  // The registry stores `d:/steam` while the manifest stores `D:\Steam`;
  // deduplicate by normalized key or the same library gets scanned twice.
  libraries.sort_by_key(|path| path_key(path));
  libraries.dedup_by(|first, second| path_key(first) == path_key(second));

  let mut saves = Vec::new();
  for library in &libraries {
    let userdata = library.join("userdata");
    let Ok(accounts) = fs::read_dir(&userdata) else {
      continue;
    };
    for account in accounts.flatten() {
      let dir = account.path().join("1446780").join("remote").join("win64_save");
      if dir.is_dir() {
        saves.push(dir);
      }
    }
  }
  saves.sort_by_key(|path| path_key(path));
  saves.dedup_by(|first, second| path_key(first) == path_key(second));
  saves
}

/// Lists the character slot files inside every discovered save directory.
fn discover_steam_slot_files() -> Vec<(PathBuf, String, Option<u32>)> {
  let mut files = Vec::new();
  let mut seen = std::collections::BTreeSet::new();
  for dir in discover_save_dirs() {
    for name in ["data001Slot.bin", "data002Slot.bin", "data003Slot.bin"] {
      let path = dir.join(name);
      let key = path_key(&path);
      if path.is_file() && seen.insert(key) {
        let account = dir
          .parent()
          .and_then(|remote| remote.parent())
          .and_then(|appid| appid.parent())
          .and_then(|account| account.file_name())
          .map(|name| name.to_string_lossy().to_string())
          .unwrap_or_default();
        files.push((path, format!("Account {account} / {name}"), account.parse::<u32>().ok()));
      }
    }
  }
  files
}

fn derived_output(source: &Path, suffix: &str) -> PathBuf {
  let mut name = source.file_stem().and_then(|stem| stem.to_str()).unwrap_or("save").to_owned();
  name.push('.');
  name.push_str(suffix);
  name.push_str(".bin");
  source.with_file_name(name)
}

/// `userdata\<account>\1446780\...`: the component right after `userdata`
/// (one step before it in the ancestor walk) is the 32-bit account id.
fn account_id_from_path(path: &Path) -> Option<u32> {
  let ancestors: Vec<&Path> = path.ancestors().collect();
  for (index, ancestor) in ancestors.iter().enumerate() {
    if index > 0
      && let Some(name) = ancestor.file_name()
      && name.to_str()?.eq_ignore_ascii_case("userdata")
    {
      return ancestors[index - 1].file_name()?.to_str()?.parse::<u32>().ok();
    }
  }
  None
}

/// SteamID64 = 76561197960265728 + 32-bit account id.
fn steamid64_from_account(account: u32) -> u64 {
  76_561_197_960_265_728 + u64::from(account)
}

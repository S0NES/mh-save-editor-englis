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
  itembox::{self, SortKey},
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

  fn summary(&self) -> String {
    let hunter = if self.hunter_name.is_empty() { "(unknown)" } else { &self.hunter_name };
    format!("{hunter} · Curve {}", self.document.curve_index())
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
  theme: Theme,
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
    self.steamid64.trim().parse::<u64>().map_err(|_| "SteamID64 must be digits only")
  }

  fn spawn(&mut self, context: &egui::Context, task: WorkerTask) {
    let (sender, receiver) = mpsc::channel();
    self.receiver = Some(receiver);
    self.busy = true;
    self.busy_text = task.describe();
    self.log_line(&format!("▶ {}", task.describe()));
    let context = context.clone();
    thread::spawn(move || {
      let event = task.run();
      let _ = sender.send(event);
      context.request_repaint();
    });
  }

  /// Applies the player-data edits (money / points / lifetime counter) that
  /// differ from the opened document. Runs on the UI thread: pure tree walks.
  fn apply_player_edits(&mut self) -> Result<(), String> {
    let Some(opened) = self.target.as_mut() else {
      return Err("No target save is open".to_owned());
    };
    let (current_money, current_total) = edit::read_money(opened.document.payload())
      .ok_or_else(|| "Money field not found (schema mismatch)".to_owned())?;
    let current_points = edit::read_points(opened.document.payload())
      .ok_or_else(|| "Points field not found (schema mismatch)".to_owned())?;

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
      let value = trimmed.parse::<u32>().map_err(|_| format!("Rank must be a number: {text:?}"))?;
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
        return Err(format!("{name} out of range (1-{cap}): {value}"));
      }
    }
    if hunter_rank.is_some() || master_rank.is_some() || mystery_rank.is_some() {
      let current = edit::read_ranks(opened.document.payload())
        .ok_or_else(|| "Rank field not found (schema mismatch)".to_owned())?;
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
      return Err("Nothing to apply (values unchanged or empty)".to_owned());
    }
    // Refresh the input boxes from the document so they show the new values.
    let (money, total) = edit::read_money(opened.document.payload())
      .ok_or_else(|| "Money field not found (schema mismatch)".to_owned())?;
    let points = edit::read_points(opened.document.payload())
      .ok_or_else(|| "Points field not found (schema mismatch)".to_owned())?;
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
      if let Ok(new_value) = text.parse::<u32>()
        && new_value != *num
      {
        changes.push((*id, new_value));
      }
    }
    changes
  }

  fn apply_box_edits(&mut self) -> Result<String, String> {
    let changes = self.pending_item_changes();
    if changes.is_empty() {
      return Err("Nothing to apply (quantities unchanged or empty)".to_owned());
    }
    let Some(opened) = self.target.as_mut() else {
      return Err("No target save is open".to_owned());
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
    Ok(format!("{} updated, {} new items placed in empty slots", report.applied, report.filled))
  }

  /// Merge-transfer only the source item slots whose category group is
  /// enabled; other slots keep the target's contents.
  fn transfer_items_filtered(&mut self) -> Result<String, String> {
    let Some(source) = self.source.as_ref() else {
      return Err("No source save is open".to_owned());
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
      return Err("No target save is open".to_owned());
    };
    let report = edit::transfer_item_box_filtered(target.document.payload_mut(), source.document.payload(), keep)
      .map_err(|error| error.to_string())?;
    self.refresh_target_items();
    Ok(format!(
      "Item Box merged: of {} slots, {} items from source, {} kept from target (source has {} items)",
      report.slots, report.transferred, report.kept, report.source_items
    ))
  }

  /// Transfer only the ticked equipment components.
  fn transfer_equipment_selected(&mut self) -> Result<String, String> {
    let Some(source) = self.source.as_ref() else {
      return Err("No source save is open".to_owned());
    };
    let Some(target) = self.target.as_mut() else {
      return Err("No target save is open".to_owned());
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
    Ok(format!(
      "Equipment transferred: {}; Equipment Box {} slots, {} items{}",
      report
        .classes
        .iter()
        .map(|(label, fields)| format!("{label}={fields} fields"))
        .collect::<Vec<_>>()
        .join(", "),
      report.box_slots,
      report.box_used,
      if partial { " (partial transfer: check worn gear and loadouts in game)" } else { "" }
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
      self.log_line("✘ Output path equals the source file; the source save is never modified");
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
  fn describe(&self) -> String {
    match self {
      WorkerTask::Open { path, is_source, .. } => {
        if *is_source {
          format!("Open source save: {}", path.display())
        } else {
          format!("Open save: {}", path.display())
        }
      }
      WorkerTask::Save { output, .. } => format!("Save to: {}", output.display()),
      WorkerTask::SaveInPlace { backup_dir, .. } => {
        format!("Write to original save (backup first: {})", backup_dir.display())
      }
      WorkerTask::Backup { dir } => format!("Back up folder: {}", dir.display()),
      WorkerTask::Restore { backup, target_dir } => {
        format!("Restore {} to {}", backup.display(), target_dir.display())
      }
    }
  }

  fn run(self) -> WorkerEvent {
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
            format!(
              "Backed up {} files ({} bytes) to {}; every copy verified byte by byte",
              report.files.len(),
              report.total_bytes(),
              report.destination.display()
            )
          })
          .map_err(|error| format!("{error:?}"));
        WorkerEvent::Report(result)
      }
      WorkerTask::Restore { backup, target_dir } => {
        let result = crate::archive::restore_dir(&backup, &target_dir, true)
          .map(|report| {
            format!(
              "Restored {} files ({} bytes) to {}; reopen the save to see it",
              report.files.len(),
              report.total_bytes(),
              target_dir.display()
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
              self.log_line(&format!(
                "! SteamID64 corrected to {} from the save path; try again",
                self.steamid64
              ));
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
              self.log_line(&format!(
                "✔ Opened {}: {} ({})",
                if is_source { "source save" } else { "target save" },
                opened.path.display(),
                opened.summary()
              ));
              if is_source {
                self.source = Some(opened);
              } else {
                self.target = Some(opened);
                self.dirty = false;
                self.reopen_clicked_once = false;
              }
            }
            Err(error) => self.log_line(&format!("✘ Open failed: {error}")),
          }
        }
        Ok(WorkerEvent::Saved(result)) => {
          self.busy = false;
          self.receiver = None;
          match result {
            Ok((output, backup)) => {
              self.dirty = false;
              self.log_line(&format!(
                "✔ Written: {}{}",
                output.display(),
                backup
                  .map(|backup| format!(" (backed up to {})", backup.display()))
                  .unwrap_or_default()
              ));
            }
            Err(error) => self.log_line(&format!("✘ Save failed: {error}")),
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
          self.log_line("✘ Worker thread exited unexpectedly");
        }
      }
    }

    egui::CentralPanel::default().show(ui, |ui| {
      self.show_top_bar(ui);
      ui.add_space(4.0);
      self.show_tab_bar(ui);
      ui.add_space(4.0);
      // Reserve room for the save bar and log below; without an explicit cap
      // the tab content claims every remaining pixel and pushes the save
      // button out of the visible area.
      let bottom_reserve = 180.0;
      let content_height = (ui.available_height() - bottom_reserve).max(120.0);
      egui::ScrollArea::vertical().auto_shrink(false).max_height(content_height).show(ui, |ui| {
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
      self.show_save_bar(ui);
      ui.label("Activity log");
      egui::ScrollArea::vertical().stick_to_bottom(true).max_height(120.0).show(ui, |ui| {
        ui.monospace(if self.log.is_empty() { "(ready)" } else { &self.log });
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

impl GuiApp {
  fn show_top_bar(&mut self, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
      ui.heading("MH Save Editor");
      ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.button(format!("Theme: {}", self.theme.label())).clicked() {
          self.theme = self.theme.toggled();
          ui.ctx().set_visuals(self.theme.visuals());
        }
      });
    });
    ui.horizontal(|ui| {
      ui.label("Game");
      for game in Game::ALL {
        let text = format!("{} · {}", game.name(), game.status());
        if ui.selectable_label(self.game == game, text).clicked() {
          self.game = game;
        }
      }
    });
    if !self.game.supported() {
      ui.colored_label(
        self.theme.palette().danger,
        format!("✘ {} saves are not supported yet. Only Monster Hunter Rise can be edited.", self.game.name()),
      );
    }
    ui.add_space(4.0);
    Grid::new("top-bar").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label("Quick select");
      let selected = self
        .discovered
        .iter()
        .find(|(path, _, _)| path.display().to_string() == self.target_path)
        .map(|(_, label, _)| label.clone())
        .unwrap_or_else(|| "(no saves found)".to_owned());
      egui::ComboBox::from_id_salt("discovered-saves").selected_text(selected).show_ui(ui, |ui| {
        for (path, label, account) in &self.discovered {
          if ui.selectable_value(&mut self.target_path, path.display().to_string(), label).clicked()
            && let Some(account) = account
          {
            // The save folder name is the 32-bit account id; derive the
            // SteamID64 automatically so users never have to look it up.
            self.steamid64 = steamid64_from_account(*account).to_string();
            let note = format!(
              "SteamID64 filled in from the save path: {}
",
              self.steamid64
            );
            self.log.push_str(&note);
          }
        }
      });
      ui.label("SteamID64");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.steamid64));
      ui.end_row();

      ui.label("Target save");
      ui.add_sized([420.0, 20.0], egui::TextEdit::singleline(&mut self.target_path));
      if ui.add_enabled(!self.busy, egui::Button::new("Browse…")).clicked()
        && let Some(path) = rfd::FileDialog::new()
          .add_filter("MH Rise save", &["bin"])
          .set_title("Choose target save")
          .pick_file()
      {
        self.target_path = path.display().to_string();
      }
      let steamid = self.parsed_steamid64();
      let open_label = if self.dirty && !self.reopen_clicked_once {
        "Open save (unsaved changes)"
      } else {
        "Open save"
      };
      if ui
        .add_enabled(
          !self.busy
            && self.game.supported()
            && steamid.is_ok()
            && !self.target_path.trim().is_empty(),
          egui::Button::new(open_label),
        )
        .clicked()
      {
        if self.dirty && !self.reopen_clicked_once {
          self.reopen_clicked_once = true;
          self.log_line("! Unsaved changes: click Open save again to discard them and reopen");
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
      ui.colored_label(self.theme.palette().danger, format!("✘ Last operation failed: {error}"));
    }
    ui.label(match (&self.target, self.busy) {
      (_, true) => format!("⏳ {}…", self.busy_text),
      (Some(opened), _) => format!(
        "Opened: {} ({}){}",
        opened.path.display(),
        opened.summary(),
        if self.dirty { " · ● unsaved changes" } else { "" }
      ),
      (None, _) => "No save open".to_owned(),
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
        ui.selectable_value(&mut self.tab, tab, label);
      }
    });
    ui.separator();
  }

  fn show_info_tab(&mut self, ui: &mut egui::Ui) {
    let Some(opened) = self.target.as_ref() else {
      ui.label("No save open. Choose one with Quick select or Browse above, then click Open save.");
      return;
    };
    Grid::new("info").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label("Hunter name");
      ui.label(if opened.hunter_name.is_empty() {
        "(not found)".to_owned()
      } else {
        opened.hunter_name.clone()
      });
      ui.end_row();
      ui.label("Curve Index");
      ui.label(opened.document.curve_index().to_string());
      ui.end_row();
      ui.label("Decrypted size");
      ui.label(format!("{} bytes", opened.document.original_decrypted().len()));
      ui.end_row();
      ui.label("Item Box");
      ui.label(format!(
        "{} slots, {} non-empty",
        opened.box_items.len(),
        opened.box_items.iter().filter(|(_, num)| *num > 0).count()
      ));
      ui.end_row();
      ui.label("Zenny / Points");
      let (money, total) = edit::read_money(opened.document.payload()).unwrap_or((0, 0));
      let points = edit::read_points(opened.document.payload()).unwrap_or(0);
      ui.label(format!("{money} / {points} ({total} earned in total)"));
      ui.end_row();
      ui.label("Rank (HR / MR / Anomaly)");
      let ranks = edit::read_ranks(opened.document.payload());
      ui.label(match ranks {
        Some(ranks) => format!("{} / {} / {}", ranks.hunter, ranks.master, ranks.mystery_research),
        None => "(not found)".to_owned(),
      });
      ui.end_row();
      ui.label("File");
      ui.label(opened.path.display().to_string());
      ui.end_row();
    });
    if let Ok(data) = fs::read(&opened.path)
      && let Ok(header) = parse_header(&data)
    {
      ui.label(format!(
        "Container: DSSS v{}, flags=0x{:08x}, file {} bytes",
        header.version,
        header.raw_flags,
        data.len()
      ));
    }
  }

  fn show_player_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label("No save open.");
      return;
    }
    Grid::new("player").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label("Hunter name");
      let name = self.target.as_ref().map(|opened| opened.hunter_name.clone()).unwrap_or_default();
      ui.label(if name.is_empty() { "(not found)".to_owned() } else { name });
      ui.end_row();
      ui.label("Zenny");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.money_input));
      ui.end_row();
      ui.label("Total Zenny earned");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.total_input));
      ui.end_row();
      ui.label("Points");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.points_input));
      ui.end_row();
      ui.label("Hunter Rank (HR)");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.hunter_rank_input));
      ui.end_row();
      ui.label("Master Rank (MR)");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.master_rank_input));
      ui.end_row();
      ui.label("Anomaly Research Rank");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.mystery_rank_input));
      ui.end_row();
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
      let steamid = self.parsed_steamid64();
      let ready = !self.busy && steamid.is_ok() && self.target.is_some();
      if ui.add_enabled(ready, egui::Button::new("Apply changes")).clicked() {
        match self.apply_player_edits() {
          Ok(()) => self.log_line("✔ Changes applied (in memory)"),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      if ui
        .add_enabled(!self.busy && self.target.is_some(), egui::Button::new("Write to save (backs up first)"))
        .clicked()
      {
        self.save_target_in_place(ui);
      }
      ui.label("(leave blank = no change; the name is read-only)");
    });
    ui.colored_label(
      self.theme.palette().accent,
      "! Ranks are limited by the unlock system: if the matching unlock quests (Hub / Master Rank / Anomaly Investigation urgent chains) are not complete, the game shows an achievement popup on load but still clamps the rank to the current cap. Unlock state is stored in a quest-clear bitmap that this tool cannot rewrite yet; finish the unlock quests in game, or use Transfer to move over a high-rank save.",
    );
  }

  fn show_itembox_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label("No save open.");
      return;
    }
    ui.horizontal(|ui| {
      ui.add_sized(
        [200.0, 20.0],
        egui::TextEdit::singleline(&mut self.box_filter).hint_text("Search name / ID"),
      );
      let current = if self.box_category == 0 {
        "All categories"
      } else {
        ITEM_CATEGORY_GROUPS[self.box_category - 1].0
      };
      egui::ComboBox::from_id_salt("box-category").selected_text(current).show_ui(ui, |ui| {
        ui.selectable_value(&mut self.box_category, 0, "All categories");
        for (i, (label, _)) in ITEM_CATEGORY_GROUPS.iter().enumerate() {
          ui.selectable_value(&mut self.box_category, i + 1, *label);
        }
      });
      ui.checkbox(&mut self.box_show_empty, "Show empty slots");
      let changes = self.pending_item_changes();
      let apply_text = format!("Apply item changes ({})", changes.len());
      if ui.add_enabled(!self.busy && !changes.is_empty(), egui::Button::new(apply_text)).clicked()
      {
        match self.apply_box_edits() {
          Ok(text) => self.log_line(&format!("✔ Item changes applied: {text} (in memory)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      if ui
        .add_enabled(!self.busy && self.target.is_some(), egui::Button::new("Write to save (backs up first)"))
        .clicked()
      {
        self.save_target_in_place(ui);
      }
      if self.dirty {
        ui.label(egui::RichText::new("● Unsaved changes").color(self.theme.palette().accent));
      }
    });
    if self.box_edits.len() != self.box_items_len() {
      ui.label("(loading…)");
      return;
    }
    let filter = self.box_filter.trim().to_lowercase();
    let rows = self.visible_box_rows(&filter);
    let muted = self.theme.palette().ink_muted;
    let danger = self.theme.palette().danger;
    let selected_count = self.box_selected.len();
    let actionable = !self.busy && selected_count > 0;
    ui.horizontal(|ui| {
      ui.label(format!("{selected_count} selected"));
      if ui.button("Select all shown").clicked() {
        let with_items: Vec<usize> =
          rows.iter().copied().filter(|&index| self.box_slot_has_item(index)).collect();
        self.box_selected.extend(with_items);
      }
      if ui.add_enabled(selected_count > 0, egui::Button::new("Clear selection")).clicked() {
        self.box_selected.clear();
        self.box_last_clicked = None;
      }
      ui.separator();
      ui.label("Set quantity");
      ui.add_sized(
        [60.0, 20.0],
        egui::TextEdit::singleline(&mut self.box_bulk_qty).hint_text("0-9999"),
      );
      if ui.add_enabled(actionable, egui::Button::new("Set")).clicked() {
        let result = self.box_set_quantity_selected();
        self.log_box_result(result);
      }
      let delete_label = egui::RichText::new("Delete selected").color(danger);
      if ui.add_enabled(actionable, egui::Button::new(delete_label)).clicked() {
        let result = self.box_delete_selected();
        self.log_box_result(result);
      }
    });
    ui.horizontal(|ui| {
      ui.label("Move selection to slot");
      ui.add_sized(
        [60.0, 20.0],
        egui::TextEdit::singleline(&mut self.box_move_slot).hint_text("1-1800"),
      );
      if ui.add_enabled(actionable, egui::Button::new("Move")).clicked() {
        let result = self.box_move_selected();
        self.log_box_result(result);
      }
      ui.separator();
      ui.label("Sort box by");
      let current = SortKey::ALL[self.box_sort].label();
      egui::ComboBox::from_id_salt("box-sort").selected_text(current).show_ui(ui, |ui| {
        for (i, key) in SortKey::ALL.iter().enumerate() {
          ui.selectable_value(&mut self.box_sort, i, key.label());
        }
      });
      if ui.add_enabled(!self.busy, egui::Button::new("Sort (empty slots go last)")).clicked() {
        let result = self.box_sort_all();
        self.log_box_result(result);
      }
    });
    ui.horizontal(|ui| {
      let left = egui::Layout::left_to_right(egui::Align::Center);
      let right = egui::Layout::right_to_left(egui::Align::Center);
      let header = |text: &str| egui::RichText::new(text.to_owned()).small().strong().color(muted);
      for (width, text) in [(24.0, ""), (52.0, "Slot"), (240.0, "Name"), (90.0, "Category"), (84.0, "ID")] {
        cell(ui, width, left, |ui| ui.label(header(text)));
      }
      cell(ui, 72.0, right, |ui| ui.label(header("Qty")));
      cell(ui, 90.0, left, |ui| ui.label(header("New qty")));
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
    ui.label(format!(
      "Showing {} of {} slots. Tick rows (shift-click for a range) to edit many at once. \
       Per row: leave New qty blank = no change, 0 = clear the item.",
      rows.len(),
      self.box_items_len()
    ));
  }

  fn box_slot_has_item(&self, index: usize) -> bool {
    self.target.as_ref().is_some_and(|opened| opened.box_items.get(index).is_some_and(|slot| slot.1 > 0))
  }

  fn log_box_result(&mut self, result: Result<String, String>) {
    match result {
      Ok(text) => self.log_line(&format!("✔ {text} (in memory)")),
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
      return Err("No target save is open".to_owned());
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
    let message = self.rewrite_box(|layout| {
      let removed = itembox::delete(layout, &selected);
      Ok(format!("Deleted {removed} slots"))
    })?;
    self.box_selected.clear();
    Ok(message)
  }

  fn box_set_quantity_selected(&mut self) -> Result<String, String> {
    let quantity = self
      .box_bulk_qty
      .trim()
      .parse::<u32>()
      .map_err(|_| "Quantity must be a number".to_owned())?;
    let selected: Vec<usize> = self.box_selected.iter().copied().collect();
    self.rewrite_box(|layout| {
      let changed = itembox::set_quantity(layout, &selected, quantity)?;
      Ok(format!("Set quantity to {quantity} on {changed} slots"))
    })
  }

  fn box_move_selected(&mut self) -> Result<String, String> {
    let slot = self
      .box_move_slot
      .trim()
      .parse::<usize>()
      .map_err(|_| "Target slot must be a number".to_owned())?;
    if slot == 0 {
      return Err("Slots start at 1".to_owned());
    }
    let selected: Vec<usize> = self.box_selected.iter().copied().collect();
    let mut landed = Vec::new();
    let message = self.rewrite_box(|layout| {
      landed = itembox::move_to(layout, &selected, slot - 1)?;
      Ok(format!("Moved {} slots to start at slot {slot}", landed.len()))
    })?;
    self.box_selected = landed.into_iter().collect();
    Ok(message)
  }

  fn box_sort_all(&mut self) -> Result<String, String> {
    let key = SortKey::ALL[self.box_sort];
    let message = self.rewrite_box(|layout| {
      itembox::sort(layout, key);
      Ok(format!("Item Box sorted by {}", key.label()))
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
            ui.label(egui::RichText::new("(empty slot)").italics().color(muted))
          });
        } else {
          let name = crate::items::item_display_name(id);
          let response = cell(ui, 240.0, left, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(name).strong()).truncate())
          });
          if let Some(cn) = crate::items::item_name_cn(id) {
            response.on_hover_text(cn);
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
      ui.label("Open the target save first (Quick select or Browse above).");
      return;
    }
    ui.columns(2, |columns| {
      let mut column_iter = columns.iter_mut();
      let left = column_iter.next().expect("two columns");
      let right = column_iter.next().expect("two columns");
      // Left: the target's own loadout registers.
      left.heading("Target loadouts (224 slots)");
      let loadouts = self
        .target
        .as_ref()
        .map(|opened| edit::read_equip_loadouts(opened.document.payload()))
        .unwrap_or_default();
      let used = loadouts.iter().filter(|loadout| loadout.is_using).count();
      left.label(format!("{used} registered; the Equipment Box slots a loadout references can be moved with it"));
      egui::ScrollArea::vertical().max_height(360.0).auto_shrink(false).show(left, |ui| {
        for loadout in &loadouts {
          ui.horizontal(|ui| {
            ui.monospace(format!("{:3}", loadout.index));
            if loadout.is_using {
              ui.label(&loadout.name);
            } else {
              ui.label(egui::RichText::new("(empty)").weak());
            }
          });
        }
      });

      // Right: transfer controls.
      right.heading("Transfer from source save");
      let Some(source) = self.source.as_ref() else {
        right.label("Open a source save on the Transfer tab first to transfer loadouts or single pieces.");
        return;
      };
      let source_loadouts = edit::read_equip_loadouts(source.document.payload());
      let using: Vec<&edit::EquipLoadout> =
        source_loadouts.iter().filter(|loadout| loadout.is_using).collect();
      if using.is_empty() {
        right.label("The source save has no registered loadouts.");
        return;
      }
      let selected = self
        .equip_source_combo
        .min(using.len().saturating_sub(1));
      let selected_text = format!("[{}] {}", using[selected].index, using[selected].name);
      right.label("Source loadout:");
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
        ui.label("Referenced gear:");
        ui.checkbox(&mut self.equip_with_pieces, "Also move the gear the loadout references (overwrites the same slots)");
      });
      right.horizontal(|ui| {
        ui.label("Target slot (blank = first empty slot):");
        ui.add_sized([80.0, 20.0], egui::TextEdit::singleline(&mut self.equip_target_slot_text));
      });
      let source_index = using[selected].index;
      if right
        .add_enabled(!self.busy, egui::Button::new("Transfer loadout"))
        .clicked()
      {
        match self.apply_equip_loadout_transfer(source_index) {
          Ok(text) => self.log_line(&format!("✔ {text} (in memory)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }

      right.add_space(10.0);
      right.separator();
      right.heading("Single piece transfer");
      right.horizontal(|ui| {
        ui.label("Source slot:");
        ui.add_sized([70.0, 20.0], egui::TextEdit::singleline(&mut self.equip_piece_source_text));
        ui.label("Target slot:");
        ui.add_sized([70.0, 20.0], egui::TextEdit::singleline(&mut self.equip_piece_target_text));
      });
      if right
        .add_enabled(!self.busy, egui::Button::new("Transfer piece (overwrites target slot)"))
        .clicked()
      {
        match self.apply_equip_piece_transfer() {
          Ok(text) => self.log_line(&format!("✔ {text} (in memory)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      right.label("Gear is addressed by Equipment Box slot number; make sure the target slot contents are no longer needed.");
    });
    ui.label("Every transfer applies in memory at once. When it looks right, write to disk with the buttons below (writing to the original save backs up first).");
  }

  /// Applies the selected source loadout to the chosen target slot.
  fn apply_equip_loadout_transfer(&mut self, source_index: usize) -> Result<String, String> {
    let target_slot_text = self.equip_target_slot_text.trim().to_owned();
    let target_index = if target_slot_text.is_empty() {
      None
    } else {
      Some(target_slot_text.parse::<usize>().map_err(|_| "Target slot must be a number")?)
    };
    {
      let Some(source) = self.source.as_ref() else {
        return Err("No source save is open".to_owned());
      };
      let Some(target) = self.target.as_mut() else {
        return Err("No target save is open".to_owned());
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
      let pieces = if self.equip_with_pieces { ", referenced gear moved to the same slots" } else { "" };
      Ok(format!("Loadout transferred to slot {slot}{pieces}"))
    }
  }

  /// Applies the single-piece box transfer.
  fn apply_equip_piece_transfer(&mut self) -> Result<String, String> {
    let source_index = self
      .equip_piece_source_text
      .trim()
      .parse::<usize>()
      .map_err(|_| "Source slot must be a number")?;
    let target_index = self
      .equip_piece_target_text
      .trim()
      .parse::<usize>()
      .map_err(|_| "Target slot must be a number")?;
    {
      let Some(source) = self.source.as_ref() else {
        return Err("No source save is open".to_owned());
      };
      let Some(target) = self.target.as_mut() else {
        return Err("No target save is open".to_owned());
      };
      edit::copy_equip_piece(target.document.payload_mut(), source.document.payload(), source_index, target_index)
        .map_err(|error| error.to_string())?;
      self.dirty = true;
      Ok(format!("Gear moved from source slot {source_index} to target slot {target_index}"))
    }
  }

  fn show_transfer_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label("Open the target save first (Quick select or Browse above, then Open save) before transferring.");
      return;
    }
    ui.label("Merge data from the source save into the target save (same Steam account):");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
      ui.label("Source quick select");
      let selected = self
        .discovered
        .iter()
        .find(|(path, _, _)| path.display().to_string() == self.source_path)
        .map(|(_, label, _)| label.clone())
        .unwrap_or_else(|| "(none selected)".to_owned());
      egui::ComboBox::from_id_salt("discovered-sources").selected_text(selected).show_ui(ui, |ui| {
        for (path, label, account) in &self.discovered {
          if ui.selectable_value(&mut self.source_path, path.display().to_string(), label).clicked()
            && let Some(account) = account
          {
            // The save folder name is the 32-bit account id; derive the
            // SteamID64 automatically so users never have to look it up.
            self.steamid64 = steamid64_from_account(*account).to_string();
            let note = format!("SteamID64 filled in from the save path: {}\n", self.steamid64);
            self.log.push_str(&note);
          }
        }
      });
      ui.label("(no need to transfer if same as target)");
    });
    Grid::new("transfer").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label("Source save");
      ui.add_sized([420.0, 20.0], egui::TextEdit::singleline(&mut self.source_path));
      if ui.add_enabled(!self.busy, egui::Button::new("Browse…")).clicked()
        && let Some(path) = rfd::FileDialog::new()
          .add_filter("MH Rise save", &["bin"])
          .set_title("Choose source save")
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
      if ui.add_enabled(open_ready, egui::Button::new("Open source save")).clicked() {
        let task = WorkerTask::Open {
          path: PathBuf::from(self.source_path.trim()),
          steamid64: steamid.expect("checked"),
          is_source: true,
        };
        self.spawn(ui.ctx(), task);
      }
      if self.source.is_some()
        && ui.add_enabled(!self.busy, egui::Button::new("Unload source save")).clicked()
      {
        self.source = None;
        self.log_line("Source save unloaded");
      }
      if let Some(source) = &self.source {
        ui.label(format!("Source: {} ({})", source.path.display(), source.summary()));
      }
    });

    ui.add_space(6.0);
    ui.separator();
    ui.heading("Item Box transfer");
    ui.horizontal(|ui| {
      ui.label("Categories taken from source:");
      for ((label, _), on) in ITEM_CATEGORY_GROUPS.iter().zip(&mut self.xfer_cats) {
        ui.checkbox(on, *label);
      }
      if ui.small_button("Select all").clicked() {
        self.xfer_cats.iter_mut().for_each(|on| *on = true);
      }
      if ui.small_button("Clear").clicked() {
        self.xfer_cats.iter_mut().for_each(|on| *on = false);
      }
    });
    ui.horizontal(|ui| {
      let both_ready = !self.busy && self.source.is_some();
      let any_cat = self.xfer_cats.iter().any(|on| *on);
      if ui
        .add_enabled(both_ready && any_cat, egui::Button::new("Transfer Item Box (merge by category)"))
        .clicked()
      {
        match self.transfer_items_filtered() {
          Ok(text) => self.log_line(&format!("✔ {text} (in memory)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label("Only slots in ticked categories are taken from the source; the rest stay as in the target. All ticked = full replacement");
    });

    ui.add_space(6.0);
    ui.separator();
    ui.heading("Progress transfer");
    ui.horizontal(|ui| {
      ui.checkbox(&mut self.xfer_progress, "Rank progress (HR / MR / Anomaly Research rank + points)");
    });
    ui.horizontal(|ui| {
      let both_ready = !self.busy && self.source.is_some() && self.xfer_progress;
      if ui.add_enabled(both_ready, egui::Button::new("Transfer rank progress")).clicked() {
        match self.transfer_progress() {
          Ok(text) => self.log_line(&format!("✔ {text} (in memory)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label("! If the target has not finished the unlock quests, the game still clamps the transferred ranks to the current cap");
    });

    ui.add_space(6.0);
    ui.separator();
    ui.heading("Equipment transfer");
    ui.horizontal(|ui| {
      ui.checkbox(&mut self.xfer_equip_box, "Equipment Box (weapons / armor / talismans)");
      ui.checkbox(&mut self.xfer_equip_manager, "Worn gear and loadout registry");
      ui.checkbox(&mut self.xfer_item_my_set, "Item pouch loadouts");
    });
    ui.horizontal(|ui| {
      let both_ready = !self.busy && self.source.is_some();
      let any_part = self.xfer_equip_box || self.xfer_equip_manager || self.xfer_item_my_set;
      if ui
        .add_enabled(both_ready && any_part, egui::Button::new("Transfer equipment (selected parts)"))
        .clicked()
      {
        match self.transfer_equipment_selected() {
          Ok(text) => self.log_line(&format!("✔ {text} (in memory)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
    });
    if !(self.xfer_equip_box && self.xfer_equip_manager && self.xfer_item_my_set) {
      ui.colored_label(
        self.theme.palette().accent,
        "! In a partial transfer, Equipment Box indexes referenced by loadouts or worn gear may point to different gear; check in game",
      );
    }
    ui.label("Transfers apply in memory at once; when it looks right, write to disk with Save as new file below.");
  }

  fn show_progress_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label("Open a save first (Quick select or Browse above).");
      return;
    }
    ui.heading("Ranks and accumulated points");
    ui.label("Points are the running progress toward the next rank; once the clamp is lifted the game advances the rank from them. Blank = no change.");
    ui.add_space(4.0);
    Grid::new("progress-points").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
      let row = |ui: &mut egui::Ui, label: &str, text: &mut String| {
        ui.label(label);
        ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(text));
        ui.end_row();
      };
      row(ui, "Hunter Rank points (HR)", &mut self.hr_point_input);
      row(ui, "Master Rank points (MR)", &mut self.mr_point_input);
      row(ui, "Anomaly Research points", &mut self.mystery_point_input);
    });
    ui.horizontal(|ui| {
      if ui.add_enabled(!self.busy, egui::Button::new("Apply point changes")).clicked() {
        match self.apply_point_edits() {
          Ok(text) => self.log_line(&format!("✔ {text} (in memory)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label("(points are only accumulated progress and do not change the unlock clamp; 1 ≤ value ≤ 99999999)");
    });

    ui.add_space(8.0);
    ui.separator();
    ui.heading("Guild card unlock flags (display copy)");
    ui.horizontal_wrapped(|ui| {
      for ((_, label), on) in edit::GUILD_FLAGS.iter().zip(&mut self.guild_flags) {
        ui.checkbox(on, *label);
      }
    });
    ui.horizontal(|ui| {
      if ui.add_enabled(!self.busy, egui::Button::new("Apply card flags")).clicked() {
        let values: Vec<(u32, bool)> = edit::GUILD_FLAGS
          .iter()
          .zip(&self.guild_flags)
          .map(|((hash, _), on)| (*hash, *on))
          .collect();
        match self.apply_guild_flags(&values) {
          Ok(text) => self.log_line(&format!("✔ {text} (in memory)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label("These are display copies on the hunter card; in testing the unlock clamp is not fully decided by them");
    });

    ui.add_space(8.0);
    ui.separator();
    ui.heading("Why does my rank change not stick?");
    ui.label("· The game clamps ranks: until the matching unlock quest chain is done, loading shows an achievement popup but the displayed rank is pushed back to the current cap");
    ui.label("· Unlock state lives in a packed quest-clear bitmap (mapping quests to bits needs the game data tables), which this tool does not rewrite yet");
    ui.label("· To get a new save to a high rank: finish the unlock quests in game, or use Transfer to move gear / items / rank progress from a high-rank save (unlocks are still bound by quest progress)");
  }

  /// Applies the three point-accumulator edits from the Progress tab.
  fn apply_point_edits(&mut self) -> Result<String, String> {
    let parse = |text: &str, name: &str| -> Result<Option<u32>, String> {
      let trimmed = text.trim();
      if trimmed.is_empty() {
        return Ok(None);
      }
      let value = trimmed.parse::<u32>().map_err(|_| format!("{name} must be a number: {text:?}"))?;
      if value == 0 || value > 99_999_999 {
        return Err(format!("{name} out of range (1-99999999): {value}"));
      }
      Ok(Some(value))
    };
    let hr = parse(&self.hr_point_input, "Hunter Rank points")?;
    let mr = parse(&self.mr_point_input, "Master Rank points")?;
    let mystery = parse(&self.mystery_point_input, "Anomaly Research points")?;
    let Some(opened) = self.target.as_mut() else {
      return Err("No target save is open".to_owned());
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
      return Err("Nothing to apply".to_owned());
    }
    self.dirty = true;
    Ok(format!("Updated {applied} point values"))
  }

  /// Copies ProgressSaveData (ranks + point accumulators) from the source.
  fn transfer_progress(&mut self) -> Result<String, String> {
    {
      let Some(source) = self.source.as_ref() else {
        return Err("No source save is open".to_owned());
      };
      let Some(target) = self.target.as_mut() else {
        return Err("No target save is open".to_owned());
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
    Ok(format!(
      "Rank progress transferred; target is now HR {} / MR {} / Anomaly {}",
      ranks.map(|r| r.hunter).unwrap_or(0),
      ranks.map(|r| r.master).unwrap_or(0),
      ranks.map(|r| r.mystery_research).unwrap_or(0)
    ))
  }

  /// Applies guild card unlock flags from the Progress tab.
  fn apply_guild_flags(&mut self, values: &[(u32, bool)]) -> Result<String, String> {
    let Some(opened) = self.target.as_mut() else {
      return Err("No target save is open".to_owned());
    };
    let updated = edit::set_guild_flags(opened.document.payload_mut(), values)
      .map_err(|error| error.to_string())?;
    self.dirty = true;
    Ok(format!("Updated {updated} card flags"))
  }

  fn show_backup_tab(&mut self, ui: &mut egui::Ui) {
    let Some(target_dir) = self.target.as_ref().map(|opened| opened.path.parent().map(Path::to_path_buf)).flatten() else {
      ui.label("Open a save first (Quick select or Browse above) so its save folder can be located.");
      return;
    };
    ui.label(format!("Current save folder: {}", target_dir.display()));
    ui.label(
      "A backup copies the whole save folder as-is to Documents\\MHR-Save-Backups\\<account>\\<timestamp>, verified byte by byte and outside Steam Cloud sync;",
    );
    ui.label("Restore copies the chosen backup over the save folder. Close the game first and do not restore while Steam Cloud is syncing.");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
      if ui.add_enabled(!self.busy, egui::Button::new("Back up whole save folder")).clicked() {
        self.spawn(ui.ctx(), WorkerTask::Backup { dir: target_dir.clone() });
      }
      ui.label(format!(
        "{} backups found",
        backups_for_target(&target_dir).map(|list| list.len()).unwrap_or(0)
      ));
    });

    ui.add_space(6.0);
    ui.separator();
    ui.heading("Existing backups (newest first)");
    let backups = backups_for_target(&target_dir).unwrap_or_default();
    if backups.is_empty() {
      ui.label("(no backups yet; back up once before your first edit)");
      return;
    }
    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
      for backup in &backups {
        ui.horizontal(|ui| {
          ui.label(backup.file_name().and_then(|name| name.to_str()).unwrap_or("?").to_owned());
          let armed = self.restore_confirm.as_deref() == Some(backup.as_path());
          let label = if armed { "! Click again to confirm restore" } else { "Restore this backup" };
          if ui.add_enabled(!self.busy, egui::Button::new(label)).clicked() {
            if armed {
              self.restore_confirm = None;
              self.log_line(&format!("▶ Restoring {} …", backup.display()));
              self.spawn(ui.ctx(), WorkerTask::Restore {
                backup: backup.clone(),
                target_dir: target_dir.clone(),
              });
            } else {
              self.restore_confirm = Some(backup.clone());
              self.log_line("! Restore overwrites every file in the save folder; click Restore this backup again to confirm");
            }
          }
          if armed && ui.small_button("Cancel").clicked() {
            self.restore_confirm = None;
          }
        });
      }
    });
  }

  fn show_help_tab(&mut self, ui: &mut egui::Ui) {
    ui.label(r"Save folder: <Steam library>\userdata\<32-bit account id>\1446780\remote\win64_save");
    ui.label(r"· Steam installs to C:\Program Files (x86)\Steam by default; custom libraries may be on other drives (for example D:\Steam). All Steam libraries are scanned at startup.");
    ui.label("· Character saves: data001Slot.bin ~ data003Slot.bin (up to 3 character slots in game)");
    ui.label("· System save: data00-1.bin (global data, normally not edited); SS1_* / SS4_* / SS7_* are album data");
    ui.label(
      "· Recommended flow: back up → edit → write to save (backs up the whole folder first) → check in game. Save as new file writes <name>.edited.bin that you must swap in by hand",
    );
    ui.label("· Steam Cloud: go offline in Steam while editing so an older cloud save cannot overwrite it");
    ui.label("· The character save must already exist (the game will not recognise a file dropped into an empty slot)");
    ui.add_space(8.0);
    ui.heading("Cannot type Chinese / no candidate window?");
    ui.label("· This app uses the winit window library, which has a known compatibility problem with TSF-mode input methods (keys swallowed, no candidate list)");
    ui.label("· Microsoft Pinyin: Settings → Time & language → Language & region → Chinese → Microsoft Pinyin → General → Compatibility → turn on 'Use previous version of Microsoft IME'");
    ui.label("· Sogou and other third-party IMEs: if they fail too, it is the same compatibility problem; switch to Microsoft Pinyin in compatibility mode. This affects every winit app and has nothing to do with this tool");
  }

  fn show_save_bar(&mut self, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
      let ready = !self.busy && self.target.is_some();
      if ui.add_enabled(ready, egui::Button::new("Write to save (backs up first)")).clicked() {
        self.save_target_in_place(ui);
      }
      if ui.add_enabled(ready, egui::Button::new("Save as new file")).clicked() {
        self.save_target_to_new_file(ui);
      }
      if self.dirty {
        ui.label("● Unsaved changes (in memory)");
      }
      ui.label("Close the game before writing; Write to save backs up the whole save folder to Documents\\MHR-Save-Backups first");
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

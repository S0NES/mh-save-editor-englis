//! The item view shared by every game. Each item area is a box of tiles, and
//! each tile shows the item's icon, its name and an amount box, like the
//! reference Monster Hunter: World editor. A game only describes its data
//! through [`ItemModel`]; this module does all the drawing.

use std::collections::{BTreeSet, HashMap};

use eframe::egui;

use crate::language::Language;

const TILE_WIDTH: f32 = 104.0;
const TILE_HEIGHT: f32 = 134.0;
/// Edge length of an icon texture, in pixels. Models return icons this size.
pub const ICON_PIXELS: usize = 64;
const ICON_POINTS: f32 = 64.0;
/// Height of the scrolling tile area inside one item box.
const AREA_HEIGHT: f32 = 460.0;

/// One entry of an item area. An id of 0 means the entry is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
  pub id: u32,
  pub amount: u32,
}

impl Item {
  pub const EMPTY: Item = Item { id: 0, amount: 0 };

  pub fn is_empty(self) -> bool {
    self.id == 0
  }
}

/// A named list of entries in a save, such as the item pouch.
pub struct Area {
  pub label: &'static str,
  /// The most one entry can hold, shown as the amount box's upper limit.
  pub max_amount: u32,
}

/// The save-specific data behind the item view.
pub trait ItemModel {
  /// Keys icons and textures apart between games. Use the game's short name.
  fn game(&self) -> &'static str;
  fn areas(&self) -> Vec<Area>;
  fn entries(&self, area: usize) -> Vec<Item>;
  /// Writes one entry. Amount 0 empties it.
  fn set_entry(&mut self, area: usize, index: usize, item: Item) -> Result<(), String>;
  /// The item's display name in the current language.
  fn name(&self, id: u32) -> String;
  /// `ICON_PIXELS` squared RGBA pixels, straight alpha, or `None` for no icon.
  fn icon(&self, _id: u32) -> Option<Vec<u8>> {
    None
  }
}

/// A user action on the item view, applied by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
  /// An amount was edited (amount 0 empties the entry).
  Set { area: usize, index: usize, item: Item },
  /// A filled tile was clicked. Only sent when the view has a selection.
  Pick { area: usize, index: usize },
}

/// How the item view is drawn.
pub struct View<'a> {
  pub language: Language,
  /// Show empty entries as "--------" tiles.
  pub show_empty: bool,
  /// Only filled tiles for which this returns true are shown.
  pub include: &'a dyn Fn(Item) -> bool,
  /// Tiles to highlight, as (area, index). `Some` also turns clicks into
  /// [`Action::Pick`].
  pub selected: Option<&'a BTreeSet<(usize, usize)>>,
  /// The area to draw, by index into `ItemModel::areas`.
  pub area: Option<usize>,
}

/// Decoded icon textures, kept across frames.
#[derive(Default)]
pub struct IconCache {
  textures: HashMap<(&'static str, u32), Option<egui::TextureHandle>>,
}

impl IconCache {
  fn texture(&mut self, ctx: &egui::Context, model: &dyn ItemModel, id: u32) -> Option<egui::TextureId> {
    let key = (model.game(), id);
    if !self.textures.contains_key(&key) {
      let texture = model.icon(id).map(|pixels| {
        let image = egui::ColorImage::from_rgba_unmultiplied([ICON_PIXELS, ICON_PIXELS], &pixels);
        ctx.load_texture(format!("{}-item-{id}", model.game()), image, egui::TextureOptions::LINEAR)
      });
      self.textures.insert(key, texture);
    }
    self.textures.get(&key).and_then(|texture| texture.as_ref().map(|texture| texture.id()))
  }
}

/// Draws the tiles of one area of `model` and returns what the user did. The
/// caller picks the area (with tabs or otherwise) and shows its heading.
pub fn show(
  ui: &mut egui::Ui,
  model: &dyn ItemModel,
  icons: &mut IconCache,
  view: &View,
) -> Vec<Action> {
  let mut actions = Vec::new();
  let gap = ui.spacing().item_spacing;
  let columns = ((ui.available_width() + gap.x) / (TILE_WIDTH + gap.x)).floor().max(1.0) as usize;
  let areas = model.areas();
  let Some((area_index, area)) = areas.iter().enumerate().find(|(index, _)| Some(*index) == view.area) else {
    return actions;
  };
  let entries = model.entries(area_index);
  let shown: Vec<(usize, Item)> = entries
    .iter()
    .copied()
    .enumerate()
    .filter(|&(_, item)| if item.is_empty() { view.show_empty } else { (view.include)(item) })
    .collect();
  let rows = shown.len().div_ceil(columns);
  egui::ScrollArea::vertical()
    .id_salt(("item-area", model.game(), area_index))
    .max_height(AREA_HEIGHT)
    .auto_shrink([false, false])
    .show_rows(ui, TILE_HEIGHT + gap.y, rows, |ui, range| {
      for row in range {
        ui.horizontal(|ui| {
          for &(index, item) in shown.iter().skip(row * columns).take(columns) {
            let tile = Tile { model, view, area: area_index, max_amount: area.max_amount, index, item };
            tile.show(ui, icons, &mut actions);
          }
        });
      }
    });
  actions
}

struct Tile<'a> {
  model: &'a dyn ItemModel,
  view: &'a View<'a>,
  area: usize,
  max_amount: u32,
  index: usize,
  item: Item,
}

impl Tile<'_> {
  fn show(&self, ui: &mut egui::Ui, icons: &mut IconCache, actions: &mut Vec<Action>) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(TILE_WIDTH, TILE_HEIGHT), egui::Sense::click());
    let visuals = ui.visuals();
    let selected = self.view.selected.is_some_and(|set| set.contains(&(self.area, self.index)));
    let background = if selected {
      visuals.selection.bg_fill
    } else if self.item.is_empty() {
      visuals.faint_bg_color
    } else {
      visuals.widgets.inactive.weak_bg_fill
    };
    ui.painter().rect_filled(rect, 6.0, background);
    if response.hovered() && !self.item.is_empty() {
      ui.painter().rect_stroke(rect, 6.0, visuals.widgets.hovered.bg_stroke, egui::StrokeKind::Inside);
    }

    let inner = rect.shrink(6.0);
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
      ui.vertical_centered(|ui| {
        if self.item.is_empty() {
          ui.add_space((ICON_POINTS - 16.0) / 2.0);
          ui.label(egui::RichText::new("--------").weak());
          ui.add_space((ICON_POINTS - 16.0) / 2.0);
        } else {
          match icons.texture(ui.ctx(), self.model, self.item.id) {
            Some(texture) => {
              let image = egui::load::SizedTexture::new(texture, egui::vec2(ICON_POINTS, ICON_POINTS));
              ui.add(egui::Image::from_texture(image));
            }
            None => {
              let initial = self.model.name(self.item.id).chars().next().unwrap_or('?').to_string();
              ui.add_sized([ICON_POINTS, ICON_POINTS], egui::Label::new(egui::RichText::new(initial).size(28.0)));
            }
          }
          ui.add(egui::Label::new(egui::RichText::new(self.model.name(self.item.id)).small()).truncate());
        }
        let mut amount = self.item.amount;
        let edit = egui::DragValue::new(&mut amount).range(0..=self.max_amount).speed(1.0);
        if ui.add_enabled(!self.item.is_empty(), edit).changed() {
          let item = if amount == 0 { Item::EMPTY } else { Item { id: self.item.id, amount } };
          actions.push(Action::Set { area: self.area, index: self.index, item });
        }
      });
    });

    if response.clicked() && !self.item.is_empty() && self.view.selected.is_some() {
      actions.push(Action::Pick { area: self.area, index: self.index });
    }
  }
}

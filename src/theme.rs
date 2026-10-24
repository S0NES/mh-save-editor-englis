//! Colour themes for the GUI. Values come from the MH Save Editor design
//! system (`tokens.json`): Forge (dark) and Ledger (light).

use eframe::egui::{Color32, CornerRadius, Stroke, Visuals};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
  #[default]
  Forge,
  Ledger,
}

/// Semantic colours the UI paints directly (everything else goes through
/// [`Visuals`]).
#[derive(Clone, Copy)]
pub struct Palette {
  pub accent: Color32,
  pub on_accent: Color32,
  pub ink_muted: Color32,
  pub success: Color32,
  pub danger: Color32,
  pub info: Color32,
}

const fn rgb(hex: u32) -> Color32 {
  Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
  pub fn label(self) -> &'static str {
    match self {
      Theme::Forge => "Dark",
      Theme::Ledger => "Light",
    }
  }

  pub fn toggled(self) -> Self {
    match self {
      Theme::Forge => Theme::Ledger,
      Theme::Ledger => Theme::Forge,
    }
  }

  pub fn palette(self) -> Palette {
    match self {
      Theme::Forge => Palette {
        accent: rgb(0xe3a847),
        on_accent: rgb(0x1a1209),
        ink_muted: rgb(0xab9f8c),
        success: rgb(0x4cc3b0),
        danger: rgb(0xf2776a),
        info: rgb(0x7fb6f0),
      },
      Theme::Ledger => Palette {
        accent: rgb(0x8a4b0a),
        on_accent: rgb(0xfff8ec),
        ink_muted: rgb(0x5c5242),
        success: rgb(0x076a5d),
        danger: rgb(0xb3261e),
        info: rgb(0x1f5fa8),
      },
    }
  }

  pub fn visuals(self) -> Visuals {
    let (mut v, surface_base, raised, sunken, overlay, subtle, strong, ink, soft) = match self {
      Theme::Forge => (
        Visuals::dark(),
        rgb(0x181613),
        rgb(0x221f1b),
        rgb(0x121110),
        rgb(0x2d2924),
        rgb(0x38332c),
        rgb(0x877c6a),
        rgb(0xece4d4),
        rgb(0x3b2f18),
      ),
      Theme::Ledger => (
        Visuals::light(),
        rgb(0xf3eee4),
        rgb(0xfbf8f1),
        rgb(0xe9e2d3),
        rgb(0xffffff),
        rgb(0xd8cfbc),
        rgb(0x867a64),
        rgb(0x241e15),
        rgb(0xf3e2bf),
      ),
    };
    let p = self.palette();
    v.panel_fill = surface_base;
    v.window_fill = overlay;
    v.extreme_bg_color = sunken;
    v.faint_bg_color = raised;
    v.override_text_color = Some(ink);
    v.selection.bg_fill = soft;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;
    v.error_fg_color = p.danger;
    v.warn_fg_color = p.accent;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, subtle);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, strong);
    v.widgets.inactive.weak_bg_fill = raised;
    v.widgets.inactive.bg_fill = raised;
    v.widgets.hovered.weak_bg_fill = overlay;
    v.widgets.hovered.bg_fill = overlay;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, strong);
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
      w.corner_radius = CornerRadius::same(6);
    }
    v.window_corner_radius = CornerRadius::same(10);
    v
  }
}

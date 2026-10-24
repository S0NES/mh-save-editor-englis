//! Game profiles. The GUI lets the user pick the game up front; only games
//! whose save format has been reverse-engineered can be opened.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Game {
  #[default]
  Rise,
  World,
  Wilds,
}

impl Game {
  pub const ALL: [Game; 3] = [Game::Rise, Game::World, Game::Wilds];

  pub fn name(self) -> &'static str {
    match self {
      Game::Rise => "Monster Hunter Rise",
      Game::World => "Monster Hunter: World",
      Game::Wilds => "Monster Hunter Wilds",
    }
  }

  /// Whether the editor can read and write this game's saves.
  pub fn supported(self) -> bool {
    matches!(self, Game::Rise)
  }

  /// One-line status shown under the game name in the selector.
  pub fn status(self) -> &'static str {
    if self.supported() { "Editable" } else { "Not supported yet" }
  }

  /// Embedded 640x360 transparent PNG logo (Capcom trademark, used only to
  /// label the game in the selector).
  pub fn logo_bytes(self) -> &'static [u8] {
    match self {
      Game::Rise => include_bytes!("../assets/games/rise.png"),
      Game::World => include_bytes!("../assets/games/world.png"),
      Game::Wilds => include_bytes!("../assets/games/wilds.png"),
    }
  }

  /// Steam app id, used to find the save folder under `userdata\<account>\`.
  pub fn steam_app_id(self) -> u32 {
    match self {
      Game::Rise => 1_446_780,
      Game::World => 582_010,
      Game::Wilds => 2_246_340,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn every_logo_decodes_to_a_non_empty_image() {
    for game in Game::ALL {
      let image = image::load_from_memory_with_format(game.logo_bytes(), image::ImageFormat::Png)
        .unwrap_or_else(|error| panic!("{} logo does not decode: {error}", game.name()));
      assert!(image.width() > 0 && image.height() > 0, "{} logo is empty", game.name());
    }
  }
}

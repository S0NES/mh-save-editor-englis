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

  /// Steam app id, used to find the save folder under `userdata\<account>\`.
  pub fn steam_app_id(self) -> u32 {
    match self {
      Game::Rise => 1_446_780,
      Game::World => 582_010,
      Game::Wilds => 2_246_340,
    }
  }
}

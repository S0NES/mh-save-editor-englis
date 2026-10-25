//! Display language for item names. Chinese is the official Rise localization
//! the editor started with; English is the best-effort translation added on top.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
  Chinese,
  #[default]
  English,
}

impl Language {
  pub const ALL: [Language; 2] = [Language::English, Language::Chinese];

  /// Name of the language as shown in the selector, in that language.
  pub fn label(self) -> &'static str {
    match self {
      Language::Chinese => "中文",
      Language::English => "English",
    }
  }
}

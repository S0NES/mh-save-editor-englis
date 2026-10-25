//! Monster Hunter: World (Iceborne) saves. Unlike Rise, these are flat binary
//! structs rather than RSZ class streams, so they get their own document layer.

pub mod crypto;
pub mod save;
mod tables;

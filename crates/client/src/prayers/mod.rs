//! Built-in Prayer Times workspace: solar timetable, icon-only daily prayer log,
//! week Day Inspector, monthly exportable schedule, and Qibla compass.

pub mod api;
pub mod calc;
pub mod cities;
pub mod export;
pub mod model;
pub mod month;
pub mod qibla;
pub mod settings;
pub mod setup;
pub mod sidebar;
pub mod today;
pub mod tz;
pub mod week;
pub mod workspace;

pub use workspace::PrayersWorkspace;

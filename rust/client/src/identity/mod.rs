//! Dar's Settings and Identity pages (`identity.tsx`, `account.tsx`, `profile.tsx`,
//! `regional.tsx`, `appearance.tsx`, `instance-appearance.tsx`,
//! `installed-app-settings.tsx`, `settings-overview.tsx`).

pub mod account;
pub mod appearance;
pub mod installed_app_settings;
pub mod instance_appearance;
pub mod page;
pub mod profile;
pub mod regional;
pub mod settings_overview;

pub use appearance::use_appearance;
pub use page::{IdentityInteraction, IdentitySettings};

//! Chat: WhatsApp and Discord accounts, chats, servers, channels, voice calls,
//! formatting, media lightbox, voice notes, inspector, onboarding, and settings.

pub mod conversation;
pub mod inspector;
pub mod model;
pub mod onboarding;
pub mod panes;
pub mod settings;
pub mod sidebar;
pub mod thread_list;
pub mod voice;
pub mod workspace;

pub use model::*;
pub use workspace::ChatWorkspace;

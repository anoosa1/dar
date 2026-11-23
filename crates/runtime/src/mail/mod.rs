//! Mail: administrator-configured providers; accounts over IMAP and SMTP or JMAP, signed in with
//! passwords, tokens or OAuth2; every message mirrored as an `.eml` file in Files, with changes
//! made there carried back to the server.

pub mod http;
pub mod imap;
pub mod jmap;
pub mod markdown;
pub mod message;
pub mod net;
pub mod runtime;
pub mod smtp;

pub use http::handle_http;
pub use runtime::MailRuntime;

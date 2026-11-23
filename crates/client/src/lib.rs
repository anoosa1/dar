//! Rust (Leptos) port of Dar's browser client (`next/client`).
//!
//! Markup and class names follow the React components exactly, so Dar's own stylesheet
//! (concatenated from `styles/` in the order of `styles/order.txt` by `cargo xtask build`) gives the same look.
//! Every app is ported: the shell, sign-in, Files, Contacts, Calendar, Notes, Tasks, News,
//! Chat, Photos (and shared links), Books, Git, Agent, installed apps, and Settings/Identity.

pub mod agent;
pub mod api;
pub mod books;
pub mod chat;
pub mod installed_apps;
pub mod lazy_apps;
pub mod mail;
pub mod news;
pub mod photos;
pub mod workspace;
pub mod app;
pub mod date_popover;
pub mod dates;
pub mod files;
pub mod git;
pub mod history;
pub mod icons;
pub mod identity;
pub mod live;
pub mod markdown;
pub mod outline;
/// The module's generated bindings (crates/bindings).
pub use dar_bindings as module_bindings;
pub mod navigation;
pub mod pim;
pub mod settings;
pub mod shared;
pub mod shell;
pub mod ui;


use wasm_bindgen::JsCast;

/// A random v4 UUID from the browser (`crypto.randomUUID()`).
pub fn random_uuid() -> String {
    let crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).expect("crypto");
    let function: js_sys::Function = js_sys::Reflect::get(&crypto, &"randomUUID".into()).expect("randomUUID").unchecked_into();
    function.call0(&crypto).ok().and_then(|v| v.as_string()).expect("uuid")
}

/// Resolves after `ms` milliseconds.
pub async fn sleep(ms: u32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let _ = web_sys::window().expect("window").set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// A 32-byte base64url secret and the hex SHA-256 of its text (public download links).
pub async fn secret_and_digest() -> Result<(String, String), String> {
    let crypto: web_sys::Crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).map_err(|_| "crypto")?.unchecked_into();
    let mut bytes = [0u8; 32];
    crypto.get_random_values_with_u8_array(&mut bytes).map_err(|_| "Randomness is unavailable.")?;
    let token = base64url(&bytes);
    let data = js_sys::Uint8Array::from(token.as_bytes());
    let promise = crypto.subtle().digest_with_str_and_buffer_source("SHA-256", &data).map_err(|_| "Hashing is unavailable.")?;
    let digest = wasm_bindgen_futures::JsFuture::from(promise).await.map_err(|_| "Hashing is unavailable.")?;
    let digest = js_sys::Uint8Array::new(&digest).to_vec();
    Ok((token, digest.iter().map(|b| format!("{b:02x}")).collect()))
}

fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Mounts the app on `#root`; called by the binary's `main`.
pub fn start() {
    console_error_panic_hook::set_once();
    let root = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id("root")).expect("#root");
    root.set_inner_html("");
    leptos::mount::mount_to(root.unchecked_into(), app::App).forget();
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64url_without_padding() {
        assert_eq!(super::base64url(b"hello"), "aGVsbG8");
        assert_eq!(super::base64url(&[0xfb, 0xff]), "-_8");
    }
}

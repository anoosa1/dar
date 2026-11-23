//! The browser entry point. A binary rather than a `cdylib`: a cdylib exports every
//! `#[no_mangle]` symbol of its dependencies (the SDK's brotli C API, which drags in the whole
//! encoder), while a binary keeps only what it uses.
fn main() {
    dar_client::start();
}

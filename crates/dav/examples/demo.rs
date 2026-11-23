//! Serves the DAV port on in-memory backends: `cargo run -p dar-dav --example demo -- 127.0.0.1:8085`.
//! Account `amina`, app password printed at start. Nothing is persisted.

use std::sync::Arc;

use dar_dav::memory::{MemoryAccounts, MemoryFiles, MemoryPim};
use dar_dav::pim::PimKind;
use dar_dav::{App, Dav};

#[tokio::main]
async fn main() {
    let address = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:8085".into());
    let listener = tokio::net::TcpListener::bind(&address).await.expect("bind");
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let secret = "demoDemoDemoDemoDemoDemoDemoDemoDemoDemo123";
    let accounts = Arc::new(MemoryAccounts::default());
    accounts.add("amina", secret, &[App::Files, App::Contacts, App::Calendar], true);
    let pim = MemoryPim::new(accounts.clone());
    pim.add_collection("amina", "personal", PimKind::Calendar, "Personal");
    pim.add_collection("amina", "contacts", PimKind::Contacts, "Contacts");
    let dav = Dav { origin: origin.clone(), accounts: accounts.clone(), files: Arc::new(MemoryFiles::new(accounts.clone())), pim: Arc::new(pim) };
    println!("{origin}/dav/  user amina  password {secret}");
    axum::serve(listener, dar_dav::router(dav)).await.unwrap();
}

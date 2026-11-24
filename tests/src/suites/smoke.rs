//! The harness itself: an instance, a session, an upload, a live connection and the browser.

use dar_acceptance::prelude::*;

pub const SUITE: Suite = Suite {
    name: "smoke",
    cases: &[case!(sign_in_and_upload, 120)],
};

pub async fn sign_in_and_upload(t: &Ctx) -> Result<()> {
    let instance = t.fixture().await?;
    let session = login(&instance.base).await?;
    let uploaded = upload(&session, "smoke.txt", "hello").await?;
    let db = session.connect().await?;
    ensure!(
        db.db
            .my_files()
            .iter()
            .any(|row| row.id == uploaded.file_id),
        "the upload is not in my_files"
    );
    let browser = Browser::shared().await?;
    let context = browser
        .new_context(ContextOptions::sized(1280, 800))
        .await?;
    let page = context.new_page().await?;
    let errors = page.collect_errors(|text| text.contains("401"));
    ui::sign_in(&page, &instance.base, "admin", PASSWORD).await?;
    page.get_by_role("button", "smoke.txt").wait_for().await?;
    page.screenshot(t.screenshot("files.png"), true).await?;
    context.close().await?;
    ensure!(
        errors.lock().unwrap().is_empty(),
        "browser errors: {:?}",
        errors.lock().unwrap()
    );
    Ok(())
}

//! Appearance & scope matrix browser journey (`appearance-scopes-browser.mjs`): custom sign-in
//! logo, persisted admin appearance, logo reset, and multi-app scope matrix at desktop/phone sizes.

use dar_acceptance::prelude::*;

pub const SUITE: Suite = Suite {
    name: "appearance-scopes-browser",
    cases: &[case!(appearance_and_scopes_browser, 120)],
};

const LOGO_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aX1sAAAAASUVORK5CYII=";

pub async fn appearance_and_scopes_browser(t: &Ctx) -> Result<()> {
    let base = t.fixture().await?.base.clone();
    let png = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(LOGO_B64)
            .map_err(|e| Error::new(e.to_string()))?
    };

    let browser = Browser::shared().await?;
    let context = browser.new_context_sized(1440, 1000).await?;
    let anonymous = browser.new_context(ContextOptions::default()).await?;

    let result = async {
        let page = context.new_page().await?;
        let errors = page.collect_errors(|text| text.contains("401") && text.contains("Unauthorized"));

        page.goto(&base).await?;
        page.locator(".signin-brand .brand").wait_for().await?;
        page.get_by_label("Username").fill("admin").await?;
        page.get_by_label("Password").fill(PASSWORD).await?;
        page.get_by_role("button", "Sign in").click().await?;
        page.get_by_role("button", "Open account menu").wait_for().await?;

        page.goto(&format!("{base}/settings?section=instanceAppearance")).await?;
        page.get_by_label("Upload logo")
            .set_input_files(vec![FilePayload::new("logo.png", "image/png", png)])
            .await?;
        page.get_by_role("button", "Save appearance").click().await?;
        page.get_by_role("status", ()).filter_has_text("Appearance saved").wait_for().await?;
        page.reload().await?;
        page.locator(r#".signin-logo-preview img[src^="/api/v1/appearance/logo"]"#).wait_for().await?;

        let login_page = anonymous.new_page().await?;
        login_page.goto(&base).await?;
        login_page.locator(".signin-brand img").wait_for().await?;
        login_page
            .wait_for_function(r#"() => { const img = document.querySelector(".signin-brand img"); return Boolean(img?.complete && img.naturalWidth > 0); }"#)
            .await?;
        login_page.screenshot(t.screenshot("signin-logo-desktop.png"), false).await?;
        login_page.set_viewport_size(390, 844).await?;
        assert_eq!(login_page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));

        page.goto(&format!("{base}/settings?section=security")).await?;
        page.get_by_label("Device or application name").fill("Browser mixed scopes").await?;
        page.get_by_role("checkbox", "Files read").check().await?;
        page.get_by_role("checkbox", "Contacts read").check().await?;
        page.get_by_role("checkbox", "Contacts write").check().await?;
        page.get_by_role("checkbox", "Calendar write").check().await?;
        page.get_by_role("button", "Create app password").click().await?;
        page.get_by_label("App password").wait_for().await?;
        page.get_by_role("button", "I saved it").click().await?;
        page.reload().await?;
        page.get_by_text(contains("Files: Read · Contacts: Read and write · Calendar: Write")).wait_for().await?;

        page.set_viewport_size(390, 844).await?;
        assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
        page.screenshot(t.screenshot("app-scopes-phone.png"), true).await?;

        page.goto(&format!("{base}/settings?section=instanceAppearance")).await?;
        page.get_by_role("button", "Use Dar logo").click().await?;
        page.get_by_role("button", "Save appearance").click().await?;
        page.get_by_role("status", ()).filter_has_text("Appearance saved").wait_for().await?;

        login_page.reload().await?;
        login_page.locator(".signin-brand .brand").wait_for().await?;

        let collected = errors.lock().unwrap().clone();
        assert!(collected.is_empty(), "browser errors: {collected:?}");
        Ok::<(), Error>(())
    }
    .await;

    anonymous.close().await.ok();
    context.close().await.ok();
    result
}

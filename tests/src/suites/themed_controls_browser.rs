//! Shared CSS control styling (`themed-controls-browser.mjs`): keyboard selection, mixed and
//! disabled states, form values, switches, scrolling, Latte/Mocha themes, phone width, and
//! forced-colors mode, exercised directly against the built `app.css` without a service.

use dar_acceptance::browser::{Browser, ContextOptions};
use dar_acceptance::prelude::*;
use dar_acceptance::stack::stack;
use std::sync::Arc;

pub const SUITE: Suite = Suite {
    name: "themed-controls-browser",
    cases: &[case!(themed_controls, 60)],
};

pub async fn themed_controls(t: &Ctx) -> Result<()> {
    let css = dar_acceptance::read_to_string(stack().artifacts.client.join("app.css"))?;
    let browser = Arc::new(Browser::launch_with(false)?);
    let context = browser.new_context(ContextOptions::default()).await?;

    let result = async {
        let page = context.new_page().await?;
        let html = format!(
            r#"<style>{css}
 .samples{{padding:20px;max-width:600px}}.samples form{{display:grid;gap:16px}}.samples label{{display:flex;flex-direction:row;align-items:center;margin:0}}
 .scroll-sample{{height:120px;overflow:scroll;border:1px solid var(--border);border-radius:12px;padding:12px}}.scroll-sample p{{height:240px;margin:0}}
 </style><main class="samples"><h1>Quiet Twilight</h1><form>
 <label><input type="checkbox" name="selected" value="yes">Select file</label>
 <label><input type="checkbox" id="mixed">Some files selected</label>
 <label><input type="checkbox" disabled checked>Unavailable</label>
 <label><input type="radio" name="sort" value="name" checked>Name</label>
 <label><input type="radio" name="sort" value="date">Modified</label>
 <label class="ui-switch">Hidden files<input type="checkbox" role="switch"></label>
 <input type="file" aria-label="Import file"><input type="date" aria-label="Date" value="2026-11-08">
 <div class="scroll-sample" tabindex="0" aria-label="Scrollable files"><p>Notes.md<br>Planning.txt<br>Archive<br>Photos</p></div>
 </form></main>"#
        );
        page.set_content(&html).await?;
        page.locator("#mixed").evaluate("el => { el.indeterminate = true; }", Value::Null).await?;

        let selected = page.get_by_role("checkbox", "Select file");
        selected.focus().await?;
        page.keyboard().press("Space").await?;
        assert!(selected.is_checked().await?);

        page.get_by_role("radio", "Name").focus().await?;
        page.keyboard().press("ArrowRight").await?;
        assert!(page.get_by_role("radio", "Modified").is_checked().await?);

        let form_data = page.locator("form").evaluate("el => Object.fromEntries(new FormData(el))", Value::Null).await?;
        assert_eq!(form_data, json!({ "selected": "yes", "sort": "date" }));
        assert!(page.get_by_role("checkbox", "Unavailable").is_disabled().await?);
        assert_eq!(page.locator("#mixed").evaluate("el => el.indeterminate", Value::Null).await?, json!(true));
        page.get_by_role("switch", "Hidden files").check().await?;

        for theme in ["latte", "mocha"] {
            page.evaluate_with("(theme) => { document.documentElement.dataset.theme = theme; }", json!(theme)).await?;
            for width in [1100u32, 390] {
                page.set_viewport_size(width, 800).await?;
                assert_eq!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await?, json!(false));
                let scroll = page.get_by_label("Scrollable files");
                scroll.focus().await?;
                page.keyboard().press("ArrowDown").await?;
                page.wait_for_function("() => document.querySelector('.scroll-sample').scrollTop > 0").await?;
                page.screenshot(t.screenshot(&format!("controls-{theme}-{width}.png")), false).await?;
            }
        }

        page.emulate_media(None, None, Some("active")).await?;
        assert_eq!(selected.evaluate("el => getComputedStyle(el).appearance", Value::Null).await?, json!("auto"));
        selected.focus().await?;
        page.keyboard().press("Space").await?;
        assert!(!selected.is_checked().await?);
        page.screenshot(t.screenshot("controls-forced-colors.png"), false).await?;
        Ok::<(), Error>(())
    }
    .await;

    context.close().await.ok();
    result
}

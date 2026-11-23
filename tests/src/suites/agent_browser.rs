//! Port of `test/agent-browser.mjs`.

use crate::suites::agent_git_support::{sign_in_at, url_param};
use dar_acceptance::browser::{Fulfill, State, contains, regex};
use dar_acceptance::prelude::*;
use dar_acceptance::ui;
use dar_bindings::create_folder_reducer::create_folder;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SUITE: Suite = Suite {
    name: "agent-browser",
    cases: &[case!(agent_browser_flow)],
};

async fn agent_browser_flow(ctx: &Ctx) -> Result<()> {
    let instance = ctx.fixture().await?;
    let base = instance.base.clone();
    let root = ctx.dir().to_path_buf();

    // Isolated fixtures start with an empty Files tree; seed a folder for the Dar folder picker.
    let admin = login(&base).await?;
    let conn = admin.connect().await?;
    let folder_res = live::reducer(|done| {
        conn.reducers
            .create_folder_then(uuid(), String::new(), format!("Agent files {}", &uuid()[..6]), done)
    })
    .await;
    conn.disconnect();
    folder_res?;

    let browser = Browser::shared().await?;
    let context = browser.new_context_sized(1440, 950).await?;
    let page = context.new_page().await?;
    let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let errors = errors.clone();
        page.on_page_error(move |msg| {
            errors.lock().unwrap().push(msg.to_owned());
        });
    }

    let result: Result<()> = async {
        sign_in_at(&page, &base, "admin", PASSWORD).await?;
        page.get_by_role("navigation", "Applications").get_by_role("link", "Agent").click().await?;
        page.get_by_role("heading", "Agent").first().wait_for().await?;

        let marker = format!("browser-project-{}", &uuid()[..6]);
        page.get_by_role("button", "New thread").first().click().await?;
        let change_project = page.get_by_role("button", regex("^Project: .*Change project$"));
        let source_field = page.get_by_role("combobox", "Project source");
        source_field.or(&change_project).first().wait_for().await?;
        if change_project.is_visible().await? {
            ensure!(url_param(&page.url(), "thread").is_none());
            change_project.click().await?;
            page.get_by_role("complementary", "Choose project").get_by_role("button", "Add project").click().await?;
        }
        ui::choose(&page.get_by_role("combobox", "Project source"), ui::Choice::Value("git")).await?;
        page.get_by_label("Repository URL").fill(&format!("https://example.test/you/{marker}.git")).await?;
        page.locator("[data-editor-pane]").get_by_role("button", "Add project").click().await?;
        page.get_by_label("Message").wait_for().await?;
        page.get_by_role("button", format!("Project: {marker}. Change project")).wait_for().await?;
        ensure!(url_param(&page.url(), "thread").is_none(), "A draft must not create a thread before submit.");
        ensure!(page.get_by_label("Thread title").count().await? == 0);
        ensure!(page.get_by_label("Project name").count().await? == 0);

        let message = "Build keyboard navigation.\n<script>window.agentInjected = true</script>";
        let draft = "A follow-up draft";
        page.get_by_label("Message").fill(message).await?;
        page.get_by_role("button", "Save message").click().await?;
        page.get_by_text("Message saved. It has not been sent to a model.").wait_for().await?;
        ensure!(page.evaluate("() => window.agentInjected === undefined").await? == json!(true));

        page.get_by_label("Message").fill(draft).await?;
        let deep = page.url();
        page.reload().await?;
        page.get_by_label("Conversation messages").get_by_text(message).wait_for().await?;
        ensure!(page.get_by_label("Message").input_value().await? == draft);

        let session = page.request().get(&format!("{base}/api/v1/auth/session")).await?.json()?;
        let csrf = session["csrfToken"].as_str().context("csrfToken")?.to_owned();
        let thread_id = url_param(&deep, "thread").context("thread param")?;
        let project_id = url_param(&deep, "project").context("project param")?;
        let history = page.request().get(&format!("{base}/api/v1/agent/threads/{thread_id}/messages")).await?.json()?;
        let last_msg = history["items"].as_array().and_then(|items| items.last()).context("last history item")?;
        ensure!(last_msg["content"] == message);
        let thread_title = history["thread"]["title"].as_str().context("thread title")?.to_owned();
        let history_rev = history["thread"]["revision"].as_u64().context("thread revision")?;

        let remote = page
            .request()
            .post(
                &format!("{base}/api/v1/agent/threads/{thread_id}/messages"),
                &[
                    ("origin", base.as_str()),
                    ("x-csrf-token", csrf.as_str()),
                    ("if-match", &format!("\"{history_rev}\"")),
                ],
                json!({ "id": uuid(), "content": "From another window" }),
            )
            .await?;
        ensure!(remote.status == 201, "remote post failed: {} {}", remote.status, remote.text());

        page.get_by_role("button", "Save message").click().await?;
        page.get_by_text(regex("This thread changed in another window")).wait_for().await?;
        ensure!(page.get_by_label("Message").input_value().await? == draft);
        page.get_by_role("button", "Refresh conversation").click().await?;
        page.get_by_text("From another window").wait_for().await?;
        page.get_by_role("button", "Save message").click().await?;
        page.get_by_text("Message saved. It has not been sent to a model.").wait_for().await?;

        let uncertain = "Saved while the acknowledgement was lost";
        let endpoint = format!("{base}/api/v1/agent/threads/{thread_id}/messages");
        page.route(endpoint.clone(), None, |route| async move {
            if route.request().method == "POST" {
                let _ = route.fetch().await?;
                route.fulfill(Fulfill::json(503, &json!({ "error": { "message": "Acknowledgement lost" } }))).await
            } else {
                route.continue_().await
            }
        })
        .await?;
        page.get_by_label("Message").fill(uncertain).await?;
        page.get_by_role("button", "Save message").click().await?;
        page.get_by_text("Acknowledgement lost").wait_for().await?;
        page.unroute_all().await?;

        page.reload().await?;
        page.get_by_label("Conversation messages").get_by_text(uncertain).wait_for().await?;
        ensure!(page.get_by_label("Message").input_value().await? == uncertain);
        page.get_by_role("button", "Save message").click().await?;
        page.get_by_text("Message saved. It has not been sent to a model.").wait_for().await?;
        let replayed = page.request().get(&endpoint).await?.json()?;
        let uncertain_count = replayed["items"]
            .as_array()
            .map(|items| items.iter().filter(|m| m["content"] == uncertain).count())
            .unwrap_or(0);
        ensure!(uncertain_count == 1, "expected 1 replayed message, got {uncertain_count}");

        page.set_viewport_size(390, 844).await?;
        page.get_by_role("button", "Changes").click().await?;
        page.locator(".agent-review").wait_for_state(State::Visible).await?;
        page.keyboard().press("Escape").await?;
        page.locator(".agent-conversation").wait_for_state(State::Visible).await?;
        ensure!(
            page.get_by_role("button", "Conversation")
                .evaluate("el => document.activeElement === el", Value::Null)
                .await?
                == json!(true)
        );

        for theme in ["latte", "mocha"] {
            page.evaluate_with("value => { document.documentElement.dataset.theme = value; }", json!(theme)).await?;
            for (width, height) in [(1440u32, 950u32), (900, 760), (801, 700), (390, 844), (320, 568)] {
                page.set_viewport_size(width, height).await?;
                if width <= 800 {
                    for (name, selector) in [
                        ("Details", ".agent-context"),
                        ("Changes", ".agent-review"),
                        ("Terminal", ".agent-terminal"),
                        ("Conversation", ".agent-conversation"),
                    ] {
                        page.get_by_role("button", name).click().await?;
                        page.locator(selector).wait_for_state(State::Visible).await?;
                        ensure!(
                            page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await? == json!(false),
                            "overflow on {theme} {width} panel {name}"
                        );
                    }
                    page.get_by_role("button", "Back to threads").click().await?;
                    page.locator(".agent-sidebar").wait_for_state(State::Visible).await?;
                    page.get_by_role("navigation", "Applications").wait_for().await?;
                    ensure!(page.evaluate("() => document.documentElement.scrollWidth > innerWidth").await? == json!(false));

                    page.locator(".agent-thread-select[aria-current=page]").click().await?;
                    page.locator(".agent-conversation").wait_for_state(State::Visible).await?;
                    page.get_by_role("navigation", "Applications").wait_for_state(State::Hidden).await?;

                    let labels_fit = page
                        .locator(".agent-mobile-views")
                        .evaluate(
                            "nav => [...nav.children].every(button => { const range = document.createRange(); range.selectNodeContents(button); return range.getBoundingClientRect().width <= button.clientWidth - 2; })",
                            Value::Null,
                        )
                        .await?;
                    ensure!(labels_fit == json!(true), "Phone panel labels must fit without overlapping ({theme} {width})");
                }
                let layout = page
                    .evaluate(
                        "() => ({ overflow: document.documentElement.scrollWidth > innerWidth, height: document.documentElement.scrollHeight, header: document.querySelector('.agent-shell').getBoundingClientRect().top })",
                    )
                    .await?;
                ensure!(layout["overflow"] == json!(false), "overflow on {theme} {width}: {layout}");
                ensure!(
                    layout["height"].as_f64().unwrap_or(f64::MAX) <= (height as f64) + 1.0,
                    "height exceeded on {theme} {width}: {layout}"
                );
                ensure!(layout["header"].as_f64() == Some(0.0), "header top != 0 on {theme} {width}: {layout}");
                page.screenshot(root.join(format!("agent-{theme}-{width}.png")), true).await?;
            }
        }

        page.set_viewport_size(1440, 950).await?;
        page.get_by_role("button", "Thread actions").click().await?;
        page.get_by_role("menuitem", "Settle thread").click().await?;
        ui::open_menu_for(&page, &page.get_by_role("button", "Thread actions"), &page.get_by_role("menuitem", "Restore thread")).await?;
        page.keyboard().press("Escape").await?;
        page.reload().await?;
        ui::open_menu_for(&page, &page.get_by_role("button", "Thread actions"), &page.get_by_role("menuitem", "Restore thread")).await?;
        page.get_by_role("menuitem", "Restore thread").click().await?;
        ui::open_menu_for(&page, &page.get_by_role("button", "Thread actions"), &page.get_by_role("menuitem", "Settle thread")).await?;
        page.keyboard().press("Escape").await?;

        ui::open_menu_for(&page, &page.get_by_role("button", "Thread actions"), &page.get_by_role("menuitem", "Settle thread")).await?;
        page.keyboard().press("Escape").await?;

        page.get_by_role("button", "Show changes").click().await?;
        page.get_by_role("button", "Maximize review").click().await?;
        page.get_by_role("button", "Restore review size").click().await?;
        page.get_by_role("button", "Close changes").click().await?;

        page.get_by_role("button", "Toggle thread details").click().await?;
        page.reload().await?;
        ensure!(!page.get_by_role("complementary", "Thread details").is_visible().await?);
        page.get_by_role("button", "Toggle thread details").click().await?;

        page.get_by_role("button", "Toggle command output").click().await?;
        page.reload().await?;
        page.get_by_role("region", "Terminal output").wait_for_state(State::Visible).await?;
        page.get_by_role("button", "Close command output").click().await?;

        page.get_by_role("button", "Hide threads").click().await?;
        page.get_by_role("navigation", "Applications").wait_for().await?;
        page.locator(".agent-sidebar").wait_for_state(State::Hidden).await?;
        page.get_by_role("button", "Show threads").click().await?;

        page.get_by_role("button", "Thread actions").click().await?;
        page.get_by_role("menuitem", "Find in thread").click().await?;
        page.get_by_role("searchbox", "Find in thread").fill("acknowledgement").await?;
        page.locator(r#"[data-match="true"]"#).wait_for().await?;
        page.keyboard().press("Escape").await?;

        page.get_by_role("searchbox", "Search threads").fill(&format!("no-such-thread-{marker}")).await?;
        page.get_by_text("No matching threads.").wait_for().await?;
        page.get_by_role("searchbox", "Search threads").fill(&marker).await?;
        ensure!(page.locator(".agent-thread-select").count().await? > 0);
        page.get_by_role("searchbox", "Search threads").fill("").await?;

        let scroll_id = uuid();
        let create_scroll = page
            .request()
            .post(
                &format!("{base}/api/v1/agent/threads"),
                &[("origin", base.as_str()), ("x-csrf-token", csrf.as_str())],
                json!({ "id": scroll_id, "projectId": project_id, "title": "Scroll restoration" }),
            )
            .await?;
        ensure!(create_scroll.status == 201);
        let mut scroll_rev = 1u64;
        for i in 0..12 {
            let body = format!("Scroll fixture {i}\n{}", "A long message to exercise reading older work.\n".repeat(25));
            let res = page
                .request()
                .post(
                    &format!("{base}/api/v1/agent/threads/{scroll_id}/messages"),
                    &[
                        ("origin", base.as_str()),
                        ("x-csrf-token", csrf.as_str()),
                        ("if-match", &format!("\"{scroll_rev}\"")),
                    ],
                    json!({ "id": uuid(), "content": body }),
                )
                .await?;
            ensure!(res.status == 201);
            scroll_rev = res.json()?["thread"]["revision"].as_u64().context("scroll revision")?;
        }

        page.goto(&format!("{base}/agent?project={project_id}&thread={scroll_id}")).await?;
        page.get_by_text(contains("Scroll fixture 11")).wait_for().await?;
        let scroll = page.locator(".agent-conversation");
        scroll.evaluate("element => { element.scrollTop = 0; }", Value::Null).await?;
        page.get_by_role("button", contains("Jump to latest")).wait_for().await?;

        page.locator(".agent-thread-select").filter_has_text(&thread_title).click().await?;
        page.get_by_label("Message").wait_for().await?;
        page.locator(".agent-thread-select").filter_has_text("Scroll restoration").click().await?;
        page.get_by_text(contains("Scroll fixture 11")).wait_for().await?;
        ensure!(scroll.evaluate("element => element.scrollTop < 10", Value::Null).await? == json!(true));
        page.get_by_role("button", contains("Jump to latest")).click().await?;
        ensure!(
            scroll
                .evaluate("element => element.scrollHeight - element.scrollTop - element.clientHeight < 48", Value::Null)
                .await?
                == json!(true)
        );

        page.get_by_label("Message").fill("First line").await?;
        page.get_by_label("Message").press("Shift+Enter").await?;
        ensure!(page.get_by_label("Message").input_value().await? == "First line\n");

        page.get_by_role("button", "New thread").first().click().await?;
        page.get_by_role("button", regex("^Project: .*Change project$")).click().await?;
        page.get_by_role("complementary", "Choose project").get_by_role("button", "Add project").click().await?;
        let picker = page.get_by_role("region", "Dar folder picker");
        picker.locator(".dar-picker-row").first().click().await?;
        page.get_by_text(regex("^Destination:")).wait_for().await?;
        ensure!(page.get_by_role("button", "Choose this folder").count().await? == 0);
        page.locator("[data-editor-pane]").get_by_role("button", "Add project").click().await?;
        page.get_by_label("Message").wait_for().await?;
        page.locator("[data-editor-pane]").wait_for_state(State::Hidden).await?;
        ensure!(url_param(&page.url(), "thread").is_none());

        let projects = page.request().get(&format!("{base}/api/v1/agent/projects")).await?.json()?;
        let project_items = projects["items"].as_array().context("project items")?;
        let files_source = project_items.iter().find(|p| p["sourceType"] == "files").context("files project")?;
        ensure!(project_items.iter().any(|p| p["id"] == project_id && p["sourceType"] == "git"));

        page.get_by_role("button", "Agent settings").click().await?;
        page.get_by_role("dialog", "Agent settings").wait_for().await?;
        ensure!(
            page.get_by_role("button", "Agent settings")
                .evaluate("el => !!el.closest('.topbar-tools')", Value::Null)
                .await?
                == json!(true)
        );
        page.get_by_role("tab", "Keyboard shortcuts").click().await?;
        page.get_by_role("heading", "Keyboard shortcuts").wait_for().await?;
        page.keyboard().press("Escape").await?;
        ensure!(page.get_by_role("dialog", "Agent settings").count().await? == 0);

        let source_ref = files_source["sourceRef"].as_str().context("files sourceRef")?;
        let folder = page.request().get(&format!("{base}/api/v1/files/picker?parentId={source_ref}")).await?.json()?;
        let folder_name = folder["folder"]["name"].as_str().context("folder name")?.to_owned();
        let filename = format!("shared-picker-{}.txt", &uuid()[..6]);
        let reservation = page
            .request()
            .post(
                &format!("{base}/api/v1/files/uploads"),
                &[("origin", base.as_str()), ("x-csrf-token", csrf.as_str())],
                json!({ "name": filename, "parentId": "", "size": 5, "mediaType": "text/plain" }),
            )
            .await?
            .json()?;
        let res_id = reservation["id"].as_str().context("reservation id")?;
        let put_reply = page
            .request()
            .put_bytes(
                &format!("{base}/api/v1/files/uploads/{res_id}"),
                &[("origin", base.as_str()), ("x-csrf-token", csrf.as_str()), ("content-type", "text/plain")],
                b"hello".to_vec(),
            )
            .await?;
        ensure!(put_reply.status == 200);

        page.goto(&base).await?;
        page.get_by_role("button", format!("Actions for {filename}")).click().await?;
        page.get_by_role("menuitem", "Copy to…").click().await?;
        let copy_name = format!("{filename} copy");
        page.get_by_label("Name").fill(&copy_name).await?;
        page.get_by_role("region", "Dar folder picker").get_by_role("button", &folder_name).click().await?;
        page.get_by_role("button", "Copy here").click().await?;
        page.locator("[data-editor-pane]").wait_for_state(State::Hidden).await?;

        let copied = page
            .request()
            .get(&format!("{base}/api/v1/files/picker?parentId={source_ref}&kind=file"))
            .await?
            .json()?;
        ensure!(
            copied["items"].as_array().is_some_and(|items| items.iter().any(|item| item["name"] == copy_name && item["kind"] == "file")),
            "copied file not found in picker: {copied}"
        );

        page.goto(&format!("{base}/agent")).await?;
        page.get_by_label("Message").wait_for().await?;
        page.keyboard().press("Control+p").await?;
        let palette = page.get_by_role("dialog", "Command palette");
        palette.get_by_role("combobox", ()).fill("Scroll restoration").await?;
        page.keyboard().press("Enter").await?;
        page.get_by_role("heading", "Scroll restoration").wait_for().await?;

        page.get_by_label("Message").fill("/").await?;
        page.get_by_role("listbox", "Commands").wait_for().await?;
        page.get_by_label("Message").fill("/find").await?;
        page.get_by_label("Message").press("Enter").await?;
        page.get_by_role("searchbox", "Find in thread").wait_for().await?;
        ensure!(page.get_by_label("Message").input_value().await? == "");
        page.keyboard().press("Escape").await?;

        let renamed_title = format!("Renamed scroll {marker}");
        page.get_by_role("button", "Thread actions").click().await?;
        page.get_by_role("menuitem", "Rename thread").click().await?;
        page.get_by_label("Thread title").fill(&renamed_title).await?;
        page.get_by_label("Thread title").press("Enter").await?;
        page.get_by_role("heading", &renamed_title).wait_for().await?;

        eventually(Duration::from_secs(5), "renamed thread title persisted", || {
            let page = page.clone();
            let base = base.clone();
            let scroll_id = scroll_id.clone();
            let renamed_title = renamed_title.clone();
            async move {
                let threads = page.request().get(&format!("{base}/api/v1/agent/threads")).await?.json()?;
                let found = threads["items"]
                    .as_array()
                    .and_then(|items| items.iter().find(|row| row["id"] == scroll_id))
                    .and_then(|row| row["title"].as_str());
                Ok((found == Some(renamed_title.as_str())).then_some(()))
            }
        })
        .await?;

        page.get_by_role("button", "Thread actions").click().await?;
        page.get_by_role("menuitem", "Pin thread").click().await?;
        page.get_by_role("group", "Pinned threads").get_by_text(&renamed_title).wait_for().await?;
        page.reload().await?;
        page.get_by_role("group", "Pinned threads").get_by_text(&renamed_title).wait_for().await?;

        page.get_by_role("button", "Thread actions").click().await?;
        page.get_by_role("menuitem", "Archive thread").click().await?;
        page.locator(".agent-thread-select").filter_has_text(&renamed_title).wait_for_state(State::Detached).await?;

        page.get_by_role("button", "Agent settings").click().await?;
        page.get_by_role("tab", "Archived threads").click().await?;
        let archived_row = page.locator(".agent-archived li").filter_has_text(&renamed_title);
        archived_row.get_by_role("button", "Delete").click().await?;
        page.get_by_role("dialog", "Delete thread").get_by_role("button", "Delete thread").click().await?;
        page.get_by_role("dialog", "Delete thread").wait_for_state(State::Detached).await?;

        let threads_after = page.request().get(&format!("{base}/api/v1/agent/threads")).await?.json()?;
        let still_exists = threads_after["items"].as_array().is_some_and(|items| items.iter().any(|row| row["id"] == scroll_id));
        ensure!(!still_exists, "deleted thread still listed");
        page.keyboard().press("Escape").await?;

        let page_errors = errors.lock().unwrap().clone();
        ensure!(page_errors.is_empty(), "unexpected page errors: {page_errors:?}");
        Ok(())
    }
    .await;

    context.close().await.ok();
    result
}

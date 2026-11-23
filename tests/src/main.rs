//! Runs Dar's acceptance suites against the built stack (`cargo xtask verify` builds it and
//! sets the paths). Suites run in parallel, each with its own instances; every suite runs even
//! after a failure, and a summary closes the run.
//!
//! ```text
//! dar-acceptance [--only <substring>|=<name>…] [--jobs <n>] [--list]
//! dar-acceptance fixture <name> [args…]   (fixture programs the suites start, such as the fake chat bridge)
//! ```

use dar_acceptance::harness::{Ctx, Suite};
use std::io::Write as _;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod suites;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("fixture") {
        return suites::fixture(&args[1..]);
    }
    let only: Vec<&str> = args.iter().enumerate().filter(|(i, _)| *i > 0 && args[i - 1] == "--only").map(|(_, v)| v.as_str()).collect();
    let jobs = args.iter().position(|a| a == "--jobs").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
        // A third of the cores, at most 5: each suite runs a runtime and often a Chromium, and
        // browser journeys get flaky on a saturated machine.
        .unwrap_or_else(|| (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) / 3).clamp(2, 5));
    // `=name` selects one suite; otherwise a pattern is a substring of suite names.
    let selected = |name: &str| only.iter().any(|o| match o.strip_prefix('=') { Some(exact) => name == exact, None => name.contains(o) });
    let suites: Vec<Suite> = suites::SUITES.iter().copied().filter(|suite| only.is_empty() || selected(suite.name)).collect();
    if args.iter().any(|a| a == "--list") {
        for suite in &suites {
            println!("{} ({} cases)", suite.name, suite.cases.len());
        }
        return ExitCode::SUCCESS;
    }
    if suites.is_empty() {
        eprintln!("No suite matches {only:?}.");
        return ExitCode::FAILURE;
    }
    let root = match dar_acceptance::temp_dir("dar-acceptance-") {
        Ok(root) => root,
        Err(error) => { eprintln!("{error}"); return ExitCode::FAILURE; }
    };
    let started = Instant::now();
    let setup = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
    if let Err(error) = setup.block_on(dar_acceptance::stack::start(root.clone())) {
        eprintln!("The stack did not start: {error}");
        return ExitCode::FAILURE;
    }
    eprintln!("Running {} suites, {jobs} at a time. Artifacts: {}", suites.len(), root.display());
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, &'static str, Vec<(String, bool, f64, String)>, f64)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(suites.len()) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(suite) = suites.get(index) else { break };
                let begun = Instant::now();
                let cases = run_suite(*suite, &root);
                let seconds = begun.elapsed().as_secs_f64();
                let ok = cases.iter().all(|(_, ok, _, _)| *ok);
                let _ = writeln!(std::io::stderr(), "{} {:>4.0}s {}", if ok { "PASS" } else { "FAIL" }, seconds, suite.name);
                for (name, ok, _, error) in &cases {
                    if !ok {
                        let _ = writeln!(std::io::stderr(), "  FAIL {name}: {error}");
                    }
                }
                results.lock().unwrap().push((index, suite.name, cases, seconds));
            });
        }
    });
    setup.block_on(dar_acceptance::browser::shutdown());
    dar_acceptance::stack::shutdown();
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(index, ..)| *index);
    let failed: Vec<_> = results.iter().filter(|(_, _, cases, _)| cases.iter().any(|(_, ok, _, _)| !ok)).collect();
    eprintln!("\nAcceptance suites:");
    for (_, name, cases, seconds) in &results {
        let ok = cases.iter().all(|(_, ok, _, _)| *ok);
        eprintln!("{} {:>4.0}s {name} ({} cases)", if ok { "PASS" } else { "FAIL" }, seconds, cases.len());
        for (case, ok, seconds, error) in cases {
            if !ok {
                eprintln!("       FAIL {case} after {seconds:.0}s: {}", error.lines().take(30).collect::<Vec<_>>().join("\n         "));
            }
        }
    }
    eprintln!("{}/{} suites passed in {:.0}s. Artifacts: {}", results.len() - failed.len(), results.len(), started.elapsed().as_secs_f64(), root.display());
    if failed.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// Runs a suite's cases in order on a runtime of its own; a failing case does not stop the rest.
fn run_suite(suite: Suite, root: &std::path::Path) -> Vec<(String, bool, f64, String)> {
    if suite.cases.is_empty() {
        return vec![("not ported".into(), false, 0.0, "this suite has no cases yet".into())];
    }
    let dir = root.join(suite.name);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return vec![("setup".into(), false, 0.0, error.to_string())];
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name(suite.name).build().expect("runtime");
    let ctx = Arc::new(Ctx::new(suite.name, dir));
    let mut outcomes = Vec::new();
    for case in suite.cases {
        let begun = Instant::now();
        let ctx_case = ctx.clone();
        let timeout = case.timeout;
        let run = case.run;
        let outcome = runtime.block_on(async move {
            let task = tokio::spawn(async move { (run)(&ctx_case).await });
            match tokio::time::timeout(timeout, task).await {
                Ok(Ok(Ok(()))) => Ok(()),
                Ok(Ok(Err(error))) => Err(error.to_string()),
                Ok(Err(join)) => Err(match join.try_into_panic() {
                    Ok(panic) => panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "panicked".into()),
                    Err(error) => error.to_string(),
                }),
                Err(_) => Err(format!("timed out after {timeout:?}")),
            }
        });
        let short = case.name.rsplit("::").next().unwrap_or(case.name).to_owned();
        if outcome.is_err() {
            runtime.block_on(dar_acceptance::browser::screenshot_open_pages(suite.name, &short, ctx.dir()));
        }
        outcomes.push((short, outcome.is_ok(), begun.elapsed().as_secs_f64(), outcome.err().unwrap_or_default()));
    }
    runtime.block_on(async { ctx.close().await; dar_acceptance::browser::close_suite(suite.name).await });
    runtime.shutdown_timeout(Duration::from_secs(5));
    outcomes
}

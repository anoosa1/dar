//! Helpers and fixtures shared by the comms suites (ported from `test/shared/`, `test/support/` and
//! `test/fixtures/`). Only that area's owner edits this file.

#![allow(dead_code)]

use std::process::ExitCode;

/// The fake chat bridge (`test/fixtures/fake-chat-bridge.mjs`), run by the runtime as
/// `DAR_CHAT_BRIDGE`; `DAR_FAKE_CHAT_CONTROL` names its control folder.
pub fn chat_bridge_fixture(_args: &[String]) -> ExitCode {
    eprintln!("the fake chat bridge is not ported yet");
    ExitCode::FAILURE
}

//! M6.1 Spec 2 relay pins: the wire contract the toolkit text-input-v3
//! client depends on, driven with harness doubles only.
//!
//! This deliberately overlaps `client_protocol.rs`' Spec 1 coverage: that
//! file proves the relay, this file pins the client's assumptions about it
//! (enable→activate→surrounding echo, re-forward on re-commit, IME→app
//! preedit/commit/delete, disable→deactivate). `client_protocol.rs` stays
//! untouched; the toolkit-level e2e lives in `ui/tests/
//! m6_spec2_text_input_e2e.rs` (compositor dev-deps cannot depend on
//! `icedtea-ui`).

use icedtea_harness::{Compositor, InputMethodClient, TextInputClient, VirtualKeyboardClient};

/// The client's first assumption: `enable` + surrounding text activates the
/// IME and the surrounding echo (text, cursor, anchor) arrives intact.
#[test]
fn enable_activates_ime_and_echoes_surrounding() {
    let comp = Compositor::spawn();
    let _vk = VirtualKeyboardClient::spawn(&comp.socket);
    let mut im = InputMethodClient::spawn(&comp.socket);
    let mut ti = TextInputClient::spawn(&comp.socket);
    assert!(
        ti.wait_until(|c| c.entered() >= 1),
        "text-input never focused"
    );

    ti.enable();
    ti.commit_with("hello", 5, 5, (10, 20, 2, 16));

    assert!(im.wait_until(|s| s.activates() >= 1), "IME never activated");
    assert!(
        im.wait_until(|s| s
            .surroundings()
            .iter()
            .any(|t| t == &("hello".to_string(), 5, 5))),
        "IME never saw the surrounding echo; saw {:?}",
        im.surroundings()
    );
    assert!(comp.input_method_active(), "oracle: IME must be active");
}

/// The client's typing assumption: every edit re-syncs surrounding text, so
/// a commit on an already-active text-input must re-forward the UPDATED
/// values, not stick on the activation-time ones.
#[test]
fn resync_reforwards_updated_surrounding() {
    let comp = Compositor::spawn();
    let _vk = VirtualKeyboardClient::spawn(&comp.socket);
    let mut im = InputMethodClient::spawn(&comp.socket);
    let mut ti = TextInputClient::spawn(&comp.socket);
    assert!(
        ti.wait_until(|c| c.entered() >= 1),
        "text-input never focused"
    );

    ti.enable();
    ti.commit_with("first", 5, 5, (10, 20, 2, 16));
    assert!(
        im.wait_until(|s| s.surroundings().last() == Some(&("first".to_string(), 5, 5))),
        "IME never saw the initial surrounding; saw {:?}",
        im.surroundings()
    );

    ti.commit_with("にちa", 4, 4, (11, 21, 3, 17));
    assert!(
        im.wait_until(|s| s.surroundings().last() == Some(&("にちa".to_string(), 4, 4))),
        "IME never saw the re-forwarded surrounding; saw {:?}",
        im.surroundings()
    );
}

/// The client's composition assumption: an IME batch (preedit + commit +
/// delete + done) reaches the focused app's text-input as its three event
/// kinds, with a `done` per commit.
#[test]
fn ime_batch_reaches_the_app_as_preedit_commit_delete_done() {
    let comp = Compositor::spawn();
    let _vk = VirtualKeyboardClient::spawn(&comp.socket);
    let mut im = InputMethodClient::spawn(&comp.socket);
    let mut ti = TextInputClient::spawn(&comp.socket);
    assert!(
        ti.wait_until(|c| c.entered() >= 1),
        "text-input never focused"
    );
    ti.enable();
    ti.commit_with("", 0, 0, (0, 0, 1, 1));
    assert!(im.wait_until(|s| s.activates() >= 1), "IME never activated");
    let dones_before = ti.dones();

    im.send_commit(Some("に"), Some("日本"), Some((1, 0)));

    assert!(
        ti.wait_until(|c| c.preedits().last().map(String::as_str) == Some("に")),
        "app never got the preedit; saw {:?}",
        ti.preedits()
    );
    assert!(
        ti.wait_until(|c| c.commits().last().map(String::as_str) == Some("日本")),
        "app never got the commit; saw {:?}",
        ti.commits()
    );
    assert!(
        ti.wait_until(|c| c.deletes().last() == Some(&(1, 0))),
        "app never got the delete; saw {:?}",
        ti.deletes()
    );
    assert!(
        ti.wait_until(|c| c.dones() > dones_before),
        "app never got the batch done"
    );
}

/// The client's teardown assumption: `disable` deactivates the IME and the
/// oracle goes idle.
#[test]
fn disable_deactivates_the_ime() {
    let comp = Compositor::spawn();
    let _vk = VirtualKeyboardClient::spawn(&comp.socket);
    let mut im = InputMethodClient::spawn(&comp.socket);
    let mut ti = TextInputClient::spawn(&comp.socket);
    assert!(
        ti.wait_until(|c| c.entered() >= 1),
        "text-input never focused"
    );
    ti.enable();
    ti.commit_with("hello", 5, 5, (10, 20, 2, 16));
    assert!(im.wait_until(|s| s.activates() >= 1), "IME never activated");
    assert!(comp.input_method_active(), "oracle: IME must be active");

    ti.disable();
    assert!(
        im.wait_until(|s| s.deactivates() >= 1),
        "IME never deactivated on disable"
    );
    assert!(
        !comp.input_method_active(),
        "oracle: IME must be idle after disable"
    );
}

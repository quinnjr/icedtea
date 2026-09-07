//! A live D-Bus round-trip between the shell's real `ClipProxy` and the real
//! `icedtea-clipboard` service. This is the coverage the taskbar/popover panel
//! tests deliberately mock away — and the exact thing that hid the shipping bug
//! where the shell called snake_case members (`activate`) while zbus exposes
//! PascalCase (`Activate`). If the member names drift again, this fails.
//!
//! Skipped (visibly) only when the session bus is unavailable or the
//! `org.icedtea.Clipboard` name is already owned (a real daemon is running).

use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use icedtea_clipboard::manager::Command;
use icedtea_clipboard::service;
use icedtea_contract::{ClipEntry, ClipKind};
use icedtea_shell::clip_client::{ClipCommands, ClipProxy};

fn seeded(preview: &str) -> Vec<ClipEntry> {
    vec![ClipEntry {
        id: 1,
        kind: ClipKind::Text,
        preview: preview.into(),
        mime: "text/plain".into(),
        pinned: false,
        source_app: None,
    }]
}

#[test]
fn clip_proxy_commands_reach_the_real_service() {
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<Command>();
    let (_chg_tx, chg_rx) = crossbeam_channel::unbounded();
    let (_wake_read, wake_write) = UnixStream::pair().expect("wake pipe");
    let snapshot = Arc::new(Mutex::new(seeded("clipboard entry")));

    let _conn = match service::spawn(snapshot.clone(), cmd_tx, wake_write, chg_rx) {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!(
                "SKIP: cannot register org.icedtea.Clipboard ({err}) -- bus down or a daemon is running"
            );
            return;
        }
    };

    let proxy = ClipProxy::new().expect("connect to session bus");

    // get_history over the wire (the shell's clip_client uses "GetHistory"): a
    // raw proxy call must return the seeded entry, proving the member exists.
    let blocking = zbus::blocking::Connection::session().expect("session bus");
    let reply = blocking
        .call_method(
            Some(icedtea_contract::CLIP_BUS_NAME),
            icedtea_contract::CLIP_PATH,
            Some("org.icedtea.Clipboard"),
            "GetHistory",
            &(),
        )
        .expect("GetHistory call failed -- member name mismatch?");
    let history: Vec<ClipEntry> = reply.body().deserialize().expect("history body");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].preview, "clipboard entry");

    // A ClipProxy command must arrive at the service as the right Command.
    proxy.activate(77);
    let cmd = cmd_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("service never got the command");
    assert!(
        matches!(cmd, Command::Activate(77)),
        "wrong command reached the service: {cmd:?}"
    );

    proxy.pin(9, true);
    let cmd = cmd_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("service never got Pin");
    assert!(
        matches!(cmd, Command::Pin(9, true)),
        "wrong command: {cmd:?}"
    );
}

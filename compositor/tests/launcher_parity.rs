//! Task 4 review drift pin: the compositor keeps its own `.desktop`
//! parse copy (`parse_desktop_exec`, `default_app_dirs` in `state.rs`)
//! instead of depending on the shell crate, so this file feeds both sides
//! the same inputs and asserts the same parse decisions. If either side's
//! rules change without the other, these fail. Do NOT fix that by
//! restructuring into a shared crate (explicitly out of scope) -- update
//! both copies and keep this file green.
//!
//! Also covers the shadowing parity: a hidden/malformed user overlay falls
//! through to the system entry on the compositor side (`lookup_app_exec`)
//! and the launcher-side index resolves the same entry (`DesktopIndex`).

use icedtea_compositor::state::{self, State};
use icedtea_registry_schema::default_config;

mod support;

use support::{assert_quiescent, spawn_app, tmpdir, wait_for, write_entry};

/// Shell-side decision for one file's text: the raw `Exec` iff the entry
/// parses and is visible in this desktop, mirroring exactly what the
/// launcher's index keeps for launching.
fn shell_decision(id: &str, text: &str) -> Option<String> {
    icedtea_shell::launcher::parse_entry_with_id(id, text)
        .filter(|entry| entry.visible_in(icedtea_shell::launcher::SHOW_IN_ENV))
        .map(|entry| entry.exec)
}

/// Compositor-side decision for the same input.
fn compositor_decision(text: &str) -> Option<String> {
    state::parse_desktop_exec(text)
}

#[test]
fn parse_decisions_agree_on_both_sides() {
    let cases = [
        // All key types, visible.
        (
            "[Desktop Entry]\nName=Terminal\nExec=gnome-terminal %F\nIcon=t\nCategories=System;\nKeywords=shell;\n",
            Some("gnome-terminal %F".to_string()),
        ),
        // NoDisplay hides.
        (
            "[Desktop Entry]\nName=Hidden\nExec=h\nNoDisplay=true\n",
            None,
        ),
        // OnlyShowIn gates on this desktop's name.
        ("[Desktop Entry]\nName=G\nExec=g\nOnlyShowIn=GNOME;\n", None),
        (
            "[Desktop Entry]\nName=I\nExec=i\nOnlyShowIn=icedtea;\n",
            Some("i".to_string()),
        ),
        // NotShowIn gates the other way.
        (
            "[Desktop Entry]\nName=N\nExec=n\nNotShowIn=icedtea;\n",
            None,
        ),
        (
            "[Desktop Entry]\nName=M\nExec=m\nNotShowIn=GNOME;\n",
            Some("m".to_string()),
        ),
        // First occurrence of a plain key wins (empty first Name poisons).
        (
            "[Desktop Entry]\nName=First\nName=Second\nExec=x\n",
            Some("x".to_string()),
        ),
        ("[Desktop Entry]\nName=\nName=Foo\nExec=x\n", None),
        (
            "[Desktop Entry]\nName=Foo\nExec=one\nExec=two\n",
            Some("one".to_string()),
        ),
        // Any locale Name variant counts as a name.
        (
            "[Desktop Entry]\nName=Files\nName[de]=Dateien\nExec=files\n",
            Some("files".to_string()),
        ),
        (
            "[Desktop Entry]\nName[de]=Dateien\nExec=files\n",
            Some("files".to_string()),
        ),
        // Exec field codes stay raw in the launch value on both sides.
        (
            "[Desktop Entry]\nName=F\nExec=foo %f %F %u %U %i %c %k %% bar\n",
            Some("foo %f %F %u %U %i %c %k %% bar".to_string()),
        ),
        // Missing/empty Name or Exec, missing group: unparseable both sides.
        ("[Desktop Entry]\nName=NoExec\n", None),
        ("[Desktop Entry]\nExec=no-name\n", None),
        ("[Desktop Entry]\nName=\nExec=\n", None),
        ("[Other]\nName=X\nExec=x\n", None),
        ("not a desktop file", None),
        // Unknown keys (incl. Type) are ignored leniently by both.
        (
            "[Desktop Entry]\nType=Application\nName=T\nExec=t\n",
            Some("t".to_string()),
        ),
        // Comments, blank lines, and other groups never leak in.
        (
            "# c\n\n[Desktop Entry]\nName=Foo\n# Name=Wrong\nExec=foo\n\n[Desktop Action X]\nName=Shadow\nExec=shadow\n",
            Some("foo".to_string()),
        ),
    ];
    for (i, (text, expected)) in cases.iter().enumerate() {
        assert_eq!(
            &shell_decision("app", text),
            expected,
            "shell decision, case {i}"
        );
        assert_eq!(
            &compositor_decision(text),
            expected,
            "compositor decision, case {i}"
        );
    }
}

#[test]
fn default_app_dirs_agree_on_both_sides() {
    assert_eq!(
        state::default_app_dirs(),
        icedtea_shell::launcher::default_dirs(),
        "XDG scan roots must stay identical or the two index copies diverge"
    );
}

/// A hidden/malformed overlay in one root falls through to the other
/// root's entry on BOTH sides (compositor `lookup_app_exec` keeps looking;
/// the launcher index drops hidden/unparseable files so `find` lands on
/// the same surviving entry).
#[test]
fn shadowing_fall_through_agrees_on_both_sides() {
    let root = tmpdir("shadow");
    let sys = root.join("sys");
    let user = root.join("user");
    std::fs::create_dir_all(&sys).expect("sys dir");
    std::fs::create_dir_all(&user).expect("user dir");

    // Case A: hidden system entry, visible user overlay -- both resolve user.
    let user_sentinel = user.join("foo.user");
    write_entry(
        &sys,
        "foo",
        "[Desktop Entry]\nName=Foo\nExec=touch /nonexistent-sys-foo\nNoDisplay=true\n",
    );
    write_entry(
        &user,
        "foo",
        &format!(
            "[Desktop Entry]\nName=Foo\nExec=touch \"{}\"\n",
            user_sentinel.display()
        ),
    );
    // Case B: malformed system entry (no Exec), visible user overlay --
    // both resolve user.
    let user_sentinel_b = user.join("bar.user");
    write_entry(&sys, "bar", "[Desktop Entry]\nName=Bar\n");
    write_entry(
        &user,
        "bar",
        &format!(
            "[Desktop Entry]\nName=Bar\nExec=touch \"{}\"\n",
            user_sentinel_b.display()
        ),
    );
    // Case C: visible on both sides -- first hit (system root) wins both.
    let sys_sentinel_c = sys.join("baz.sys");
    let user_sentinel_c = user.join("baz.user");
    write_entry(
        &sys,
        "baz",
        &format!(
            "[Desktop Entry]\nName=Baz\nExec=touch \"{}\"\n",
            sys_sentinel_c.display()
        ),
    );
    write_entry(
        &user,
        "baz",
        &format!(
            "[Desktop Entry]\nName=Baz\nExec=touch \"{}\"\n",
            user_sentinel_c.display()
        ),
    );

    // Compositor side: order is sys-then-user, fall-through on hidden.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(default_config(), tx);
    state.set_app_dirs(vec![sys.clone(), user.clone()]);
    assert!(
        spawn_app(&mut state, "foo"),
        "foo resolves via user overlay"
    );
    assert!(
        spawn_app(&mut state, "bar"),
        "bar resolves via user overlay"
    );
    assert!(
        spawn_app(&mut state, "baz"),
        "baz resolves via system entry"
    );
    wait_for(&user_sentinel);
    wait_for(&user_sentinel_b);
    wait_for(&sys_sentinel_c);

    // Launcher side: the index drops the hidden files, `find` lands same.
    let apps = icedtea_shell::launcher::DesktopIndex::scan(&[sys, user]);
    let foo = apps
        .iter()
        .find(|entry| entry.id == "foo")
        .expect("foo survives filtering");
    assert!(
        foo.exec.contains("foo.user"),
        "launcher resolves foo via user overlay, got {:?}",
        foo.exec
    );
    let bar = apps
        .iter()
        .find(|entry| entry.id == "bar")
        .expect("bar survives filtering");
    assert!(
        bar.exec.contains("bar.user"),
        "launcher resolves bar via user overlay, got {:?}",
        bar.exec
    );
    let baz = apps
        .iter()
        .find(|entry| entry.id == "baz")
        .expect("baz survives filtering");
    assert!(
        baz.exec.contains("baz.sys"),
        "launcher resolves baz via system entry, got {:?}",
        baz.exec
    );
    assert_quiescent("system first-hit must shadow the user overlay", || {
        !user_sentinel_c.exists()
    });

    let _ = std::fs::remove_dir_all(&root);
}

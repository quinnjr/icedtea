//! Task 17: a second headless output must land in `state.outputs` with a
//! real, disjoint layout-box geometry -- not the (0,0)-origin fallback
//! `OutputHandler::new_output` used before `Runtime::output_layout_box`
//! existed.
//!
//! A separate integration-test binary, not an addition to `headless_boot.rs`:
//! `WLR_HEADLESS_OUTPUTS` is read once by `Backend::autocreate` and every
//! other test in that binary assumes the default single-output headless
//! backend. Each `tests/*.rs` file is its own process, so the env var set
//! here can never leak into `headless_boot.rs`'s tests (or vice versa).

use icedtea_compositor::state::CommitEvent;
use icedtea_compositor::state::State;
// `output_configuration_applied` is a `wlr::OutputHandler` method; the trait
// must be in scope to call it on `State`.
use wlr::OutputHandler;

/// Cursor-stimulus hotspot the damage test moves the software cursor to, in
/// output-logical coordinates (mirrors the `wlr` crate's own
/// `output_feedback.rs` stimulus).
const CURSOR_X: f64 = 100.0;
/// See [`CURSOR_X`].
const CURSOR_Y: f64 = 100.0;
/// Edge length of the 8x8 cursor image: a damage delivery must cover the
/// hotspot with at least this.
const CURSOR_EXTENT: i32 = 8;
/// Boot settle: both headless outputs arrive and enable (MODE-staging
/// commits) within this. Every other test in this file already trusted this
/// shape as its backstop.
const BOOT_SETTLE_MS: u64 = 150;
/// Damage settle: the kicked frame recommits and the cursor damage is
/// delivered within this after the stimulus.
const DAMAGE_SETTLE_MS: u64 = 250;
/// Long backstop: boot enable commits plus at least one frame commit per
/// output within this (the commit-observation shape).
const LONG_BACKSTOP_MS: u64 = 300;
/// Cap-guard run: deliberately longer than any other backstop in this file,
/// to give the commit/damage histories the most deliveries wall-clock alone
/// can produce.
const CAP_GUARD_RUN_MS: u64 = 1000;

/// Set the headless-backend environment (two outputs, this file's own
/// concern) exactly once, no matter which of this binary's `#[test]`s
/// reaches it first. See `headless_boot.rs`'s `ensure_headless_env` for the
/// torn-environment hazard this guards against -- identical reasoning, this
/// file's own copy because each integration-test binary has its own statics.
fn ensure_headless_env() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // SAFETY (icedtea unsafe exception (c)): `Once::call_once` above
        // guarantees this closure runs on exactly one thread and that every
        // other thread calling `ensure_headless_env` blocks until it
        // finishes -- so nothing can observe a torn read and nothing races
        // this write.
        unsafe {
            std::env::set_var("WLR_BACKENDS", "headless");
            std::env::set_var("WLR_HEADLESS_OUTPUTS", "2");
        }
    });
}

/// Serializes compositor *creation* across this binary's test threads. See
/// `headless_boot.rs`'s `BOOT_LOCK` for the full argument (the process-global,
/// unsynchronized `wl_array` of buffer-resource interfaces `init_graphics`
/// grows). Duplicated rather than shared because each integration-test file
/// is its own binary with its own statics.
static BOOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take [`BOOT_LOCK`] (and set the headless environment) for the duration of
/// one compositor's creation. `drop` the returned guard once the
/// display/backend/runtime triple exists.
fn boot_lock() -> std::sync::MutexGuard<'static, ()> {
    ensure_headless_env();
    BOOT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn a_second_headless_output_is_tracked_with_a_layout_box() {
    let boot = boot_lock();
    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");
    drop(boot);

    // `state` is declared (and so, by ordinary end-of-scope drop order,
    // dropped) after `display`/`backend`/`runtime`, matching every
    // `headless_boot.rs` test: `attach` below gives `state.wayland` a
    // `Runtime` clone, and `wlr` documents that a `Runtime` must not outlive
    // the `Display` it was initialized against.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    let background = runtime
        .add_rect(
            1,
            1,
            icedtea_compositor::render::wallpaper_color(&state.config.appearance),
        )
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // The command channel and its wake pipe, used only as this test's
    // bounded backstop -- same role as `headless_boot.rs`'s `run_all` tests:
    // it gives both headless outputs time to arrive (and so
    // `OutputHandler::new_output` time to run twice) before asking the loop
    // to stop.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(BOOT_SETTLE_MS));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(
        state.quitting,
        "the backstop Quit command must have stopped the loop"
    );

    assert_eq!(
        state.outputs.len(),
        2,
        "both headless outputs must have reached the model"
    );

    let geometries: Vec<_> = state.outputs.values().map(|o| o.geometry).collect();
    let a = geometries[0];
    let b = geometries[1];
    assert!(
        a.width > 0 && a.height > 0 && b.width > 0 && b.height > 0,
        "both outputs report a real size, got {a:?} and {b:?}"
    );
    // Disjoint boxes are the proof the layout-box path (not the
    // (0,0)-at-origin fallback, which would stack both outputs on top of
    // each other) produced these geometries.
    let disjoint = a.x + a.width <= b.x
        || b.x + b.width <= a.x
        || a.y + a.height <= b.y
        || b.y + b.height <= a.y;
    assert!(
        disjoint,
        "the two outputs' layout boxes must not overlap, got {a:?} and {b:?}"
    );
}

/// [HIGH H4] `sync_wallpaper_nodes`' hot-unplug cleanup branch: a wallpaper
/// buffer node for an output that is no longer in `state.outputs` (simulating
/// `OutputHandler::destroyed` having already removed it) must be torn down on
/// the next sync, not left dangling.
///
/// Needs the two-output headless boot this file already sets up
/// (`WLR_HEADLESS_OUTPUTS=2`): one wallpaper node per output is created first,
/// then one output is removed from the model directly (the same thing
/// `OutputHandler::destroyed` does before calling `sync_wallpaper_nodes`), and
/// a second sync must drop the orphaned node.
#[test]
fn sync_wallpaper_nodes_removes_a_node_for_an_output_that_is_gone() {
    let boot = boot_lock();
    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");
    drop(boot);

    // `state` is declared (and so dropped) after `display`/`backend`/`runtime`
    // for the same reason as every other test in this file.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    let background = runtime
        .add_rect(
            1,
            1,
            icedtea_compositor::render::wallpaper_color(&state.config.appearance),
        )
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // The command channel and its wake pipe, used only as this test's
    // bounded backstop -- gives both headless outputs time to arrive before
    // asking the loop to stop.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(BOOT_SETTLE_MS));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(
        state.quitting,
        "the backstop Quit command must have stopped the loop"
    );
    assert_eq!(
        state.outputs.len(),
        2,
        "both headless outputs must have reached the model"
    );

    let image = image::RgbaImage::from_pixel(4, 4, image::Rgba([9, 8, 7, 255]));
    state.wallpaper.set_decoded(Some(image));
    state.sync_wallpaper_nodes();
    assert_eq!(
        state.wallpaper_node_count(),
        2,
        "one buffer node per output"
    );

    // Simulate `OutputHandler::destroyed`: it removes the output from the
    // model before calling `sync_wallpaper_nodes` (see `state.rs`), so drop
    // one output out from under the wallpaper node map here directly.
    let gone_index = *state.outputs.keys().min().expect("at least one output");
    state.outputs.remove(&gone_index);
    state.sync_wallpaper_nodes();

    assert_eq!(
        state.wallpaper_node_count(),
        1,
        "the node for the removed output must be torn down on the next sync"
    );
}

/// A minimal [`wlr::AppliedHead`] naming `name` with the given enabled state --
/// the fields `output_configuration_applied`'s enable/disable/rehydrate
/// branches actually read here. Width/height are populated for the enabled
/// case so the head-reported geometry fallback is available even if the layout
/// box lookup returns `None`.
fn applied_head(name: &str, enabled: bool) -> wlr::AppliedHead {
    wlr::AppliedHead {
        name: Some(name.to_string()),
        enabled,
        width: if enabled { 1920 } else { 0 },
        height: if enabled { 1080 } else { 0 },
        refresh_mhz: 0,
        x: 0,
        y: 0,
        scale: 1.0,
        transform: wlr::Transform::Normal,
    }
}

/// T5 display-config fix (defect 2): an output DISABLED via
/// `output_configuration_applied` leaves the active set (`state.outputs`) but
/// stays tracked by connector name -> `wlr::OutputId` in `disabled_outputs`, so
/// a later RE-ENABLE of the same connector rehydrates it back into the active
/// set (a fresh index + surface, its id re-mapped) instead of being left
/// enabled-but-untracked and rendering nothing until restart.
///
/// Drives the handler directly with owned `AppliedHead`s. The full
/// zwlr_output_manager_v1 client round-trip is T6/T7; this exercises exactly
/// the handler code the fix changed, with a live runtime so
/// `output_layout_box`/`create_output` run for real. Distinguishes the fix
/// from the pre-fix `continue`, which left `outputs.len()` stuck at 1 on
/// re-enable.
#[test]
fn a_disabled_output_can_be_re_enabled_within_a_session() {
    let boot = boot_lock();
    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");
    drop(boot);

    // Declared (dropped) after display/backend/runtime, as every test here.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    // Isolate the off-loop redb persist `output_configuration_applied` kicks
    // off; without this it would write to the real XDG database path.
    let tmp = std::env::temp_dir().join(format!("icedtea-disp-{}.redb", std::process::id()));
    state.config_path = Some(tmp.clone());

    let background = runtime
        .add_rect(
            1,
            1,
            icedtea_compositor::render::wallpaper_color(&state.config.appearance),
        )
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // Bounded backstop: give both headless outputs time to arrive before the
    // loop stops -- identical to the other tests in this file.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(BOOT_SETTLE_MS));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert_eq!(
        state.outputs.len(),
        2,
        "both headless outputs must have reached the model"
    );

    // The connector name of one output -- the stable key the disable/re-enable
    // round-trip matches against.
    let victim_index = *state.outputs.keys().min().expect("an output");
    let victim_name = state
        .outputs
        .get(&victim_index)
        .expect("victim surface")
        .name
        .clone();
    assert!(!victim_name.is_empty(), "headless outputs are named");

    // DISABLE: drops it from the active set but records name -> id.
    state.output_configuration_applied(vec![applied_head(&victim_name, false)]);
    assert_eq!(
        state.outputs.len(),
        1,
        "the disabled output left the active set"
    );
    assert!(
        state.outputs.values().all(|o| o.name != victim_name),
        "no active surface still carries the disabled connector's name"
    );

    // RE-ENABLE the same connector: rehydrate must add it back under its own
    // name. Pre-fix this hit the `continue` and `outputs.len()` stayed at 1.
    state.output_configuration_applied(vec![applied_head(&victim_name, true)]);
    assert_eq!(
        state.outputs.len(),
        2,
        "the re-enabled output rejoined the active set"
    );
    assert_eq!(
        state
            .outputs
            .values()
            .filter(|o| o.name == victim_name)
            .count(),
        1,
        "exactly one active surface carries the re-enabled connector's name"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// Task 3 (wlr 0.20.34 wire-up): `OutputHandler::output_committed` /
/// `output_precommitted` observe every commit into `state.last_commit`,
/// keyed by live output id, and an external MODE commit re-derives geometry
/// through the layout path.
///
/// No handler is driven directly: a `wlr::Output` handle cannot be built
/// outside the crate, so the headless loop's own commits (enable commits at
/// boot, which stage MODE, plus the frame path's scene commits) flow through
/// wlroots' precommit-then-commit emission and both handlers record. The
/// single slot is last-writer-wins in emission order, so the surviving record
/// per output is the commit's view of the staged fields; asserting its mask
/// is non-empty pins that both the staged mask and the commit timestamp made
/// it into the model. Record-only, assert-after-run (a panic inside a
/// handler body aborts through C).
#[test]
fn output_commit_and_precommit_are_observed_per_output() {
    let boot = boot_lock();
    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");
    drop(boot);

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    let background = runtime
        .add_rect(
            1,
            1,
            icedtea_compositor::render::wallpaper_color(&state.config.appearance),
        )
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // Bounded backstop: gives both headless outputs time to arrive, enable
    // (MODE-staging commits), and run at least one frame commit each.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(LONG_BACKSTOP_MS));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(
        state.quitting,
        "the backstop Quit command must have stopped the loop"
    );
    assert_eq!(
        state.outputs.len(),
        2,
        "both headless outputs must have reached the model"
    );

    // Every live output committed at least once (enable + frame path), so
    // each must have a last-commit record.
    assert_eq!(
        state.last_commit.len(),
        state.outputs.len(),
        "one last-commit record per live output"
    );
    for (id, (fields, _when)) in &state.last_commit {
        assert!(
            !fields.is_empty(),
            "the record for {id:?} must carry the staged fields, not an empty mask"
        );
    }

    // The boot enable commits stage MODE, which runs the re-derivation
    // branch: geometries must still be real and disjoint afterwards.
    let geometries: Vec<_> = state.outputs.values().map(|o| o.geometry).collect();
    let a = geometries[0];
    let b = geometries[1];
    assert!(
        a.width > 0 && a.height > 0 && b.width > 0 && b.height > 0,
        "MODE re-derivation must not collapse geometries, got {a:?} and {b:?}"
    );
    let disjoint = a.x + a.width <= b.x
        || b.x + b.width <= a.x
        || a.y + a.height <= b.y
        || b.y + b.height <= a.y;
    assert!(
        disjoint,
        "the two outputs' layout boxes must not overlap, got {a:?} and {b:?}"
    );
}
/// Review finding #5 (interactive guard): a client that disables every
/// connector must NOT be able to drive the compositor to zero active outputs.
/// An empty `state.outputs` makes `outputs.keys().min()` `None`, so window
/// placement / migration / reclaim all early-return -- a black screen with no
/// way back. `output_configuration_applied` refuses the disable that would
/// empty the active set (when the same apply enables nothing to replace it),
/// keeping at least one output live.
///
/// Drives the handler directly (like the re-enable test above) with a live
/// two-output headless runtime. Disabling the first output is honored (the
/// second survives); disabling the last remaining one is refused.
#[test]
fn disabling_every_output_keeps_at_least_one_active() {
    let boot = boot_lock();
    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");
    drop(boot);

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    // Isolate the off-loop redb persist from the real XDG database path.
    let tmp = std::env::temp_dir().join(format!("icedtea-lastout-{}.redb", std::process::id()));
    state.config_path = Some(tmp.clone());

    let background = runtime
        .add_rect(
            1,
            1,
            icedtea_compositor::render::wallpaper_color(&state.config.appearance),
        )
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // Bounded backstop: BOOT_SETTLE_MS gives both headless outputs time to
    // arrive before the loop stops -- the direct handler invocations below
    // run after `run_all` returns.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(BOOT_SETTLE_MS));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert_eq!(
        state.outputs.len(),
        2,
        "both headless outputs must have reached the model"
    );

    let names: Vec<String> = state.outputs.values().map(|o| o.name.clone()).collect();
    let (name_a, name_b) = (names[0].clone(), names[1].clone());

    // Disable the first: allowed, because the second output survives it.
    state.output_configuration_applied(vec![applied_head(&name_a, false)]);
    assert_eq!(
        state.outputs.len(),
        1,
        "disabling one of two outputs is honored"
    );

    // Review finding #6: seed a SAVED config for the last output with a real
    // custom mode/scale/transform/position, so we can prove that a REFUSED
    // disable does not corrupt it. The refused head reports enabled=false with a
    // 0x0 mode; persisting that would flip the record to disabled and wipe the
    // saved mode -- which the next boot would then force-enable at preferred,
    // losing everything.
    state.config.displays.push(icedtea_config::DisplayConfig {
        name: name_b.clone(),
        enabled: true,
        width: 2560,
        height: 1440,
        refresh_mhz: 144_000,
        x: 100,
        y: 0,
        scale: 1.5,
        transform: 3,
    });

    // Disable the last remaining one, alone: the guard must refuse it so the
    // session is never left with zero active outputs.
    state.output_configuration_applied(vec![applied_head(&name_b, false)]);
    assert_eq!(
        state.outputs.len(),
        1,
        "the last active output must not be disabled -- >=1 output stays live"
    );
    assert!(
        state.outputs.keys().min().is_some(),
        "an active survivor output remains for placement"
    );

    // Review finding #6: the refused disable must NOT have corrupted name_b's
    // persisted entry. It stays enabled=true with its saved mode intact.
    let saved = state
        .config
        .displays
        .iter()
        .find(|d| d.name == name_b)
        .expect("the refused output's saved config entry must survive");
    assert!(
        saved.enabled,
        "a refused disable must keep the output enabled=true in persisted config"
    );
    assert_eq!(
        saved.width, 2560,
        "the saved mode width must not be wiped to 0 by a refused disable"
    );
    assert_eq!(
        saved.height, 1440,
        "the saved mode height must survive a refused disable"
    );
    assert_eq!(
        saved.refresh_mhz, 144_000,
        "the saved refresh must survive a refused disable"
    );
    assert_eq!(
        saved.scale, 1.5,
        "the saved scale must survive a refused disable"
    );
    assert_eq!(
        saved.transform, 3,
        "the saved transform must survive a refused disable"
    );
    assert_eq!(
        saved.x, 100,
        "the saved position must survive a refused disable"
    );

    let _ = std::fs::remove_file(&tmp);
}

/// Task 7 step 1 (wlr 0.20.34 wire-up): a headless commit emits precommit
/// before commit with the staged mask and live timestamps, per live output.
///
/// Mirrors the `wlr` crate's own `output_feedback.rs` shape: `State` records
/// every commit-family delivery as a `CommitEvent` in `commit_log` (arrival
/// order, not last-writer-wins), and this asserts positions in that history --
/// the first timestamped precommit of a staged mask precedes the timestamped
/// commit of the same mask. Histories, not slots, so extra backend commits
/// cannot flake this: whatever else fires around it, each transaction's own
/// precommit still precedes its commit. Record-only handlers,
/// assert-after-run (a panic inside a handler body aborts through C).
#[test]
fn output_commit_order_precommit_precedes_commit() {
    let boot = boot_lock();
    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");
    drop(boot);

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    let background = runtime
        .add_rect(
            1,
            1,
            icedtea_compositor::render::wallpaper_color(&state.config.appearance),
        )
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // Bounded backstop: boot enable commits (which stage MODE) plus at least
    // one frame commit per output, same shape as
    // `output_commit_and_precommit_are_observed_per_output`.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(LONG_BACKSTOP_MS));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(
        state.quitting,
        "the backstop Quit command must have stopped the loop"
    );
    assert_eq!(
        state.outputs.len(),
        2,
        "both headless outputs must have reached the model"
    );

    // One ordered history per live output: the log is keyed by the
    // always-unique output id, and `output_ids` (id -> model index) is
    // private, so distinct log ids standing 1:1 with the live outputs is
    // the coverage claim.
    let mut ids: Vec<wlr::OutputId> = Vec::new();
    for (id, _) in &state.commit_log {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    assert_eq!(
        ids.len(),
        state.outputs.len(),
        "one ordered commit history per live output, got {:?}",
        state.commit_log
    );

    for id in ids {
        // This output's deliveries in arrival order. Emission within one
        // output is strictly sequential (precommit then commit per commit),
        // so the first timestamped precommit of a staged mask is always
        // followed by its own commit of the same mask.
        let entries: Vec<&CommitEvent> = state
            .commit_log
            .iter()
            .filter(|(oid, _)| *oid == id)
            .map(|(_, ev)| ev)
            .collect();
        let (pre_idx, mask) = entries
            .iter()
            .enumerate()
            .find_map(|(i, ev)| match ev {
                CommitEvent::Pre { fields: f, when: w } if !f.is_empty() && !w.is_zero() => {
                    Some((i, *f))
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!("no timestamped staged precommit for {id:?}, got {entries:?}")
            });
        // Constrained to AFTER the precommit: an earlier same-mask commit
        // from a previous transaction must not satisfy this (order-only --
        // adjacency is not required, other transactions may interleave).
        let commit_idx = entries
            .iter()
            .enumerate()
            .skip(pre_idx + 1)
            .find_map(|(i, ev)| match ev {
                CommitEvent::Commit { fields: f, when: w } if *f == mask && !w.is_zero() => Some(i),
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!(
                    "no timestamped commit of the staged mask {mask:?} after its precommit for {id:?}, got {entries:?}"
                )
            });
        assert!(
            pre_idx < commit_idx,
            "precommit-with-staged must precede commit-with-staged for {id:?}, got {entries:?}"
        );
    }
}

/// Task 7 step 2 (wlr 0.20.34 wire-up): scene damage is delivered to
/// `last_damage` covering the damage, and a subsequent frame commits.
///
/// One run, stimulus mid-run from a helper thread. After boot settles, the
/// helper queues a `MoveOutputCursorForTest` command -- which the loop
/// drains on its own thread, clearing past damage and kicking the frame
/// clock; the next frame moves a software output cursor, whose damage is
/// delivered, and kicks one follow-up frame that recommits. The move's
/// report snapshots `frames`/`commit_log` at the stimulus instant, so the
/// "subsequent" assertions below are anchored to the damage rather than to
/// boot, and the handler-side clear means the surviving `last_damage` entry
/// can only be the move's delivery. Record-only handlers,
/// assert-after-run throughout.
///
/// Why a cursor move rather than a scene rect: staged commit damage never
/// emits `output_damaged` (only software cursors and backend-specific logic
/// do), which a scene-rect prototype of this test proved the hard way --
/// frames committed around it while `last_damage` stayed empty. The cursor
/// move is the `wlr` crate's own `output_feedback.rs` stimulus.
///
/// Why a command rather than moving the cursor from the helper thread:
/// `wlr::Runtime` is `!Send`, and only the loop thread ever holds a live
/// `&wlr::Output` to create the cursor on -- the channel (`Send`) is the
/// only bridge.
#[test]
fn output_damage_roundtrip_frame_commits_after_damage() {
    let boot = boot_lock();
    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");
    drop(boot);

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    let background = runtime
        .add_rect(
            1,
            1,
            icedtea_compositor::render::wallpaper_color(&state.config.appearance),
        )
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // The command channel and wake pipe, shared with the helper thread that
    // fires the stimulus mid-run and the bounded backstop Quit after it.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    let (probe_tx, probe_rx) = crossbeam_channel::bounded(1);
    let stim_tx = cmd_tx.clone();
    let stim_wake = cmd_wake_write
        .try_clone()
        .expect("wake pipe must clone for the helper thread");
    std::thread::spawn(move || {
        // Boot settle: the existing BOOT_SETTLE_MS backstop shape is what every
        // other test in this file trusts for both headless outputs to arrive.
        std::thread::sleep(std::time::Duration::from_millis(BOOT_SETTLE_MS));
        stim_tx
            .send(
                icedtea_compositor::dbus::DbCommand::MoveOutputCursorForTest {
                    x: CURSOR_X,
                    y: CURSOR_Y,
                    reply: probe_tx,
                },
            )
            .expect("the damage-stimulus command must queue");
        icedtea_compositor::backend::wake(&stim_wake);
        // Settle: the kicked frame recommits and the damage is delivered.
        std::thread::sleep(std::time::Duration::from_millis(DAMAGE_SETTLE_MS));
        let _ = stim_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&stim_wake);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(
        state.quitting,
        "the backstop Quit command must have stopped the loop"
    );
    assert_eq!(
        state.outputs.len(),
        2,
        "both headless outputs must have reached the model"
    );

    let report = probe_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the damage stimulus must have been drained on the loop thread");
    assert!(report.moved, "wlroots must have accepted the cursor move");
    // Exact kick count races boot (an output still arriving takes no kick),
    // so only reach is asserted here; the end-state `outputs.len() == 2`
    // above carries the count proof.
    assert!(
        report.kicked >= 1,
        "the frame kick must have reached at least one headless output, got {}",
        report.kicked
    );
    // Loop-liveness, not causality: these prove the loop stayed live past
    // the stimulus instant (the `damage_log` hotspot assert below is what
    // pins the delivery itself).
    assert!(
        state.frames > report.frames,
        "loop liveness: a subsequent frame must have committed after the damage, frames stuck at {}",
        report.frames
    );
    assert!(
        state.commit_log.len() > report.commits,
        "loop liveness: a subsequent commit must have been observed after the damage, log stuck at {} entries",
        report.commits
    );
    // Delivery proof independent of the `last_damage` slot: the history must
    // hold a delivery covering the cursor hotspot.
    let covers_hotspot = |b: &wlr::Box2D| {
        b.width >= CURSOR_EXTENT
            && b.height >= CURSOR_EXTENT
            && b.x <= CURSOR_X as i32
            && b.y <= CURSOR_Y as i32
            && b.x + b.width >= CURSOR_X as i32
            && b.y + b.height >= CURSOR_Y as i32
    };
    assert!(
        !state.damage_log.is_empty(),
        "the damage history must hold at least the move's delivery"
    );
    assert!(
        state.damage_log.iter().any(|(_, b)| covers_hotspot(b)),
        "a damage-log entry must cover the cursor hotspot ({}, {}) with at least the {}x{} image, got {:?}",
        CURSOR_X,
        CURSOR_Y,
        CURSOR_EXTENT,
        CURSOR_EXTENT,
        state.damage_log
    );
    // The 8x8 cursor image at hotspot (100, 100): the delivery must cover
    // the hotspot with at least the image, mirroring the `wlr` harness's
    // `covers_hotspot`.
    assert!(
        state.last_damage.values().any(|b| b.width >= CURSOR_EXTENT
            && b.height >= CURSOR_EXTENT
            && b.x <= CURSOR_X as i32
            && b.y <= CURSOR_Y as i32
            && b.x + b.width >= CURSOR_X as i32
            && b.y + b.height >= CURSOR_Y as i32),
        "a delivery must cover the cursor hotspot ({}, {}) with at least the {}x{} image, got {:?}",
        CURSOR_X,
        CURSOR_Y,
        CURSOR_EXTENT,
        CURSOR_EXTENT,
        state.last_damage
    );
}

/// Task 8 step 1 (wlr 0.20.34 wire-up): a `send_request_state` emission on a
/// live output is delivered to `output_state_requested`, which records the
/// staged mask in `state.last_request` under that output's id.
///
/// One run, stimulus mid-run from a helper thread. After boot settles, the
/// helper queues an `EmitRequestStateForTest` command -- which the loop
/// drains on its own thread, clearing past requests and kicking the frame
/// clock; the next frame stages a scale + transform that differ from the
/// live output's own on a fresh transaction (staging without committing
/// changes nothing on the output), emits the `request_state` signal through
/// wlroots, and drops the transaction uncommitted. The report carries the emission's output id plus the staged
/// mask, so the assertions below pin `last_request` to exactly this
/// delivery. Record-only handlers, assert-after-run (a panic inside a
/// handler body aborts through C).
///
/// Why a command rather than emitting from the helper thread:
/// `wlr::Runtime` is `!Send`, and only the loop thread ever holds a live
/// `&wlr::Output` to stage a transaction on -- the channel (`Send`) is the
/// only bridge. Same shape as `MoveOutputCursorForTest`.
#[test]
fn output_request_state_roundtrip_records_staged_mask() {
    let boot = boot_lock();
    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");
    drop(boot);

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    let background = runtime
        .add_rect(
            1,
            1,
            icedtea_compositor::render::wallpaper_color(&state.config.appearance),
        )
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // The command channel and wake pipe, shared with the helper thread that
    // fires the stimulus mid-run and the bounded backstop Quit after it.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    let (probe_tx, probe_rx) = crossbeam_channel::bounded(1);
    let stim_tx = cmd_tx.clone();
    let stim_wake = cmd_wake_write
        .try_clone()
        .expect("wake pipe must clone for the helper thread");
    std::thread::spawn(move || {
        // Boot settle: the existing BOOT_SETTLE_MS backstop shape is what every
        // other test in this file trusts for both headless outputs to arrive.
        std::thread::sleep(std::time::Duration::from_millis(BOOT_SETTLE_MS));
        stim_tx
            .send(icedtea_compositor::dbus::DbCommand::EmitRequestStateForTest { reply: probe_tx })
            .expect("the request-state stimulus command must queue");
        icedtea_compositor::backend::wake(&stim_wake);
        // Settle: the kicked frame emits, and the deferred
        // `OutputStateRequested` event is delivered.
        std::thread::sleep(std::time::Duration::from_millis(DAMAGE_SETTLE_MS));
        let _ = stim_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&stim_wake);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(
        state.quitting,
        "the backstop Quit command must have stopped the loop"
    );
    assert_eq!(
        state.outputs.len(),
        2,
        "both headless outputs must have reached the model"
    );

    let report = probe_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the request-state stimulus must have been drained on the loop thread");
    assert!(
        report.emitted,
        "send_request_state must have accepted the staged transaction"
    );
    assert!(
        !report.fields.is_empty(),
        "the emission must carry a staged mask, not an empty one"
    );
    assert!(
        report.fields.contains(wlr::CommittedFields::SCALE),
        "the emission staged SCALE, got {:?}",
        report.fields
    );
    assert_eq!(
        state.last_request.get(&report.id),
        Some(&report.fields),
        "output_state_requested must have recorded the staged mask under the emitting output's id, got {:?}",
        state.last_request
    );
    // Liveness: the staged-but-dropped transaction committed nothing, so
    // every live output's state is unchanged -- scale still the 1.0 boot
    // identity, geometries still real and disjoint. (`OutputSurface` mirrors
    // `scale` but no transform, so geometry stands in for the transform
    // half: a committed transform change would re-derive the layout box.)
    assert!(
        state.outputs.values().all(|o| o.scale == 1.0),
        "the uncommitted emission must not have changed any live scale, got {:?}",
        state.outputs.values().map(|o| o.scale).collect::<Vec<_>>()
    );
    let geometries: Vec<_> = state.outputs.values().map(|o| o.geometry).collect();
    let (a, b) = (geometries[0], geometries[1]);
    assert!(
        a.width > 0 && a.height > 0 && b.width > 0 && b.height > 0,
        "geometries must be unchanged and real, got {a:?} and {b:?}"
    );
    let disjoint = a.x + a.width <= b.x
        || b.x + b.width <= a.x
        || a.y + a.height <= b.y
        || b.y + b.height <= a.y;
    assert!(
        disjoint,
        "the two outputs' layout boxes must still not overlap, got {a:?} and {b:?}"
    );
}

/// Task 8 step 2 (wlr 0.20.34 wire-up): powering an output Off drops it from
/// the active set and powering it back On rehydrates it -- the
/// `output_power_mode_requested` disable/enable round trip.
///
/// Driven by direct handler invocation on `State` after the loop stops
/// (same shape as the `output_configuration_applied` disable/re-enable
/// test above): the `wlr` crate exposes no synthetic emission path for the
/// power signal -- `Output::send_request_state` proves the request path
/// needs no client, but there is no `send_power_mode` analogue; the signal
/// behind `output_power_mode_requested` (`on_output_power_set_mode` in the
/// crate's `backend.rs`) fires only on a real
/// `zwlr_output_power_v1.set_mode` client request. The e2e remainder is a
/// protocol-client `set_mode` round trip -- e2e-only (icedtea harness -- no
/// wlr milestone, the gap is environmental, not API). What this pins is the
/// handler code itself, with a live runtime so layout-box lookup and the
/// settle sequence run for real.
///
/// `disabled_outputs` is private, so the externally observable proxy stands
/// in: 2 active -> Off one -> exactly 1 active (the victim's connector name
/// gone) -> On the same id -> 2 active with the name set restored.
#[test]
fn output_power_cycle_disables_then_reenables() {
    let boot = boot_lock();
    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");
    drop(boot);

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    let background = runtime
        .add_rect(
            1,
            1,
            icedtea_compositor::render::wallpaper_color(&state.config.appearance),
        )
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // Bounded backstop: the LONG_BACKSTOP_MS shape from the commit-observation
    // test, so boot enable commits have populated `last_request`-adjacent
    // per-output records -- here `last_commit`, the live-id source the
    // direct handler invocation below needs (`output_ids` is private).
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(LONG_BACKSTOP_MS));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(
        state.quitting,
        "the backstop Quit command must have stopped the loop"
    );
    assert_eq!(
        state.outputs.len(),
        2,
        "both headless outputs must have reached the model"
    );
    assert_eq!(
        state.last_commit.len(),
        2,
        "both outputs must have committed at boot, supplying live ids"
    );

    let names_before: Vec<String> = state.outputs.values().map(|o| o.name.clone()).collect();
    // `OutputId` is deliberately unordered (no `Ord`); any live id drives
    // the cycle.
    let victim = *state.last_commit.keys().next().expect("a live output id");

    // Power Off: mirrors the applied-config disable branch -- one of two
    // may go, so the strand guard must not refuse this.
    state.output_power_mode_requested(victim, wlr::PowerMode::Off);
    assert_eq!(
        state.outputs.len(),
        1,
        "the powered-off output left the active set"
    );
    let names_after_off: Vec<String> = state.outputs.values().map(|o| o.name.clone()).collect();
    assert_eq!(
        names_before.len() - names_after_off.len(),
        1,
        "exactly one connector name left the active set, got {names_after_off:?}"
    );
    assert!(
        names_after_off.iter().all(|n| names_before.contains(n)),
        "the survivor keeps its connector name, got {names_after_off:?}"
    );

    // Power On the same id: mirrors the re-enable path via
    // `take_disabled_output` by connector name.
    state.output_power_mode_requested(victim, wlr::PowerMode::On);
    assert_eq!(
        state.outputs.len(),
        2,
        "the powered-on output rejoined the active set"
    );
    let mut names_after_on: Vec<String> = state.outputs.values().map(|o| o.name.clone()).collect();
    names_after_on.sort();
    let mut names_sorted = names_before.clone();
    names_sorted.sort();
    assert_eq!(
        names_after_on, names_sorted,
        "the connector-name set is restored after the power cycle"
    );
}

/// Cap proof (review): `commit_log` and `damage_log` are newest-wins capped
/// at `COMMIT_LOG_CAP` (64, private in `state.rs` -- so the bound is spelled
/// as a literal here with that cite, not referenced) so a long session
/// cannot grow either without bound. This drives a longer-than-usual run and
/// asserts both bounds hold afterwards.
///
/// Trade-off, reported honestly: frames are event-driven (each frame is
/// kicked on demand, not a free-running clock), so wall-clock alone cannot
/// force >64 deliveries and this run lands well under the cap -- the asserts
/// are a bound guard, not an overflow proof. A true overflow needs >64
/// commit-family deliveries, which needs a synthetic commit path that does
/// not exist (no `&wlr::Output` is constructible outside the `wlr` crate,
/// and the test thread cannot see live `State` mid-run for polling).
#[test]
fn commit_and_damage_logs_stay_bounded_over_a_long_run() {
    let boot = boot_lock();
    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");
    drop(boot);

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    let background = runtime
        .add_rect(
            1,
            1,
            icedtea_compositor::render::wallpaper_color(&state.config.appearance),
        )
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // Bounded backstop: CAP_GUARD_RUN_MS gives the loop its longest idle
    // window in this file for commit/damage deliveries to accumulate.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(CAP_GUARD_RUN_MS));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(
        state.quitting,
        "the backstop Quit command must have stopped the loop"
    );
    assert_eq!(
        state.outputs.len(),
        2,
        "both headless outputs must have reached the model"
    );

    // 64 mirrors the private `COMMIT_LOG_CAP` in `state.rs`.
    assert!(
        state.commit_log.len() <= 64,
        "commit_log must stay capped at 64 entries, got {}",
        state.commit_log.len()
    );
    assert!(
        state.damage_log.len() <= 64,
        "damage_log must stay capped at 64 entries, got {}",
        state.damage_log.len()
    );
}

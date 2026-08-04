# MEMORY.md — icedtea-wm session handoff

Written: 2026-08-03. Branch `develop`. Working directory `/home/joseph/Projects/icedtea-wm`.
This file captures everything a fresh agent needs to continue the work. It is a working
handoff note, not part of the spec/plan/execution docs.

## Objective

Build `icedtea-wm`: a floating Wayland window manager (Cinnamon/Windows-10 feel)
written in Rust on **Smithay**, with a separate **GTK4 shell** process, a
graphical, GUI-driven config stored in a **redb** database, a single **DBus**
control plane, and **systemd** session integration. Currently executing
**Plan 1 (compositor core, 14 tasks)** via subagent-driven development (SDD),
working in place on `develop`.

## Repo layout (virtual workspace, edition 2024, MSRV 1.94, `version.workspace = true`)

- `Cargo.toml` — workspace root (root `src/main.rs` is an orphaned hello-world
  scaffold, slated for removal before merge).
- `contract/` — `icedtea-contract`: pure-data DBus wire types + internal `Event` enum.
- `config/` — `icedtea-config`: redb-backed Config store with per-field default fallback.
- `compositor/` — `icedtea-compositor`: the WM itself (scaffold only so far).
- `docs/superpowers/` — spec + plan (authoritative requirements).
- `.superpowers/sdd/2026-08-01-compositor-core/` — SDD workspace (briefs, reports, review diffs, ledger).

## Authoritative docs

- **Spec:** `docs/superpowers/specs/2026-08-01-icedtea-wm-design.md` — approved design.
  Contains the **Threading model** section (added this session).
- **Plan 1:** `docs/superpowers/plans/2026-08-01-compositor-core.md` — the 14 tasks,
  with exact file contents and commands. **The plan is the requirement**; SDD briefs
  are generated from it.
- Plans 2 (settings GUI), 3 (shell), 4 (systemd integration) exist only in outline; not yet written.

## Locked decisions (do not relitigate)

- Pure-Rust Smithay compositor; floating-first; **edge snapping (halves/quadrants) is the only "tiling"**.
- One GTK4 shell process; **hybrid SSD** (SSD by default, CSD where apps request via `xdg-decoration`).
- **GUI-only config**, persisted to redb at `$XDG_CONFIG_HOME/icedtea/config.redb`.
  Config must **never panic on corrupt data** — fall back to defaults.
- **Single DBus control plane** (no unix-socket IPC): own service `org.icedtea.WM`,
  object path `/org/icedtea/WM`, on the session bus.
- **Compositor is the sole owner of window state.** Every state change flows
  compositor → DBus signal → shell.
- systemd: units under `graphical-session.target`, logind power menu + fullscreen
  suspend inhibit, autostart via systemd-xdg-autostart-generator.
- Pins: `smithay = "0.7"`, `zbus = "5"`, `redb = "3"`, `gtk4-layer-shell = "0.8"`
  (GTK3 layer-shell wrapper is abandoned — GTK4 only, no fallback). No `unsafe`
  except `redb::Database::create/open` (each with a comment).

## Threading model (added this session — user asked "make this as multi-threaded as possible")

The Wayland/render/input core runs on a **single calloop loop** — a hard
Smithay/Wayland constraint (protocol ordering, per-output GL contexts), and the
loop is latency-bound, not throughput-bound. Everything that can leave the
critical path runs on **worker threads communicating with the main loop only by
message passing** (crossbeam channels registered as calloop sources; no shared
mutable state; results applied on the main loop):

1. DBus service + signal emitter — own thread (Task 12).
2. Config load/reload — redb I/O + JSON parse off the loop (Tasks 7, 13).
3. Wallpaper image decode — CPU-heavy decode on a worker; RGBA shipped to the
   main loop, GL upload happens on the render thread; solid color until ready (Task 8).

Encoded in: spec "Threading model" section, plan global constraint, and Task 7/8/13
amendments. Committed as `30b0618`.

## Wire contract (Task 2 — shared by compositor, shell, settings over DBus)

`option-as-array` zvariant feature enabled; signatures recorded in the plan's Task 2
"Wire signatures" note:
- `WindowId` = `u`, `Rectangle` = `(iiii)`.
- `WindowUpdate` = `(asa(iiii)auabababab)`, `Appearance` = `(siii(sss)as)`.
- Every Rust crate deriving these types must use the same feature set (single
  zvariant 5.13.1, feature unification via Cargo.lock). The GTK shell (gio, Plan 3)
  must marshal the recorded signatures.

## Git history (develop)

```
26e29ce feat(config): redb-backed config store with per-field default fallback   <- Task 3 (see open items)
d2e2f46 chore(contract): document option-as-array wire signatures and test Option round-trip  <- Task 2 follow-up
30b0618 docs: encode threading model (worker threads off the calloop loop)       <- docs only
dfa0935 feat(contract): define DBus wire types and internal event enum           <- Task 2
85e2289 chore: scaffold icedtea workspace with contract/config/compositor crates <- Task 1
70d4d6c docs: define State::emit helper in plan task 11                          <- pre-handoff baseline
```

## SDD method (how the work runs)

One implementer subagent + one read-only reviewer subagent per task, using scripts
in `/home/joseph/.claude/skills/subagent-driven-development/scripts/` and prompt
templates in the same skill. Pattern per task:
1. `scripts/task-brief <plan> <N>` → writes `task-N-brief.md`; record BASE SHA.
2. Dispatch implementer subagent (general) → `task-N-report.md`, commit, `cargo test` + `clippy`.
3. `scripts/review-package <plan> <base> <head>` → `review-<base>..<head>.diff`.
4. Dispatch reviewer subagent (read-only) → `task-N-review.md`, verdict.
5. APPROVED_WITH_MINOR_ISSUES → resume the implementer subagent via its `task_id`
   to fix, then commit again.
6. Update `progress.md` ledger + todo list; continue. Keep moving; no check-ins.

## Task status

- **[Task 1]** DONE, reviewed clean, `85e2289` (workspace + 3 crates scaffolded).
- **[Task 2]** DONE, `dfa0935` + follow-up `d2e2f46`, APPROVED_WITH_MINOR_ISSUES (both fixed). Wire signatures recorded in plan.
- **[Task 3]** Implemented (`26e29ce`), self-report DONE_WITH_CONCERNS. **Review was cancelled by the user** — see open items.
- Tasks 4–14 + final whole-branch review: not started.

## Open items / state the next agent must resolve

1. **Task 3 review is pending** (the reviewer was cancelled mid-flight). Before
   starting Task 4: run `scripts/review-package docs/superpowers/plans/2026-08-01-compositor-core.md d2e2f46 26e29ce`,
   dispatch the Task 3 reviewer, triage the verdict. Verify the core invariant
   (config corruption → defaults, never panic) yourself.
2. **Uncommitted work in the tree from the Task 3 implementer** (never committed, never reviewed):
   - `config/tests/corruption_manual.rs` (untracked) — corruption/truncation integration tests.
   - `Cargo.lock` — adds `tempfile` + `tracing` to `icedtea-config` deps.
   These look intentional (they exercise the no-panic-on-corruption invariant) but
   were left out of `26e29ce`. Decide: fold into Task 3 (commit, include in review)
   or discard. Don't carry them silently into later tasks.
3. **Orphaned** `src/main.rs` ("Hello, world!") unowned by any crate — remove before merge.
4. Plan corrections baked in: Task 4 `add_window` signature becomes 5-arg (adds
   geometry) at Task 11; `compositor/src/config_combo.rs` re-exports
   `icedtea_config::KeyCombo`; `remove_window`/`set_active_workspace` fixes already
   applied to the plan.

## Task 3 concerns raised by the implementer (verify during review)

- The config layout is **5 JSON tables** (not "r/w content table/key main/bincode");
  the implementer followed the brief/plan verbatim, and later consumers (esp. Task 13)
  must expect the 5-JSON-table layout.
- `config/Cargo.toml` aliases the contract dep as `contract` (package `icedtea-contract`)
  so the brief's `contract::` paths compile — config-internal only.
- Several compiler/clippy-driven deviations from the brief's verbatim code (dropped
  now-unnecessary `unsafe`, `.as_bytes()`, let-chains, extra imports) — claimed
  behavior-identical; reviewer should confirm.

## Environment / tooling notes

- anvil (smithay 0.7 tag) is the API authority for compositor glue; nested backend
  `--nested` for dev/test.
- Config DB path helper: `icedtea_config::default_db_path()` → `$XDG_CONFIG_HOME/icedtea/config.redb`.
- SDD scripts + prompt templates live in the Claude skill at
  `/home/joseph/.claude/skills/subagent-driven-development/` (subagent-driven-development skill).
- Gates: `cargo test --workspace` and `cargo clippy --all-targets -- -D warnings`.

## Recommended next steps

1. Resolve open item #2 (commit or discard the uncommitted Task 3 files).
2. Run + triage the Task 3 review (open item #1).
3. Update `progress.md` + todos (Task 3 → done).
4. `scripts/task-brief docs/superpowers/plans/2026-08-01-compositor-core.md 4` →
   implement Task 4 (window.rs window/workspace model) → review → Task 5, and so on
   through Task 14, then the final whole-branch review.
5. Remove orphaned `src/main.rs` before merge.

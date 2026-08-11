# icedtea-fm Design

Date: 2026-08-05

## Overview

`icedtea-fm` is a Dolphin-like file manager for the icedtea desktop, built on
GTK4 in Rust. It is a companion app to the icedtea-wm shell: a regular GTK
window launched from the start menu, sharing the workspace's plain-GTK4 +
custom-CSS theming convention (no libadwaita). GIO (`gio`) is the filesystem
and virtual-filesystem layer; a small redb persistence layer (via the existing
`icedtea-config` crate's patterns) stores places, bookmarks, and per-folder
view settings — but **not** a content/search index.

The application is split into a **pure, GTK-free model core** and a **thin GTK
view layer**. The core is a command/event state machine with a pure reducer —
the same idiom the icedtea shell uses for its DBus signal reducer — so
navigation, history, selection, file-op orchestration, search, and undo/redo
are all unit-testable without a display.

## Scope (v1)

In scope:

- Core Dolphin navigation: places sidebar, breadcrumb/location bar, icon/list/
  details/compact views, tabs, forward/back/up history.
- Info panel (right-hand, collapsible) and split (dual-pane) view.
- Keyboard navigation & selection (type-to-find, F2 rename inline, Home/End/
  PgUp/PgDn, state-aware rubber-band selection).
- File operations via GIO with a progress dialog and an undo/redo log.
- Eager metadata for the details view, and per-folder view settings.
- Streaming (non-indexed) filename/path search.

Out of scope for v1:

- Embedded terminal panel.
- Full preview panel / embedded document viewer.
- Ratings, tags, or a persistent metadata/search index (future work).
- A DBus service / own control plane (launched on demand only).
- "Open terminal here", drag-based mount/unmount management.

## Positioning

- Workspace member crate: `icedtea-fm/` (binary `icedtea-fm`).
- App id `org.icedtea.FM`; window title "IcedTea Files".
- MSRV 1.94, edition 2024, `version.workspace = true` — matches the workspace.

## Architecture

```
┌───────────────────────────────────────────────────────────┐
│ icedtea-fm (GTK4 binary)                                  │
│                                                            │
│  ┌─ gtk/ (view — dumb, renders, forwards gestures)        │
│  │   window, panes/splitter, notebook tabs, places,       │
│  │   breadcrumb/location, view panels, info, ops dialogs,  │
│  │   css                                                  │
│  └──────────────┬─────────────────────────────────────────┘
│                 │ Commands + subscriptions (GLib main ctx)
│  ┌──────────────▼─────────────────────────────────────────┐
│  │ core/ (pure Rust, NO GTK)                              │
│  │  reducer:(State, Command) → (State, [Effect])          │
│  │  nav, search, fileops (+undo log), state model         │
│  └──────────────┬─────────────────────────────────────────┘
│                 │ async effects (GIO / glib worker threads)
│                 ▼  results returned as Events → reducer again
│  GIO (fs + VFS, mounts, cancellables) · redb (placed state)
└───────────────────────────────────────────────────────────┘
```

### Threading & asynchronicity

- No blocking I/O on the GTK main thread.
- Directory listing: `gio::File::list_children_async`.
- Thumbnails: bounded glib worker, decode off-thread, RGBA shipped back and
  posted into the model as an event.
- Copy/move/trash/rename: GIO async with progress callbacks. Every callback
  result re-enters the reducer as an `Event`; results are applied on the main
  context.
- No shared mutable state across threads; the model is mutated only by the
  reducer on the main thread.

## Core model

Command enum (user gestures → model):

```
Open(location) | OpenParent | Up | Back | Forward | GoToHome | GoTo(path)
Select(paths) | Focus(path) | SelectRange(end) | MoveFocus(delta) | RubberBand(rect)
SetViewKind(kind) | SetSortBy(field) | ToggleShowHidden | SetFilter(text)
SetLocationEntry(text) | CompleteLocationEntry
Copy {paths, dest} | Move {paths, dest} | Trash {paths} | Delete {paths}
Rename {path, new_name} | NewFolder {parent, name} | OpenPotentialExecutable(path)
Undo() | Redo()
Search {query, recursive}
```

Effect (async work to perform, dispatched by the view):

```
ListDir(location) → Event::DirListed(entries)
LoadMetadata(location) → Event::Metadata(attrs)
Thumb(path) → Event::ThumbReady(path, rgba)
RequestedOp(Op) → progress + Event::OpProgress/Event::OpFinished
Search(location, query) → Event::SearchResults(entries)
```

The reducer is `pub fn reduce(state: &mut State, cmd: Command) -> Vec<Effect>` —
pure apart from producing effects; unit-tested exhaustively.

`State` (immutable snapshots held by the view):

```
State {
  window: WindowState {
     tabs: Vec<Tab>,
     active_tab,
     split: Single | Split {ratio},
     active_pane,
  },
  history: / per-tab forward/back stack
  selection: ordered Vec<Path>,
  view_kind (from per-folder settings),
  sort, show_hidden, filter,
  undo: Vec<undoable op>, redo: Vec<Reundoable>,
  running_op: Option<Operation> (progress, cancellable)
}
Tab { path (current folder), own history, panes }
Pane { current folder, selection, scroll position }
```

## Views (GTK layer)

- **Places sidebar** (`gtk/places.rs`): Home, Desktop, Documents/Downloads/
  Pictures (from XDG user dirs), root `/`, removable/mount entries (from
  `gio::FileMonitor`/mounts), user bookmarks (from redb). Item = `Location`.
  Selection emits `Command`. Drag a folder entry onto the list →
  `Command::AddBookmark`.
- **Breadcrumb bar** (`gtk/breadcrumb.rs`): clickable ancestors; `Ctrl+L` (or
  double-click a segment) toggles to an editable location entry with tab
  completion. This is the classic Dolphin breadcrumb ↔ location-bar swap.
- **View panels** (`gtk/view.rs`), built on GTK4 list APIs:
  - Icons → `GtkGridView`
  - List → `GtkListView`
  - Details → `GtkColumnView` — columns: name, size, date, type, permissions
  - Compact → `GtkListView`, one-line rows
  - One shared selection adapter per pane. Eager metadata fetch per directory
    (fast enough for typical folders; sort-by-size/date complete).
- **Info panel** (`gtk/info.rs`): right-hand, collapsible. Icon or cheap
  image/text preview, name, type, size, modified, permissions, and a
  Properties button that opens attributes driven by `gio` metadata.
- **Tabs + split** (`gtk/window.rs`): `Gtk::Notebook` tabs; each tab holds a
  view container that is a single pane or a split (`GtkPaned`) of two panes.
  Panes in a split share the tab's history model but have independent
  selection, focus, and current folder.
- **Progress & confirm dialogs** (`gtk/ops.rs`): one operations window for
  copy/move/trash with overall + per-file progress, pause/cancel, and conflict
  resolution (skip / replace / rename / apply-to-all). Deletes confirm;
  trash does not.

## File operations, undo/redo (core `fileops.rs`)

- All ops use GIO: copy/move via `GFile`, trash via `trash()`, delete via
  `delete()`, rename via `rename()`. Cancel via `gio::Cancellable`.
- Cross-device copy handled by GIO off-thread.
- Each mutating op records an undoable `Op` with its inverse precomputed:

| Op          | Inverse |
| ----------- | ------- |
| Copy a→b    | Delete b |
| Move a→b   | Move b→a |
| Rename a→b | Rename b→a |
| Trash a | Restore a from trash |
| Delete a | (not undoable) |
| NewFolder  | Remove the folder (if empty) |

- Undo/redo stack bounded (default 100 ops); every undo/redo re-runs through
  the same async GIO path, re-entering the reducer with progress events.

## Persistence (redb)

Separate DB at `$XDG_CONFIG_HOME/icedtea/fm.redb` (the config DB is the WM's).
Tables, following the `icedtea-config` default-fallback ethos — corruption or a
missing value falls back to defaults, never panics:

- `bookmarks` — ordered location list.
- `places` — collapsed/expanded state of the sidebar groups.
- `view_state` — keyed by folder path → { view kind, sort by, sort dir,
  show_hidden } (per-folder settings).
- `session` — last open folder / tab URLs (simple, best-effort).

## Theming / integration

- Plain GTK4 + a custom CSS sheet (mirrors the shell). Colors follow the same
  palette conventions as `icedtea-config` appearance so the app can adopt the
  WM's theme (pulled over a portal/DBus in a future iteration; v1 uses the same
  CSS constants and `prefers-color-scheme` awareness).
- Launched from the start menu like any installed app; opened files go through
  `org.freedesktop.portal.OpenURI`/`xdg-open`.

## Error handling

- Config/state corruption or missing values → defaults (matches
  `icedtea-config` never-panic contract).
- Failed file ops → model event → ops dialog error, operation can be retried;
  partial progress preserved for a cancelled op is surfaced to the user.
- Unmapped VFS (e.g. mount drops mid-view) → view refreshes to the nearest
  live ancestor location; the location model risks **no unknown paths**.

## Testing

- `core/reducer.rs`, `core/nav.rs`, `core/fileops.rs`, `core/search.rs`: pure
  unit tests — command→state transitions, history, undo/redo sequences, sort
  order, path parsing, details-metadata mapping. No GTK; plain `cargo test`.
- `core/fileops.rs`: inverse-computation tests over a `tempfile` temp dir tree
  (like `config`'s `corruption_manual.rs` integration pattern), no GTK.
- GTK layer: minimal `gtk`-backed smoke tests where practical; primary manual
  verification against a real folder-tree, plus the workspace gates
  `cargo test --workspace` and `cargo clippy --all-targets -- -D warnings`.

## Risks

- GTK4 list selection-model complexity (multi-select + keyboard + rubber band
  across two panes) is the main UI risk; mitigated by driving all selection
  through the pure core so widget quirks stay bounded in `gtk/view.rs`.
- Eager metadata on very large folders can make first paint slow; mitigations:
  the sort is applied once the (eager) metadata loads asynchronously, and a
  spinner/placeholder renders immediately.
- GIO API surface is large; v1 uses only list/trash/delete/rename/copy/move/
  metadata + portals — bounded and documented per call site.

## Unknowns / to confirm during writing of the plan

- Exact `gtk4` crate features and versions to pin (`gtk4 = { version = ..., features = [...] }`), plus `gio`, `glib` feature unification with the rest of the workspace.
- Whether thumbnailing uses `image = "0.25"` (already a workspace dep) or GTK's built-in `GdkPixbuf`.
- Placement of the `redb::Database::create`-style unsafe boundary in a helper matching `icedtea_config::open`.

## Crate layout

```
icedtea-fm/
├─ Cargo.toml
├─ src/
│  ├─ main.rs
│  ├─ core/
│  │  ├─ mod.rs · reducer.rs · state.rs · nav.rs · fileops.rs · search.rs
│  ├─ gtk/
│  │  ├─ mod.rs · window.rs · view.rs · places.rs · breadcrumb.rs · info.rs ·
│  │  │  ops.rs · css.rs
│  └─ persist.rs        (redb: bookmarks/places/view_state/session)
└─ tests/
   └─ e2e.rs            (optional smoke)
```
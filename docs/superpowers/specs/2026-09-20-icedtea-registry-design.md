# icedtea Registry — Design

**Date:** 2026-09-20
**Status:** proposed design — for review, then an implementation plan
**Scope:** a unified, hierarchical, typed, live-notifying configuration and
registration store for the whole desktop environment, replacing `icedtea-config`.

## Motivation

`icedtea-config` is a redb file with **flat, Rust-typed section tables**
(`config/src/schema.rs`): `appearance`, `behavior`, `power`, `workspaces`,
`displays`, `launcher`, `keybindings` are each a single JSON blob under one key.
It works, but three properties cap it:

1. **Whole-section granularity.** A change to one appearance field rewrites and
   reloads the whole `Appearance` struct. There is no way to watch, default, or
   revert a single setting.
2. **Multiple writers over one file.** redb takes a whole-file lock, so the
   settings app, the compositor, the launcher and the session daemon cannot all
   hold the store. This is a live error path (`DatabaseAlreadyOpen`,
   `config/src/lib.rs`) and the reason for the per-writer scoped saves
   (`save_launcher`, `save_displays`) — a workaround that still leaves a
   documented open race (`full_save_with_stale_launcher_clobbers_fresh_rows`).
3. **Nowhere to register anything.** The DE has settings but no home for
   *registrations*: default MIME/URI handlers, autostart entries, or the DE's own
   daemon/dbus facts. The roadmap lists the resulting dead ends — no default-apps
   page, no XDG autostart runner, third-party `.desktop` files with no registry
   of who owns what (`docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md`).

The Windows Registry is the wrong shape to copy, but it names the right
category: a single place where the system's settings *and* its registrations
live, that everything can query and watch. The Linux-native form of that is the
dconf/GSettings architecture — a broker daemon over a typed, path-keyed store
with change notification. This spec designs icedtea's version.

## Goals

- One namespace holding both settings and registrations; one change bus; one
  tool. After cutover there is exactly **one** source of configuration truth.
- Per-key typed values with schema-declared defaults, and a clean distinction
  between "unset" (revert to default) and "explicitly empty".
- Live change notification: a write by any process is observed by every watcher
  with no manual reload call.
- Registrations (default handlers, autostart, DE daemons) queryable through the
  same API as settings.
- `icedtea-config` deleted; an existing `config.redb` migrates without data loss
  and with its semantics preserved.

## Non-goals

- A system/root configuration layer. The registry is **one user-owned file**.
- Authorization, policy, lockdown, or sandboxing of writers. The write model is
  open (see Security posture).
- A standalone schema artifact consumable by non-Rust authors.
- Multi-user, remote, or networked registries.
- Secret storage — secrets belong to the keyring (roadmap A8), not here.
- Windows-Registry parity beyond the concept: no hives, no env-expanding values,
  no cross-process transactions beyond the single daemon.

## Decisions

Every direction below was chosen explicitly during design; they are load-bearing
and each later change should be treated as a rewrite, not a tweak.

| # | Question | Decision |
|---|----------|----------|
| Q1 | Scope of the registry | **Unified** — settings *and* registrations in one namespace |
| Q2 | Who may write | **Open** — any process running as the user; no per-key authorization |
| Q3 | On-disk model | **Single path-keyed redb file** |
| Q4 | Layering | **One user-owned file**; distro/admin defaults are schema defaults |
| Q5 | Schema declaration | **Rust**, declared in code |
| Q6 | Access model | **Broker daemon** (`icedtea-registryd`), sole file opener, D-Bus API |
| Q7 | Value type model | **Per-key Rust types**, pinned by the schema |
| Q8 | Unregistered paths | **Linked schema crate + open fallback** — unregistered writes allowed |
| Q9 | Registrations | **One composite `Record`/`RecordList` type**, not relational rows |
| Q10 | Migration | **Full replacement of `icedtea-config`, one milestone** |

## Architecture

Five parts, one writer, no direct file access by anyone but the broker.

- **`icedtea-registry`** (library) — the client and the vocabulary: `Value`
  (tagged), `KeySpec`, the schema-registration API, the D-Bus proxy
  (`org.icedtea.Registry`), typed accessors, and watch handles. Depends on
  `zbus` and the schema crate only.
- **`icedtea-registry-schema`** (leaf crate) — every first-party schema, one
  module per domain (`appearance`, `behavior`, `power`, `workspaces`,
  `keybindings`, `displays`, `launcher`, `handlers`, `autostart`, `daemons`),
  each exposing `pub const SPECS: &[KeySpec]`, plus the typed structs that today
  live in `contract`/`config`. The crate unions them into `first_party_schema()`.
  Linked by the daemon **and** by components, so both share one definition
  (Q8). A leaf crate avoids the dependency cycle of the daemon depending on the
  compositor.
- **`icedtea-registryd`** (binary) — the broker. Sole opener/writer of
  `registry.redb`; unions the schema at boot; enforces path grammar, tag and
  range conformance, and resource caps; commits atomically; emits `Changed`.
  Boring by contract: no policy, no values it was not handed, no business logic.
- **`icedtea-registry-cli`** (binary `registry`) — generic
  `list/get/set/reset/watch/dump/load`. Reads the wire type tag, so it renders
  unregistered keys it has no schema for.
- **Importer** — a daemon first-boot mode (`icedtea-registryd --import
  <config.redb>`), not a separate binary, so there is exactly one opener ever.

Flow: a component links the schema crate, calls `Registry::connect()` over the
session bus, then `get`/`set`/`watch`. The daemon starts before consumers
(systemd `Before=` on the DE target), loads the schema union, opens the DB, and
serves. **No process reads the file directly.** If the daemon is down, reads
serve last-known cache plus schema defaults; writes fail loudly.

## Data model

### Paths

One canonical grammar, lower-case slash-separated segments. The top-level roots
act as the registry's hives:

- `/org/icedtea/<domain>/…` — first-party settings (`appearance`, `behavior`,
  `power`, `workspaces`, `displays`, `keybindings`, `launcher`, `session`).
- `/desktop/mime/<type>/…`, `/desktop/scheme/<scheme>/…`,
  `/desktop/autostart/<id>`, `/desktop/daemons/<name>` — registrations.
- `/apps/<app-id>/…` — third-party keys (the open Q2/Q8 surface).

Segments are `[a-z0-9._-]+`, except MIME type keys, where the `type/subtype`
slash is folded by a single canonicalizer (`mime_path("text/html")`). No caller
hand-writes an escaped path; the canonicalizer is the only place the fold is
defined.

### Values

A closed, tagged set:

| Tag | Payload |
|-----|---------|
| `Str` | UTF-8 string |
| `Int` | i64 |
| `Uint` | u64 |
| `Bool` | bool |
| `Double` | f64 |
| `Bytes` | byte string |
| `StrList` | ordered list of string |
| `Null` | explicitly empty, **distinct from unset** |
| `Record` | string-keyed, values from the scalar rows above (one level) |
| `RecordList` | ordered list of `Record` |

No deeper nesting. Every value carries its tag on the wire, so the daemon can
validate a registered key and the CLI can render an unregistered one. "Per-key
Rust type" (Q7) means the schema pins the tag and the library offers a typed
accessor over it — not a monomorphized wire type.

`Null` is in v1 because `Option`-typed keys (`wallpaper`, `locker_command`)
would otherwise be unable to express "explicitly none" against a future non-null
default, and adding the tag later would be a wire change.

### Unset vs default

The load-bearing distinction: `Unset(path)` deletes the stored row and the key
reverts to its **schema default**; it never means "set to null". `Reset(prefix)`
is recursive unset. A registered key resolves `stored → default → absent`; an
unregistered key has no default and simply is or is not there.

### Per-field keys

`Appearance` becomes `/org/icedtea/appearance/{bar_position,bar_height,
corner_radius,snap_gap,palette,wallpaper}`, each its own key. This is the
difference between a registry and a config file: a change to `bar_height` wakes
only watchers of `bar_height`; a per-field `Reset` is meaningful; a settings page
becomes a rendered list of specs. `Record`/`RecordList` are then used only where
a value genuinely is compound — a handler entry, tile groups, recency — not for
the settings structs themselves.

Displays become per-output subtrees (`/org/icedtea/displays/<output>/…`),
reconstructed by listing the subtree. Pins are a `StrList`; tile groups a
`RecordList`; recency a **per-app subtree**
`/org/icedtea/launcher/recency/<app-id>/{count,last_seq}`, because today's value
is a map of tuples, which a one-level `Record` cannot hold.

### Commits

`SetMany` writes in one redb transaction and emits a single coalesced `Changed`
with a monotonic `seq`. Watch subscriptions can resume from a `seq`, so a watcher
that reconnects never silently misses a change.

## Wire protocol

One interface, `org.icedtea.Registry`, at `/org/icedtea/Registry` on the
**session** bus, one daemon instance (a second owner attempt fails and exits).
Values marshal as a self-describing `(y tag, v payload)` pair — `v` is a
`zvariant` Variant, `Record`/`RecordList` ride as `a{sv}`/`aa{sv}` — so any
D-Bus client can read a value it has no schema for.

### Methods

- `Get(path) -> (y tag, v value, b is_default)` — effective value (stored, else
  schema default); `NotFound` for an absent unregistered key.
- `GetStored(path) -> (b present, y tag, v value)` — "did the user set this, or
  am I seeing the default?" — what the settings app's Revert needs.
- `Set(path, tag, value)` — tag validated against the schema for registered
  keys; any valid tag accepted for unregistered keys.
- `Unset(path)` / `Reset(prefix)` — remove one row / recursively revert a
  subtree.
- `SetMany(changes: a(syv))` / `UnsetMany(paths: as)` — one transaction, one
  `seq`, one signal.
- `List(prefix, recurse) -> a(sy)` — paths and tags, cheap.
- `Dump(prefix) -> a(syv)` — paths, tags and values.
- `Spec(path) -> a{sv}` — description, type, default, and allowed range/enum for
  a registered key. This is what lets the settings app grow a page for a domain
  it does not hardcode.
- `ListAppend(path, value, before: b)` / `ListRemove(path, value)` — atomic
  daemon-side list mutation (see Registrations).
- `Watch(prefix) -> u` / `Unwatch(u)`.

### Signals

- `Changed(changes: a(syb), seq: t)` — `b` = present, so unset/revert is
  signalled distinctly; watchers re-`Get` rather than trusting a racy value.
- `LostChanges(from_seq)` — emitted when a watcher falls behind the daemon's
  coalescing window, so it knows to re-`Dump` rather than resume.

### Lifetime and errors

Subscriptions are per-connection and die with it; clients re-`Watch` and pass
their last `seq` to detect a gap. `NameOwnerChanged` tells clients the daemon
restarted, after which they re-subscribe and re-read. Errors are
`org.icedtea.Registry.Error.{InvalidPath, TypeMismatch, NotFound,
OutOfRange, LimitExceeded, DaemonUnavailable}`. There is deliberately no
`UnknownKey` error: unregistered writes are legal.

## Schema and typed access

**`KeySpec`** is the unit of declaration: `path`, `tag`, `default`,
`description`, optional `Choices`/`Range`, and `since: u64`. Each domain module
in `icedtea-registry-schema` exposes `pub const SPECS: &[KeySpec]`; the crate
unions them into one `first_party_schema()`. Plain consts; a `define!` wrapper
can come later if the boilerplate earns it.

**Typed access.** `KeySpec`-driven getters —
`registry.get::<u32>("/org/icedtea/appearance/bar_height")?` — plus a domain
convenience that assembles `Appearance::load(&reg)` and re-assembles on a prefix
watch. Compile-time safety at the Rust boundary, the generic tagged model
underneath.

**Validation**, all in the daemon, one code path:

1. path grammar → `InvalidPath`;
2. tag vs. spec → `TypeMismatch`;
3. `Choices`/`Range` → `OutOfRange`;
4. resource caps (path depth, segment length, key count, value size ~1 MiB, list
   length) → `LimitExceeded`.

Unregistered keys skip steps 2–3.

**Versioning.** `meta.schema_version` inherits `icedtea-config`'s discipline
exactly: one-time migrations gated on a **fixed** boundary constant (never
re-armed by later version bumps; `config/src/lib.rs` documents why), and an
unreadable version means warn and do **not** migrate. Migrations are per-key Rust
functions in the daemon. The A3 power-key backfill becomes registry migration #1.
The never-panic/never-lose-data contract carries over: a corrupt file degrades to
schema defaults with a warning, never a crash.

## Registrations

Same store, same bus, `/desktop/…` and `/apps/…` roots.

### Shapes

- `/desktop/mime/<type>/handlers` — `RecordList`; **array order is
  authoritative**; each entry `{id, source}` where `source ∈ {user, distro,
  vendor}` records why it is there (provenance for debugging and ranking, not a
  layer system).
- `/desktop/scheme/<scheme>/handlers` — identical shape for URI schemes.
- `/desktop/mime/<type>/removed` — `StrList` of ids the user suppressed.
  A rescan must not resurrect a removed association — the same class of bug as
  the A3 backfill re-adding a deliberately removed binding, and it gets the same
  explicit treatment.
- `/desktop/autostart/<id>` — `Record{exec, name, icon, enabled}`.
- `/desktop/daemons/<name>` — `Record{bus_name, enabled}`: the DE's own service
  registry (clipboard, notifications, session, registry itself).
- `/apps/<app-id>` — `Record{name, icon, exec, categories, mime_types}`: what a
  `.desktop` file normalized to.

### Ingestion

A first-party scanner reads `.desktop` files from `/usr/share/applications`,
`$XDG_DATA_DIRS`, and `$XDG_DATA_HOME`, plus shared-MIME-info, and writes
normalized entries with `source=vendor`, merging claims into `handlers` while
**never** re-adding anything in `removed` and never reordering a user-chosen
default. It is idempotent: running it twice changes nothing. Exec lines are
parsed, never shell-evaluated, and desktop-entry escaping is validated on the way
in — this is the one genuinely hostile input in the design.

### List mutation

Because the daemon is the sole opener, list edits are atomic inside its
transaction. `ListAppend`/`ListRemove` are daemon-side operations, not a client
`Get`/`Set` dance: a third-party app registering a handler is one call, and two
concurrent registrations cannot lose each other.

## Migration

**Importer.** `import_config(&Config) -> Vec<(path, Value)>` is a pure function
in `icedtea-registry-schema` (unit-testable with no DB); the daemon invokes it on
first boot when `registry.redb` is absent and `config.redb` is present.
`config.redb` is renamed to `config.redb.migrated`, never deleted. The mapping is
mechanical:

| Today | Registry |
|-------|----------|
| `appearance` | `/org/icedtea/appearance/*` (six keys) |
| `behavior` | `/org/icedtea/behavior/*` (three keys) |
| `power` | `/org/icedtea/power/*` (three keys) |
| `workspaces.names` | `/org/icedtea/workspaces/names` (`StrList`) |
| `displays` | `/org/icedtea/displays/<output>/*` |
| `launcher.pinned` | `/org/icedtea/launcher/pinned` (`StrList`) |
| `launcher.tile_groups` | `/org/icedtea/launcher/tile_groups` (`RecordList`) |
| `launcher.recency` | `/org/icedtea/launcher/recency/<app-id>/{count,last_seq}` |
| `keybindings.<action>` | `/org/icedtea/keybindings/<action>` (`Record{modifiers, key}`) |

**Semantics carried over, not reinvented**

- *Per-field fallback*: an unparsable field imports as absent, so the schema
  default applies — net-identical to today, except the importer **logs** each
  dropped field instead of degrading silently.
- *Never-panic*: reuse `icedtea-config`'s `catch_unwind_silently` open path; a
  corrupt `config.redb` imports nothing, warns, and starts on schema defaults.
- *One-time backfill*: the A3 power-key migration becomes registry migration #1,
  gated on the same fixed boundary, with the same "do not overwrite the user's
  action, do not introduce a duplicate chord" rules. Its existing tests are
  ported verbatim.

**Cutover** (one milestone; inside it, a firm order): build alongside →
transcribe every `Config` struct into `KeySpec`s and port the config test suite →
land importer and prove `import then reassemble == original` for a real
`config.redb` → flip compositor, settings, shell, launcher and session to
registry reads/watch in one branch → delete `icedtea-config` and the old
`Reloaded`/scoped-save paths. No dual-write, ever: one truth at the flip.

**Completion gate**: a test asserting every path is declared exactly once, and a
dependency check showing no crate depends on `icedtea-config`.

## Security and trust posture

**There is no authorization boundary between processes running as the same
user.** Any app can read, write, or reset any key, including core DE settings.
That is the dconf model and it is a deliberate, documented choice — not an
oversight. The daemon enforces well-formedness, never permission.

What it does enforce:

- `registry.redb` is created user-owned, mode `0600`; the daemon is the only
  opener.
- Path grammar, tag-vs-spec, `Choices`/`Range`, and the resource caps above. A
  runaway or malicious writer gets an error, not a full disk or an OOM'd daemon.
- Hostile input on the way in: ingested `Exec` lines are parsed, never
  shell-evaluated, with desktop-entry escaping validated; MIME XML is read
  element-wise with entity expansion and DTDs disabled and a size cap (no
  billion-laughs/XXE); oversize or malformed entries are dropped and logged,
  never partially applied.

Two things are explicit:

- The `source` field is **informational, not authenticated** — a writer can claim
  `user`. Nothing may make a security decision from it.
- `/desktop/autostart` and locker-command-shaped values are a
  **persistence/execution surface**. This grants no privilege a user's own
  process did not already have, but it is login persistence, and consumers
  (`session`) must validate values before executing them rather than trusting the
  store. Gating autostart writes on an explicit user action is a flagged option,
  **not** a v1 requirement.

## Testing and verification

- **Schema (pure, no DB):** every path declared exactly once; every `default`
  tag-matches its `KeySpec`; `import_config` round-trips a real `Config`
  (`import` then reassemble == original) — the property that makes the cutover
  safe.
- **Daemon (temp redb, real D-Bus client):** `Set/Get/Unset/Reset/SetMany/List/
  Watch`, following the repo's headless-daemon discipline; `Watch` correctness
  via a writer loop plus a watcher that disconnects and resumes from `seq`,
  asserting no gap or a `LostChanges`.
- **Ported verbatim from `icedtea-config`:** per-section round-trip, corruption →
  defaults without panic (truncate mid-write, mirroring
  `config/tests/corruption_manual.rs`), A3 one-time backfill, unreadable version
  → do-not-migrate. The old
  `full_save_with_stale_launcher_clobbers_fresh_rows` test is replaced by its
  inverse: concurrency without clobber, now true by construction.
- **Concurrency:** N writers and M watchers; final state consistent and every
  watcher's observed set a superset of every commit (no lost update).
- **Cutover gate:** schema-completeness test, dependency check for
  `icedtea-config`, and a harness E2E where a settings write reaches the
  compositor live with no reload call.
- **CLI:** `get/set/list/watch` against a live daemon, including an unregistered
  key.

## Risks

- **Scope.** Touches every crate and deletes a working subsystem. Mitigated by
  the staged build, the single flip, the ported test suite, and the
  `import then reassemble == original` property test.
- **New single point of failure.** Today a missing library just reopens the file;
  now a dead daemon costs configuration reads. Mitigated by cache+defaults reads,
  a deliberately trivial daemon, and boot ordering — but it is a real regression
  in failure mode and should be measured, not waved away.
- **Open writes.** A buggy app can corrupt core settings. Capped and ranged where
  declared; residual accepted under Q2.
- **Change-storm.** A writer setting many keys in a loop can wake all watchers.
  Mitigated by `SetMany`, a daemon coalescing window, and `LostChanges`;
  watch granularity must stay prefix-based.
- **Schema drift.** Two crates declaring the same path. Mitigated by a
  completeness test in CI and by the daemon refusing to boot on a duplicate path.

## Open points resolved

1. `launcher.recency` → per-app subtree
   `/org/icedtea/launcher/recency/<app-id>/{count,last_seq}` (keeps values at one
   level; no new type).
2. `Null` → **kept in v1**, so an `Option` key can express "explicitly none"
   without a later wire change.
3. Importer → a **daemon first-boot mode**, not a separate binary.
4. Autostart write gate → **not in v1**; flagged in Security posture.

## Out of scope

System/root layer; authorization, lockdown, or writer sandboxing; standalone
schema artifact for non-Rust authors; multi-user or remote registries; secret
storage; env-var-expanding values; any Windows-Registry hive/transaction parity.

## Implementation notes (deviations from the text above)

Recorded so the design and the code agree:

1. **Wire payload is `(y tag, ay payload)`, not a `zvariant` variant.** The
   payload is the same canonical codec the store writes (one codec for disk and
   wire), and the tag stays on the wire so a generic client still learns a key's
   type. This trades cross-language variant decoding for a single, well-tested
   codec; it is consistent with Q5-A's Rust-first scope.
2. **Schemas are `fn specs() -> Vec<KeySpec>`, not `const SPECS`.** Defaults are
   real [`Value`]s, so the list is built at runtime (once, at daemon boot).
3. **`KeySpec` carries a `nullable` flag.** A key may accept `Value::Null` in
   addition to its declared tag, so `Option`-typed settings (`wallpaper`,
   `locker_command`, `lock_idle_timeout_ms`) round-trip faithfully. Without it a
   nullable key could not both declare a tag and default to `Null`.
4. **`Watch`/`Unwatch` are client-side, not daemon methods.** The daemon emits
   one broadcast `Changed` and carries no per-connection subscription state; the
   client filters by prefix. This removes a per-connection leak class at the cost
   of the daemon not being able to skip irrelevant signal delivery.
5. **The importer's A3 chord guard compares canonical key strings and
   normalized modifiers, not resolved keysyms.** `icedtea-config` resolved
   keysyms to catch spelling aliases; the settings app has only ever written the
   canonical spelling, so the two agree on every value the importer can
   encounter.
6. **The cutover is complete: `icedtea-config` is deleted.** Its in-memory
   shape survives as `icedtea_registry_schema::Config` (same field names), so
   consumer logic barely changed; only the load/save plumbing moved to
   `Config::load`/`save` over a `Registry`. The compositor's redb open-lock,
   `LoadOutcome::Locked` retry, and the `save_displays`/`save_launcher` scoped
   writes are gone — the daemon is the sole writer, so those races cannot occur.
   The two compositor tests that existed only to pin that lock behavior were
   removed with it.
7. **`Registry::direct` and `Store::in_memory` exist for hermetic tests.** A
   `Registry` is either a daemon proxy or an in-process store, so consumer unit
   tests need no bus and no file. Production still goes through the daemon.
8. **`$ICEDTEA_REGISTRY_DB` is the spawned-binary isolation seam.** It forces
   `icedtea_registry_schema::connect()` to a direct store at that path. Without
   it a test that spawns a real binary with a temp `XDG_CONFIG_HOME` would talk
   to whatever registry daemon is on the developer's session bus and ignore the
   temp file. `connect()` otherwise prefers the daemon and falls back to a direct
   store on the default path (logged as degraded). The daemon check is a real
   **probe** (`seq()`), not merely proxy construction: `Registry::connect`
   returns a lazy proxy even when nothing owns the name, and handing that back
   made every write a silent `ServiceUnknown` no-op (caught by
   `settings/tests/interaction.rs`).
9. **Dynamic names are percent-encoded path segments.** Keybinding actions
   (`snap:right`, `spawn:icedtea-session lock`) and MIME types
   (`application/atom+xml`) contain characters the grammar excludes, so
   `path::encode_segment`/`decode_segment` wrap them; the grammar itself stays
   strict.
10. **`icedtea_contract` and the schema crate each own a copy of `Appearance`
    and `DisplayConfig`, with `From` conversions between them.** The wire types
    stay in `contract`; the registry's typed structs live in `registry-schema`.
    `render::wallpaper_color` takes the schema type so the conversion stays at
    the one D-Bus boundary (`Event::ConfigReloaded`) rather than every call site.
11. **A `registryd/systemd/icedtea-registryd.service` unit ships with the
    daemon**, ordered `Before=` the consumers so the direct-store fallback is
    only ever the genuine "daemon is not running" case.
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        
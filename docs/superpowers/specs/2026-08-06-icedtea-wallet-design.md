# icedtea-wallet Design

Date: 2026-08-06

## Overview

`icedtea-wallet` is the credential/secret wallet for the icedtea desktop, built
in Rust on GTK4, in the style of KDE's KWallet but exposed through the
**Secret Service** DBus API (`org.freedesktop.secrets`). It is a thin, faithful
bridge that libsecret clients (GNOME apps, `secret-tool`, and browsers that
talk to the Secret Service) can use, plus a GTK manager GUI — all running on a
pure icedtea system with no gnome-keyring installed.

Two responsibilities live in one systemd-managed session service:

- A **Secret Service daemon** that owns `org.freedesktop.secrets` on the
  session bus, backed by an encrypted redb vault.
- A **GTK manager GUI** opened on demand from the start menu, which talks to
  the already-running service over DBus.

It follows icedtea conventions: plain GTK4 + custom CSS (no libadwaita),
panic-safe redb reads (never crash on corrupt data), message-passing threading
(no shared mutable state), MSRV 1.94 / edition 2024.

## Scope (v1)

In scope:

- A compliant `org.freedesktop.secrets` Service: Service / Collection / Item /
  Prompt objects, one default collection, search, Lock/Unlock, and **both**
  session-negotiation algorithms so real client libraries work.
- A GTK manager GUI: unlock screen, vault view, item add/edit/delete/reveal,
  settings.
- **Auto-import** of SSH keys and GPG/key material into the default collection
  (first run, plus a refreshable dialog).
- A passphrase-protected at-rest vault: argon2id-derived master key,
  per-item AES-256-GCM; never unencrypted on disk.
- Auto-lock on session lock/suspend via logind.
- A systemd user unit.

Out of scope for v1:

- A CLI (the GUI is the only control surface).
- SSH-agent integration (keys are stored but never auto-loaded into the agent).
- Multi-collection management beyond the default collection + its alias.
- FIDO/hardware-token unlock.
- Arbitrary cleanup.

## Positioning

- Workspace member crate: `wallet/` (binary `icedtea-wallet`).
- App id `org.icedtea.Wallet`; window title "IcedTea Wallet".
- Owns the session-bus name `org.freedesktop.secrets`; service object at
  `/org/freedesktop/secrets`.
- MSRV 1.94, edition 2024, `version.workspace = true`.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│ icedtea-wallet (systemd user service)                   │
│                                                           │
│  GTK main thread                                         │
│   ├─ manager window (opened on demand)                   │
│   ├─ zbus connection / object tree on the main context   │
│   ├─ unlock dialog + lock handling                       │
│   └─ SecretService model: reduce on commands/events      │
│                                                           │
│  worker threads (message passing only)                   │
│   ├─ argon2id key derivation + AES-256-GCM encrypt/decrypt
│   └─ redb vault read/write                               │
└─────────────────────────────────────────────────────────┘
```

### Threading & asynchronicity

- The GTK main thread owns the GUI, the DBus connection, the unlock flow, and
  the (pure) SecretService model reducer.
- All crypto (argon2id, AES-GCM) runs on glib worker threads; results return as
  events into the model. No blocking crypto on the UI thread, no shared mutable
  state. This mirrors icedtea's threading rule.
- The vault is mutated (unlock, item encrypt/decrypt) on a single owner thread;
  every read of a secret goes through the lock gate.

## Storage model

Vault file: `$XDG_CONFIG_HOME/icedtea/wallet.redb`. Read/opened with the same
panic-safe helper approach as `icedtea_config::open` (guarded
`redb::Database::create` with `catch_unwind_silently`), so a corrupt file falls
back to an *empty vault* and an offer to recreate — never a crash, never a
partial read.

Tables:

| Table | Contents |
| --- | --- |
| `meta` | schema version, argon2id params (t/m/p), salt, verifier |
| `items` | per-item encrypted blobs: `{ id, kind(pass/secret/ssh), created, modified, nonce, ciphertext, aad }` |
| `aliases` | Secret Service alias → collection mapping |

### Key & passphrase scheme

Nothing secret is ever stored unencrypted.

- **First run** generates a random `salt` and a per-item `nonce`. The GUI asks
  for a **master passphrase**.
- **Master key** is never persisted. On unlock it is derived as
  `k_master = argon2id(passphrase, salt, t|m|p)`.
- **Per-item key**: `k_item = HKDF-SHA256(k_master, item_id)`; each item is
  `AES-256-GCM`-encrypted under its own `k_item` with the item id in the AAD.
  Editing one item never re-encrypts the rest.
- **Verifier**: `verifier = argon2id(passphrase, salt)` output is stored in
  `meta` so a wrong passphrase is rejected after a single cheap-ish KDF call,
  *before* the expensive master derive, and passphrases are never compared
  directly. The master key is regenerated at every unlock and disposed on lock.
- There is no plaintext secret on disk. If the passphrase is lost, the vault
  cannot be unlocked — the GUI says so loudly at first run and at
  "forgot password".
- Failed unlocks apply exponential backoff before the prompt re-appears.

## Secret Service interface (`org.freedesktop.secrets`)

Object paths:

- Service: `/org/freedesktop/secrets`
- Collection: `/org/freedesktop/secrets/collection/<name>` (one default
  collection, label `login`, alias `default`)
- Item: `/org/freedesktop/secrets/collection/<name>/<id>`
- Prompt: `/org/freedesktop/secrets/prompt/<id>`

**Service** — methods: `OpenSession`, `CreateCollection`, `SearchItems`, `GetSecrets`,
`ReadAlias`, `SetAlias`, `Lock`, `Unlock`, `DeleteAlias`, `ChangePassword`.
Properties: `Collections`, `DefaultAlias`, `Locked`.

**Collection** — methods: `CreateItem`, `UpdateItem`, `Delete`,
`SearchItems`, `SetLabel`. Properties: `Items`, `Label`, `Locked`, `Created`,
`Modified`.

**Item** — methods: `SetSecret`, `GetSecret`, `GetProperties`/`SetProperties`,
`Delete`, `GetExpires`. Properties: `Attributes`, `Label`, `Locked`, `Created`,
`Modified`.

**Prompt** — `org.freedesktop.Secret.Prompt`: `Complete(result, dismissed)` and
the `Prompt` UI produced by the GUI for the unlock flow; a client that requests
`Unlock` while locked answers the prompt.

The `Secret` value is the spec `(oayays)`-shaped tuple (session, `[algorithm]]`
byte + content) — all marshalled with zbus and round-trip-tested.

**Session negotiation — both mechanisms supported (confirmed decision):**
1. **v1 (plain)**: `OpenSession("plain", ...)`; secrets go unwrapped (intended
   warning to the client).
2. **v2 (DH, `dh-ietf1024-sha256-aes128-cbc`-style):** the service/each of the
   DH exchange, derives a shared secret, and **AES-CBC-wraps** the secret bytes
   the client sends/receives over that session. This is what Chromium and
   `secret-tool` actually negotiate; implemented in `session.rs`, isolated so the
   wrap can be tuned independently.

All signatures are recorded and byte-tested in the `contract` module (matching
how `icedtea-contract` pins its wire types).

## GTK manager GUI

Plain GTK4 + custom CSS. Window opened **on demand** from a "Wallet Manager"
launcher in the shell start menu; it attaches to the already-running service
over DBus and shows:

- **Unlock screen** while `Service.Locked == true`: passphrase entry + confirm,
  verifier gate, backoff on failure.
- **Vault view**: search, item list (label, username, type hint, dates, tags),
  reveal/copy/edit/delete, "add item" and "import" actions.
- **Settings**: lock-on-suspend toggle, auto-import toggle + "rescan",
  "forgot password" notice, CSS theme following the shell.

## Auto-import

First run (and a refreshable dialog) scans known predigried locations and
registers **encrypted items** in the default collection:

- **SSH keys**: `~/.ssh/id_ed25519`, `id_rsa`, `*.pub` companions — stored as
  `ssh`-tagged items (private key held in the item's Secret); not pushed to
  `ssh-agent` automatically.
- **GPG**: `~/.gnupg` — a traceable informational item (`gpg`) with the key id /
  type; key fragment contents are not auto-copied.
- **App secrets**: scan `~/.config` for blobs that self-identify as imported
  from an existing Secret Service and present them as **import candidates** for
  the user to confirm (never silent).
- Each import records its source file for traceability.

## systemd / integration

- `icedtea-wallet.service`: `WantedBy=graphical-session.target`,
  `After=icedtea-compositor.service`, `Restart=on-failure`, owns
  `org.freedesktop.secrets`.
- Auto-lock: subscribe to logind `PrepareForSleep`/session-lock and drop the
  master key (locking to the service) — config-toggle default on.
- Theming matches the shell CSS conventions.

## Error handling

- Corrupt/missing vault → empty fallback + offer to create (panic-safe open).
- Wrong passphrase → error, backoff, never any secret bytes exposed.
- Item writes are atomic (encrypt → single transaction insert → commit) so a
  crash never leaves a half-encrypted blob.
- A client that vanishes mid-`OpenSession` is cleaned up and never takes the
  vault down.

## Testing

- `contract`: round-trip tests for every service method/signature; both
  `OpenSession` channels and the DH secret wrap/unwrap symmetry (a test-only
  client decrypts what the service wraps).
- `vault`: AES/argon2 round-trips, wrong-passphrase rejection, per-item
  encrypt/decrypt over a temp db, lock/unlock transitions, serialization.
- `crypto`: fixed-vector AES-256-GCM and argon2id param tests.
- Integration (smoke): on a real bus where available, `secret-tool` store/lookup
  against our own name; otherwise a scripted client.
- UI: manual unlock + CRUD flows.
- Gates: `cargo test --workspace` and `cargo clippy --all-targets -- -D warnings`.

## Risks

- The **DH session** is the largest interop surface; it is isolated in
  `session.rs` so the wrap strategy is swappable.
- Nit-picky clients refuse to store if the service isn't spec-exact; guarded
  with byte-tests and a real-client smoke test.
- Crypto crates (`aes-gcm`, `argon2`, `hkdf`) must build at MSRV 1.94; versions
  are pinned in the plan.
- At-rest strength rests entirely on argon2 + AES-GCM and the (never-stored)
  master key; this is by design ("passphrase-protected vault", KWallet-style).

## Crate layout

```
wallet/
├─ Cargo.toml
├─ src/
│  ├─ main.rs          — mode: service / gui, bootstrap, CSS
│  ├─ service.rs       — org.freedesktop.secrets object tree
│  ├─ session.rs       — OpenSession: v1 plain + v2 DH (AES-CBC wrap)
│  ├─ vault.rs         — redb vault, panic-safe open, atomic items
│  ├─ crypto.rs        — argon2id master key, HKDF per-item key, AES-256-GCM
│  ├─ model.rs         — pure SecretService reducer / lock gate
│  ├─ import.rs        — SSH/GPG/key auto-import
│  ├─ contract.rs      — wire types + signature round-trip tests
│  └─ gui/
│     ├─ mod.rs
│     ├─ unlock.rs     — unlock screen
│     ├─ vault.rs      — vault/item view
│     └─ settings.rs
└─ systemd/icedtea-wallet.service
```
# DE session + VM fidelity — Design

**Date:** 2026-09-19
**Status:** approved in brainstorming; awaiting implementation plan.
**Parent:** `2026-08-20-icedtea-de-roadmap.md`.
**Decisions of record:** DE startup moves into the repo as systemd `--user`
units (approach A); readiness by a repo-owned wait helper, not `Type=notify`;
the compositor is `RequiredBy` the session target and the other four units are
`Wants` (best-effort); the wallpaper falls back to an installed default asset
rather than seeding the VM's redb config; the panel clock ticks from a
shell-side timer thread, not a new toolkit wake API; one time crate is added
for local-time formatting; every change is proven by screenshot assertions
against the running VM.

## Goal

Make the VirtualBox VM boot into a **usable icedtea desktop that is a faithful
reflection of the DE, and provably so**: the same session-entry artifacts that
would start icedtea on bare metal start it in the VM; the desktop shows a
compositor, a panel, and a wallpaper; an app window (a terminal) can be opened
and used; and each change can be checked by a script that screenshots the VM
and asserts the desktop is actually there.

Two outcomes define success:

1. **Fidelity** — the VM runs the repo's own session definition, not a
   VM-only script. A change to how the DE starts is a change on both.
2. **Trust** — a component that dies is visible (journal, bounded restart)
   and a regression that produces a black or empty desktop fails the
   verification script loudly, instead of looking "fine" from a shell.

## Non-goals

- No display manager, greeter, or login UI. Startup stays "autologin tty1 runs
  the session target"; a DM on bare metal would run the same target.
- No changes to the session daemon's behaviour (lock/idle/logind semantics).
  It is started and supervised; its features are not redesigned here.
- No GPU acceleration work. The VM keeps wlroots' `pixman` renderer, which is
  what renders correctly under VirtualBox's `vmwgfx`.
- No distro packaging. Provisioning installs into the VM's own paths.

## Background: what the VM does today (observed)

The findings below are from booting the existing `icedtea-wm` VM and inspecting
it, and they are the whole reason this work is architectural rather than a
one-line fix.

- **The DE has no startup artifact in the repo.** How the compositor, shell,
  clipboard, notifications and session come up together is defined only by
  `/usr/local/bin/icedtea`, a script written *by* `vagrant/provision-system.sh`
  *into* the guest. Nothing in the repo says how an icedtea session starts, so
  the VM cannot reflect the DE — there is no DE startup to reflect.
- **The pieces exist and work individually.** With the VM resumed, the
  compositor (DRM/`vmwgfx`, `pixman`), clipboard, and shell all run; the
  compositor's DRM output is what the VirtualBox console shows. A `foot`
  terminal launches, renders a prompt, receives a server-side decorated
  window, and its taskbar tile appears — so window management, decorations,
  and the taskbar all function.
- **The panel renders but reads as a floating cluster.** `#bar` paints a
  full-width dark bar (`background-color: rgba(20,20,24,0.92)`; measured
  `(22,22,26)` at both left and right edges), but its content is one centred
  row (`panel::view` returns a single box of `start, workspaces, windows, …
  clip`), so only the buttons are visible, clustered mid-bar. There is no
  clock and no right-hand group.
- **There is no wallpaper.** `Appearance.wallpaper` defaults to `None`, and
  the desktop is therefore the compositor's flat navy `(30,30,46)`. The
  compositor already has a wallpaper decode worker and scene node; nothing
  sets a wallpaper in a fresh install.
- **The VM runs stale code and cannot say which.** The guest's binaries were
  built 2026-09-11 (a week behind `develop`, pre-M9 and pre-notification
  daemon), and rsync excludes `.git/`, so `git rev-parse` fails in the guest:
  the VM cannot report what revision it is running.
- **Observability is thin.** The shell writes nothing to its log; the
  compositor logs only its socket line and libinput "system too slow" spam
  (VM timer skew). A prior "compositor died silently, cause undetermined"
  (recorded in `MEMORY.md`) is exactly the failure mode a reflection of the
  DE must make loud.
- **The preconditions for a systemd-user session are already met.** The guest
  runs a user manager (`systemctl --user is-system-running` → `running`,
  `user@1000.service` active), logind shows the tty1 session on `seat0`, and
  the DE processes already carry `XDG_SESSION_ID=1` and run on the real user
  bus at `/run/user/1000/bus` (so the current script's `dbus-run-session`
  fallback is not taken).

## Architecture

### 1. Session composition (the core)

A new in-repo session layer defines how the DE starts. It has two parts: the
systemd user unit files, and a small wait helper.

**Units** (new, under `session/launch/units/`):

| Unit | Needs | Ordering / dependency |
|---|---|---|
| `icedtea-compositor.service` | seat + DRM | `RequiredBy=icedtea-session.target`; owns `wayland-0` and `org.icedtea.WM` |
| `icedtea-notifications.service` | session bus | `Wants`; independent of Wayland, claims `org.freedesktop.Notifications` |
| `icedtea-clipboard.service` | compositor socket, bus | `After`/`Wants` compositor; `ExecStartPre` waits for the Wayland socket |
| `icedtea-session.service` | bus, `XDG_SESSION_ID`, `org.icedtea.WM` | `After`/`Wants` compositor |
| `icedtea-shell.service` | compositor socket, `org.icedtea.Clipboard` | `After`/`Wants` compositor + clipboard; waits for both |
| `icedtea-session.target` | — | `Wants=` the five; the single entry point |

Every unit is `Type=simple`, `Restart=on-failure` with a bounded
`StartLimitBurst`, and logs to the journal. The compositor being `RequiredBy`
the target is deliberate: a desktop without a compositor is not a degraded
session, it is no session, so its exit stops the target and its own restart
policy drives recovery. The other four are `Wants` so that a crashing shell
cannot take the compositor (and with it the session) down.

**Readiness (the one real wrinkle).** `Type=simple` orders *exec*, not socket
readiness; the compositor's Wayland socket appears asynchronously, which is
why the current script polls for it and then sleeps. Rather than make the
compositor `Type=notify` (which would add a libsystemd dependency and change
compositor code), a repo-owned `icedtea-wait` helper runs as `ExecStartPre`
for the three consumers that need the socket or a bus name. It takes a mode
and an argument — `wayland` (poll `$XDG_RUNTIME_DIR/wayland-*` for a socket)
and `name <bus-name>` (poll `busctl --user` for the name) — with a timeout and
a non-zero exit on expiry, so a missing socket fails the unit rather than
racing it.

**Trigger.** tty1 autologin runs `icedtea-session-start`, a tiny wrapper that
ensures the user-manager environment is imported (`systemctl --user
import-environment` for the graphical/session variables it needs) and runs
`systemctl --user start icedtea-session.target`. This is the same command a
display manager would run on bare metal, which is the point: the VM's boot
path and the DE's real boot path are one artifact.

**Environment.** The environment the current script sets per-component moves
into the units as far as it is universal (`GDK_BACKEND=wayland`, `RUST_LOG`),
and the VM-specific renderer choice (`WLR_RENDERER=pixman`,
`GSK_RENDERER=cairo`) moves into a guest-only environment drop-in at
`~/.config/environment.d/99-icedtea-vm.conf` (read by the user manager's
environment generator, so it reaches every user unit) rather than being baked
into the shipped units — a bare-metal session must not be forced onto software
rendering by a unit written for VirtualBox.

### 2. Wallpaper default

The compositor gains a **default-wallpaper fallback**: when
`Appearance.wallpaper` is `None`, it resolves, in order, an env override
(`ICEDTEA_DEFAULT_WALLPAPER`) and then an installed path
(`/usr/share/icedtea/default-wallpaper.png`), and uses the first that exists.
An explicit config value always wins, and a `None` config with no installed
default keeps today's flat background.

This is chosen over seeding the guest's redb config because it fixes the
first-run desktop for *every* install (VM and bare metal) instead of making
the VM's appearance depend on VM-only setup — which is the same smell this
whole design removes. Provisioning installs the asset; the asset itself is a
generated, redistributable image kept in the repo.

### 3. Panel: layout and clock

- **Layout.** `panel::view` splits into a left group (`start`, workspaces,
  window tiles) and a right group (the existing indicators, a new clock, and
  `clip`), laid out with `justify-content: space-between` — already supported
  by the toolkit's layout (`ui/src/layout.rs:692` maps it to
  `JustifyContent::SPACE_BETWEEN`) — plus the matching `#bar`/group rules in
  `shell/style.css`. The single centred row disappears.
- **Clock.** A right-aligned HH:MM label. The toolkit deliberately does not
  repaint when idle, so the clock needs a wake: a shell-side timer thread
  sends a `Tick` into the shell's existing inbox on the minute boundary (and
  once at startup), which the panel handles by reformatting and requesting a
  redraw. This mirrors the shell's existing worker-thread + inbox/wake
  pattern rather than adding a toolkit scheduling API. The formatter is a
  pure function (wall-clock instant → string) and is unit-tested directly.
- **Wall-clock formatting** needs local time, and the workspace has no time
  crate. Add one (`jiff`, modern and safe; `chrono` is the fallback) for
  local-time formatting. This is the only new dependency in this design; it
  is confined to the shell's clock.

### 4. Provisioning, freshness, verification

- **Provisioning** (`vagrant/provision-system.sh`, `Vagrantfile`): install the
  wait helper, the units to the systemd user unit path, and the wallpaper
  asset; make tty1 start `icedtea-session.target`; drop the guest-only
  renderer environment drop-in; remove the old `/usr/local/bin/icedtea`
  script and its hint. The build phase builds the current tree; rsync stays
  the source of truth for what is built.
- **Freshness.** Because `.git/` is excluded, provisioning writes a
  `BUILD_INFO` file into the guest (revision + build time) at build time, and
  the verification script prints it, so "the VM runs stale code" is visible
  rather than guessed.
- **Verification** (`vagrant/verify.sh`, runnable against a booted VM):
  1. wait for `icedtea-session.target` to be active and for the units to be
     `active` (via `systemctl --user is-active`), failing loudly otherwise;
  2. `VBoxManage controlvm … screenshotpng` the console;
  3. assert on pixels: the desktop is **not** the flat default (a wallpaper is
     present), the panel bar colour is present across the top edge, and the
     frame is not uniform (not a black/blank screen);
  4. launch `foot` as a client, screenshot again, and assert a window
     appeared (a large region of the frame changed and the taskbar gained a
     tile);
  5. print PASS/FAIL with the screenshot paths and the `BUILD_INFO` revision.
  The script exits non-zero on any assertion, so it is usable as a gate.

## Decisions of record

| # | Decision | Rationale |
|---|---|---|
| D1 | systemd `--user` units, not a shell script | Matches how real DEs start; gives ordering, bounded restart, and journal logs — the "trust" half — and is the same artifact on bare metal |
| D2 | Readiness by `icedtea-wait` helper, not `Type=notify` | No libsystemd dependency, no compositor change; the helper also covers bus-name waits the current script fakes with `sleep` |
| D3 | compositor `RequiredBy` target; others `Wants` | No compositor means no session; a broken shell must not kill the compositor |
| D4 | Wallpaper: installed default + fallback, not redb seeding | Fixes every fresh install, keeps appearance out of VM-only setup |
| D5 | Clock: shell timer thread, not toolkit `next_wake()` | Smallest change; reuses existing worker/wake patterns; no toolkit surface change |
| D6 | Add one time crate for local-time formatting | The workspace has none; local time (not UTC) is required for a desktop clock |
| D7 | Screenshot assertions as the acceptance gate | Turns "silent death"/black desktop into a loud, automated failure — the fidelity claim is proven, not assumed |

## Failure handling

- A unit that exits non-zero is restarted (`Restart=on-failure`), bounded by
  `StartLimitBurst`/`StartLimitIntervalSec`, after which systemd stops trying
  and the unit is visibly `failed` — not silently gone.
- The compositor's failure stops `icedtea-session.target` (D3); the verification
  script reports the inactive unit and its journal tail.
- `icedtea-wait` failing exits the dependent unit non-zero, so a missing
  Wayland socket or bus name surfaces as a failed unit with a timeout message
  rather than a component that started against nothing.
- The verification script's pixel assertions fail closed: a uniform frame, a
  missing bar, or an unchanged frame after launching `foot` are all failures.

## Testing

- **Hermetic (runs in CI, no VM):** `systemd-analyze verify` on every unit
  file (catches ordering/dependency typos); unit tests for `icedtea-wait`'s
  argument handling and timeout; unit tests for the panel clock formatter and
  the new left/right `panel::view` structure (existing shell test patterns);
  a unit test for the wallpaper fallback resolution order (env → installed →
  flat default) on the compositor side.
- **VM (the real proof):** `vagrant/verify.sh` as above — session active,
  wallpaper present, bar present, `foot` window appears — with its screenshots
  and `BUILD_INFO` revision in the output. A change that breaks the desktop
  fails this.

## Risks

- **systemd user units + seatd.** The user manager and session id are present
  today (evidence above), but a future box change (a different `generic/arch`
  snapshot, or dropping autologin) could remove them; `icedtea-wait` plus the
  verification script's unit-active checks make that a clear failure.
- **Wallpaper asset licensing.** The shipped default must be generated or
  otherwise redistributable; the spec assumes a generated image committed to
  the repo.
- **Time crate choice.** Adds a dependency to a workspace that has none for
  time; `jiff` is proposed for its safe API, `chrono` as the familiar
  fallback. Confining it to the shell's clock keeps the blast radius small.
- **Clock wake cost.** A per-minute wake is negligible, but the timer thread
  must not fire on a compositor-less shell (no inbox) and must stop when the
  shell exits; it follows the shell's existing worker lifetime rules.
- **Pixel assertions are coarse.** They prove "a desktop is present", not
  "the desktop is correct"; they are a floor, paired with the hermetic unit
  tests for the specifics.

## Decomposition

The work splits into independent units, each with its own verification:

1. **Session units + wait helper** — the repo-owned session definition, unit
   files, `icedtea-wait`, `icedtea-session-start`, `systemd-analyze verify`
   coverage. (No VM needed to write; verified in-VM at step 4.)
2. **Wallpaper default** — fallback resolution in the compositor + asset +
   provisioning install + fallback unit test.
3. **Panel layout + clock** — left/right `panel::view`, `style.css`, the time
   crate, the timer thread, the formatter and view tests.
4. **Provisioning + freshness + verification** — `Vagrantfile`/provision
   rewrite to install the units and asset and start the target; `BUILD_INFO`
   stamp; `vagrant/verify.sh` with pixel assertions; the first
   end-to-end VM run that proves the whole thing.

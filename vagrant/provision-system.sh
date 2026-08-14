#!/usr/bin/env bash
# Root-phase provisioning: packages, seat management, console autologin.
set -euo pipefail

# The box snapshot predates current package signatures; refresh the keyring
# first or the full upgrade fails on signature checks.
pacman -Sy --noconfirm archlinux-keyring
pacman -Su --noconfirm

# wlroots0.20 matches the `wlr = "0.20"` crate's pkg-config requirement
# (wlroots-0.20.pc). clang is for wlr-sys's bindgen; foot is a Wayland
# terminal to open test windows inside the compositor.
pacman -S --noconfirm --needed \
  base-devel git rustup clang pkgconf \
  wlroots0.20 wayland wayland-protocols \
  libinput libxkbcommon pixman libdrm mesa \
  seatd xorg-xwayland polkit \
  gtk4 gtk4-layer-shell dbus \
  foot

# wlroots takes the session through libseat -> seatd; the vagrant user needs
# the seat group for that, plus video/input for the DRM and evdev nodes.
systemctl enable --now seatd.service
usermod -aG seat,video,input vagrant

# Auto-login vagrant on tty1 so the VirtualBox GUI window lands on a shell
# ready to launch the compositor.
mkdir -p /etc/systemd/system/getty@tty1.service.d
cat > /etc/systemd/system/getty@tty1.service.d/autologin.conf <<'EOF'
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin vagrant --noclear %I $TERM
EOF
systemctl daemon-reload

# Launcher: run the compositor on this TTY's DRM device, then bring up the
# clipboard daemon and the GTK4 shell as layer-shell clients on the same
# Wayland socket and session bus. Run from a TTY, not over ssh.
cat > /usr/local/bin/icedtea <<'EOF'
#!/bin/sh
# Ensure a D-Bus session bus (org.icedtea.WM / .Clipboard live on it).
if [ -z "$DBUS_SESSION_BUS_ADDRESS" ]; then
  exec dbus-run-session -- "$0" "$@"
fi
export RUST_LOG="${RUST_LOG:-info}"
BIN=/home/vagrant/icedtea-wm/target/release

# VirtualBox's vmwgfx GLES2 path is flaky under wlroots; force wlroots'
# software (pixman) renderer so frames actually reach the screen.
export WLR_RENDERER="${WLR_RENDERER:-pixman}"

# The compositor takes the DRM master, creates its wayland socket, and claims
# org.icedtea.WM on the session bus (provided by the systemd --user session).
"$BIN/icedtea-compositor" >/tmp/icedtea-comp.log 2>&1 &
COMP=$!

# Tear the clients down with the compositor.
trap 'kill "$CLIP" "$SHELL_PID" 2>/dev/null; kill "$COMP" 2>/dev/null' INT TERM EXIT

# Wait for the compositor's wayland socket to appear.
WAYLAND_DISPLAY=
i=0
while [ "$i" -lt 100 ]; do
  for s in "$XDG_RUNTIME_DIR"/wayland-[0-9]*; do
    case "$s" in *.lock) continue ;; esac
    [ -S "$s" ] && WAYLAND_DISPLAY=$(basename "$s") && break
  done
  [ -n "$WAYLAND_DISPLAY" ] && break
  i=$((i + 1)); sleep 0.1
done
if [ -z "$WAYLAND_DISPLAY" ]; then
  echo "icedtea: compositor never created a wayland socket" >&2
  wait "$COMP"; exit 1
fi
export WAYLAND_DISPLAY
export GDK_BACKEND=wayland                      # force GTK onto Wayland, not X11
export GSK_RENDERER="${GSK_RENDERER:-cairo}"    # GTK software rendering, no GPU

# The socket file appears the instant add_socket_auto runs, before the
# compositor is dispatching; give it a beat so early clients don't get
# NoCompositor.
sleep 1

# Start the clipboard daemon first and give it a moment to claim
# org.icedtea.Clipboard before the shell's clip client connects.
"$BIN/icedtea-clipboard" >/tmp/icedtea-clip.log 2>&1 & CLIP=$!
sleep 0.7
"$BIN/icedtea-shell" >/tmp/icedtea-shell.log 2>&1 & SHELL_PID=$!

wait "$COMP"
EOF
chmod +x /usr/local/bin/icedtea

# One-line hint on the autologin shell.
cat > /etc/profile.d/icedtea-hint.sh <<'EOF'
[ "$(tty)" = /dev/tty1 ] && echo 'icedtea-wm VM ready: run `icedtea` to start the compositor + shell.'
EOF

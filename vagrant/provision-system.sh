#!/usr/bin/env bash
# Root-phase provisioning: packages, seat management, the DE's session units,
# and this box's login harness (wdm and its config — harness, not the DE's
# session definition).
set -euo pipefail

# The box snapshot predates current package signatures; refresh the keyring
# first or the full upgrade fails on signature checks.
pacman -Sy --noconfirm archlinux-keyring
pacman -Su --noconfirm

# wlroots0.20 matches the `wlr = "0.20"` crate's pkg-config requirement
# (wlroots-0.20.pc). clang is for wlr-sys's bindgen; foot is a Wayland
# terminal to open test windows inside the compositor.
#
# wdm (the box's display manager) is deliberately NOT in this list: the
# generic/arch snapshot ships it, and it is not in `core`/`extra` by name, so
# a `pacman -S wdm-wayland` would abort this whole phase. It is checked for
# below and its absence is a clear, early failure.
pacman -S --noconfirm --needed \
  base-devel git rustup clang pkgconf \
  wlroots0.20 wayland wayland-protocols \
  libinput libxkbcommon pixman libdrm mesa \
  seatd xorg-xwayland polkit \
  gtk4 gtk4-layer-shell dbus \
  foot

# The display manager is preinstalled box state, not something this script
# installs (it is not resolvable from `core`/`extra`). Fail early and clearly
# if it is missing, rather than half-provisioning a box with no login path.
if ! command -v wdm >/dev/null 2>&1; then
  echo "provision-system: wdm is not installed on this box; it ships with the" >&2
  echo "generic/arch snapshot and is not in core/extra. Install it before" >&2
  echo "provisioning, or the VM has no login path." >&2
  exit 1
fi

# wlroots takes the session through libseat -> seatd; the vagrant user needs
# the seat group for that, plus video/input for the DRM and evdev nodes.
systemctl enable --now seatd.service
usermod -aG seat,video,input vagrant

# The DE's own session definition, installed where systemd finds it. The
# units and helpers come from the repo (session/launch/), so the VM runs the
# same session artifacts bare metal would.
mkdir -p /etc/systemd/user /usr/share/icedtea
install -m 0644 /home/vagrant/icedtea-wm/session/launch/units/*.service /etc/systemd/user/
install -m 0644 /home/vagrant/icedtea-wm/session/launch/units/icedtea-session.target /etc/systemd/user/
install -m 0755 /home/vagrant/icedtea-wm/session/launch/icedtea-wait /usr/local/bin/icedtea-wait
install -m 0755 /home/vagrant/icedtea-wm/session/launch/icedtea-session-start /usr/local/bin/icedtea-session-start
install -m 0644 /home/vagrant/icedtea-wm/session/assets/default-wallpaper.png /usr/share/icedtea/default-wallpaper.png

# The DE ships one display-manager-agnostic session entry: a freedesktop
# .desktop offered to both Wayland and X11 session managers, byte-for-byte
# identical either way. The DE does not choose or configure a display manager.
install -d -m 0755 /usr/share/wayland-sessions /usr/share/xsessions
install -m 0644 /home/vagrant/icedtea-wm/session/launch/icedtea.desktop /usr/share/wayland-sessions/icedtea.desktop
install -m 0644 /home/vagrant/icedtea-wm/session/launch/icedtea.desktop /usr/share/xsessions/icedtea.desktop

# Container harness, not DE architecture: this box's login path is wdm (a
# Wayland display manager) and its config is checked in so provisioning is
# reproducible. Nothing in the DE's own session layer (the units, the entry,
# `icedtea-session-start`) names or depends on wdm. Never disable
# display-manager.service — the old tty1 path did, leaving the VM with no login
# path of its own.
install -d -m 0755 /etc/wdm
install -m 0644 /home/vagrant/icedtea-wm/session/launch/wdm.toml /etc/wdm/wdm.toml
systemctl enable wdm.service

# The old launcher and its hint are gone: the session is the systemd target
# wdm starts, not a script that spawns the components itself.
rm -f /usr/local/bin/icedtea /usr/local/bin/icedtea-hint.sh /usr/bin/icedtea-session
rm -f /etc/profile.d/icedtea-session.sh

# An already-provisioned VM carries the retired tty1 trigger files on disk;
# remove them (not just stop writing them) so tty1 stays a plain recovery
# console and wdm is the only session trigger.
rm -f /etc/systemd/system/getty@tty1.service.d/autologin.conf
rmdir /etc/systemd/system/getty@tty1.service.d 2>/dev/null || true
rm -f /etc/profile.d/icedtea-hint.sh
systemctl daemon-reload

# Guest-only rendering choices: wlroots on VirtualBox's vmwgfx needs the
# software paths, and these must NOT be baked into the units (a bare-metal
# session would otherwise be forced onto software rendering).
install -d -m 0755 /home/vagrant/.config/environment.d
cat > /home/vagrant/.config/environment.d/99-icedtea-vm.conf <<'EOF'
WLR_RENDERER=pixman
GSK_RENDERER=cairo
GDK_BACKEND=wayland
EOF
chown -R vagrant:vagrant /home/vagrant/.config

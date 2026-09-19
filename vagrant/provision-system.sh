#!/usr/bin/env bash
# Root-phase provisioning: packages, seat management, session units, and the
# wdm display-manager login path.
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

# The DE's own session definition, installed where systemd finds it. The
# units and helpers come from the repo (session/launch/), so the VM runs the
# same session artifacts bare metal would.
mkdir -p /etc/systemd/user /usr/share/icedtea
install -m 0644 /home/vagrant/icedtea-wm/session/launch/units/*.service /etc/systemd/user/
install -m 0644 /home/vagrant/icedtea-wm/session/launch/units/icedtea-session.target /etc/systemd/user/
install -m 0755 /home/vagrant/icedtea-wm/session/launch/icedtea-wait /usr/local/bin/icedtea-wait
install -m 0755 /home/vagrant/icedtea-wm/session/launch/icedtea-session-start /usr/local/bin/icedtea-session-start
install -m 0644 /home/vagrant/icedtea-wm/session/assets/default-wallpaper.png /usr/share/icedtea/default-wallpaper.png

# The display manager is wdm, the box's Wayland DM. It lists the repo's
# session entry and runs it at login, so both are repo-owned; enable wdm and
# never disable display-manager.service (the old tty1 path did, leaving the
# VM with no login path of its own).
install -m 0644 /home/vagrant/icedtea-wm/session/launch/icedtea.desktop /usr/share/wayland-sessions/icedtea.desktop
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

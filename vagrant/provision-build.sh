#!/usr/bin/env bash
# User-phase provisioning: rust toolchain, release build, and the pieces the
# session units exec (binaries on PATH + a build-revision stamp).
set -euo pipefail

rustup default stable
cd ~/icedtea-wm
cargo build --release

# The units resolve binaries through PATH (`/usr/bin/env icedtea-…`), so link
# every session binary the same way an installed system would. This phase runs
# as vagrant; /usr/local/bin is root-owned, so create the links with sudo.
for bin in icedtea-compositor icedtea-clipboard icedtea-notifications icedtea-session icedtea-shell; do
  sudo ln -sf ~/icedtea-wm/target/release/$bin /usr/local/bin/$bin
done

# rsync excludes .git/, so the guest cannot ask git what it is running. The
# Vagrantfile computes the host revision at provision time and passes it here
# in the provisioner environment; `vagrant/verify.sh` prints this, so "the VM
# runs stale code" is visible rather than guessed.
#
# Written outside ~/icedtea-wm on purpose: that tree is an rsync mirror with
# `--delete`, and a guest-only file inside it is removed by the next sync.
sudo mkdir -p /etc/icedtea
{
  echo "rev=${ICEDTEA_BUILD_REV:-unknown}"
  echo "built=$(date -u +%FT%TZ)"
} | sudo tee /etc/icedtea/BUILD_INFO

# Re-provisioning must leave a login prompt, not a stale session: restart wdm
# so the greeter is what the console shows. It is safe to restart while a
# session is live (wdm has already handed the display over); wdm picks the
# console back up when the session command exits.
sudo systemctl restart wdm.service || true

#!/usr/bin/env bash
# User-phase provisioning: rust toolchain + release build of the workspace.
set -euo pipefail

rustup default stable
cd ~/icedtea-wm
cargo build --release
echo "Build done: $(ls -lh target/release/icedtea-compositor | awk '{print $5, $9}')"

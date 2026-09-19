#!/usr/bin/env bash
# Prove the VM is showing a usable icedtea desktop, or fail loudly.
#
# wdm is the login path (the VM has no autologin), so this logs in by typing
# the credentials at the greeter with injected scancodes, then asserts the
# desktop by pixels and unit state. A black screen, a missing panel, or a
# missing wallpaper fails here instead of looking "fine" from a shell — the
# failure mode the design exists to remove.
#
# Hosts need: vagrant, VBoxManage, ImageMagick (`magick`). Usage:
#   vagrant/verify.sh            # resume/boot, log in, wait, assert
set -euo pipefail

VM="icedtea-wm"
OUT="${VERIFY_OUT:-/tmp/icedtea-verify}"
mkdir -p "$OUT"

# Typed at the greeter. Override for a non-default box.
GREETER_USER="${GREETER_USER:-vagrant}"
GREETER_PASS="${GREETER_PASS:-vagrant}"

fail() { echo "FAIL: $*" >&2; exit 1; }

command -v VBoxManage >/dev/null || fail "VBoxManage not found"
command -v magick >/dev/null || fail "ImageMagick (magick) not found"

# `systemctl --user` over ssh needs the user manager's runtime dir and bus;
# a bare `vagrant ssh -c` has neither, so route every user-manager call here.
ssh_user() {
  vagrant ssh -c "XDG_RUNTIME_DIR=/run/user/\$(id -u) DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/\$(id -u)/bus $*"
}

# --- console keyboard injection (PS/2 set 1 scancodes) -------------------
declare -A SC=(
  [a]=1e [b]=30 [c]=2e [d]=20 [e]=12 [f]=21 [g]=22 [h]=23 [i]=17 [j]=24
  [k]=25 [l]=26 [m]=32 [n]=31 [o]=18 [p]=19 [q]=10 [r]=13 [s]=1f [t]=14
  [u]=16 [v]=2f [w]=11 [x]=2d [y]=15 [z]=2c
  [0]=0b [1]=02 [2]=03 [3]=04 [4]=05 [5]=06 [6]=07 [7]=08 [8]=09 [9]=0a
  [-]=0c [.]=34 [_]=0c [@]=1a
)
type_scancode() {  # each arg: a hex make code
  local codes=() c
  for c in "$@"; do codes+=("$c" "$(printf '%02x' $(( 0x$c | 0x80 )))"); done
  VBoxManage controlvm "$VM" keyboardputscancode "${codes[@]}" >/dev/null
}
type_text() {  # $1 = lower-case ASCII
  local s="$1" i ch
  for (( i=0; i<${#s}; i++ )); do
    ch="${s:i:1}"
    [ -n "${SC[$ch]:-}" ] || fail "no scancode for '$ch'"
    type_scancode "${SC[$ch]}"
    sleep 0.05
  done
}

# 1. Boot if needed.
state=$(VBoxManage showvminfo "$VM" --machinereadable | sed -n 's/^VMState="\(.*\)"$/\1/p')
case "$state" in
  running) ;;
  saved) VBoxManage startvm "$VM" --type headless >/dev/null ;;
  *) fail "VM '$VM' is $state; provision it first (vagrant up)" ;;
esac

echo "== BUILD_INFO =="
build_info=$(vagrant ssh -c 'cat /etc/icedtea/BUILD_INFO' | tr -d '\r') || fail "cannot read BUILD_INFO"
printf '%s\n' "$build_info"
rev=$(printf '%s\n' "$build_info" | sed -n 's/^rev=//p')
[ -n "$rev" ] && [ "$rev" != "unknown" ] \
  || fail "BUILD_INFO carries no revision — the VM cannot say what it runs"

# 2. Log in at the wdm greeter (there is no autologin). Skip if a session is
# already up from a previous login.
session_active() {
  [ "$(ssh_user systemctl --user is-active icedtea-session.target 2>/dev/null | tr -d '\r')" = "active" ]
}
if ! session_active; then
  echo "== logging in at the wdm greeter as $GREETER_USER =="
  VBoxManage controlvm "$VM" screenshotpng "$OUT/greeter.png" >/dev/null
  # The webkit greeter preselects the last user (its User dropdown already
  # reads `vagrant`) and focuses the password field, so there is no username
  # to type: type the password and submit. A greeter that focused the user
  # field instead would need `type_text "$GREETER_USER"; type_scancode 0f`
  # first — verified against $OUT/greeter.png, not assumed.
  type_text "$GREETER_PASS"
  type_scancode 1c   # Enter
  sleep 8
  if ! session_active; then
    VBoxManage controlvm "$VM" screenshotpng "$OUT/greeter-after.png" >/dev/null
    fail "login did not start the session — inspect $OUT/greeter.png and $OUT/greeter-after.png"
  fi
fi

# 3. The session is actually up. Poll, because systemd start is async.
active() { ssh_user "systemctl --user is-active $1" 2>/dev/null | tr -d '\r'; }
i=0
until [ "$(active icedtea-session.target)" = "active" ] \
   && [ "$(active icedtea-shell.service)" = "active" ] \
   && [ "$(active icedtea-compositor.service)" = "active" ]; do
  i=$((i + 1)); [ "$i" -lt 60 ] || {
    ssh_user 'systemctl --user list-units "icedtea-*" --no-pager; journalctl --user -u icedtea-compositor -n 20 --no-pager' || true
    fail "session units did not become active within 60s"
  }
  sleep 1
done

# 4. Capture and assert the desktop is there.
VBoxManage controlvm "$VM" screenshotpng "$OUT/desktop.png" >/dev/null
mean() { magick "$1" -colorspace Gray -format '%[fx:mean]' info:; }

desktop_mean=$(mean "$OUT/desktop.png")
awk "BEGIN { exit !($desktop_mean > 0.005) }" || fail "screen is (near) black (mean $desktop_mean)"

# The panel is a dark full-width band on the configured edge
# (`Appearance.bar_position`; this VM has no config, so the default is
# bottom). A fixed top strip would just read the wallpaper, so the bar edge
# is whichever of the two edge strips is darker.
w=$(magick identify -format '%w' "$OUT/desktop.png")
h=$(magick identify -format '%h' "$OUT/desktop.png")
top_strip=$(magick "$OUT/desktop.png" -crop "x4+0+0" +repage -format '%[fx:mean]' info:)
bottom_strip=$(magick "$OUT/desktop.png" -crop "x4+0+$((h - 4))" +repage -format '%[fx:mean]' info:)
bar_mean=$(awk "BEGIN { print ($top_strip < $bottom_strip) ? $top_strip : $bottom_strip }")
bar_y=$(awk "BEGIN { print ($top_strip < $bottom_strip) ? 0 : $h - 28 }")
# The bar is near-black; a missing bar leaves the wallpaper there (~0.15).
awk "BEGIN { exit !($bar_mean < 0.13) }" || fail "no dark panel band at either edge (top $top_strip bottom $bottom_strip)"

# The wallpaper must differ from the flat default background (the flat navy
# reads ~0.119 in grey; a wallpaper shifts it). Assert the centre region is
# not the flat colour.
centre=$(magick "$OUT/desktop.png" -crop 400x300+440+300 +repage -format '%[fx:standard_deviation]' info:)
awk "BEGIN { exit !($centre > 0.01) }" || fail "desktop is a flat colour — no wallpaper (sd $centre), bar mean $bar_mean"

# The panel's right group (clock + clip) must actually ink glyphs: a bar with
# no text is the "panel present but unreadable" failure. Only labels paint
# bright pixels (the button backgrounds read ~0.22 grey, the bar ~0.10).
right_ink=$(magick "$OUT/desktop.png" -crop "300x28+$((w - 300))+$bar_y" +repage -colorspace Gray -threshold 50% -format '%[fx:mean]' info:)
awk "BEGIN { exit !($right_ink > 0.005) }" || fail "panel's right group has no glyphs (clock/clip labels missing; bright fraction $right_ink)"

echo "desktop mean=$desktop_mean bar mean=$bar_mean bar y=$bar_y centre sd=$centre right ink=$right_ink"

# 5. An app window opens and changes the frame. `setsid` + `</dev/null` +
#    redirected output keep the detached terminal from holding the ssh
#    channel open; `timeout` keeps a wedged client from hanging the gate.
wl=$(ssh_user '/usr/local/bin/icedtea-wait wayland' 2>/dev/null | tr -d '\r')
[ -n "$wl" ] || fail "no Wayland socket to launch a client against"
timeout 30 vagrant ssh -c "export XDG_RUNTIME_DIR=/run/user/\$(id -u) WAYLAND_DISPLAY='$wl'; setsid foot </dev/null >/tmp/foot.log 2>&1 & sleep 3; pgrep -x foot >/dev/null" \
  || { vagrant ssh -c 'cat /tmp/foot.log' 2>/dev/null || true; fail "foot did not start"; }
VBoxManage controlvm "$VM" screenshotpng "$OUT/desktop-foot.png" >/dev/null
delta=$(magick compare -metric AE "$OUT/desktop.png" "$OUT/desktop-foot.png" null: 2>&1 || true)
awk "BEGIN { exit !($delta > 10000) }" || fail "no window appeared after launching foot (delta $delta px)"

echo "PASS: icedtea desktop verified (rev ${rev:-unknown}). Screenshots in $OUT"

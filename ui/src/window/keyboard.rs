//! `wl_keyboard` through `libxkbcommon`.
//!
//! The compositor owns the keymap and the modifier state; this module owns the
//! translation, the compose table and -- because xkbcommon has no scheduler --
//! the repeat timer (Task 5).
//!
//! The compose types must be spelled `xkb::compose::{State, Table}`:
//! `xkbcommon::xkb`'s glob re-export of the compose module loses `State` to the
//! keyboard state defined in the same module.

use std::os::fd::OwnedFd;

use xkbcommon::xkb;

bitflags::bitflags! {
    /// The modifiers a widget can key an accelerator off.
    ///
    /// Deliberately not every xkb modifier: `Mod3`/`Mod5` have no GTK meaning,
    /// and an accelerator that matched on them would fire on layouts that use
    /// them for level shifts.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct Mods: u8 {
        /// `Shift`
        const SHIFT = 1 << 0;
        /// `Control`
        const CTRL = 1 << 1;
        /// `Mod1`
        const ALT = 1 << 2;
        /// `Mod4`
        const LOGO = 1 << 3;
        /// `Lock`
        const CAPS = 1 << 4;
        /// `Mod2`
        const NUM = 1 << 5;
    }
}

/// One `wl_keyboard.key`, translated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    /// The raw `wl_keyboard.key` evdev code. XKB's is this + 8.
    pub keycode: u32,
    /// The keysym for the current layout and level, composed if a dead-key
    /// sequence just completed.
    pub keysym: xkb::Keysym,
    /// The text to insert. `None` for a key with no text (modifiers, arrows,
    /// F-keys), for a release, for a control character, and for a keystroke
    /// swallowed mid-compose.
    pub utf8: Option<String>,
    /// Every modifier that was effectively active.
    pub mods: Mods,
    /// The modifiers this keysym consumed to reach its level -- `Shift` for
    /// `A`, nothing for `a`. Contract §3.3's `effective_mods` subtracts them.
    pub consumed: Mods,
    pub pressed: bool,
    /// Synthesised by our own repeat timer, never sent by the compositor.
    pub repeat: bool,
    pub serial: u32,
    pub time_ms: u32,
}

impl KeyEvent {
    /// `mods` with the modifiers this keysym consumed removed -- the mask to
    /// compare an accelerator against.
    ///
    /// On a US layout `Shift+1` is `!`, and the `Shift` was *consumed* to get
    /// there: an accelerator for "plain `!`" must match it, and one for
    /// "`Shift` plus `!`" must not.
    #[must_use]
    pub fn effective_mods(&self) -> Mods {
        self.mods.difference(self.consumed)
    }

    /// Whether this event is `keysym` with exactly `mods` held.
    #[must_use]
    pub fn matches(&self, keysym: xkb::Keysym, mods: Mods) -> bool {
        self.keysym == keysym && self.effective_mods() == mods
    }
}

/// Why a keymap could not be built.
#[derive(Debug)]
pub enum KeymapError {
    /// `xkb_context_new` failed -- effectively out of memory.
    Context,
    /// The keymap text did not compile.
    Compile,
    /// The compositor's keymap fd could not be mapped.
    Mmap(std::io::Error),
}

impl std::fmt::Display for KeymapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Context => write!(f, "cannot create an xkb context"),
            Self::Compile => write!(f, "the keymap did not compile"),
            Self::Mmap(e) => write!(f, "cannot map the keymap fd: {e}"),
        }
    }
}

impl std::error::Error for KeymapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mmap(e) => Some(e),
            Self::Context | Self::Compile => None,
        }
    }
}

/// The modifier indices this keymap uses, resolved once.
///
/// `xkb_state_serialize_mods` reports a mask over *this keymap's* modifier
/// indices, which are not fixed by the protocol: looking them up per event
/// would be a string comparison per modifier per keystroke.
#[derive(Debug, Clone, Copy)]
struct ModIndices {
    shift: xkb::ModIndex,
    ctrl: xkb::ModIndex,
    alt: xkb::ModIndex,
    logo: xkb::ModIndex,
    caps: xkb::ModIndex,
    num: xkb::ModIndex,
}

impl ModIndices {
    fn new(keymap: &xkb::Keymap) -> Self {
        Self {
            shift: keymap.mod_get_index(xkb::MOD_NAME_SHIFT),
            ctrl: keymap.mod_get_index(xkb::MOD_NAME_CTRL),
            alt: keymap.mod_get_index(xkb::MOD_NAME_ALT),
            logo: keymap.mod_get_index(xkb::MOD_NAME_LOGO),
            caps: keymap.mod_get_index(xkb::MOD_NAME_CAPS),
            num: keymap.mod_get_index(xkb::MOD_NAME_NUM),
        }
    }

    /// The [`Mods`] a raw xkb modifier mask stands for.
    ///
    /// A keymap that simply has no `Mod4` reports `MOD_INVALID`, whose bit
    /// would be shifted out of range -- hence the guard rather than a bare
    /// `1 << index`.
    fn decode(self, mask: xkb::ModMask) -> Mods {
        let bit = |index: xkb::ModIndex| {
            index != xkb::MOD_INVALID && index < 32 && mask & (1 << index) != 0
        };
        let mut mods = Mods::empty();
        mods.set(Mods::SHIFT, bit(self.shift));
        mods.set(Mods::CTRL, bit(self.ctrl));
        mods.set(Mods::ALT, bit(self.alt));
        mods.set(Mods::LOGO, bit(self.logo));
        mods.set(Mods::CAPS, bit(self.caps));
        mods.set(Mods::NUM, bit(self.num));
        mods
    }
}

/// A compiled keymap, its live state, and the compose table.
pub struct Keymap {
    /// Kept alive for the keymap and compose table that borrow it in C.
    #[allow(
        dead_code,
        reason = "keeps the xkb_context alive for the keymap and compose table that borrow it"
    )]
    context: xkb::Context,
    keymap: xkb::Keymap,
    state: xkb::State,
    compose: Option<xkb::compose::State>,
    mods: ModIndices,
    // Repeat fields arrive in Task 5.
}

impl Keymap {
    /// Build from an already-compiled `xkb::Keymap`.
    fn wrap(context: xkb::Context, keymap: xkb::Keymap) -> Self {
        let state = xkb::State::new(&keymap);
        let mods = ModIndices::new(&keymap);
        let compose = compose_state_from_locale(&context);
        Self {
            context,
            keymap,
            state,
            compose,
            mods,
        }
    }

    /// From `wl_keyboard.keymap`.
    ///
    /// `MAP_PRIVATE` is required from protocol version 7 on and is what
    /// `xkb::Keymap::new_from_fd` already does (`map_copy_read_only`); never
    /// hand-roll an `mmap` with `MAP_SHARED` here.
    ///
    /// # Errors
    ///
    /// [`KeymapError::Mmap`] if the fd cannot be mapped, [`KeymapError::Compile`]
    /// if what it holds is not a keymap.
    pub fn from_fd(fd: OwnedFd, size: usize) -> Result<Self, KeymapError> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        // SAFETY: the fd came from `wl_keyboard.keymap`, `size` is the size
        // the same event reported, and the mapping is read-only and private.
        let compiled = unsafe {
            xkb::Keymap::new_from_fd(
                &context,
                fd,
                size,
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
        }
        .map_err(KeymapError::Mmap)?
        .ok_or(KeymapError::Compile)?;
        Ok(Self::wrap(context, compiled))
    }

    /// RMLVO, for hermetic tests: `Keymap::from_names("us", "")`.
    ///
    /// # Errors
    ///
    /// [`KeymapError::Compile`] if `xkeyboard-config` is absent or the names
    /// do not resolve.
    pub fn from_names(layout: &str, variant: &str) -> Result<Self, KeymapError> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let compiled = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            layout,
            variant,
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or(KeymapError::Compile)?;
        Ok(Self::wrap(context, compiled))
    }

    /// From keymap text (a vendored fixture, or a keymap read back out).
    ///
    /// # Errors
    ///
    /// [`KeymapError::Compile`] for anything that is not a keymap. This is the
    /// untrusted path: `xkb_keymap_new_from_string` returns null rather than
    /// aborting, and that null becomes this error.
    pub fn from_string(text: &str) -> Result<Self, KeymapError> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let compiled = xkb::Keymap::new_from_string(
            &context,
            text.to_owned(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or(KeymapError::Compile)?;
        Ok(Self::wrap(context, compiled))
    }

    /// From `wl_keyboard.modifiers`.
    ///
    /// `group` fills all three layout components: the protocol carries one
    /// active group, and every other toolkit maps it this way.
    pub fn update_mask(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        self.state
            .update_mask(depressed, latched, locked, group, group, group);
    }

    /// From `wl_keyboard.key`, for a client tracking state itself.
    ///
    /// A Wayland client normally uses [`Self::update_mask`] instead -- the
    /// compositor's state includes keys held before this surface had focus.
    /// This is the hermetic-test path, and xkbcommon warns against mixing the
    /// two on one state.
    pub fn update_key(&mut self, keycode: u32, pressed: bool) {
        let direction = if pressed {
            xkb::KeyDirection::Down
        } else {
            xkb::KeyDirection::Up
        };
        self.state.update_key(xkb_keycode(keycode), direction);
    }

    /// Translate one `wl_keyboard.key`.
    ///
    /// Called *before* any `update_key` for the same event: xkb resolves a
    /// keysym against the state as it was when the key went down.
    pub fn translate(
        &mut self,
        keycode: u32,
        pressed: bool,
        serial: u32,
        time_ms: u32,
    ) -> KeyEvent {
        let key = xkb_keycode(keycode);
        let raw_sym = self.state.key_get_one_sym(key);
        let mods = self.mods();
        let consumed = self.mods.decode(self.state.key_get_consumed_mods(key));
        let (utf8, keysym) = if pressed {
            self.compose(key, raw_sym)
        } else {
            // A release inserts nothing; feeding it to the compose table would
            // also cancel a sequence the press just started.
            (None, raw_sym)
        };
        KeyEvent {
            keycode,
            keysym,
            utf8,
            mods,
            consumed,
            pressed,
            repeat: false,
            serial,
            time_ms,
        }
    }

    /// The effective modifier state.
    #[must_use]
    pub fn mods(&self) -> Mods {
        self.mods
            .decode(self.state.serialize_mods(xkb::STATE_MODS_EFFECTIVE))
    }

    /// `xkb_keymap_key_repeats` -- modifiers say `false`.
    #[must_use]
    pub fn repeats(&self, keycode: u32) -> bool {
        self.keymap.key_repeats(xkb_keycode(keycode))
    }

    /// Run the compose table over one keysym.
    ///
    /// Returns the text to insert and the keysym to report. A sequence in
    /// progress inserts nothing and reports the dead key itself, so a widget
    /// sees "no text" rather than a stray acute accent.
    fn compose(&mut self, key: xkb::Keycode, keysym: xkb::Keysym) -> (Option<String>, xkb::Keysym) {
        // Computed before the mutable borrow of `self.compose`: a closure
        // over `&self.state` and `&mut self.compose` cannot coexist.
        let direct = insertable(self.state.key_get_utf8(key));
        let Some(compose) = self.compose.as_mut() else {
            return (direct, keysym);
        };
        if compose.feed(keysym) == xkb::FeedResult::Ignored {
            return (direct, keysym);
        }
        match compose.status() {
            xkb::Status::Composing => (None, keysym),
            xkb::Status::Composed => {
                let text = compose.utf8().and_then(insertable);
                let sym = compose.keysym().unwrap_or(keysym);
                compose.reset();
                (text, sym)
            }
            xkb::Status::Cancelled => {
                compose.reset();
                (None, keysym)
            }
            xkb::Status::Nothing => (direct, keysym),
        }
    }

    /// The keymap's index for a modifier name. Test-only: the mask
    /// `update_mask` takes is over indices this keymap chose.
    #[cfg(test)]
    fn mod_index_for_test(&self, name: &str) -> xkb::ModIndex {
        self.keymap.mod_get_index(name)
    }
}

/// `wl_keyboard.key`'s evdev code as an XKB keycode.
///
/// "to determine the xkb keycode, clients must add 8 to the key event
/// keycode" -- `wayland.xml`'s `keymap_format.xkb_v1`. Saturating, so a
/// hostile `u32::MAX` is a keycode the keymap simply does not have rather
/// than an overflow panic in a debug build.
fn xkb_keycode(keycode: u32) -> xkb::Keycode {
    xkb::Keycode::new(keycode.saturating_add(8))
}

/// Text a widget should actually insert.
///
/// `xkb_state_key_get_utf8` reports the C0 control character for `Ctrl+A`
/// (`\x01`) and an empty string for a key with no text; neither is something
/// an entry inserts.
fn insertable(text: String) -> Option<String> {
    if text.is_empty() || text.chars().any(char::is_control) {
        return None;
    }
    Some(text)
}

/// The compose state for the process's locale, if it has a usable table.
///
/// `LC_ALL` > `LC_CTYPE` > `LANG`, as every other toolkit resolves it. A
/// missing or unparsable table is not an error: the client just does not
/// compose.
fn compose_state_from_locale(context: &xkb::Context) -> Option<xkb::compose::State> {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|name| std::env::var_os(name).filter(|v| !v.is_empty()))
        .unwrap_or_else(|| std::ffi::OsString::from("C"));
    let table =
        xkb::compose::Table::new_from_locale(context, &locale, xkb::compose::COMPILE_NO_FLAGS)
            .ok()?;
    Some(xkb::compose::State::new(
        &table,
        xkb::compose::STATE_NO_FLAGS,
    ))
}

#[cfg(test)]
mod tests {
    use super::{Keymap, KeymapError, Mods};
    use xkbcommon::xkb;

    /// Linux evdev codes, as `wl_keyboard.key` reports them (XKB's are +8).
    const KEY_A: u32 = 30;
    const KEY_B: u32 = 48;
    const KEY_1: u32 = 2;
    const KEY_TAB: u32 = 15;
    const KEY_LEFTSHIFT: u32 = 42;
    const KEY_LEFTCTRL: u32 = 29;
    const KEY_CAPSLOCK: u32 = 58;

    fn us() -> Keymap {
        Keymap::from_string(include_str!("../../tests/fixtures/keymaps/us.xkb"))
            .expect("the vendored us keymap compiles")
    }

    #[test]
    fn a_plain_letter_translates_to_its_keysym_and_text() {
        let mut keymap = us();
        let event = keymap.translate(KEY_A, true, 7, 1234);
        assert_eq!(event.keysym, xkb::Keysym::a);
        assert_eq!(event.utf8.as_deref(), Some("a"));
        assert_eq!(
            event.keycode, KEY_A,
            "the raw evdev code is reported, not XKB's +8"
        );
        assert_eq!(event.mods, Mods::empty());
        assert!(event.pressed);
        assert!(!event.repeat);
        assert_eq!(event.serial, 7);
        assert_eq!(event.time_ms, 1234);
    }

    #[test]
    fn a_release_carries_no_text() {
        // GTK inserts text on press only; a release that also carried "a"
        // would type every character twice.
        let mut keymap = us();
        let event = keymap.translate(KEY_A, false, 8, 1250);
        assert_eq!(event.keysym, xkb::Keysym::a);
        assert!(event.utf8.is_none(), "a key release must produce no text");
        assert!(!event.pressed);
    }

    #[test]
    fn shift_shifts_the_level_and_reports_the_modifier() {
        let mut keymap = us();
        keymap.update_key(KEY_LEFTSHIFT, true);
        assert_eq!(keymap.mods(), Mods::SHIFT);
        let event = keymap.translate(KEY_A, true, 9, 0);
        assert_eq!(event.keysym, xkb::Keysym::A);
        assert_eq!(event.utf8.as_deref(), Some("A"));
        assert_eq!(event.mods, Mods::SHIFT);
        keymap.update_key(KEY_LEFTSHIFT, false);
        assert_eq!(keymap.mods(), Mods::empty());
        assert_eq!(
            keymap.translate(KEY_A, true, 10, 0).utf8.as_deref(),
            Some("a")
        );
    }

    #[test]
    fn the_servers_modifier_state_wins_over_our_own_bookkeeping() {
        // A Wayland client feeds `wl_keyboard.modifiers`, not its own key
        // tracking: the compositor is the one that knows about keys pressed
        // before we were focused. `update_mask` must therefore be able to
        // assert a modifier we never saw go down.
        let mut keymap = us();
        let shift = 1u32 << keymap.mod_index_for_test(xkb::MOD_NAME_SHIFT);
        let caps = 1u32 << keymap.mod_index_for_test(xkb::MOD_NAME_CAPS);
        keymap.update_mask(shift, 0, caps, 0);
        assert_eq!(keymap.mods(), Mods::SHIFT | Mods::CAPS);
        assert_eq!(
            keymap.translate(KEY_A, true, 11, 0).utf8.as_deref(),
            Some("a"),
            "shift with caps lock cancels back to lower case, as xkb defines it"
        );
        keymap.update_mask(0, 0, 0, 0);
        assert_eq!(keymap.mods(), Mods::empty());
    }

    #[test]
    fn control_reports_ctrl_and_no_printable_text() {
        let mut keymap = us();
        keymap.update_key(KEY_LEFTCTRL, true);
        let event = keymap.translate(KEY_A, true, 12, 0);
        assert!(event.mods.contains(Mods::CTRL));
        // xkb hands back the C0 control character; a text-entry widget must
        // not insert it, and `utf8` is what it inserts.
        assert!(
            event
                .utf8
                .as_deref()
                .is_none_or(|s| s.chars().all(|c| !c.is_control())),
            "Ctrl+A produced insertable control text: {:?}",
            event.utf8
        );
        assert_eq!(
            event.keysym,
            xkb::Keysym::a,
            "Ctrl does not change the keysym"
        );
    }

    #[test]
    fn keys_with_no_text_report_none() {
        let mut keymap = us();
        for code in [KEY_TAB, KEY_LEFTSHIFT, KEY_CAPSLOCK] {
            let event = keymap.translate(code, true, 0, 0);
            assert!(
                event
                    .utf8
                    .as_deref()
                    .is_none_or(|s| s.chars().all(|c| !c.is_control())),
                "{code} produced control text {:?}",
                event.utf8
            );
        }
        assert_eq!(us().translate(KEY_TAB, true, 0, 0).keysym, xkb::Keysym::Tab);
    }

    #[test]
    fn which_keys_repeat_comes_from_the_keymap() {
        let keymap = us();
        assert!(keymap.repeats(KEY_A), "letters repeat");
        assert!(keymap.repeats(KEY_1), "digits repeat");
        assert!(!keymap.repeats(KEY_LEFTSHIFT), "modifiers do not repeat");
        assert!(!keymap.repeats(KEY_CAPSLOCK), "locks do not repeat");
        assert!(
            !keymap.repeats(u32::MAX),
            "an out-of-range keycode is not a panic"
        );
    }

    #[test]
    fn a_malformed_keymap_is_an_error_and_never_a_panic() {
        // The keymap arrives on an fd from the compositor: it is untrusted
        // input, and a compositor that hands us rubbish must not take the
        // client down with it.
        for text in [
            "",
            "\0",
            "xkb_keymap {",
            "xkb_keymap { xkb_keycodes { <A> = notanumber; }; };",
            "not a keymap at all",
            &"x".repeat(100_000),
        ] {
            match Keymap::from_string(text) {
                Err(KeymapError::Compile) => {}
                Err(other) => panic!("unexpected error for {text:?}: {other:?}"),
                Ok(_) => panic!("{text:?} compiled as a keymap"),
            }
        }
        // Non-UTF-8-safe slicing is the other classic panic here: a keymap
        // that is valid text but truncated mid-rule.
        let truncated = &include_str!("../../tests/fixtures/keymaps/us.xkb")[..500];
        assert!(matches!(
            Keymap::from_string(truncated),
            Err(KeymapError::Compile)
        ));
    }

    #[test]
    fn from_names_builds_the_same_layout_as_the_fixture() {
        // Not a fixture check: this is the path a test with no vendored file
        // uses, and it must produce the same translations.
        let Ok(mut named) = Keymap::from_names("us", "") else {
            // A build host with no xkeyboard-config installed: the vendored
            // fixture is exactly why every other test here does not need it.
            return;
        };
        assert_eq!(
            named.translate(KEY_A, true, 0, 0).utf8.as_deref(),
            Some("a")
        );
        assert_eq!(named.translate(KEY_B, true, 0, 0).keysym, xkb::Keysym::b);
    }
}

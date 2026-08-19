use xkbcommon::xkb;

/// The modifier tokens a `KeyCombo` may name.
pub const MODIFIER_TOKENS: [&str; 4] = ["SUPER", "CTRL", "ALT", "SHIFT"];

/// Resolve a binding's key name (`"KEY_<xkb keysym name>"`, e.g. `"KEY_q"`,
/// `"KEY_Return"`, `"KEY_F5"`, `"KEY_bracketleft"`) to its keysym, or `0`
/// when the name names no keysym at all.
///
/// Review finding I4: this used to be a hand-written table of nine keys plus
/// the digits `1..=9`; *everything* else -- every letter but `q`/`f`/`r`,
/// every function key, every punctuation key -- resolved to `0`, so a
/// perfectly ordinary custom binding like `SUPER+a` could never fire. It
/// also logged a `tracing::warn!` on the miss, and `match_action` calls this
/// once per binding per key press, so a single unresolvable binding produced
/// a warn line on *every* keystroke. Resolution now goes through xkb (the
/// same keysym database the input path itself uses, so names and runtime
/// keysyms cannot drift apart), and the diagnostics moved to
/// `validate_keybindings`, which runs once at config load/reload.
pub fn key_name_to_keysym(name: &str) -> u32 {
    let bare = name.strip_prefix("KEY_").unwrap_or(name);
    let sym = xkb::keysym_from_name(bare, xkb::KEYSYM_NO_FLAGS);
    if sym.raw() != xkb::keysyms::KEY_NoSymbol {
        return sym.raw();
    }
    // Second pass, per xkbcommon's own recommendation: a case-insensitive
    // lookup catches `"KEY_TAB"`/`"KEY_return"`-style spellings. Resolving
    // only on this pass is accepted silently -- `validate_keybindings` calls
    // this same function and reports a binding only when *both* passes come
    // back `KEY_NoSymbol`, i.e. when the name matches no keysym at all.
    xkb::keysym_from_name(bare, xkb::KEYSYM_CASE_INSENSITIVE).raw()
}

/// Inverse of [`key_name_to_keysym`]; used for D-Bus/debug output.
pub fn keysym_to_key_name(keysym: u32) -> String {
    let name = xkb::keysym_get_name(xkb::Keysym::new(keysym));
    if name.is_empty() {
        return format!("KEY_{keysym}");
    }
    format!("KEY_{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// I4: the nine-key hand table is gone -- every xkb keysym name is
    /// bindable now -- while the names the old table did know keep resolving
    /// to exactly the same keysyms (so no existing config silently changes
    /// meaning).
    #[test]
    fn key_names_resolve_through_xkb_without_regressing_the_old_table() {
        for (name, sym) in [
            ("KEY_Return", 0xff0d),
            ("KEY_Tab", 0xff09),
            ("KEY_Left", 0xff51),
            ("KEY_Up", 0xff52),
            ("KEY_Right", 0xff53),
            ("KEY_Down", 0xff54),
            ("KEY_q", 0x71),
            ("KEY_f", 0x66),
            ("KEY_r", 0x72),
            ("KEY_1", 0x31),
            ("KEY_9", 0x39),
        ] {
            assert_eq!(key_name_to_keysym(name), sym, "{name} must keep its keysym");
        }
        // Newly bindable: the other 23 letters, function keys, punctuation.
        assert_eq!(key_name_to_keysym("KEY_a"), 0x61);
        assert_eq!(key_name_to_keysym("KEY_z"), 0x7a);
        assert_eq!(key_name_to_keysym("KEY_F5"), 0xffc2);
        assert_eq!(key_name_to_keysym("KEY_space"), 0x20);
        assert_eq!(key_name_to_keysym("KEY_bracketleft"), 0x5b);
        // Still unresolvable, still reported as `NoSymbol`.
        assert_eq!(key_name_to_keysym("KEY_definitely_not_a_key"), xkb::keysyms::KEY_NoSymbol);
    }

    #[test]
    fn key_name_round_trips_through_keysym_to_key_name() {
        for name in ["KEY_Return", "KEY_Tab", "KEY_q", "KEY_a", "KEY_F5", "KEY_1", "KEY_space"] {
            assert_eq!(keysym_to_key_name(key_name_to_keysym(name)), name);
        }
    }
}

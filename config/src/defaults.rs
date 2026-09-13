use std::collections::HashMap;

use crate::{Behavior, Config, KeyCombo, LauncherConfig, Power};
use contract::Appearance;

/// The default power/suspend/hibernate key bindings (spec Decision 4), as
/// `(action, keysym)`. Exposed to the config read path: a stored keybindings
/// table replaces the defaults wholesale, so a config saved before A3 would
/// otherwise leave these keys unbound while the session daemon still takes the
/// power-key block inhibitor — a silent no-op. The read path backfills only the
/// missing entries, so a user's own bindings are kept.
pub const POWER_KEY_BINDINGS: [(&str, &str); 3] = [
    ("spawn:icedtea-session lock", "XF86_PowerOff"),
    ("spawn:icedtea-session suspend", "XF86_Sleep"),
    ("spawn:icedtea-session hibernate", "XF86_Hibernate"),
];

pub fn default_config() -> Config {
    let mut keybindings = HashMap::new();
    let insert = |map: &mut HashMap<String, KeyCombo>, action: &str, mods: &[&str], key: &str| {
        map.insert(
            action.to_string(),
            KeyCombo {
                modifiers: mods.iter().map(|m| m.to_string()).collect(),
                key: key.to_string(),
            },
        );
    };
    insert(&mut keybindings, "close", &["SUPER"], "KEY_q");
    insert(&mut keybindings, "fullscreen", &["SUPER"], "KEY_f");
    insert(&mut keybindings, "reload", &["SUPER", "SHIFT"], "KEY_r");
    insert(&mut keybindings, "quit", &["SUPER", "SHIFT"], "KEY_q");
    insert(&mut keybindings, "cycle:alt_tab", &["SUPER"], "KEY_Tab");
    insert(&mut keybindings, "spawn:terminal", &["SUPER"], "KEY_Return");
    insert(&mut keybindings, "snap:left", &["SUPER"], "KEY_Left");
    insert(&mut keybindings, "snap:right", &["SUPER"], "KEY_Right");
    insert(&mut keybindings, "snap:up", &["SUPER"], "KEY_Up");
    insert(&mut keybindings, "snap:down", &["SUPER"], "KEY_Down");
    insert(
        &mut keybindings,
        "snap:restore",
        &["SUPER", "SHIFT"],
        "KEY_Left",
    );
    for n in 1..=9u32 {
        let key = format!("KEY_{n}");
        insert(
            &mut keybindings,
            &format!("workspace:{n}"),
            &["SUPER"],
            &key,
        );
        insert(
            &mut keybindings,
            &format!("move_to_workspace:{n}"),
            &["SUPER", "CTRL"],
            &key,
        );
    }
    for (action, key) in POWER_KEY_BINDINGS {
        insert(&mut keybindings, action, &[], key);
    }

    Config {
        keybindings,
        appearance: Appearance {
            bar_position: "bottom".into(),
            bar_height: 42,
            corner_radius: 8,
            snap_gap: 8,
            palette: contract::Palette {
                background: "#1e1e2e".into(),
                foreground: "#cdd6f4".into(),
                accent: "#89b4fa".into(),
            },
            wallpaper: None,
        },
        behavior: Behavior {
            raise_on_focus: true,
            hide_bar_on_fullscreen: true,
            snap_enabled: true,
        },
        power: Power::default(),
        launcher: LauncherConfig {
            pinned: vec![],
            tile_groups: vec![],
            recency: HashMap::new(),
        },
        workspace_names: vec!["1".into(), "2".into(), "3".into(), "4".into()],
        displays: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_binds_the_power_keys_to_the_session_cli() {
        let config = default_config();
        for (action, key) in [
            ("spawn:icedtea-session lock", "XF86_PowerOff"),
            ("spawn:icedtea-session suspend", "XF86_Sleep"),
            ("spawn:icedtea-session hibernate", "XF86_Hibernate"),
        ] {
            let combo = config
                .keybindings
                .get(action)
                .unwrap_or_else(|| panic!("default config is missing {action:?}"));
            assert!(
                combo.modifiers.is_empty(),
                "{action:?} must carry no modifiers"
            );
            assert_eq!(combo.key, key, "{action:?} must be bound to {key}");
            assert_ne!(
                crate::key_name_to_keysym(&combo.key),
                0,
                "{action:?} key {key} must resolve to a real keysym"
            );
        }
    }
}

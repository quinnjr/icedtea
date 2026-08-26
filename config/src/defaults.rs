use std::collections::HashMap;

use crate::{Behavior, Config, KeyCombo};
use contract::Appearance;

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
        workspace_names: vec!["1".into(), "2".into(), "3".into(), "4".into()],
        displays: vec![],
    }
}

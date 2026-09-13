use redb::TableDefinition;

pub const DB_META: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("meta");
pub const DB_KEYBINDINGS: TableDefinition<&'static str, &'static [u8]> =
    TableDefinition::new("keybindings");
pub const DB_APPEARANCE: TableDefinition<&'static str, &'static [u8]> =
    TableDefinition::new("appearance");
pub const DB_BEHAVIOR: TableDefinition<&'static str, &'static [u8]> =
    TableDefinition::new("behavior");
pub const DB_POWER: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("power");
pub const DB_WORKSPACES: TableDefinition<&'static str, &'static [u8]> =
    TableDefinition::new("workspaces");
pub const DB_DISPLAYS: TableDefinition<&'static str, &'static [u8]> =
    TableDefinition::new("displays");
pub const DB_LAUNCHER: TableDefinition<&'static str, &'static [u8]> =
    TableDefinition::new("launcher");

pub const KEY_SCHEMA_VERSION: &str = "schema_version";
pub const KEY_APPEARANCE: &str = "appearance";
pub const KEY_BEHAVIOR: &str = "behavior";
pub const KEY_POWER: &str = "power";
pub const KEY_WORKSPACES: &str = "workspaces";
pub const KEY_DISPLAYS: &str = "displays";
pub const KEY_LAUNCHER: &str = "launcher";
pub const KEY_ACTION_COUNT: &str = "action_count";
pub const KEY_ACTION: &str = "action:";

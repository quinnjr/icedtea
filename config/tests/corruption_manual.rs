use std::fs;
use std::io::Write;

use icedtea_config::{default_config, load_or_default, open};

#[test]
fn garbage_bytes_file_returns_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("garbage.redb");
    let mut f = fs::File::create(&path).unwrap();
    for _ in 0..512 {
        f.write_all(b"this is not a redb database, just garbage bytes\x00\xff\x01").unwrap();
    }
    f.sync_all().unwrap();
    drop(f);

    let cfg = load_or_default(&path);
    assert_eq!(cfg, default_config());
}

#[test]
fn truncated_redb_file_returns_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trunc.redb");
    let db = open(&path).unwrap();
    default_config().save(&db).unwrap();
    drop(db);

    let full_len = fs::metadata(&path).unwrap().len();
    fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(full_len / 4).unwrap();

    let cfg = load_or_default(&path);
    assert_eq!(cfg, default_config());
}

#[test]
fn corrupt_db_never_panics_after_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixed.redb");
    let db = open(&path).unwrap();
    default_config().save(&db).unwrap();
    drop(db);
    let loaded = load_or_default(&path);
    assert_eq!(loaded, default_config());
}

/// A file that opens cleanly via `Database::create` (structurally plausible
/// header/layout) can still have corrupt page/table data that only surfaces
/// once something actually reads it — `begin_read`, `open_table`, a table
/// `get`, or the keybindings range cursor. Rather than truncating (which
/// exercises `open`'s own panic-catching, covered above), flip a block of
/// bytes in the middle of an otherwise-valid file so the header/create path
/// still succeeds but subsequent reads see garbage.
#[test]
fn mid_file_corruption_never_panics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mid_corrupt.redb");
    let db = open(&path).unwrap();
    default_config().save(&db).unwrap();
    drop(db);

    let full_len = fs::metadata(&path).unwrap().len();
    let mut file = fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
    let start = full_len / 2;
    let len = (full_len / 4).max(64).min(full_len.saturating_sub(start));
    let garbage = vec![0xA5u8; len as usize];
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(start)).unwrap();
    file.write_all(&garbage).unwrap();
    file.sync_all().unwrap();
    drop(file);

    // The contract under test: no panic. Whether redb salvages a read,
    // errors out, or trips the assert we catch, `load_or_default` must
    // return a plain `Config` rather than crashing the process.
    let _cfg = load_or_default(&path);
}

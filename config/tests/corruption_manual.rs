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

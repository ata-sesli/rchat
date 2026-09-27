use rchat_storage::open_rchat;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

// Uses the independent SQLite CLI, never a second SQLite library in this process.
#[test]
fn converts_live_wal_snapshot_and_preserves_sql_objects_and_source() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("rchat.sqlite");
    let mut child = Command::new("sqlite3")
        .arg(&source)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("sqlite3 test prerequisite");
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    writeln!(input, "PRAGMA journal_mode=WAL; CREATE TABLE history(id TEXT PRIMARY KEY, alias TEXT, payload BLOB); CREATE INDEX aliases ON history(alias); CREATE TABLE audit(id TEXT); CREATE TRIGGER inserted AFTER INSERT ON history BEGIN INSERT INTO audit VALUES(new.id); END; INSERT INTO history VALUES('wal-only','Alice',X'0001FF'); INSERT INTO history VALUES('null',NULL,NULL);").unwrap();
    // sqlite dot commands must start on their own line.
    writeln!(input, ".print READY").unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    loop {
        line.clear();
        assert!(output.read_line(&mut line).unwrap() > 0);
        if line.trim() == "READY" {
            break;
        }
    }
    assert!(root.path().join("rchat.sqlite-wal").exists());
    let db = open_rchat(root.path(), |_| Ok(())).unwrap();
    assert_eq!(
        db.query_row("SELECT alias FROM history WHERE id='wal-only'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "Alice"
    );
    assert_eq!(
        db.query_row("SELECT payload FROM history WHERE id='wal-only'", [], |r| r
            .get::<_, Vec<u8>>(0))
            .unwrap(),
        [0, 1, 255]
    );
    assert_eq!(
        db.query_row("SELECT alias FROM history WHERE id='null'", [], |r| r
            .get::<_, Option<String>>(0))
            .unwrap(),
        None
    );
    db.execute("INSERT INTO history VALUES('new','Bob',NULL)", [])
        .unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(input);
    assert!(child.wait().unwrap().success());
    assert!(source.exists());
    let original = Command::new("sqlite3")
        .arg(source)
        .arg("SELECT COUNT(*) FROM history")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(original.stdout).unwrap().trim(), "2");
}

#[test]
fn invalid_source_is_not_replaced_and_cannot_be_silently_selected() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("rchat.sqlite");
    std::fs::write(&source, b"not a database").unwrap();
    assert!(open_rchat(root.path(), |_| Ok(())).is_err());
    assert!(!root.path().join("rchat.zova").exists());
    assert_eq!(std::fs::read(source).unwrap(), b"not a database");
}

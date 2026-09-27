use rchat_storage::Connection;

#[test]
fn verified_backup_preserves_sql_and_object_bytes() {
    let root = tempfile::tempdir().unwrap();
    let db = Connection::open_in_memory().unwrap();
    let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    db.put_chunk(hash, b"abc").unwrap();
    db.assemble_object(hash, &[(hash.into(), 3)]).unwrap();
    db.execute("CREATE TABLE message(alias TEXT)", []).unwrap();
    db.execute("INSERT INTO message VALUES('Alice')", [])
        .unwrap();
    let backup = root.path().join("backup.zova");
    db.backup_to(&backup).unwrap();
    let restored = Connection::open(&backup).unwrap();
    assert_eq!(restored.get_object(hash).unwrap(), b"abc");
    assert_eq!(
        restored
            .query_row("SELECT alias FROM message", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "Alice"
    );
}

#[test]
fn transport_chunks_keep_their_identity_and_assemble_verified_object() {
    let db = Connection::open_in_memory().unwrap();
    let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    assert!(db.put_chunk(hash, b"wrong").is_err());
    db.put_chunk(hash, b"abc").unwrap();
    assert_eq!(db.get_chunk(hash).unwrap(), b"abc");
    db.assemble_object(hash, &[(hash.to_string(), 3)]).unwrap();
    assert_eq!(db.get_object(hash).unwrap(), b"abc");
}

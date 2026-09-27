use rchat_storage::{open_rchat, Connection};

#[test]
fn new_store_reopens_and_ambiguous_existing_destination_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let db = open_rchat(root.path(), |_| Ok(())).unwrap();
    db.execute("CREATE TABLE records(value TEXT)", []).unwrap();
    db.execute("INSERT INTO records VALUES('kept')", [])
        .unwrap();
    drop(db);
    let db = open_rchat(root.path(), |_| panic!("must not reinitialize")).unwrap();
    assert_eq!(
        db.query_row("SELECT value FROM records", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "kept"
    );
    drop(db);
    let other = tempfile::tempdir().unwrap();
    drop(Connection::open(other.path().join("rchat.zova")).unwrap());
    std::fs::write(other.path().join("rchat.sqlite"), b"old").unwrap();
    assert!(open_rchat(other.path(), |_| Ok(())).is_err());
}

#[test]
fn interrupted_initialization_never_selects_partial_destination() {
    let root = tempfile::tempdir().unwrap();
    assert!(open_rchat(root.path(), |db| {
        db.execute("CREATE TABLE partial(value TEXT)", [])?;
        Err(rchat_storage::Error::InvalidValue("interrupted".into()))
    })
    .is_err());
    assert!(!root.path().join("rchat.zova").exists());
    assert!(open_rchat(root.path(), |_| Ok(())).is_err());
}

#[test]
fn database_full_during_initialization_does_not_publish_destination() {
    let root = tempfile::tempdir().unwrap();
    let result = open_rchat(root.path(), |db| {
        db.execute_batch("CREATE TABLE full(value BLOB); PRAGMA max_page_count=1;")?;
        db.execute("INSERT INTO full VALUES(zeroblob(1000000))", [])?;
        Ok(())
    });
    assert!(result.is_err());
    assert!(!root.path().join("rchat.zova").exists());
}

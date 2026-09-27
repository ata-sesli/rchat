use zova::{SharedDatabase, Step};

fn count(db: &SharedDatabase) -> i64 {
    let mut statement = db.prepare("SELECT count(*) FROM records").unwrap();
    assert_eq!(statement.step().unwrap(), Step::Row);
    statement.column_i64(0).unwrap()
}

#[test]
fn zova_1_1_requires_object_staging_before_sql_transaction() {
    let db = SharedDatabase::create_memory().unwrap();
    db.exec("CREATE TABLE records(id INTEGER PRIMARY KEY, value BLOB)")
        .unwrap();
    db.begin_immediate().unwrap();
    db.exec("INSERT INTO records VALUES (1, x'0001ff')")
        .unwrap();
    let error = db.put_object(b"archive attachment").unwrap_err();
    assert!(error.to_string().contains("ObjectTransactionActive"));
    db.rollback().unwrap();
    assert_eq!(count(&db), 0);
    let object = db.put_object(b"archive attachment").unwrap();
    assert_eq!(db.get_object(object).unwrap(), b"archive attachment");
}

#[test]
fn zova_1_1_requires_explicit_rollback_after_transaction_panic() {
    let db = SharedDatabase::create_memory().unwrap();
    db.exec("CREATE TABLE records(id INTEGER PRIMARY KEY, value BLOB)")
        .unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: zova::Result<()> = db.transaction(|transaction| {
            transaction.exec("INSERT INTO records VALUES(1, NULL)")?;
            panic!("archive interrupted");
        });
    }));
    assert!(panic.is_err());
    // RChat must use its rollback-on-drop boundary, not this raw closure API.
    assert_eq!(count(&db), 1);
    db.rollback().unwrap();
    assert_eq!(count(&db), 0);
    db.exec("INSERT INTO records VALUES(2, NULL)").unwrap();
    assert_eq!(count(&db), 1);
}

#[test]
fn savepoint_rollback_preserves_outer_transaction() {
    let db = SharedDatabase::create_memory().unwrap();
    db.exec("CREATE TABLE records(id INTEGER PRIMARY KEY, value BLOB)")
        .unwrap();
    db.begin().unwrap();
    db.exec("INSERT INTO records VALUES(1, NULL)").unwrap();
    db.savepoint("archive").unwrap();
    db.exec("INSERT INTO records VALUES(2, x'00ff')").unwrap();
    db.rollback_to_savepoint("archive").unwrap();
    db.release_savepoint("archive").unwrap();
    db.commit().unwrap();
    assert_eq!(count(&db), 1);
}

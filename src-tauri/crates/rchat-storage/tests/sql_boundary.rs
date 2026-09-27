use rchat_storage::{params, Connection, OptionalExtension};

#[test]
fn finished_and_dropped_queries_release_read_transaction() {
    let db = Connection::open_in_memory().unwrap();
    db.execute("CREATE TABLE values_table(value INTEGER)", [])
        .unwrap();
    db.execute_batch("INSERT INTO values_table VALUES(1),(2);")
        .unwrap();
    let mut statement = db.prepare("SELECT value FROM values_table").unwrap();
    assert_eq!(statement.query_row([], |r| r.get::<_, i64>(0)).unwrap(), 1);
    let hash = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    db.put_chunk(hash, b"abc").unwrap();
    db.assemble_object(hash, &[(hash.into(), 3)]).unwrap();
    let mut rows = statement.query([]).unwrap();
    assert!(rows.next().unwrap().is_some());
    drop(rows);
    use sha2::{Digest, Sha256};
    let second = hex::encode(Sha256::digest(b"def"));
    db.put_chunk(&second, b"def").unwrap();
    db.assemble_object(&second, &[(second.clone(), 3)]).unwrap();
}

#[test]
fn sql_values_and_optional_rows_round_trip() {
    let db = Connection::open_in_memory().unwrap();
    db.execute(
        "CREATE TABLE records(id INTEGER PRIMARY KEY, name TEXT, data BLOB, missing TEXT)",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO records VALUES(?1, ?2, ?3, ?4)",
        params![7_i64, "hello", vec![0_u8, 255], None::<String>],
    )
    .unwrap();
    let value: (i64, String, Vec<u8>, Option<String>) = db
        .query_row("SELECT * FROM records", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap();
    assert_eq!(value, (7, "hello".into(), vec![0, 255], None));
    assert_eq!(
        db.query_row("SELECT id FROM records WHERE id=8", [], |row| row
            .get::<_, i64>(0))
            .optional()
            .unwrap(),
        None
    );
}

#[test]
fn dropped_and_panicked_transactions_rollback() {
    let db = Connection::open_in_memory().unwrap();
    db.execute("CREATE TABLE records(id INTEGER)", []).unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let tx = db.unchecked_transaction().unwrap();
        tx.execute("INSERT INTO records VALUES(1)", []).unwrap();
        panic!("archive interrupted");
    }));
    assert!(panic.is_err());
    assert_eq!(
        db.query_row("SELECT count(*) FROM records", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let tx = db.unchecked_transaction().unwrap();
    tx.execute("INSERT INTO records VALUES(2)", []).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM records", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn outer_mutex_serializes_whole_worker_transactions() {
    use std::sync::{Arc, Mutex};
    let db = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
    db.lock()
        .unwrap()
        .execute("CREATE TABLE writes(worker INTEGER, part INTEGER)", [])
        .unwrap();
    let workers = (0..4)
        .map(|worker| {
            let db = db.clone();
            std::thread::spawn(move || {
                for _ in 0..10 {
                    let conn = db.lock().unwrap();
                    let tx = conn.unchecked_transaction().unwrap();
                    tx.execute("INSERT INTO writes VALUES(?1,1)", [worker])
                        .unwrap();
                    std::thread::yield_now();
                    tx.execute("INSERT INTO writes VALUES(?1,2)", [worker])
                        .unwrap();
                    tx.commit().unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    let conn = db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT worker,part FROM writes ORDER BY rowid")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 80);
    for pair in rows.as_chunks::<2>().0 {
        assert_eq!(pair[0].0, pair[1].0);
        assert_eq!((pair[0].1, pair[1].1), (1, 2));
    }
}

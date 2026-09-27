//! Small SQL boundary backed exclusively by Zova.
//!
//! Connection is Send but not Sync or Clone. RChat's outer mutex therefore
//! owns the entire transaction, not individual statements. No native handles
//! escape this module and no second SQLite implementation is linked here.
use std::{cell::Cell, marker::PhantomData, path::Path};
use zova::{ColumnType, SharedDatabase, SharedStatement, Step};
mod migration;
mod objects;
pub use migration::open_rchat;

#[derive(Debug)]
pub enum Error {
    Zova(zova::Error),
    NoRows,
    InvalidValue(String),
}
impl From<zova::Error> for Error {
    fn from(e: zova::Error) -> Self {
        Self::Zova(e)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Zova(e) => e.fmt(f),
            Self::NoRows => f.write_str("query returned no rows"),
            Self::InvalidValue(e) => f.write_str(e),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}
impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(Error::NoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}
pub trait ToSql {
    fn value(&self) -> Result<Value>;
}
impl<T: ToSql + ?Sized> ToSql for &T {
    fn value(&self) -> Result<Value> {
        (*self).value()
    }
}
impl ToSql for str {
    fn value(&self) -> Result<Value> {
        Ok(Value::Text(self.into()))
    }
}
impl ToSql for String {
    fn value(&self) -> Result<Value> {
        self.as_str().value()
    }
}
impl ToSql for [u8] {
    fn value(&self) -> Result<Value> {
        Ok(Value::Blob(self.into()))
    }
}
impl ToSql for Vec<u8> {
    fn value(&self) -> Result<Value> {
        self.as_slice().value()
    }
}
impl ToSql for bool {
    fn value(&self) -> Result<Value> {
        Ok(Value::Integer(i64::from(*self)))
    }
}
impl ToSql for f64 {
    fn value(&self) -> Result<Value> {
        Ok(Value::Real(*self))
    }
}
impl<T: ToSql> ToSql for Option<T> {
    fn value(&self) -> Result<Value> {
        self.as_ref().map(ToSql::value).unwrap_or(Ok(Value::Null))
    }
}
macro_rules! integers { ($($t:ty),*) => { $(impl ToSql for $t { fn value(&self)->Result<Value>{ Ok(Value::Integer(i64::try_from(*self).map_err(|_|Error::InvalidValue("integer overflow".into()))?)) } })* }; }
integers!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);
pub trait Params {
    fn values(self) -> Result<Vec<Value>>;
}
impl Params for [(); 0] {
    fn values(self) -> Result<Vec<Value>> {
        Ok(vec![])
    }
}
impl Params for &[&dyn ToSql] {
    fn values(self) -> Result<Vec<Value>> {
        self.iter().map(|v| v.value()).collect()
    }
}
macro_rules! arrays { ($($n:expr),*) => { $(impl<T:ToSql> Params for [T;$n] { fn values(self)->Result<Vec<Value>>{ self.iter().map(ToSql::value).collect() } })* }; }
arrays!(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16);
macro_rules! tuples { ($($t:ident:$i:tt),+) => { impl<$($t:ToSql),+> Params for ($($t,)+) { fn values(self)->Result<Vec<Value>>{ Ok(vec![$(self.$i.value()?),+]) } } }; }
tuples!(A:0);
tuples!(A:0,B:1);
tuples!(A:0,B:1,C:2);
tuples!(A:0,B:1,C:2,D:3);
tuples!(A:0,B:1,C:2,D:3,E:4);
tuples!(A:0,B:1,C:2,D:3,E:4,F:5);
tuples!(A:0,B:1,C:2,D:3,E:4,F:5,G:6);
tuples!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7);
tuples!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8);
tuples!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9);
tuples!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10);
tuples!(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11);
#[macro_export]
macro_rules! params { ($($v:expr),* $(,)?) => { &[$(&$v as &dyn $crate::ToSql),*] as &[&dyn $crate::ToSql] }; }

pub struct Connection {
    db: SharedDatabase,
    _not_sync: PhantomData<Cell<()>>,
}
impl Connection {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = if path.as_ref().exists() {
            SharedDatabase::open(path)?
        } else {
            SharedDatabase::create(path)?
        };
        Ok(Self {
            db,
            _not_sync: PhantomData,
        })
    }
    pub fn open_in_memory() -> Result<Self> {
        Ok(Self {
            db: SharedDatabase::create_memory()?,
            _not_sync: PhantomData,
        })
    }
    pub fn backup_to(&self, path: impl AsRef<Path>) -> Result<()> {
        Ok(self
            .db
            .backup_to(path, zova::BackupOptions { verify: true })?)
    }
    pub fn execute(&self, sql: &str, params: impl Params) -> Result<usize> {
        let mut stmt = self.prepare(sql)?;
        stmt.bind(params)?;
        while stmt.inner.step()? == Step::Row {}
        usize::try_from(self.db.changes()?)
            .map_err(|_| Error::InvalidValue("invalid change count".into()))
    }
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        Ok(self.db.exec(sql)?)
    }
    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        Ok(Statement {
            inner: self.db.prepare(sql)?,
            _connection: PhantomData,
        })
    }
    pub fn query_row<T>(
        &self,
        sql: &str,
        params: impl Params,
        map: impl FnOnce(&Row) -> Result<T>,
    ) -> Result<T> {
        self.prepare(sql)?.query_row(params, map)
    }
    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.unchecked_transaction()
    }
    pub fn unchecked_transaction(&self) -> Result<Transaction<'_>> {
        self.db.begin()?;
        Ok(Transaction {
            connection: self,
            finished: false,
        })
    }
    pub fn pragma_update(
        &self,
        database: Option<&str>,
        key: &str,
        value: impl ToSql,
    ) -> Result<()> {
        if database.is_some()
            || !matches!(
                key,
                "foreign_keys" | "busy_timeout" | "journal_mode" | "synchronous"
            )
        {
            return Err(Error::InvalidValue("unsupported pragma".into()));
        }
        let value = match value.value()? {
            Value::Integer(n) => n.to_string(),
            Value::Text(s) if s.bytes().all(|c| c.is_ascii_alphanumeric()) => s,
            _ => return Err(Error::InvalidValue("invalid pragma value".into())),
        };
        Ok(self.db.exec(&format!("PRAGMA {key}={value}"))?)
    }
}
pub struct Transaction<'a> {
    connection: &'a Connection,
    finished: bool,
}
impl std::ops::Deref for Transaction<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection
    }
}
impl Transaction<'_> {
    pub fn commit(mut self) -> Result<()> {
        self.connection.db.commit()?;
        self.finished = true;
        Ok(())
    }
}
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.connection.db.rollback();
        }
    }
}

pub struct Statement<'a> {
    inner: SharedStatement,
    _connection: PhantomData<&'a Connection>,
}
impl<'db> Statement<'db> {
    fn bind(&mut self, params: impl Params) -> Result<()> {
        self.inner.reset()?;
        self.inner.clear_bindings()?;
        let values = params.values()?;
        if self.inner.parameter_count()? != values.len() {
            return Err(Error::InvalidValue("parameter count mismatch".into()));
        }
        for (index, value) in values.iter().enumerate() {
            let i = index + 1;
            match value {
                Value::Null => self.inner.bind_null(i)?,
                Value::Integer(v) => self.inner.bind_i64(i, *v)?,
                Value::Real(v) => self.inner.bind_f64(i, *v)?,
                Value::Text(v) => self.inner.bind_text(i, v)?,
                Value::Blob(v) => self.inner.bind_blob(i, v)?,
            }
        }
        Ok(())
    }
    pub fn query_row<T>(
        &mut self,
        params: impl Params,
        map: impl FnOnce(&Row) -> Result<T>,
    ) -> Result<T> {
        let mut rows = self.query(params)?;
        let row = rows.next()?.ok_or(Error::NoRows)?;
        map(&row)
    }
    fn next_row(&mut self) -> Result<Option<Row>> {
        if self.inner.step()? == Step::Done {
            return Ok(None);
        }
        let mut values = Vec::new();
        for i in 0..self.inner.column_count()? {
            values.push(match self.inner.column_type(i)? {
                ColumnType::Null => Value::Null,
                ColumnType::Integer => Value::Integer(self.inner.column_i64(i)?),
                ColumnType::Float => Value::Real(self.inner.column_f64(i)?),
                ColumnType::Text => Value::Text(
                    self.inner
                        .column_text(i)?
                        .ok_or_else(|| Error::InvalidValue("null text".into()))?,
                ),
                ColumnType::Blob => Value::Blob(
                    self.inner
                        .column_blob(i)?
                        .ok_or_else(|| Error::InvalidValue("null blob".into()))?,
                ),
            });
        }
        Ok(Some(Row(values)))
    }
    pub fn query(&mut self, params: impl Params) -> Result<Rows<'_, 'db>> {
        self.bind(params)?;
        Ok(Rows {
            stmt: self,
            done: false,
        })
    }
    pub fn query_map<T, F: FnMut(&Row) -> Result<T>>(
        &mut self,
        params: impl Params,
        map: F,
    ) -> Result<MappedRows<'_, 'db, F>> {
        Ok(MappedRows {
            rows: self.query(params)?,
            map,
        })
    }
}
pub struct Rows<'a, 'db> {
    stmt: &'a mut Statement<'db>,
    done: bool,
}
pub struct MappedRows<'a, 'db, F> {
    rows: Rows<'a, 'db>,
    map: F,
}
impl Drop for Rows<'_, '_> {
    fn drop(&mut self) {
        // Release the implicit read transaction even when iteration stops early
        // or the mapping callback unwinds. The statement remains reusable.
        let _ = self.stmt.inner.reset();
    }
}
impl<T, F: FnMut(&Row) -> Result<T>> Iterator for MappedRows<'_, '_, F> {
    type Item = Result<T>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.rows.next() {
            Ok(Some(row)) => Some((self.map)(&row)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}
impl Rows<'_, '_> {
    // Fallible cursor API intentionally matches existing call sites; unlike
    // Iterator, an error is separate from the end-of-results sentinel.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Row>> {
        if self.done {
            return Ok(None);
        }
        let result = self.stmt.next_row();
        if !matches!(&result, Ok(Some(_))) {
            self.done = true;
        }
        result
    }
}
pub struct Row(Vec<Value>);
pub trait FromSql: Sized {
    fn from_sql(value: &Value) -> Result<Self>;
}
impl Row {
    pub fn get<I: TryInto<usize>, T: FromSql>(&self, index: I) -> Result<T> {
        let index = index
            .try_into()
            .map_err(|_| Error::InvalidValue("invalid column index".into()))?;
        T::from_sql(
            self.0
                .get(index)
                .ok_or_else(|| Error::InvalidValue("column out of range".into()))?,
        )
    }
}
impl FromSql for String {
    fn from_sql(v: &Value) -> Result<Self> {
        if let Value::Text(s) = v {
            Ok(s.clone())
        } else {
            Err(Error::InvalidValue("expected text".into()))
        }
    }
}
impl FromSql for Vec<u8> {
    fn from_sql(v: &Value) -> Result<Self> {
        if let Value::Blob(s) = v {
            Ok(s.clone())
        } else {
            Err(Error::InvalidValue("expected blob".into()))
        }
    }
}
impl FromSql for i64 {
    fn from_sql(v: &Value) -> Result<Self> {
        if let Value::Integer(n) = v {
            Ok(*n)
        } else {
            Err(Error::InvalidValue("expected integer".into()))
        }
    }
}
impl FromSql for bool {
    fn from_sql(v: &Value) -> Result<Self> {
        Ok(i64::from_sql(v)? != 0)
    }
}
impl<T: FromSql> FromSql for Option<T> {
    fn from_sql(v: &Value) -> Result<Self> {
        if matches!(v, Value::Null) {
            Ok(None)
        } else {
            T::from_sql(v).map(Some)
        }
    }
}

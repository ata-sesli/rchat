use std::fs;
use crate::{
    clipboard::copy_text,
    crypto::{decrypt_with_key, encrypt_with_key},
    error::DatabaseError,
    vault::VaultEntry,
};
use argon2::{password_hash::SaltString, Argon2};
use rusqlite::{Connection,params};
use directories::ProjectDirs;
use rand::rng;

const CURRENT_DB_PATH: &str = "RVAULT_CURRENT_DB_PATH";
const CURRENT_VAULT_NAME: &str = "RVAULT_CURRENT_VAULT_NAME";

pub struct Database {
    connection: Connection
}
impl Database {
    pub fn new() -> Result<Self,DatabaseError>{
        if let Some(project_dirs) = ProjectDirs::from("io.github","ata-sesli","RVault"){
            let project_dirs = project_dirs.data_dir();
            let database_dir = project_dirs.join("databases");
            let _ = std::fs::create_dir_all(&database_dir)?;
            let final_path = database_dir.join("default_vault.sqlite");
            let db_exists = final_path.exists();
            let connection = Connection::open(&final_path)?;
            Ok(Self { connection })
        }
        else {
            Err(DatabaseError::Path)
        }
    }
}

pub struct Table {
    table_name: String
}
impl Table {
    pub fn new(db: &Database,table_name: Option<String>) -> Result<Self,DatabaseError>{
        let connection = &db.connection;
        let full_table_name = match table_name {
            Some(name) => {
                if Self::is_valid_identifier(&name){
                    return Err(DatabaseError::Sqlite(rusqlite::Error::InvalidParameterName(name)));
                } else {
                    name
                }
            },
            None => String::from("main")
        };
        let query = format!(
            "CREATE TABLE IF NOT EXISTS {} (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                platform TEXT NOT NULL,
                user_id TEXT NOT NULL,
                password TEXT NOT NULL,
                nonce TEXT,
                salt TEXT,
                UNIQUE(platform, user_id)
                )",
            full_table_name
        );
        match connection.execute(&query,[]){
            Ok(_) => {
                Ok(Self {
                    table_name: full_table_name,
                })
            },
            Err(e) => {
                eprintln!("Update failed: {e}");
                Err(DatabaseError::Sqlite(rusqlite::Error::InvalidParameterName(full_table_name)))
            }
        }
    }
    pub fn add_entry(&self,db: &Database, platform: String,id_and_password: String){
        let (user_id,password) = id_and_password.split_once(':').unwrap();
        let query = format!(
            "INSERT INTO {} (platform,user_id,password)
             VALUES (?1,?2,?3)
            ", &self.table_name
        );
        let _ = db.connection.execute(&query, [platform.to_string(),user_id.to_string(),password.to_string()]);
    }
    /// Add an entry encrypted with the provided master password.
    /// Ciphertext is stored in the `password` column; nonce and salt are stored alongside.

    pub fn remove_entry(&self,db: &Database, platform: String, user_id: String){
        let query = format!(
            "DELETE FROM {}
             WHERE platform = (?1) AND user_id = (?2)
            ",&self.table_name
        );
        let _ = db.connection.execute(&query, [platform.to_string(),user_id.to_string()]);
    }
    pub fn get_password(&self,db: &Database, platform: String, user_id: String) -> Result<(),DatabaseError>{
        // Legacy/plaintext path: keep behavior for existing rows
        let query: String = format!(
            "SELECT password FROM {}
             WHERE platform = (?1) AND user_id = (?2)",
            &self.table_name
        );
        let password_result = db.connection.query_row(&query, [platform.to_string(),user_id.to_string()],
        |row| row.get::<_,String>(0));
        match password_result {
            Ok(password) => {
                let _ = copy_text(password);
                Ok(())
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                eprintln!("Error: No entry found for user '{}' on platform '{}'.", user_id, platform);
                Err(DatabaseError::from(rusqlite::Error::QueryReturnedNoRows))
            }
            Err(e) => {
                eprintln!("Database query failed: {e}");
                Err(DatabaseError::from(e))
            }
        }
    }
   /// Adds an entry using the main Encryption Key to derive a unique key for this entry.
    pub fn add_entry_with_key(&self, db: &Database, encryption_key: &[u8], platform: String, id_and_password: String) {
        let (user_id, password) = id_and_password.split_once(':').unwrap();

        // 1. Generate a new, unique salt for this specific entry.
        let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);

        // 2. Derive a unique key for this entry from the main EK and the new salt.
        let mut entry_key = [0u8; 32];
        Argon2::default().hash_password_into(encryption_key, salt.as_ref().as_bytes(), &mut entry_key).unwrap();

        // 3. Encrypt the data with the derived per-entry key.
        let (ciphertext, nonce) = encrypt_with_key(&entry_key, password.as_bytes()).unwrap();

        let query = format!(
            "INSERT INTO {} (platform, user_id, password, nonce, salt)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(platform, user_id) DO UPDATE SET
             password = ?3,
             nonce = ?4,
             salt = ?5;",
             &self.table_name
        );
        let _ = db.connection.execute(&query, params![platform, user_id.to_string(), ciphertext, nonce, salt.to_string()]);
    }

    /// Retrieves the decrypted password for an entry.
    /// Returns the plaintext password if successful.
    pub fn retrieve_password_with_key(&self, db: &Database, encryption_key: &[u8], platform: String, user_id: String) -> Result<String, DatabaseError> {
        let query = format!("SELECT password, nonce, salt FROM {} WHERE platform = (?1) AND user_id = (?2)", &self.table_name);

        let row = db.connection.query_row(&query, [platform.to_string(), user_id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        });

        match row {
            Ok((ciphertext, nonce, salt_str)) => {
                // 1. Re-derive the exact same per-entry key using the fetched salt.
                let salt = salt_str.as_bytes();
                let mut entry_key = [0u8; 32];
                Argon2::default().hash_password_into(encryption_key, salt, &mut entry_key).unwrap();

                // 2. Decrypt with the derived key.
                match decrypt_with_key(&entry_key, &ciphertext, &nonce) {
                    Ok(plaintext) => Ok(plaintext),
                    Err(e) => {
                        eprintln!("Decryption failed: {}", e);
                        Err(DatabaseError::from(rusqlite::Error::InvalidQuery))
                    }
                }
            }
            Err(e) => {
                eprintln!("Database query failed: {}", e);
                Err(DatabaseError::from(e))
            }
        }
    }

    /// Retrieves an entry by re-deriving its unique key from the main Encryption Key and the entry's salt.
    /// Copies the password to clipboard and prints success message.
    pub fn get_password_with_key(&self, db: &Database, encryption_key: &[u8], platform: String, user_id: String) -> Result<(), DatabaseError> {
        match self.retrieve_password_with_key(db, encryption_key, platform, user_id) {
            Ok(plaintext) => {
                copy_text(plaintext);
                Ok(())
            },
            Err(e) => Err(e)
        }
    }
    pub fn list(&self, db: &Database) -> Result<Vec<VaultEntry>, DatabaseError> {
        let query = format!(
            "SELECT platform, user_id, password, salt, nonce FROM {}",
            &self.table_name
        );
        let mut statement = db.connection.prepare(&query)?;
        let rows = statement.query_map([], |row| {
            Ok(VaultEntry {
                platform: row.get(0)?,
                user_id: row.get(1)?,
                password: row.get(2)?,
                salt: row.get(3)?,
                nonce: row.get(4)?,
            })
        })?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
    fn is_valid_identifier(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}
}
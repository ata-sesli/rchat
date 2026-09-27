use std::fs;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::error::ConfigError;


#[derive(Serialize,Deserialize,Debug)]
pub struct Config {
    pub version: String,
    pub master_password_hash: Option<String>,
    pub last_used_vault: String,
    pub last_used_database: String,
    pub session_timeout: String

}
impl Default for Config {
    fn default() -> Self {
        Config {
            version: String::from("0.0.2"),
            master_password_hash: None,
            last_used_database: String::from("default.sqlite"),
            last_used_vault: String::from("main"),
            session_timeout: String::from("60")
        }
    }
}
impl Config {
    pub fn new() -> Result<Self,ConfigError>{
        if let Some(project_dirs) = ProjectDirs::from("io.github","ata-sesli","RVault"){
            let config_dir = project_dirs.config_dir();
            let _ = fs::create_dir_all(config_dir);
            let config_file_path = config_dir.join("config.json");
            if config_file_path.exists(){
                // println!("Config file found. Loading...");
                let config_str = fs::read_to_string(config_file_path)?;
                let config: Config = serde_json::from_str(&config_str)?;
                Ok(config)
            }
            else {
                println!("No config file found. Creating a default one...");
                let config = Config::default();
                let _ = config.save_config();
                Ok(config)
            }
        }
        else {
            Err(ConfigError::Path)
        }
    }
    pub fn save_config(&self) -> Result<(),ConfigError> {
        if let Some(project_dirs) = ProjectDirs::from("io.github","ata-sesli","RVault"){
            let config_dir = project_dirs.config_dir();
            let _ = fs::create_dir_all(config_dir);
            let config_file_path = config_dir.join("config.json");
            let json_string = serde_json::to_string(&self)?;
            fs::write(config_file_path, json_string)?;
            Ok(())
        }
        else {
            Err(ConfigError::Path)
        }
    }
}
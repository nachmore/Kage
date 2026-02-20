use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub hotkey: HotkeyConfig,
    pub acp: AcpConfig,
    pub ui: UiConfig,
    pub system: SystemConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyConfig {
    pub modifiers: Vec<String>,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpConfig {
    pub mode: AcpMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AcpMode {
    Local {
        spawn_command: String,
    },
    Remote {
        host: String,
        port: u16,
        timeout_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    pub theme: String,
    pub floating_window_opacity: f32,
    pub chat_window_width: u32,
    pub chat_window_height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemConfig {
    pub auto_start: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            hotkey: HotkeyConfig {
                modifiers: vec!["Alt".to_string()],
                key: "Space".to_string(),
            },
            acp: AcpConfig {
                mode: AcpMode::Remote {
                    host: "127.0.0.1".to_string(),
                    port: 8765,
                    timeout_ms: 30000,
                },
            },
            ui: UiConfig {
                theme: "dark".to_string(),
                floating_window_opacity: 1.0,
                chat_window_width: 800,
                chat_window_height: 600,
            },
            system: SystemConfig {
                auto_start: false,
            },
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;
        
        if !config_path.exists() {
            let config = Self::default();
            config.save()?;
            return Ok(config);
        }
        
        let content = fs::read_to_string(&config_path)
            .context("Failed to read config file")?;
        
        let config: Config = serde_json::from_str(&content)
            .context("Failed to parse config file")?;
        
        Ok(config)
    }
    
    pub fn save(&self) -> Result<()> {
        let config_path = Self::get_config_path()?;
        
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)
                .context("Failed to create config directory")?;
        }
        
        let content = serde_json::to_string_pretty(self)
            .context("Failed to serialize config")?;
        
        fs::write(&config_path, content)
            .context("Failed to write config file")?;
        
        Ok(())
    }
    
    pub fn get_config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Failed to get config directory")?;
        
        Ok(config_dir.join("kiro-assistant").join("config.json"))
    }
    
    pub fn get_hotkey_string(&self) -> String {
        let mut parts = self.hotkey.modifiers.clone();
        parts.push(self.hotkey.key.clone());
        parts.join("+")
    }
}

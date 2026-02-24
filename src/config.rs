use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub processor: ProcessorConfig,
    #[serde(default)]
    pub telnet: TelnetConfig,
    #[serde(default)]
    pub zones: Vec<ZoneMapping>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessorConfig {
    pub host: String,
    #[serde(default = "default_leap_port")]
    pub leap_port: u16,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TelnetConfig {
    #[serde(default = "default_telnet_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ZoneMapping {
    pub ra2_id: u32,
    pub leap_href: String,
    #[serde(default)]
    pub name: String,
}

fn default_leap_port() -> u16 {
    8081
}

fn default_telnet_port() -> u16 {
    6023
}

impl Default for TelnetConfig {
    fn default() -> Self {
        Self {
            port: default_telnet_port(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }
}

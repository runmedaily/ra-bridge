use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub processor: ProcessorConfig,
    #[serde(default)]
    pub telnet: TelnetConfig,
    #[serde(default)]
    pub web: WebConfig,
    #[serde(default)]
    pub zones: Vec<ZoneMapping>,
    #[serde(default)]
    pub savant: Option<SavantConfig>,
    #[serde(default)]
    pub savant_zones: Vec<SavantZoneMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessorConfig {
    pub host: String,
    #[serde(default = "default_leap_port")]
    pub leap_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelnetConfig {
    #[serde(default = "default_telnet_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_web_port")]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavantConfig {
    pub host: String,
    #[serde(default = "default_savant_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavantZoneMapping {
    pub ra2_id: u32,
    pub address: String,
    pub load_offset: usize,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub room: String,
}

fn default_leap_port() -> u16 {
    8081
}

fn default_telnet_port() -> u16 {
    6023
}

fn default_web_port() -> u16 {
    8080
}

fn default_savant_port() -> u16 {
    8480
}

impl Default for TelnetConfig {
    fn default() -> Self {
        Self {
            port: default_telnet_port(),
        }
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            port: default_web_port(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let toml_str = toml::to_string_pretty(self)?;
        std::fs::write(path, &toml_str)?;
        Ok(())
    }

    pub fn has_leap(&self) -> bool {
        !self.zones.is_empty()
    }

    pub fn has_savant(&self) -> bool {
        self.savant.is_some() && !self.savant_zones.is_empty()
    }

    /// Check for duplicate ra2_ids across both zone lists.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = HashSet::new();
        for z in &self.zones {
            if !seen.insert(z.ra2_id) {
                return Err(format!("Duplicate ra2_id {} in LEAP zones", z.ra2_id));
            }
        }
        for z in &self.savant_zones {
            if !seen.insert(z.ra2_id) {
                return Err(format!(
                    "Duplicate ra2_id {} (Savant zone '{}' conflicts with existing zone)",
                    z.ra2_id, z.name
                ));
            }
        }
        Ok(())
    }
}

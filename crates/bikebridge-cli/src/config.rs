use anyhow::{Context, Result, bail};
use bikebridge_core::SafetyLimits;
use serde::Deserialize;
use std::{
    net::{IpAddr, Ipv4Addr},
    path::Path,
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Config {
    pub server: Server,
    pub trainer: SafetyLimits,
    pub logging: Logging,
    pub bluetooth: Bluetooth,
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Bluetooth {
    pub auto_scan: bool,
    pub adapter_index: Option<usize>,
    pub device_names: Vec<String>,
}
impl Default for Bluetooth {
    fn default() -> Self {
        Self {
            auto_scan: true,
            adapter_index: None,
            device_names: Vec::new(),
        }
    }
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Server {
    pub host: IpAddr,
    pub port: u16,
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Logging {
    pub level: String,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 9376,
        }
    }
}
impl Default for Logging {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}
impl Config {
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let path = explicit.unwrap_or_else(|| Path::new("bikebridge.toml"));
        if explicit.is_none() && !path.try_exists()? {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("Invalid config: {}", path.display()))
    }
    pub fn validate(&self) -> Result<()> {
        if !self.server.host.is_loopback() {
            bail!("BikeBridge requires a loopback host; remote access is not implemented.");
        }
        if self.server.port == 0 {
            bail!("Port must be between 1 and 65535.");
        }
        self.trainer.validate()?;
        for name in &self.bluetooth.device_names {
            if name.trim().is_empty()
                || name.chars().count() > 128
                || name.chars().any(char::is_control)
            {
                bail!(
                    "Bluetooth device names must contain 1–128 characters without control characters."
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_defaults_overrides_and_validation() {
        let c: Config = toml::from_str("[trainer]\nmax_erg_watts = 500\n[server]\nport = 9380")
            .expect("config");
        c.validate().expect("valid");
        assert_eq!(c.trainer.max_erg_watts, 500);
        assert_eq!(c.server.port, 9380);
        assert_eq!(c.server.host, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(!c.trainer.auto_reconnect);
        let reconnect: Config =
            toml::from_str("[trainer]\nauto_reconnect = true").expect("reconnection config");
        assert!(reconnect.trainer.auto_reconnect);
        assert!(toml::from_str::<Config>("[trainer]\nmax_erg_watt = 500").is_err());
        let c: Config = toml::from_str("[server]\nhost = '0.0.0.0'").expect("parse");
        assert!(c.validate().is_err());
    }
    #[test]
    fn bluetooth_config_defaults_and_adapter_override() {
        let default = Config::default();
        assert!(default.bluetooth.auto_scan);
        assert!(default.bluetooth.adapter_index.is_none());
        let config: Config =
            toml::from_str("[bluetooth]\nauto_scan = false\nadapter_index = 2").expect("config");
        assert!(!config.bluetooth.auto_scan);
        assert_eq!(config.bluetooth.adapter_index, Some(2));
        assert!(toml::from_str::<Config>("[bluetooth]\nadapter_index = -1").is_err());
    }
}

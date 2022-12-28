use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use url::Url;

use anyhow::Context as _;

use crate::nix;

pub fn get() -> Config {
    tracing::info!("Reading config from env");

    let config = (|| -> anyhow::Result<Config> {
        let config_path = std::env::var("NICACHER_CONFIG")?;
        let config_str = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Unable to read config from {config_path:?}"))?;

        Ok(toml::from_str::<Config>(&config_str)?)
    })()
    .unwrap_or_else(|e| {
        tracing::warn!("Unable to read config from env: {e}");
        tracing::info!("Using default config");
        Config::default()
    });

    tracing::trace!("Using config: {config:?}");

    config
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub upstreams: Vec<nix::Upstream>,

    pub channel_url: Url,
    pub channels: Vec<nix::Channel>,

    pub local_data_path: PathBuf,
    pub database_max_connections: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            upstreams: vec![nix::Upstream {
                url: Url::parse("https://cache.nixos.org/").unwrap(),
                prioriy: nix::Priority::default(),
            }],
            channel_url: Url::parse("https://channels.nixos.org/").unwrap(),
            channels: vec![nix::Channel::NixpkgsUnstable()],
            local_data_path: ".".into(),
            database_max_connections: 20,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        println!("{:#?}", Config::default())
    }
}

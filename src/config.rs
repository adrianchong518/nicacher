use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::nix::Channel;

pub fn get() -> Config {
    tracing::info!("Reading config from env");

    let config = (|| -> anyhow::Result<Config> {
        use anyhow::Context as _;

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
    pub upstreams: Vec<Upstream>,

    pub channel_url: Url,
    pub channels: Vec<Channel>,

    pub local_data_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            upstreams: vec![Upstream {
                url: Url::parse("https://cache.nixos.org/").unwrap(),
                prioriy: Priority::default(),
            }],
            channel_url: Url::parse("https://channels.nixos.org/").unwrap(),
            channels: vec![Channel::NixpkgsUnstable()],
            local_data_path: ".".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Upstream {
    pub url: Url,
    #[serde(default)]
    pub prioriy: Priority,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Priority(u32);

impl Default for Priority {
    fn default() -> Self {
        Self(40)
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

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
        tracing::warn!("Using default config");
        Config::default()
    });

    tracing::trace!("Using config: {config:?}");

    config
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    upstreams: Vec<Upstream>,

    channel_url: Url,
    channels: Vec<Channel>,

    local_cache_path: PathBuf,
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
            local_cache_path: ".".into(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Upstream {
    url: Url,
    #[serde(default)]
    prioriy: Priority,
}

#[derive(Debug, Serialize, Deserialize)]
struct Priority(u32);

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

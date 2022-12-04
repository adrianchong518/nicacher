use crate::nar::Hash;
use crate::nar::NarInfo;
use std::io::Read;
use std::str::FromStr;

use anyhow::{Context, Result};

use crate::nar::StorePath;

fn decode_xz_to_string(bytes: &[u8]) -> Result<String> {
    let mut content = String::new();
    xz2::read::XzDecoder::new(bytes)
        .read_to_string(&mut content)
        .context("Failed to decode bytes as ascii string")?;

    Ok(content)
}

pub async fn get_store_paths(channel: &str) -> Result<Vec<StorePath>> {
    // https://channels.nixos.org/nixpkgs-unstable/store-paths.xz
    let store_paths_url = format!("https://channels.nixos.org/{channel}/store-paths.xz");
    let res = reqwest::get(&store_paths_url)
        .await?
        .error_for_status()
        .with_context(|| format!("Failed to get store paths from {channel}"))?;

    decode_xz_to_string(&res.bytes().await?)?
        .trim()
        .split('\n')
        .map(StorePath::from_str)
        .collect::<Result<Vec<_>, _>>()
        .map_err(anyhow::Error::from)
}

pub async fn get_nar_info_raw(hash: &Hash) -> Result<String> {
    let nar_info_url = format!("https://cache.nixos.org/{hash}.narinfo");

    reqwest::get(&nar_info_url)
        .await?
        .error_for_status()
        .with_context(|| format!("Failed to get narinfo for hash: {hash}"))?
        .text()
        .await
        .map_err(anyhow::Error::from)
}

pub async fn get_nar_info(hash: &Hash) -> Result<NarInfo> {
    NarInfo::from_str(&get_nar_info_raw(hash).await?)
        .with_context(|| format!("Failed to parse narinfo when fetching {hash}"))
}

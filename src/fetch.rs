use std::{
    io::{self, Read},
    path::Path,
    str::FromStr,
};

use anyhow::{Context, Result};
use reqwest::Response;
use tokio::{fs::File, io::AsyncWriteExt};

use crate::nix::{Hash, NarInfo, StorePath};

pub async fn get_store_paths(channel: &str) -> Result<Vec<StorePath>> {
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

pub async fn get_nar_info_raw(hash: &Hash) -> Result<Response> {
    let nar_info_url = format!("https://cache.nixos.org/{hash}.narinfo");

    reqwest::get(&nar_info_url)
        .await?
        .error_for_status()
        .with_context(|| format!("Failed to get narinfo for hash: {hash}"))
}

pub async fn get_nar_info(hash: &Hash) -> Result<NarInfo> {
    let nar_info_text = &get_nar_info_raw(hash).await?.text().await?;
    NarInfo::from_str(&nar_info_text)
        .with_context(|| format!("Failed to parse narinfo when fetching {hash}"))
}

pub async fn download_nar_info(hash: &Hash, path: impl AsRef<Path>) -> Result<()> {
    let nar_info_bytes = get_nar_info_raw(hash).await?.bytes().await?;
    write_bytes_to_file(nar_info_bytes, &path)
        .await
        .with_context(|| {
            format!(
                "Failed to write narinfo ({hash}) to {}",
                path.as_ref().display()
            )
        })
}

pub async fn get_nar_file_raw(nar_info: &NarInfo) -> Result<Response> {
    let nar_file_url = format!("https://cache.nixos.org/{}", nar_info.url);

    reqwest::get(&nar_file_url)
        .await?
        .error_for_status()
        .with_context(|| format!("Failed to get nar file: {}", nar_info.url))
}

pub async fn download_nar_file(nar_info: &NarInfo, path: impl AsRef<Path>) -> Result<()> {
    let nar_file_bytes = get_nar_file_raw(nar_info).await?.bytes().await?;
    write_bytes_to_file(&nar_file_bytes, &path)
        .await
        .with_context(|| {
            format!(
                "Failed to write narfile ({}) to {}",
                nar_info.url,
                path.as_ref().display()
            )
        })
}

fn decode_xz_to_string(bytes: &[u8]) -> Result<String> {
    let mut content = String::new();
    xz2::read::XzDecoder::new(bytes)
        .read_to_string(&mut content)
        .context("Failed to decode bytes as ascii string")?;

    Ok(content)
}

async fn write_bytes_to_file<B, P>(bytes: B, path: P) -> io::Result<()>
where
    B: AsRef<[u8]>,
    P: AsRef<Path>,
{
    File::create(&path).await?.write_all(bytes.as_ref()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn download_nar_info_test() -> Result<()> {
        download_nar_info(
            &Hash::try_from("0006a1aaikgmpqsn5354wi6hibadiwp4").unwrap(),
            "out/test/0006a1aaikgmpqsn5354wi6hibadiwp4.narinfo",
        )
        .await
    }

    #[tokio::test]
    async fn download_nar_file_test() -> Result<()> {
        let nar_info =
            get_nar_info(&Hash::try_from("0006a1aaikgmpqsn5354wi6hibadiwp4").unwrap()).await?;

        download_nar_file(&nar_info, format!("out/test/{}", nar_info.url)).await
    }
}

use std::io;
use std::path::Path;

use std::str::FromStr as _;

use futures::stream;

use anyhow::Context as _;
use futures::StreamExt as _;

use crate::{cache, config, nix};

const STORE_PATHS_FILE: &str = "store-paths.xz";

#[tracing::instrument(skip(config))]
pub async fn request_store_paths<T>(
    config: &config::Config,
    channel: &nix::Channel,
) -> anyhow::Result<T>
where
    T: std::iter::FromIterator<nix::StorePath>,
{
    tracing::info!("Requesting store paths of {channel}");

    let store_paths_url = config
        .channel_url
        .join(&format!("{channel}/{STORE_PATHS_FILE}"))
        .with_context(|| {
            format!(
                "Failed to build store paths url with {}, {channel} and {STORE_PATHS_FILE}",
                config.channel_url,
            )
        })?;

    tracing::debug!("Fetching newest store paths list from {store_paths_url}");

    let res = reqwest::get(store_paths_url.clone())
        .await?
        .error_for_status()
        .with_context(|| format!("Failed to get store paths from {channel} ({store_paths_url})"))?;

    tracing::debug!("Decoding received {store_paths_url}");

    decode_xz_to_string(&res.bytes().await?)?
        .trim()
        .lines()
        .map(nix::StorePath::from_str)
        .collect::<Result<T, _>>()
        .map_err(anyhow::Error::from)
}

async fn request_nar_info_raw(
    config: &config::Config,
    hash: &nix::Hash,
) -> anyhow::Result<(reqwest::Response, nix::Upstream)> {
    let stream = stream::iter(config.upstreams.iter()).filter_map(|upstream| async {
        let url = upstream
            .url
            .join(&format!("{}.narinfo", hash.string))
            .map_err(|e| {
                tracing::warn!(
                    "Failed to build narinfo url with {} and {}: {e}",
                    upstream.url,
                    hash.string
                );
            })
            .ok()?;

        let res = (|| async { reqwest::get(url.clone()).await?.error_for_status() })()
            .await
            .map_err(|e| {
                tracing::warn!("Failed to request {}.narinfo from {url}: {e}", hash.string,);
            })
            .ok()?;

        Some((res, upstream.clone()))
    });
    futures::pin_mut!(stream);

    stream.next().await.ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to request {}.narinfo from all upstreams",
            hash.string
        )
    })
}

#[tracing::instrument(skip(config))]
pub async fn request_nar_info(
    config: &config::Config,
    hash: &nix::Hash,
) -> anyhow::Result<(nix::NarInfo, nix::Upstream)> {
    tracing::info!("Requesting {}.narinfo from upstream", hash.string);

    let (res, upstream) = request_nar_info_raw(config, hash).await?;
    let nar_info = nix::NarInfo::from_str(&res.text().await?)
        .with_context(|| format!("Failed to parse narinfo when fetching {hash}"))?;

    Ok((nar_info, upstream))
}

async fn request_nar_file_raw(
    upstream: &nix::Upstream,
    url_path: &str,
) -> anyhow::Result<reqwest::Response> {
    let url = upstream.url.join(url_path)?;

    reqwest::get(url.clone())
        .await?
        .error_for_status()
        .with_context(|| format!("Failed to request nar file from {url}"))
}

#[tracing::instrument(skip_all, fields(nar_info_url = nar_info.url.to_string(), upstream_url = upstream.url.to_string()))]
pub async fn download_nar_file(
    config: &config::Config,
    upstream: &nix::Upstream,
    nar_info: &nix::NarInfo,
) -> anyhow::Result<()> {
    tracing::info!("Downloading {} from {}", nar_info.url, upstream.url);

    let nar_file_bytes = request_nar_file_raw(upstream, &nar_info.url)
        .await?
        .bytes()
        .await?;

    let file_path = config
        .local_data_path
        .join(cache::NAR_FILE_DIR)
        .join(nar_info.nar_filename());

    tracing::debug!(
        "Writing contents of {} to {}",
        nar_info.url,
        file_path.display()
    );

    write_bytes_to_file(&nar_file_bytes, &file_path)
        .await
        .with_context(|| {
            format!(
                "Failed to write narfile ({}) to {}",
                nar_info.url,
                file_path.display()
            )
        })
}

fn decode_xz_to_string(bytes: &[u8]) -> anyhow::Result<String> {
    use io::Read as _;

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
    use tokio::io::AsyncWriteExt as _;

    tokio::fs::File::create(&path)
        .await?
        .write_all(bytes.as_ref())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn download_nar_file_test() -> anyhow::Result<()> {
        let config = config::Config {
            local_data_path: "./out/test".into(),
            ..Default::default()
        };

        let (nar_info, upstream) = request_nar_info(
            &config,
            &nix::Hash::try_from("0006a1aaikgmpqsn5354wi6hibadiwp4").unwrap(),
        )
        .await?;

        download_nar_file(&config, &upstream, &nar_info).await
    }
}

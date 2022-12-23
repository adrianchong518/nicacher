use std::path::Path;
use std::str::FromStr as _;
use std::{fmt, io};

use anyhow::Context as _;
use futures::{stream, StreamExt as _};

use crate::{config, nix};

pub async fn request_store_paths(channel: &str) -> anyhow::Result<Vec<nix::StorePath>> {
    let store_paths_url = format!("https://channels.nixos.org/{channel}/store-paths.xz");
    let res = reqwest::get(&store_paths_url)
        .await?
        .error_for_status()
        .with_context(|| format!("Failed to get store paths from {channel}"))?;

    decode_xz_to_string(&res.bytes().await?)?
        .trim()
        .split('\n')
        .map(nix::StorePath::from_str)
        .collect::<Result<Vec<_>, _>>()
        .map_err(anyhow::Error::from)
}

#[tracing::instrument(skip(config))]
pub async fn request_nar_info_raw(
    config: &config::Config,
    hash: &nix::Hash,
) -> anyhow::Result<reqwest::Response> {
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

        (|| async { reqwest::get(url.clone()).await?.error_for_status() })()
            .await
            .map_err(|e| {
                tracing::warn!("Failed to request {}.narinfo from {url}: {e}", hash.string,);
            })
            .ok()
    });
    futures::pin_mut!(stream);

    stream.next().await.ok_or(anyhow::anyhow!(
        "Failed to request {}.narinfo from all upstreams",
        hash.string
    ))
}

pub async fn request_nar_info(
    config: &config::Config,
    hash: &nix::Hash,
) -> anyhow::Result<nix::NarInfo> {
    let nar_info_text = request_nar_info_raw(config, hash).await?.text().await?;
    nix::NarInfo::from_str(&nar_info_text)
        .with_context(|| format!("Failed to parse narinfo when fetching {hash}"))
}

#[tracing::instrument(skip(config))]
pub async fn download_nar_info(
    config: &config::Config,
    hash: &nix::Hash,
    file_path: impl AsRef<Path> + fmt::Debug,
) -> anyhow::Result<()> {
    let nar_info_bytes = request_nar_info_raw(config, hash).await?.bytes().await?;

    tracing::debug!(
        "Writing contents of {}.narinfo to {}",
        hash.string,
        file_path.as_ref().display()
    );

    write_bytes_to_file(nar_info_bytes, &file_path)
        .await
        .with_context(|| {
            format!(
                "Failed to write narinfo ({hash}) to {}",
                file_path.as_ref().display()
            )
        })
}

#[tracing::instrument(skip(config))]
pub async fn request_nar_file_raw(
    config: &config::Config,
    url_path: &str,
) -> anyhow::Result<reqwest::Response> {
    let stream = stream::iter(config.upstreams.iter()).filter_map(|upstream| async {
        let url = upstream
            .url
            .join(url_path)
            .map_err(|e| {
                tracing::warn!(
                    "Failed to build nar file url with {} and {}: {e}",
                    upstream.url,
                    url_path
                );
            })
            .ok()?;

        (|| async { reqwest::get(url.clone()).await?.error_for_status() })()
            .await
            .map_err(|e| {
                tracing::warn!("Failed to request {url_path} from {url}: {e}");
            })
            .ok()
    });
    futures::pin_mut!(stream);

    stream.next().await.ok_or(anyhow::anyhow!(
        "Failed to request {url_path} from all upstreams"
    ))
}

#[tracing::instrument(skip(config))]
pub async fn download_nar_file(
    config: &config::Config,
    url_path: &str,
    file_path: impl AsRef<Path> + fmt::Debug,
) -> anyhow::Result<()> {
    let nar_file_bytes = request_nar_file_raw(config, url_path)
        .await?
        .bytes()
        .await?;

    tracing::debug!(
        "Writing contents of {} to {}",
        url_path,
        file_path.as_ref().display()
    );

    write_bytes_to_file(&nar_file_bytes, &file_path)
        .await
        .with_context(|| {
            format!(
                "Failed to write narfile ({}) to {}",
                url_path,
                file_path.as_ref().display()
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
    async fn download_nar_info_test() -> anyhow::Result<()> {
        download_nar_info(
            &config::Config::default(),
            &nix::Hash::try_from("0006a1aaikgmpqsn5354wi6hibadiwp4").unwrap(),
            "out/test/0006a1aaikgmpqsn5354wi6hibadiwp4.narinfo",
        )
        .await
    }

    #[tokio::test]
    async fn download_nar_file_test() -> anyhow::Result<()> {
        let config = config::Config::default();

        let nar_info = request_nar_info(
            &config,
            &nix::Hash::try_from("0006a1aaikgmpqsn5354wi6hibadiwp4").unwrap(),
        )
        .await?;

        download_nar_file(&config, &nar_info.url, format!("out/test/{}", nar_info.url)).await
    }
}

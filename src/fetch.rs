use std::{collections::HashSet, io, str::FromStr as _};

use anyhow::Context as _;
use futures::{stream, StreamExt as _, TryStreamExt as _};

use crate::{config, nix};

const STORE_PATHS_FILE: &str = "store-paths.xz";

pub async fn request_all_channel_stores(
    config: &config::Config,
) -> anyhow::Result<HashSet<nix::StorePath>> {
    tracing::info!("Requesting the store paths of all configured channels");

    stream::iter(config.channels.iter())
        .then(|channel| request_channel_store::<Vec<_>>(config, channel))
        .try_fold(HashSet::new(), |mut set, paths| async {
            set.extend(paths.into_iter());
            Ok(set)
        })
        .await
}

#[tracing::instrument(skip(config))]
pub async fn request_channel_store<T>(
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

#[tracing::instrument(skip(config))]
pub async fn request_derivation(
    config: &config::Config,
    hash: &nix::Hash,
) -> Option<nix::Derivation> {
    let stream = stream::iter(config.upstreams.iter()).filter_map(|upstream| async {
        (|| async {
            let url = upstream
                .url()
                .join(&format!("{}.narinfo", hash.string))
                .with_context(|| {
                    format!(
                        "Failed to build narinfo url with {} and {}",
                        upstream.url(),
                        hash.string
                    )
                })?;

            let nar_info = {
                let text = (|| async {
                    reqwest::get(url.clone())
                        .await?
                        .error_for_status()?
                        .text()
                        .await
                })()
                .await
                .with_context(|| format!("Failed to request {}.narinfo from {url}", hash.string))?;

                nix::NarInfo::from_str(&text).with_context(|| {
                    format!(
                        "Failed to parse narinfo when fetching {}.narinfo from {url}",
                        hash.string
                    )
                })?
            };

            let info = nar_info.store_path.derivation_info.clone();

            let nar_file = {
                let url = upstream.url().join(&nar_info.url)?;

                let info = nix::NarFileInfo {
                    hash: nar_info.file_hash.clone(),
                    compression: nar_info.compression.clone(),
                };

                let data = (|| async {
                    reqwest::get(url.clone())
                        .await?
                        .error_for_status()?
                        .bytes()
                        .await
                })()
                .await
                .with_context(|| format!("Failed to request nar file from {url}"))?;

                nix::NarFile { info, data }
            };

            Ok::<nix::Derivation, anyhow::Error>(nix::Derivation {
                info,
                nar_info,
                nar_file,
                upstream: upstream.clone().into(),
            })
        })()
        .await
        .map_err(|e| {
            tracing::warn!(
                "Failed to fetch {}.narinfo from {}: {e:#}",
                hash.string,
                upstream.url()
            );
        })
        .ok()
    });

    futures::pin_mut!(stream);

    stream.next().await
}

fn decode_xz_to_string(bytes: &[u8]) -> anyhow::Result<String> {
    use io::Read as _;

    let mut content = String::new();
    xz2::read::XzDecoder::new(bytes)
        .read_to_string(&mut content)
        .context("Failed to decode bytes as ascii string")?;

    Ok(content)
}

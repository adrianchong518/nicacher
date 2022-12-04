use std::{
    ffi::OsStr,
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{bail, Context, Result};
use derive_builder::Builder;
use serde::Deserialize;

const NARINFO_MIME: &'static str = "text/x-nix-narinfo";

#[derive(Debug, Builder)]
#[builder(private, setter(into, strip_option))]
pub struct NarInfo {
    store_path: StorePath,
    url: String,
    compression: CompressionType,
    file_hash: Hash,
    file_size: usize,
    nar_hash: Hash,
    nar_size: usize,
    #[builder(default)]
    deriver: Option<String>,
    #[builder(default)]
    system: Option<String>,
    references: Vec<Derivation>,
    signature: String,
}

impl fmt::Display for NarInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "StorePath: {}\n\
            URL: {}\n\
            Compression: {}\n\
            FileHash: {}\n\
            FileSize: {}\n\
            NarHash: {}\n\
            NarSize: {}\n",
            self.store_path,
            self.url,
            self.compression,
            self.file_hash,
            self.file_size,
            self.nar_hash,
            self.nar_size,
        )?;

        if let Some(ref deriver) = self.deriver {
            writeln!(f, "Deriver: {deriver}")?;
        }

        if let Some(ref system) = self.system {
            writeln!(f, "System: {system}")?;
        }

        write!(f, "References:")?;
        self.references
            .iter()
            .map(|d| write!(f, " {d}"))
            .collect::<fmt::Result>()?;
        write!(f, "\n")?;

        write!(f, "Sig: {}", self.signature)?;

        Ok(())
    }
}

impl NarInfo {
    pub fn from_str(text: &str) -> Result<Self> {
        let mut nar_info_builder = NarInfoBuilder::default();

        for line in text.lines() {
            if let Some((key, value)) = line.split_once(": ") {
                (|| {
                    match key {
                        "StorePath" => nar_info_builder.store_path(value.parse::<StorePath>()?),
                        "URL" => nar_info_builder.url(value),
                        "Compression" => {
                            nar_info_builder.compression(value.parse::<CompressionType>()?)
                        }
                        "FileHash" => nar_info_builder.file_hash(value),
                        "FileSize" => nar_info_builder.file_size(value.parse::<usize>()?),
                        "NarHash" => nar_info_builder.nar_hash(value),
                        "NarSize" => nar_info_builder.nar_size(value.parse::<usize>()?),
                        "Deriver" => nar_info_builder.deriver(value),
                        "System" => nar_info_builder.system(value),
                        "References" => nar_info_builder.references(
                            value
                                .split(' ')
                                .map(Derivation::from_str)
                                .collect::<Result<Vec<_>, _>>()
                                .context("Failed to parse references")?,
                        ),
                        "Sig" => nar_info_builder.signature(value),
                        _ => bail!("Unknown narinfo entry: {line}"),
                    };
                    Ok(())
                })()
                .with_context(|| format!("Parsing narinfo line: {line}"))?;
            } else {
                bail!("Invalid entry format in narinfo: {line}")
            }
        }

        nar_info_builder.build().map_err(anyhow::Error::from)
    }
}

#[derive(Debug, Deserialize)]
pub struct NarFile {
    hash: Hash,
    compression: CompressionType,
}

#[derive(Clone, Debug)]
pub struct Derivation {
    pub package: String,
    pub hash: Hash,
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid derivation: {0}")]
pub struct InvalidDerivationError(String);

impl FromStr for Derivation {
    type Err = InvalidDerivationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (hash, package) = s.split_once('-').ok_or(InvalidDerivationError(s.into()))?;
        Ok(Self {
            package: package.into(),
            hash: hash.into(),
        })
    }
}

impl fmt::Display for Derivation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.hash, self.package)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Hash(String);

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for Hash {
    fn from(s: &str) -> Self {
        Self(String::from(s))
    }
}

#[derive(Clone, Debug)]
pub struct StorePath {
    pub path: PathBuf,
    pub derivation: Derivation,
}

#[derive(Debug, thiserror::Error)]
pub enum StorePathParseError {
    #[error("Invalid Store Path: {0}")]
    InvalidPath(PathBuf),
    #[error("Invalid Derivation: {0}")]
    InvalidDerivation(InvalidDerivationError),
}

impl TryFrom<&Path> for StorePath {
    type Error = StorePathParseError;

    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        let derivation = path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or(Self::Error::InvalidPath(path.to_owned()))?
            .parse()
            .map_err(Self::Error::InvalidDerivation)?;

        Ok(StorePath {
            path: path.to_owned(),
            derivation,
        })
    }
}

impl FromStr for StorePath {
    type Err = StorePathParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.as_ref())
    }
}

impl fmt::Display for StorePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.path.display())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressionType {
    Xz,
}

#[derive(Debug, thiserror::Error)]
#[error("Unsupported compression type: {0}")]
pub struct CompressionTypeParseError(String);

impl FromStr for CompressionType {
    type Err = CompressionTypeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "xz" => Self::Xz,
            _ => return Err(CompressionTypeParseError(String::from(s))),
        })
    }
}

impl fmt::Display for CompressionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xz => write!(f, "xz"),
        }
    }
}

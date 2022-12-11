use std::{
    ffi::OsStr,
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, SerializeDisplay};

pub const NARINFO_MIME: &'static str = "text/x-nix-narinfo";

macro_rules! string_newtype_variant {
    ($method_fn:ident, $method_str:expr) => {
        #[allow(non_snake_case)]
        pub fn $method_fn() -> Self {
            Self($method_str.to_owned())
        }
    };
}

#[derive(Debug, Builder)]
#[builder(setter(into, strip_option))]
pub struct NarInfo {
    pub store_path: StorePath,
    pub url: String,
    pub compression: CompressionType,
    pub file_hash: Hash,
    pub file_size: usize,
    pub nar_hash: Hash,
    pub nar_size: usize,
    #[builder(default)]
    pub deriver: Option<String>,
    #[builder(default)]
    pub system: Option<String>,
    pub references: Vec<Derivation>,
    pub signature: String,
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

#[derive(Debug, thiserror::Error)]
pub enum NarInfoParseError {
    #[error("Invalid field value \"{0}\": {1}")]
    InvalidFieldValue(String, String),

    #[error("Missing field: {0}")]
    MissingField(NarInfoBuilderError),

    #[error("Unknown field: \"{0}\"")]
    UnknownField(String),

    #[error("Invalid valid reference: {0}")]
    InvalidReference(DerivationParseError),

    #[error("Invalid entry format: \"{0}\"")]
    InvalidEntryFormat(String),
}

impl FromStr for NarInfo {
    type Err = NarInfoParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut nar_info_builder = NarInfoBuilder::default();

        for line in s.lines() {
            if let Some((key, value)) = line.split_once(": ") {
                match key {
                    "StorePath" => {
                        nar_info_builder.store_path(value.parse::<StorePath>().map_err(|e| {
                            Self::Err::InvalidFieldValue("StorePath".to_owned(), e.to_string())
                        })?)
                    }
                    "URL" => nar_info_builder.url(value),
                    "Compression" => nar_info_builder.compression(
                        value.parse::<CompressionType>().map_err(|e| {
                            Self::Err::InvalidFieldValue("Compression".to_owned(), e.to_string())
                        })?,
                    ),
                    "FileHash" => {
                        nar_info_builder.file_hash(value.parse::<Hash>().map_err(|e| {
                            Self::Err::InvalidFieldValue("FileHash".to_owned(), e.to_string())
                        })?)
                    }
                    "FileSize" => {
                        nar_info_builder.file_size(value.parse::<usize>().map_err(|e| {
                            Self::Err::InvalidFieldValue("FileSize".to_owned(), e.to_string())
                        })?)
                    }
                    "NarHash" => nar_info_builder.nar_hash(value.parse::<Hash>().map_err(|e| {
                        Self::Err::InvalidFieldValue("NarHash".to_owned(), e.to_string())
                    })?),
                    "NarSize" => {
                        nar_info_builder.nar_size(value.parse::<usize>().map_err(|e| {
                            Self::Err::InvalidFieldValue("NarSize".to_owned(), e.to_string())
                        })?)
                    }
                    "Deriver" => nar_info_builder.deriver(value),
                    "System" => nar_info_builder.system(value),
                    "References" => nar_info_builder.references(
                        value
                            .split(' ')
                            .map(Derivation::from_str)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(Self::Err::InvalidReference)?,
                    ),
                    "Sig" => nar_info_builder.signature(value),
                    _ => return Err(Self::Err::UnknownField(line.to_owned())),
                };
            } else {
                return Err(Self::Err::InvalidEntryFormat(line.to_owned()));
            }
        }

        nar_info_builder.build().map_err(Self::Err::MissingField)
    }
}

impl TryFrom<&str> for NarInfo {
    type Error = NarInfoParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[derive(Debug, Deserialize)]
pub struct NarFile {
    pub hash: Hash,
    pub compression: CompressionType,
    pub path: Option<String>,
}

impl fmt::Display for NarFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.nar.{}", self.hash.string, self.compression)
    }
}

#[derive(Clone, Debug)]
pub struct Derivation {
    pub package: String,
    pub hash: Hash,
}

#[derive(Debug, thiserror::Error)]
pub enum DerivationParseError {
    #[error("Invalid derivation name format")]
    InvalidFormat,
    #[error("Missing package name")]
    MissingPackageName,
    #[error("Invalid hash: {0}")]
    InvalidHash(HashParseError),
}

impl FromStr for Derivation {
    type Err = DerivationParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (hash, package) = match s.split_once('-') {
            None => return Err(Self::Err::InvalidFormat),
            Some((_, "")) => return Err(Self::Err::MissingPackageName),
            Some((h, p)) => (h.try_into().map_err(Self::Err::InvalidHash)?, p.into()),
        };

        Ok(Self { package, hash })
    }
}

impl fmt::Display for Derivation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.hash, self.package)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel(String);

impl Channel {
    string_newtype_variant!(NixosUnstable, "nixos-unstable");
    string_newtype_variant!(NixpkgsUnstable, "nixpkgs-unstable");
}

#[derive(Clone, Debug, SerializeDisplay, DeserializeFromStr)]
pub struct Hash {
    method: Option<HashMethod>,
    string: String,
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { method, string } = self;

        if let Some(m) = method {
            write!(f, "{m}:{string}")
        } else {
            write!(f, "{string}")
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HashParseError {
    #[error("Missing hash string")]
    MissingHash,
    #[error("Hash string contains non-alphanumeric characters")]
    HashNonAlphanumeric,
}

impl FromStr for Hash {
    type Err = HashParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (method, string) = match s.split_once(':') {
            None => (None, s),
            Some((_, "")) => return Err(Self::Err::MissingHash),
            Some((m, s)) => (Some(HashMethod::from(m)), s),
        };

        if !string.chars().all(char::is_alphanumeric) {
            return Err(Self::Err::HashNonAlphanumeric);
        }

        Ok(Self {
            method,
            string: string.to_owned(),
        })
    }
}

impl TryFrom<&str> for Hash {
    type Error = HashParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HashMethod(String);

impl HashMethod {
    string_newtype_variant!(Sha256, "sha256");
}

impl fmt::Display for HashMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for HashMethod {
    fn from(s: &str) -> Self {
        HashMethod(s.to_owned())
    }
}

#[derive(Clone, Debug)]
pub struct StorePath {
    pub path: PathBuf,
    pub derivation: Derivation,
}

#[derive(Debug, thiserror::Error)]
pub enum StorePathParseError {
    #[error("Invalid Store Path: {0:?}")]
    InvalidPath(PathBuf),
    #[error("Invalid Derivation: {0:?}")]
    InvalidDerivation(DerivationParseError),
}

impl TryFrom<&Path> for StorePath {
    type Error = StorePathParseError;

    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        let derivation = path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or(StorePathParseError::InvalidPath(path.to_owned()))?
            .parse()
            .map_err(StorePathParseError::InvalidDerivation)?;

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
#[error("Unsupported compression type: {0:?}")]
pub struct CompressionTypeParseError(String);

impl FromStr for CompressionType {
    type Err = CompressionTypeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "xz" => Self::Xz,
            _ => return Err(CompressionTypeParseError(s.to_owned())),
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

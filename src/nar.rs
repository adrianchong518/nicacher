use anyhow::{anyhow, Result};
use serde::Deserialize;

#[derive(Debug, Default)]
pub struct NarInfo {
    store_path: String,
    url: String,
    compression: String,
    file_hash: String,
    file_size: usize,
    nar_hash: String,
    nar_size: usize,
    deriver: Option<String>,
    system: Option<String>,
    references: Vec<String>,
    signature: String,
}

impl std::fmt::Display for NarInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "StorePath: {}\n\
            URL: {}\n\
            Compression: {}\n\
            FileHash: {}\n\
            FileSize: {}\n\
            NarHash: {}\n\
            NarSize: {}\n\
            ",
            self.store_path,
            self.url,
            self.compression,
            self.file_hash,
            self.file_size,
            self.nar_hash,
            self.nar_size,
        )?;

        if let Some(ref deriver) = self.deriver {
            write!(f, "Deriver: {deriver}")?;
        }

        if let Some(ref system) = self.system {
            write!(f, "System: {system}")?;
        }

        let references = self.references.join(" ");
        write!(f, "References: {references}")?;

        write!(f, "Sig: {}", self.signature)?;

        Ok(())
    }
}

impl NarInfo {
    pub fn from_str(text: &str) -> Result<Self> {
        let mut nar_info = Self::default();

        for line in text.lines() {
            if let Some((key, value)) = line.split_once(": ") {
                match key {
                    "StorePath" => nar_info.store_path = value.to_string(),
                    "URL" => nar_info.url = value.to_string(),
                    "Compression" => nar_info.compression = value.to_string(),
                    "FileHash" => nar_info.file_hash = value.to_string(),
                    "FileSize" => nar_info.file_size = value.parse::<usize>()?,
                    "NarHash" => nar_info.nar_hash = value.to_string(),
                    "NarSize" => nar_info.nar_size = value.parse::<usize>()?,
                    "Deriver" => nar_info.deriver = Some(value.to_string()),
                    "System" => nar_info.system = Some(value.to_string()),
                    "References" => {
                        nar_info.references = value.split(' ').map(String::from).collect()
                    }
                    "Sig" => nar_info.signature = value.to_string(),
                    _ => return Err(anyhow!("Unknown .narinfo entry: {line}")),
                }
            }
        }

        Ok(nar_info)
    }
}

#[derive(Debug, Deserialize)]
pub struct NarFile {
    hash: String,
    compression: String,
}

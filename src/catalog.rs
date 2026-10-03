use crate::{Paths, network};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Clone, Debug)]
pub enum Source {
    Local(PathBuf),
    Remote(Url),
}

impl Source {
    pub fn parse(value: &str) -> Result<Self> {
        if value.starts_with("https://") {
            return Ok(Self::Remote(Url::parse(value)?));
        }
        ensure!(!value.contains("://"), "Catalog URLs must use HTTPS");
        Ok(Self::Local(std::path::absolute(value)?))
    }
    pub fn label(&self) -> String {
        match self {
            Self::Local(p) => p.display().to_string(),
            Self::Remote(u) => u.to_string(),
        }
    }
    pub fn resolve(&self, value: &str) -> Result<Self> {
        if value.starts_with("https://") {
            return Self::parse(value);
        }
        ensure!(!value.contains("://"), "Pack URLs must use HTTPS");
        match self {
            Self::Remote(base) => {
                let url = base.join(value)?;
                ensure!(url.scheme() == "https", "Pack URLs must use HTTPS");
                Ok(Self::Remote(url))
            }
            Self::Local(path) => Ok(Self::Local(
                path.parent().unwrap_or(Path::new(".")).join(value),
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub title: String,
    pub packs: Vec<Pack>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub url: String,
    pub sha256: Option<String>,
    #[serde(default)]
    pub minecraft: String,
    #[serde(default)]
    pub loader: String,
    #[serde(default = "default_memory")]
    pub memory_mb: u32,
    pub server: Option<String>,
}
fn default_memory() -> u32 {
    4096
}

impl Pack {
    pub fn instance_id(&self, source: &Source) -> String {
        // Separate versions never overwrite worlds or user settings in older installs.
        let key = format!(
            "{}\n{}\n{}\n{}",
            source.label(),
            self.version,
            self.url,
            self.sha256.as_deref().unwrap_or("")
        );
        format!(
            "neelemanet-{}-{:x}",
            self.id,
            Sha256::digest(key.as_bytes())
        )
    }
}

impl Catalog {
    pub fn parse(text: &str) -> Result<Self> {
        let catalog: Self = toml::from_str(text).context("Invalid packs.toml")?;
        ensure!(
            catalog.schema_version == 1,
            "Unsupported catalog schema version"
        );
        ensure!(
            !catalog.title.trim().is_empty(),
            "Catalog title is required"
        );
        ensure!(!catalog.packs.is_empty(), "Catalog has no packs");
        let mut ids = HashSet::new();
        for p in &catalog.packs {
            ensure!(
                !p.id.is_empty()
                    && p.id.len() <= 64
                    && p.id
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "Pack IDs must contain only lowercase letters, digits, and hyphens"
            );
            ensure!(ids.insert(&p.id), "Duplicate pack ID: {}", p.id);
            ensure!(
                !p.name.trim().is_empty()
                    && !p.version.trim().is_empty()
                    && !p.url.trim().is_empty(),
                "Pack {} needs a name, version, and URL",
                p.id
            );
            ensure!(
                !p.name.contains(['\n', '\r']) && !p.version.contains(['\n', '\r']),
                "Pack name/version cannot contain newlines"
            );
            ensure!(
                (512..=65536).contains(&p.memory_mb),
                "Pack memory must be 512–65536 MiB"
            );
            if let Some(hash) = &p.sha256 {
                ensure!(
                    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
                    "Invalid SHA-256 for {}",
                    p.id
                );
            }
        }
        Ok(catalog)
    }
}

pub fn load(source: &Source, paths: &Paths) -> Result<(Catalog, Option<String>)> {
    let cache = paths.root.join("cache").join(format!(
        "catalog-{:x}.toml",
        Sha256::digest(source.label().as_bytes())
    ));
    match source {
        Source::Local(path) => Ok((
            Catalog::parse(
                &fs::read_to_string(path)
                    .with_context(|| format!("Cannot read {}", path.display()))?,
            )?,
            None,
        )),
        Source::Remote(url) => {
            let fresh = network::get_text(url.as_str(), 1024 * 1024)
                .and_then(|text| Ok((Catalog::parse(&text)?, text)));
            match fresh {
                Ok((catalog, text)) => {
                    let mut temp = tempfile::NamedTempFile::new_in(cache.parent().unwrap())?;
                    use std::io::Write;
                    temp.write_all(text.as_bytes())?;
                    temp.persist(&cache)?;
                    Ok((catalog, None))
                }
                Err(error) => {
                    let cached = fs::read_to_string(cache)
                        .ok()
                        .and_then(|s| Catalog::parse(&s).ok());
                    match cached {
                        Some(catalog) => Ok((catalog, Some(format!("Using saved pack list. Refresh failed: {error}")))),
                        None => Err(error.context("Could not load the pack list. Check your internet connection and catalog URL.")),
                    }
                }
            }
        }
    }
}

pub fn default_source() -> Result<Source> {
    if let Some(url) = option_env!("NEELEMANET_CATALOG_URL") {
        return Source::parse(url);
    }
    let cwd = std::env::current_dir()?.join("packs.toml");
    if cwd.is_file() {
        return Ok(Source::Local(cwd));
    }
    let adjacent = std::env::current_exe()?
        .parent()
        .context("No executable directory")?
        .join("packs.toml");
    if adjacent.is_file() {
        return Ok(Source::Local(adjacent));
    }
    Source::parse(concat!(env!("CARGO_MANIFEST_DIR"), "/packs.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_repo_catalog() {
        assert_eq!(
            Catalog::parse(include_str!("../packs.toml")).unwrap().packs[0].id,
            "duck-craft"
        );
    }
    #[test]
    fn rejects_duplicate_and_unsafe_ids() {
        let input = include_str!("../packs.toml");
        assert!(
            Catalog::parse(&input.replace("id = \"duck-craft\"", "id = \"../escape\"")).is_err()
        );
        assert!(
            Catalog::parse(&format!(
                "{input}\n[[packs]]{}",
                input.split_once("[[packs]]").unwrap().1
            ))
            .is_err()
        );
    }
    #[test]
    fn resolves_relative_downloads() {
        let s = Source::parse("https://example.com/repo/packs.toml").unwrap();
        assert_eq!(
            s.resolve("packs/test.zip").unwrap().label(),
            "https://example.com/repo/packs/test.zip"
        );
        assert!(s.resolve("file:///etc/passwd").is_err());
    }
}

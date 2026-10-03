use crate::{
    Paths,
    catalog::{Pack, Source},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Write};

pub const MIN_MEMORY_MB: u32 = 512;
pub const MAX_MEMORY_MB: u32 = 65536;

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Preferences {
    #[serde(default)]
    pub memory_mb: BTreeMap<String, u32>,
}

impl Preferences {
    pub fn load(paths: &Paths) -> Result<Self> {
        let text = match fs::read(paths.root.join("preferences.json")) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error).context("Could not read launcher preferences"),
        };
        let preferences: Self =
            serde_json::from_slice(&text).context("Invalid launcher preferences")?;
        for &memory in preferences.memory_mb.values() {
            validate_memory(memory)?;
        }
        Ok(preferences)
    }

    pub fn memory_for(&self, pack: &Pack, source: &Source) -> u32 {
        self.memory_mb
            .get(&pack.instance_id(source))
            .copied()
            .unwrap_or(pack.memory_mb)
    }

    /// Caller holds Paths::lock; reload first to preserve other packs' preferences.
    pub fn save_memory(
        paths: &Paths,
        pack: &Pack,
        source: &Source,
        memory: Option<u32>,
    ) -> Result<Self> {
        let mut preferences = Self::load(paths)?;
        let key = pack.instance_id(source);
        if let Some(memory) = memory {
            validate_memory(memory)?;
            preferences.memory_mb.insert(key, memory);
        } else {
            preferences.memory_mb.remove(&key);
        }
        let mut file = tempfile::NamedTempFile::new_in(&paths.root)?;
        file.write_all(&serde_json::to_vec_pretty(&preferences)?)?;
        file.as_file().sync_all()?;
        file.persist(paths.root.join("preferences.json"))
            .context("Could not save launcher preferences")?;
        Ok(preferences)
    }
}

pub fn validate_memory(memory: u32) -> Result<()> {
    ensure!(
        (MIN_MEMORY_MB..=MAX_MEMORY_MB).contains(&memory),
        "RAM must be between 512 MiB and 64 GiB"
    );
    Ok(())
}

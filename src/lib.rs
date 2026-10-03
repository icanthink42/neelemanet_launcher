pub mod archive;
pub mod catalog;
pub mod launcher;
pub mod network;
pub mod preferences;
pub mod prism;

use anyhow::{Context, Result};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(root: Option<PathBuf>) -> Result<Self> {
        let root = match root {
            Some(root) => root,
            None => directories::ProjectDirs::from("net", "neelemanet", "NeelemaNet")
                .context("Could not find your application data directory")?
                .data_local_dir()
                .to_owned(),
        };
        #[cfg(unix)]
        let new_directory = !root.exists();
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Prism stores account tokens beneath this directory.
            if new_directory {
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            }
        }
        let paths = Self {
            root: root.canonicalize()?,
        };
        fs::create_dir_all(paths.instances())?;
        fs::create_dir_all(paths.root.join("cache"))?;
        fs::create_dir_all(paths.root.join("runtime"))?;
        Ok(paths)
    }
    pub fn prism_data(&self) -> PathBuf {
        self.root.join("prism")
    }
    pub fn instances(&self) -> PathBuf {
        self.prism_data().join("instances")
    }
    pub fn lock(&self) -> Result<fs::File> {
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("install.lock"))?;
        fs2::FileExt::try_lock_exclusive(&file)
            .context("Another NeelemaNet operation is running. Please wait and try again.")?;
        Ok(file)
    }
}

pub type Progress<'a> = dyn Fn(String, Option<f32>) + 'a;

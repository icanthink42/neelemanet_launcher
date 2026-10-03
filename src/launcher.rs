use crate::{
    Paths, Progress, archive,
    catalog::{Pack, Source},
    network,
};
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File},
    path::PathBuf,
};

pub fn installed(paths: &Paths, pack: &Pack, source: &Source) -> bool {
    let root = paths.instances().join(pack.instance_id(source));
    root.join(".neelemanet-installed.json").is_file()
        && root.join("instance.cfg").is_file()
        && root.join("mmc-pack.json").is_file()
}

/// Caller holds Paths::lock throughout install/bootstrap to serialize multiple launcher processes.
pub fn install(
    paths: &Paths,
    pack: &Pack,
    source: &Source,
    progress: &Progress<'_>,
) -> Result<PathBuf> {
    let destination = paths.instances().join(pack.instance_id(source));
    if installed(paths, pack, source) {
        return Ok(destination);
    }
    ensure!(
        !destination.exists(),
        "An incomplete instance already exists at {}. Move it aside before retrying.",
        destination.display()
    );
    let staging = tempfile::tempdir_in(paths.instances())?;
    let zip = staging.path().join("pack.zip");
    progress(format!("Getting {}…", pack.name), None);
    match source.resolve(&pack.url)? {
        Source::Remote(url) => {
            network::download(url.as_str(), &zip, pack.sha256.as_deref(), progress)?
        }
        Source::Local(path) => {
            let input = File::open(&path).with_context(|| format!("Cannot open pack at {}. For distribution, replace the local ZIP path in packs.toml with an HTTPS download URL.", path.display()))?;
            let len = input.metadata()?.len();
            network::copy_checked(input, &zip, pack.sha256.as_deref(), Some(len), progress)?;
        }
    }
    let unpacked = staging.path().join("files");
    fs::create_dir(&unpacked)?;
    archive::extract_zip(&zip, &unpacked, progress)?;
    let root = archive::instance_root(&unpacked)?;
    archive::configure_instance(&root, pack)?;
    fs::write(
        root.join(".neelemanet-installed.json"),
        serde_json::to_vec_pretty(pack)?,
    )?;
    fs::rename(&root, &destination).context("Could not finish the pack installation")?;
    progress(format!("{} is ready to play", pack.name), Some(1.0));
    Ok(destination)
}

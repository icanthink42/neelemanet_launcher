use crate::{
    Paths, Progress, archive,
    catalog::{Pack, Source},
    network,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Installation {
    pub pack: Pack,
    pub source: String,
    pub instance: String,
    pub stamp: Option<network::DownloadStamp>,
}

impl Installation {
    pub fn root(&self, paths: &Paths) -> PathBuf {
        paths.instances().join(&self.instance)
    }
    pub fn catalog_changed(&self, pack: &Pack) -> bool {
        self.pack.version != pack.version || self.pack.url != pack.url
    }
}

fn state_path(paths: &Paths, pack: &Pack, source: &Source) -> PathBuf {
    paths
        .root
        .join("installed")
        .join(format!("{}.json", pack.instance_id(source)))
}

/// Read the active revision, or discover an installation made by the older launcher.
pub fn current_install(
    paths: &Paths,
    pack: &Pack,
    source: &Source,
) -> Result<Option<Installation>> {
    let state = state_path(paths, pack, source);
    if state.exists() {
        let record: Installation = serde_json::from_slice(&fs::read(&state)?)
            .with_context(|| format!("Cannot read installed pack record {}", state.display()))?;
        let relative = archive::safe_path(&record.instance)?;
        ensure!(
            relative.components().count() == 1
                && record
                    .instance
                    .starts_with(&format!("neelemanet-{}-", pack.id)),
            "Invalid installed instance path"
        );
        ensure!(
            record.source == source.label() && record.pack.id == pack.id,
            "Installed pack record does not match this catalog"
        );
        ensure!(
            record.root(paths).join("instance.cfg").is_file()
                && record.root(paths).join("mmc-pack.json").is_file(),
            "Installed pack files are missing. Restore the instance folder before updating."
        );
        return Ok(Some(record));
    }
    let mut candidates = Vec::new();
    for entry in fs::read_dir(paths.instances())? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let marker = entry.path().join(".neelemanet-installed.json");
        let Some(old) = fs::read(&marker)
            .ok()
            .and_then(|b| serde_json::from_slice::<Pack>(&b).ok())
        else {
            continue;
        };
        let instance = entry.file_name().to_string_lossy().into_owned();
        if old.id != pack.id || instance != old.legacy_instance_id(source) {
            continue;
        }
        if !entry.path().join("instance.cfg").is_file()
            || !entry.path().join("mmc-pack.json").is_file()
        {
            continue;
        }
        candidates.push((
            fs::metadata(&marker)?.modified()?,
            Installation {
                pack: old,
                source: source.label(),
                instance,
                stamp: None,
            },
        ));
    }
    candidates.sort_by_key(|(modified, _)| *modified);
    Ok(candidates.pop().map(|(_, record)| record))
}

pub fn installed(paths: &Paths, pack: &Pack, source: &Source) -> bool {
    current_install(paths, pack, source)
        .ok()
        .flatten()
        .is_some_and(|i| !i.catalog_changed(pack))
}

/// Caller holds Paths::lock throughout install/bootstrap to serialize multiple launcher processes.
pub fn install(
    paths: &Paths,
    pack: &Pack,
    source: &Source,
    progress: &Progress<'_>,
) -> Result<PathBuf> {
    sync_pack(paths, pack, source, false, progress)
}

/// Force a fresh download even if the server does not provide change metadata.
pub fn update(
    paths: &Paths,
    pack: &Pack,
    source: &Source,
    progress: &Progress<'_>,
) -> Result<PathBuf> {
    sync_pack(paths, pack, source, true, progress)
}

fn source_stamp(source: &Source) -> Result<network::DownloadStamp> {
    match source {
        Source::Remote(url) => network::pack_stamp(url.as_str()),
        Source::Local(path) => local_stamp(&File::open(path)?),
    }
}

fn local_stamp(file: &File) -> Result<network::DownloadStamp> {
    let metadata = file.metadata()?;
    Ok(network::DownloadStamp {
        etag: None,
        last_modified: Some(
            metadata
                .modified()?
                .duration_since(UNIX_EPOCH)?
                .as_nanos()
                .to_string(),
        ),
        length: Some(metadata.len()),
    })
}

fn sync_pack(
    paths: &Paths,
    pack: &Pack,
    source: &Source,
    force: bool,
    progress: &Progress<'_>,
) -> Result<PathBuf> {
    let memory_mb = crate::preferences::Preferences::load(paths)?.memory_for(pack, source);
    let previous = current_install(paths, pack, source)?;
    let download_source = source.resolve(&pack.url)?;
    if let Some(old) = &previous {
        if !force && !old.catalog_changed(pack) {
            progress("Checking for pack updates…".into(), None);
            match source_stamp(&download_source) {
                Ok(stamp) if old.stamp.as_ref().is_some_and(|s| !stamp.differs_from(s)) => {
                    archive::configure_memory(&old.root(paths), memory_mb)?;
                    return Ok(old.root(paths));
                }
                Ok(_) => {}
                Err(error) => {
                    progress(
                        format!("Could not check for updates; using the installed pack: {error}"),
                        None,
                    );
                    archive::configure_memory(&old.root(paths), memory_mb)?;
                    return Ok(old.root(paths));
                }
            }
        }
        ensure_not_running(&old.root(paths))?;
    }
    let staging = tempfile::tempdir_in(paths.instances())?;
    let zip = staging.path().join("pack.zip");
    progress(format!("Getting {}…", pack.name), None);
    let stamp = match download_source {
        Source::Remote(url) => network::download_pack(url.as_str(), &zip, progress)?,
        Source::Local(path) => {
            let input = File::open(&path).with_context(|| format!("Cannot open pack at {}. For distribution, replace the local ZIP path in packs.toml with an HTTPS download URL.", path.display()))?;
            let stamp = local_stamp(&input)?;
            network::copy_checked(input, &zip, None, stamp.length, progress)?;
            stamp
        }
    };
    let unpacked = staging.path().join("files");
    fs::create_dir(&unpacked)?;
    archive::extract_zip(&zip, &unpacked, progress)?;
    let root = archive::instance_root(&unpacked)?;
    let configured_pack = Pack {
        memory_mb,
        ..pack.clone()
    };
    archive::configure_instance(&root, &configured_pack)?;
    if let Some(old) = &previous {
        // Check again after a potentially long download before snapshotting player data.
        ensure_not_running(&old.root(paths))?;
        progress(
            "Bringing your worlds and personal settings into the update…".into(),
            None,
        );
        preserve_player_data(&old.root(paths), &root)?;
    }
    fs::write(
        root.join(".neelemanet-installed.json"),
        serde_json::to_vec_pretty(pack)?,
    )?;
    // Publish into a new revision. The active pointer changes only after every file is ready.
    // Older revisions remain untouched, including if download/extraction/copying fails.
    let reservation = tempfile::Builder::new()
        .prefix(&format!("{}-", pack.instance_id(source)))
        .tempdir_in(paths.instances())?;
    let destination = reservation.path().to_owned();
    fs::remove_dir(&destination)?;
    fs::rename(&root, &destination).context("Could not finish the pack installation")?;
    let record = Installation {
        pack: pack.clone(),
        source: source.label(),
        instance: destination
            .file_name()
            .context("Missing instance name")?
            .to_string_lossy()
            .into_owned(),
        stamp: Some(stamp),
    };
    let state = state_path(paths, pack, source);
    fs::create_dir_all(state.parent().context("Missing state directory")?)?;
    let mut temp = tempfile::NamedTempFile::new_in(state.parent().unwrap())?;
    temp.write_all(&serde_json::to_vec_pretty(&record)?)?;
    temp.as_file().sync_all()?;
    temp.persist(state)
        .context("Could not activate updated pack; the previous install is still selected")?;
    let _ = reservation.keep();
    progress(format!("{} is ready to play", pack.name), Some(1.0));
    Ok(destination)
}

fn game_directory(root: &Path) -> PathBuf {
    if root.join(".minecraft").is_dir() && !root.join("minecraft").exists() {
        root.join(".minecraft")
    } else {
        root.join("minecraft")
    }
}

fn preserve_player_data(old: &Path, new: &Path) -> Result<()> {
    let old = game_directory(old);
    let new = game_directory(new);
    if old.exists() {
        ensure!(
            !fs::symlink_metadata(&old)?.file_type().is_symlink(),
            "The existing game directory is a symlink; the previous install has been kept."
        );
    }
    // Pack-managed mods/configs come exclusively from the new export, so removed mods stay removed.
    for name in [
        "saves",
        "screenshots",
        "options.txt",
        "optionsof.txt",
        "optionsshaders.txt",
        "servers.dat",
        "servers.dat_old",
        "XaeroWaypoints",
        "XaeroWorldMap",
        "journeymap",
    ] {
        let source = old.join(name);
        if !source.try_exists()? {
            continue;
        }
        let destination = new.join(name);
        if destination.is_dir() {
            fs::remove_dir_all(&destination)?;
        } else if destination.exists() {
            fs::remove_file(&destination)?;
        }
        copy_player_file(&source, &destination)?;
    }
    Ok(())
}

fn copy_player_file(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "Player data contains a symlink at {}. The previous install has been kept.",
        source.display()
    );
    if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_player_file(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        ensure!(
            metadata.is_file(),
            "Unsupported player data file: {}",
            source.display()
        );
        fs::create_dir_all(destination.parent().context("Missing player data parent")?)?;
        fs::copy(source, destination)?;
    }
    Ok(())
}

fn ensure_not_running(root: &Path) -> Result<()> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .without_tasks()
            .with_cmd(UpdateKind::Always)
            .with_cwd(UpdateKind::Always),
    );
    for process in system.processes().values() {
        let name = process.name().to_string_lossy().to_lowercase();
        if !name.starts_with("java") {
            continue;
        }
        let uses_instance = process.cwd().is_some_and(|cwd| cwd.starts_with(root))
            || process
                .cmd()
                .iter()
                .any(|arg| Path::new(arg).starts_with(root));
        ensure!(
            !uses_instance,
            "Close Minecraft for this pack before updating. Your current installation has not been changed."
        );
    }
    Ok(())
}

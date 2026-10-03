use crate::{Progress, catalog::Pack};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use zip::ZipArchive;

const MAX_EXPANDED: u64 = 20 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

pub fn safe_path(name: &str) -> Result<PathBuf> {
    // Validate with Windows rules on every platform, too.
    ensure!(
        !name.contains(['\\', ':', '\0']),
        "Unsafe archive path: {name}"
    );
    let path = Path::new(name);
    ensure!(!path.is_absolute(), "Absolute archive path: {name}");
    ensure!(
        path.components().all(|c| matches!(c, Component::Normal(_))),
        "Unsafe archive path: {name}"
    );
    for part in name.trim_end_matches('/').split('/') {
        ensure!(
            !part.is_empty() && !part.ends_with(['.', ' ']),
            "Unsafe archive path: {name}"
        );
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        ensure!(
            !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.as_bytes()[3].is_ascii_digit()),
            "Reserved archive path: {name}"
        );
    }
    Ok(path.to_owned())
}

pub fn extract_zip(archive: &Path, destination: &Path, progress: &Progress<'_>) -> Result<()> {
    let mut zip = ZipArchive::new(File::open(archive)?).context("Not a valid ZIP archive")?;
    ensure!(zip.len() <= MAX_ENTRIES, "Archive has too many entries");
    let mut total = 0_u64;
    let mut seen = HashSet::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        let relative = safe_path(entry.name())?;
        ensure!(!entry.is_symlink(), "Archive contains a symbolic link");
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170000;
            ensure!(
                matches!(kind, 0 | 0o100000 | 0o040000),
                "Unsupported archive entry type"
            );
        }
        ensure!(
            seen.insert(relative.to_string_lossy().to_lowercase()),
            "Duplicate archive path"
        );
        total = total
            .checked_add(entry.size())
            .context("Archive size overflow")?;
        ensure!(total <= MAX_EXPANDED, "Archive expands beyond 20 GiB");
    }
    let mut written = 0_u64;
    let count = zip.len();
    for i in 0..count {
        let mut entry = zip.by_index(i)?;
        let output = destination.join(safe_path(entry.name())?);
        if entry.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        fs::create_dir_all(output.parent().context("Missing parent directory")?)?;
        let mut file = File::options().create_new(true).write(true).open(&output)?;
        let expected = entry.size();
        let bytes = std::io::copy(&mut (&mut entry).take(expected + 1), &mut file)?;
        ensure!(bytes == expected, "Incorrect ZIP entry size");
        written += bytes;
        if i % 10 == 0 {
            progress(
                format!("Installing files · {} / {}", i + 1, count),
                Some(written as f32 / total.max(1) as f32),
            );
        }
    }
    Ok(())
}

pub fn instance_root(directory: &Path) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if directory.join("mmc-pack.json").is_file() {
        candidates.push(directory.to_owned());
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() && path.join("mmc-pack.json").is_file() {
            candidates.push(path);
        }
    }
    ensure!(
        candidates.len() == 1,
        "Expected one Prism instance (mmc-pack.json at ZIP root or inside one folder)"
    );
    let root = candidates.remove(0);
    ensure!(
        root.join("instance.cfg").is_file(),
        "Prism export is missing instance.cfg"
    );
    validate_components(&root)?;
    Ok(root)
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(rename = "formatVersion")]
    format_version: u32,
    components: Vec<PackComponent>,
}
#[derive(Deserialize)]
struct PackComponent {
    uid: String,
    version: Option<String>,
}

pub fn validate_components(root: &Path) -> Result<()> {
    let path = root.join("mmc-pack.json");
    ensure!(
        fs::metadata(&path)?.len() <= 1024 * 1024,
        "Oversized Prism metadata"
    );
    let data: Metadata =
        serde_json::from_slice(&fs::read(path)?).context("Invalid mmc-pack.json")?;
    ensure!(data.format_version == 1, "Unsupported Prism export format");
    ensure!(
        data.components
            .iter()
            .any(|c| c.uid == "net.minecraft" && c.version.as_ref().is_some_and(|v| !v.is_empty())),
        "Prism export has no Minecraft version"
    );
    Ok(())
}

pub fn configure_instance(root: &Path, pack: &Pack) -> Result<()> {
    // Use a clean, portable configuration. Exported shell hooks and host paths aren't needed.
    let text = format!(
        "[General]\nConfigVersion=1.3\nInstanceType=OneSix\nname={} {}\niconKey=default\nOverrideJavaLocation=false\nOverrideJavaArgs=false\nOverrideMemory=true\nMinMemAlloc=512\nMaxMemAlloc={}\nOverrideCommands=true\nPreLaunchCommand=\nPostExitCommand=\nWrapperCommand=\nUseAccountForInstance=false\nOverrideEnv=true\nEnv=@Invalid()\n",
        pack.name, pack.version, pack.memory_mb
    );
    let mut file = File::create(root.join("instance.cfg"))?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::{ZipWriter, write::SimpleFileOptions};
    fn zip_file(path: &Path, entries: &[(&str, &str)]) {
        let mut zip = ZipWriter::new(File::create(path).unwrap());
        for (name, body) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    #[test]
    fn rejects_paths_on_all_platforms() {
        for name in [
            "../escape",
            "/absolute",
            "a/../../b",
            "C:/evil",
            "a\\..\\evil",
            "CON.txt",
            "a/b.",
        ] {
            assert!(safe_path(name).is_err(), "{name}");
        }
        assert!(safe_path("minecraft/mods/a.jar").is_ok());
    }
    #[test]
    fn rejects_traversal_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let zip = temp.path().join("bad.zip");
        zip_file(&zip, &[("good.txt", "ok"), ("../escape", "bad")]);
        let out = temp.path().join("out");
        fs::create_dir(&out).unwrap();
        assert!(extract_zip(&zip, &out, &|_, _| {}).is_err());
        assert!(!out.join("good.txt").exists());
    }
    #[test]
    fn accepts_wrapped_prism_export() {
        let temp = tempfile::tempdir().unwrap();
        let zip = temp.path().join("pack.zip");
        zip_file(
            &zip,
            &[
                ("Pack/instance.cfg", "[General]"),
                (
                    "Pack/mmc-pack.json",
                    r#"{"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.21.1"}]}"#,
                ),
            ],
        );
        let out = temp.path().join("out");
        extract_zip(&zip, &out, &|_, _| {}).unwrap();
        assert_eq!(instance_root(&out).unwrap(), out.join("Pack"));
    }
    #[test]
    fn rejects_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("link.zip");
        let mut zip = ZipWriter::new(File::create(&file).unwrap());
        zip.add_symlink("evil", "../outside", SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
        assert!(extract_zip(&file, &temp.path().join("out"), &|_, _| {}).is_err());
    }
}

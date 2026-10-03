use crate::{Paths, Progress, archive, network};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

// Pin a tested release. GitHub's asset digest is verified before running it.
const RELEASE: &str = "11.1.1";
const API: &str = "https://api.github.com/repos/PrismLauncher/PrismLauncher/releases/tags/11.1.1";

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

fn asset_name(os: &str, arch: &str) -> Result<String> {
    match (os, arch) {
        ("linux", "x86_64" | "aarch64") => Ok(format!("PrismLauncher-Linux-{arch}.AppImage")),
        ("windows", "x86_64") => Ok(format!(
            "PrismLauncher-Windows-MinGW-w64-Portable-{RELEASE}.zip"
        )),
        ("windows", "aarch64") => Ok(format!(
            "PrismLauncher-Windows-MinGW-arm64-Portable-{RELEASE}.zip"
        )),
        ("macos", "x86_64" | "aarch64") => Ok(format!("PrismLauncher-macOS-{RELEASE}.zip")),
        _ => bail!("Automatic Prism installation is unavailable on {os}/{arch}"),
    }
}

pub fn executable(paths: &Paths) -> PathBuf {
    let root = paths.root.join("runtime").join(RELEASE);
    if cfg!(target_os = "windows") {
        root.join("prismlauncher.exe")
    } else if cfg!(target_os = "macos") {
        root.join("Prism Launcher.app/Contents/MacOS/prismlauncher")
    } else {
        root.join("PrismLauncher.AppImage")
    }
}

pub fn ensure_installed(paths: &Paths, progress: &Progress<'_>) -> Result<PathBuf> {
    let executable = executable(paths);
    if executable.is_file() {
        return Ok(executable);
    }
    progress("Setting up the Minecraft launch engine…".into(), None);
    let name = asset_name(std::env::consts::OS, std::env::consts::ARCH)?;
    let release: Release = serde_json::from_str(&network::get_text(API, 2 * 1024 * 1024)?)?;
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name == name)
        .context("Official Prism release has no download for this computer")?;
    ensure!(
        asset
            .browser_download_url
            .starts_with("https://github.com/PrismLauncher/PrismLauncher/releases/download/"),
        "Unexpected Prism download host"
    );
    let checksum = asset
        .digest
        .as_deref()
        .and_then(|v| v.strip_prefix("sha256:"))
        .context("Official Prism download has no SHA-256 digest")?;
    let runtime = paths.root.join("runtime");
    let staging = tempfile::tempdir_in(&runtime)?;
    let download = staging.path().join("download");
    network::download(
        &asset.browser_download_url,
        &download,
        Some(checksum),
        progress,
    )?;
    let unpacked = staging.path().join("unpacked");
    fs::create_dir(&unpacked)?;
    if cfg!(target_os = "linux") {
        fs::rename(&download, unpacked.join("PrismLauncher.AppImage"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                unpacked.join("PrismLauncher.AppImage"),
                fs::Permissions::from_mode(0o755),
            )?;
        }
    } else if cfg!(target_os = "macos") {
        // Apple's extractor preserves the app's signed symlinks, permissions, and resource forks.
        let status = Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(&download)
            .arg(&unpacked)
            .status()?;
        ensure!(status.success(), "Could not unpack the Prism application");
    } else {
        archive::extract_zip(&download, &unpacked, progress)?;
    }
    let relative = executable.strip_prefix(runtime.join(RELEASE))?;
    ensure!(
        unpacked.join(relative).is_file(),
        "Prism download has an unexpected layout"
    );
    let destination = runtime.join(RELEASE);
    ensure!(
        !destination.exists(),
        "Incomplete Prism runtime exists at {}. Remove that runtime directory and retry.",
        destination.display()
    );
    fs::rename(unpacked, destination)?;
    progress("Minecraft launch engine is ready".into(), Some(1.0));
    Ok(executable)
}

pub fn prepare_data(paths: &Paths) -> Result<()> {
    fs::create_dir_all(paths.prism_data())?;
    // Keep this private profile independent of any existing MultiMC/PolyMC installation.
    // These are Prism's own markers for declining its legacy migration prompt.
    for name in ["PolyMC_nomigrate.txt", "MultiMC_nomigrate.txt"] {
        let marker = paths.prism_data().join(name);
        if !marker.exists() {
            fs::write(marker, "Managed by NeelemaNet\n")?;
        }
    }
    let config = paths.prism_data().join("prismlauncher.cfg");
    if !config.exists() {
        fs::write(
            config,
            "[General]\nLanguage=en_US\nApplicationTheme=dark\nIconTheme=pe_colored\nAutomaticJavaSwitch=true\nAutomaticJavaDownload=true\nUserAskedAboutAutomaticJavaDownload=true\nIgnoreJavaWizard=true\nMinMemAlloc=512\nMaxMemAlloc=4096\nCloseAfterLaunch=true\nQuitAfterGameStop=true\nCheckUpdateOnStart=false\n",
        )?;
    }
    Ok(())
}

pub fn command(executable: &Path, paths: &Paths) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("--dir")
        .arg(paths.prism_data())
        .current_dir(paths.prism_data());
    #[cfg(target_os = "linux")]
    command.env("APPIMAGE_EXTRACT_AND_RUN", "1"); // Works without FUSE or root.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // No console window for the child.
    }
    command
}

pub fn start(
    paths: &Paths,
    executable: &Path,
    instance: Option<&str>,
    profile: Option<&str>,
    server: Option<&str>,
) -> Result<Child> {
    prepare_data(paths)?;
    let mut command = command(executable, paths);
    if let Some(instance) = instance {
        command.arg("--launch").arg(instance);
        if let Some(profile) = profile.filter(|s| !s.is_empty()) {
            command.arg("--profile").arg(profile);
        }
        if let Some(server) = server {
            command.arg("--server").arg(server);
        }
    }
    // Prism owns authentication and logs. No tokens are copied into NeelemaNet settings.
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.root.join("prism-output.log"))?;
    command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .context("Could not start Prism. See prism-output.log in the NeelemaNet data folder.")
}

pub fn accounts(paths: &Paths) -> Vec<String> {
    // Deserialize only the display fields. Never log or persist the account document.
    #[derive(Deserialize)]
    struct Accounts {
        accounts: Vec<Account>,
    }
    #[derive(Deserialize)]
    struct Account {
        #[serde(rename = "type")]
        kind: String,
        profile: Option<Profile>,
    }
    #[derive(Deserialize)]
    struct Profile {
        name: String,
        id: String,
    }
    fs::read(paths.prism_data().join("accounts.json"))
        .ok()
        .and_then(|data| serde_json::from_slice::<Accounts>(&data).ok())
        .map(|data| {
            data.accounts
                .into_iter()
                .filter(|a| a.kind == "MSA")
                .filter_map(|a| a.profile)
                .filter(|p| !p.id.is_empty() && !p.name.is_empty())
                .map(|p| p.name)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prepares_java_and_skips_legacy_migration_without_resetting_settings() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::new(Some(temp.path().to_owned())).unwrap();
        prepare_data(&paths).unwrap();
        let config = paths.prism_data().join("prismlauncher.cfg");
        assert!(
            fs::read_to_string(&config)
                .unwrap()
                .contains("AutomaticJavaDownload=true")
        );
        assert!(paths.prism_data().join("MultiMC_nomigrate.txt").exists());
        fs::write(&config, "user settings").unwrap();
        prepare_data(&paths).unwrap();
        assert_eq!(fs::read_to_string(config).unwrap(), "user settings");
    }
    #[test]
    fn selects_official_assets() {
        assert_eq!(
            asset_name("linux", "x86_64").unwrap(),
            "PrismLauncher-Linux-x86_64.AppImage"
        );
        assert!(
            asset_name("windows", "aarch64")
                .unwrap()
                .contains("MinGW-arm64-Portable")
        );
        assert!(asset_name("macos", "aarch64").unwrap().ends_with(".zip"));
        assert!(asset_name("linux", "riscv64").is_err());
    }
    #[test]
    fn only_lists_microsoft_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::new(Some(temp.path().to_owned())).unwrap();
        fs::write(paths.prism_data().join("accounts.json"), r#"{"accounts":[{"type":"MSA","profile":{"name":"Duck","id":"abc"}},{"type":"Offline","profile":{"name":"Offline","id":"def"}}]}"#).unwrap();
        assert_eq!(accounts(&paths), vec!["Duck"]);
    }
}

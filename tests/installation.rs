use neelemanet_launcher::{
    Paths,
    catalog::{Catalog, Source},
    launcher,
    preferences::Preferences,
};
use std::{
    fs::{self, File},
    io::Write,
};
use zip::{ZipWriter, write::SimpleFileOptions};

fn fixture() -> (tempfile::TempDir, Paths, Source, Catalog) {
    let temp = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(temp.path().join("data"))).unwrap();
    let source = Source::Local(temp.path().join("packs.toml"));
    let mut catalog = Catalog::parse(include_str!("../packs.toml")).unwrap();
    catalog.packs[0].url = "pack.zip".into();
    write_pack(
        &temp.path().join("pack.zip"),
        "minecraft",
        "test.jar",
        "original pack",
    );
    (temp, paths, source, catalog)
}

fn write_pack(path: &std::path::Path, game_dir: &str, mod_name: &str, config: &str) {
    let mut zip = ZipWriter::new(File::create(path).unwrap());
    for (name, body) in [
        (
            "instance.cfg",
            "[General]\nJavaPath=/someone/elses/java\nPreLaunchCommand=unsafe\n",
        ),
        (
            "mmc-pack.json",
            r#"{"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.21.1"}]}"#,
        ),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    for (name, body) in [
        (format!("{game_dir}/mods/{mod_name}"), "mod fixture"),
        (format!("{game_dir}/config/pack.txt"), config),
        (format!("{game_dir}/options.txt"), "pack defaults"),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn installs_portably_and_reuses_without_touching_saves() {
    let (temp, paths, source, catalog) = fixture();
    let pack = &catalog.packs[0];
    let root = launcher::install(&paths, pack, &source, &|_, _| {}).unwrap();
    assert!(launcher::installed(&paths, pack, &source));
    let config = fs::read_to_string(root.join("instance.cfg")).unwrap();
    assert!(!config.contains("/someone/elses/java"));
    assert!(!config.contains("unsafe"));
    assert!(config.contains("OverrideJavaLocation=false"));
    fs::create_dir_all(root.join("minecraft/saves")).unwrap();
    fs::write(root.join("minecraft/saves/world.dat"), "precious world").unwrap();
    fs::remove_file(temp.path().join("pack.zip")).unwrap();
    assert_eq!(
        launcher::install(&paths, pack, &source, &|_, _| {}).unwrap(),
        root
    );
    assert_eq!(
        fs::read_to_string(root.join("minecraft/saves/world.dat")).unwrap(),
        "precious world"
    );
}

#[test]
fn legacy_checksum_is_ignored_and_not_written_to_new_records() {
    let (_temp, paths, source, mut catalog) = fixture();
    catalog.packs[0].sha256 = Some("an obsolete and deliberately invalid checksum".into());
    let root = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    assert!(
        !fs::read_to_string(root.join(".neelemanet-installed.json"))
            .unwrap()
            .contains("sha256")
    );
    let key = catalog.packs[0].instance_id(&source);
    catalog.packs[0].sha256 = None;
    assert_eq!(catalog.packs[0].instance_id(&source), key);
    assert_eq!(
        launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap(),
        root
    );
}

#[test]
fn new_version_keeps_previous_instance() {
    let (_temp, paths, source, mut catalog) = fixture();
    let old = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    fs::write(old.join("player-data"), "keep me").unwrap();
    catalog.packs[0].version = "2.0.0".into();
    let new = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    assert_ne!(old, new);
    assert_eq!(
        fs::read_to_string(old.join("player-data")).unwrap(),
        "keep me"
    );
}

#[test]
fn parallel_installation_is_locked() {
    let (_temp, paths, _, _) = fixture();
    let guard = paths.lock().unwrap();
    assert!(paths.lock().is_err());
    drop(guard);
    assert!(paths.lock().is_ok());
}

#[test]
fn ram_changes_apply_to_existing_installs_without_redownloading_or_resetting_other_settings() {
    let (temp, paths, source, mut catalog) = fixture();
    let root = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    let config_path = root.join("instance.cfg");
    let mut config = fs::read_to_string(&config_path).unwrap();
    config.push_str("CustomSetting=keep me\n[Other]\nMaxMemAlloc=123\n");
    fs::write(&config_path, config).unwrap();
    Preferences::save_memory(&paths, &catalog.packs[0], &source, Some(6144)).unwrap();
    assert_eq!(
        launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap(),
        root
    );
    let config = fs::read_to_string(&config_path).unwrap();
    assert!(config.contains("OverrideMemory=true\nMinMemAlloc=512\nMaxMemAlloc=6144\n"));
    assert!(config.contains("CustomSetting=keep me\n[Other]\nMaxMemAlloc=123\n"));

    // Also apply a new choice when the existing pack is used offline.
    fs::remove_file(temp.path().join("pack.zip")).unwrap();
    Preferences::save_memory(&paths, &catalog.packs[0], &source, Some(3072)).unwrap();
    assert_eq!(
        launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap(),
        root
    );
    assert!(
        fs::read_to_string(&config_path)
            .unwrap()
            .contains("MaxMemAlloc=3072\n")
    );

    // Reset follows the latest catalog default, including a default-only change.
    Preferences::save_memory(&paths, &catalog.packs[0], &source, None).unwrap();
    catalog.packs[0].memory_mb = 10240;
    assert_eq!(
        launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap(),
        root
    );
    assert!(
        fs::read_to_string(&config_path)
            .unwrap()
            .contains("MaxMemAlloc=10240\n")
    );
}

#[test]
fn ram_preferences_survive_updates_and_stay_specific_to_the_pack_and_catalog() {
    let (_temp, paths, source, mut catalog) = fixture();
    Preferences::save_memory(&paths, &catalog.packs[0], &source, Some(6144)).unwrap();
    let old = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    assert!(
        fs::read_to_string(old.join("instance.cfg"))
            .unwrap()
            .contains("MaxMemAlloc=6144\n")
    );
    catalog.packs[0].version = "2.0.0".into();
    catalog.packs[0].memory_mb = 12288;
    let updated = launcher::update(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    assert_ne!(old, updated);
    assert!(
        fs::read_to_string(updated.join("instance.cfg"))
            .unwrap()
            .contains("MaxMemAlloc=6144\n")
    );
    assert_eq!(
        launcher::current_install(&paths, &catalog.packs[0], &source)
            .unwrap()
            .unwrap()
            .pack
            .memory_mb,
        12288
    );
    let saved = Preferences::load(&paths).unwrap();
    let mut other = catalog.packs[0].clone();
    other.id = "other-pack".into();
    assert_eq!(saved.memory_for(&other, &source), 12288);
    let other_source = Source::parse("https://example.com/packs.toml").unwrap();
    assert_eq!(saved.memory_for(&catalog.packs[0], &other_source), 12288);

    for invalid in [0, 511, 65537] {
        assert!(
            Preferences::save_memory(&paths, &catalog.packs[0], &source, Some(invalid)).is_err()
        );
    }
    assert_eq!(
        Preferences::load(&paths)
            .unwrap()
            .memory_for(&catalog.packs[0], &source),
        6144
    );
}

#[test]
fn update_carries_player_data_and_replaces_removed_mods_and_pack_configs() {
    let (temp, paths, source, mut catalog) = fixture();
    let old = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    for (path, content) in [
        ("saves/My world/level.dat", "world"),
        ("screenshots/memory.png", "screenshot"),
        ("options.txt", "player controls"),
        ("servers.dat", "my servers"),
        ("XaeroWaypoints/myserver/waypoints.txt", "waypoints"),
    ] {
        let path = old.join("minecraft").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    write_pack(
        &temp.path().join("pack.zip"),
        ".minecraft",
        "replacement.jar",
        "new pack config",
    );
    catalog.packs[0].version = "2.0.0".into();
    let new = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    assert_eq!(
        fs::read_to_string(new.join(".minecraft/saves/My world/level.dat")).unwrap(),
        "world"
    );
    assert_eq!(
        fs::read_to_string(new.join(".minecraft/options.txt")).unwrap(),
        "player controls"
    );
    assert_eq!(
        fs::read_to_string(new.join(".minecraft/servers.dat")).unwrap(),
        "my servers"
    );
    assert!(new.join(".minecraft/screenshots/memory.png").exists());
    assert!(
        new.join(".minecraft/XaeroWaypoints/myserver/waypoints.txt")
            .exists()
    );
    assert_eq!(
        fs::read_to_string(new.join(".minecraft/config/pack.txt")).unwrap(),
        "new pack config"
    );
    assert!(new.join(".minecraft/mods/replacement.jar").exists());
    assert!(!new.join(".minecraft/mods/test.jar").exists());
    assert!(old.join("minecraft/mods/test.jar").exists());
    assert!(old.join("minecraft/saves/My world/level.dat").exists());
    assert_eq!(
        launcher::current_install(&paths, &catalog.packs[0], &source)
            .unwrap()
            .unwrap()
            .root(&paths),
        new
    );
}

#[test]
fn replacing_zip_at_same_location_triggers_update_without_version_bump() {
    let (temp, paths, source, catalog) = fixture();
    let old = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    write_pack(
        &temp.path().join("pack.zip"),
        "minecraft",
        "new-mod.jar",
        "different contents and length",
    );
    let new = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    assert_ne!(old, new);
    assert!(new.join("minecraft/mods/new-mod.jar").exists());
}

#[test]
fn force_update_downloads_even_when_version_and_source_are_unchanged() {
    let (_temp, paths, source, catalog) = fixture();
    let old = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    let new = launcher::update(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    assert_ne!(old, new);
    assert!(old.exists());
}

#[test]
fn broken_update_keeps_previous_active_install_and_worlds() {
    let (temp, paths, source, mut catalog) = fixture();
    let old = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    fs::create_dir_all(old.join("minecraft/saves")).unwrap();
    fs::write(old.join("minecraft/saves/level.dat"), "precious world").unwrap();
    fs::write(temp.path().join("pack.zip"), "truncated ZIP").unwrap();
    catalog.packs[0].version = "broken-release".into();
    assert!(launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).is_err());
    let active = launcher::current_install(&paths, &catalog.packs[0], &source)
        .unwrap()
        .unwrap();
    assert_eq!(active.root(&paths), old);
    assert_eq!(active.pack.version, "1.0.0");
    assert_eq!(
        fs::read_to_string(old.join("minecraft/saves/level.dat")).unwrap(),
        "precious world"
    );
    assert_eq!(fs::read_dir(paths.instances()).unwrap().count(), 1);
}

#[test]
fn recognizes_and_updates_legacy_installation_with_checksum() {
    use sha2::{Digest, Sha256};
    let (_temp, paths, source, mut catalog) = fixture();
    let pack = &mut catalog.packs[0];
    let legacy_checksum = "0".repeat(64);
    let legacy_key = format!(
        "{}\n{}\n{}\n{}",
        source.label(),
        pack.version,
        pack.url,
        legacy_checksum
    );
    let legacy_root = paths.instances().join(format!(
        "neelemanet-{}-{:x}",
        pack.id,
        Sha256::digest(legacy_key.as_bytes())
    ));
    fs::create_dir_all(legacy_root.join("minecraft/saves")).unwrap();
    fs::write(legacy_root.join("minecraft/saves/level.dat"), "old world").unwrap();
    fs::write(legacy_root.join("instance.cfg"), "[General]").unwrap();
    fs::write(legacy_root.join("mmc-pack.json"), "{}").unwrap();
    let mut marker = serde_json::to_value(&pack).unwrap();
    marker["sha256"] = legacy_checksum.into();
    fs::write(
        legacy_root.join(".neelemanet-installed.json"),
        serde_json::to_vec(&marker).unwrap(),
    )
    .unwrap();
    assert_eq!(
        launcher::current_install(&paths, pack, &source)
            .unwrap()
            .unwrap()
            .root(&paths),
        legacy_root
    );
    pack.version = "2.0.0".into();
    let new = launcher::install(&paths, pack, &source, &|_, _| {}).unwrap();
    assert_eq!(
        fs::read_to_string(new.join("minecraft/saves/level.dat")).unwrap(),
        "old world"
    );
    assert!(legacy_root.exists());
}

#[cfg(unix)]
#[test]
fn failed_player_data_copy_does_not_activate_update() {
    let (_temp, paths, source, catalog) = fixture();
    let old = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    fs::create_dir_all(old.join("minecraft/saves")).unwrap();
    std::os::unix::fs::symlink(
        old.join("minecraft/options.txt"),
        old.join("minecraft/saves/linked-file"),
    )
    .unwrap();
    assert!(launcher::update(&paths, &catalog.packs[0], &source, &|_, _| {}).is_err());
    assert_eq!(
        launcher::current_install(&paths, &catalog.packs[0], &source)
            .unwrap()
            .unwrap()
            .root(&paths),
        old
    );
    assert_eq!(fs::read_dir(paths.instances()).unwrap().count(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn running_game_prevents_update() {
    let (temp, paths, source, catalog) = fixture();
    let old = launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).unwrap();
    let java = temp.path().join("java");
    fs::copy("/bin/sleep", &java).unwrap();
    let mut child = std::process::Command::new(java)
        .arg("30")
        .current_dir(old.join("minecraft"))
        .spawn()
        .unwrap();
    let result = launcher::update(&paths, &catalog.packs[0], &source, &|_, _| {});
    let _ = child.kill();
    let _ = child.wait();
    assert!(result.unwrap_err().to_string().contains("Close Minecraft"));
    assert_eq!(
        launcher::current_install(&paths, &catalog.packs[0], &source)
            .unwrap()
            .unwrap()
            .root(&paths),
        old
    );
}

#[cfg(unix)]
#[test]
fn cli_play_launches_the_active_revision_returned_by_installer() {
    use neelemanet_launcher::prism;
    use std::os::unix::fs::PermissionsExt;
    let (temp, paths, source, catalog) = fixture();
    fs::write(
        temp.path().join("packs.toml"),
        toml::to_string(&catalog).unwrap(),
    )
    .unwrap();
    let engine = prism::executable(&paths);
    fs::create_dir_all(engine.parent().unwrap()).unwrap();
    fs::write(&engine, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).unwrap();
    Preferences::save_memory(&paths, &catalog.packs[0], &source, Some(6144)).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_neelemanet_launcher"))
        .arg("--catalog")
        .arg(source.label())
        .arg("--data-dir")
        .arg(&paths.root)
        .args(["play", "duck-craft"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let current = launcher::current_install(&paths, &catalog.packs[0], &source)
        .unwrap()
        .unwrap();
    let log = fs::read_to_string(paths.root.join("prism-output.log")).unwrap();
    assert!(log.contains(&format!("--launch\n{}\n", current.instance)));
    assert_ne!(current.instance, catalog.packs[0].instance_id(&source));
    assert!(
        fs::read_to_string(current.root(&paths).join("instance.cfg"))
            .unwrap()
            .contains("MaxMemAlloc=6144\n")
    );
}

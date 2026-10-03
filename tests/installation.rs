use neelemanet_launcher::{
    Paths,
    catalog::{Catalog, Source},
    launcher,
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
    catalog.packs[0].sha256 = None;
    let mut zip = ZipWriter::new(File::create(temp.path().join("pack.zip")).unwrap());
    for (name, body) in [
        (
            "instance.cfg",
            "[General]\nJavaPath=/someone/elses/java\nPreLaunchCommand=unsafe\n",
        ),
        (
            "mmc-pack.json",
            r#"{"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.21.1"}]}"#,
        ),
        ("minecraft/mods/test.jar", "test fixture"),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    (temp, paths, source, catalog)
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
fn checksum_failure_never_publishes_instance() {
    let (_temp, paths, source, mut catalog) = fixture();
    catalog.packs[0].sha256 = Some("0".repeat(64));
    assert!(launcher::install(&paths, &catalog.packs[0], &source, &|_, _| {}).is_err());
    assert!(!launcher::installed(&paths, &catalog.packs[0], &source));
    assert_eq!(fs::read_dir(paths.instances()).unwrap().count(), 0);
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

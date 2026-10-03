#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod ui;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use neelemanet_launcher::{
    Paths, archive,
    catalog::{self, Source},
    launcher, prism,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "NeelemaNet — your modpacks, ready to play")]
struct Args {
    /// Root packs.toml path or raw HTTPS URL. GUI opens when no command is supplied.
    #[arg(long, global = true, env = "NEELEMANET_CATALOG_URL")]
    catalog: Option<String>,
    /// Override the private application data directory.
    #[arg(long, global = true, env = "NEELEMANET_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// List available packs.
    List,
    /// Download and configure the private Prism runtime without opening a window.
    Setup,
    /// Open Minecraft account sign-in in the managed Prism installation.
    Login,
    /// Install a pack without starting Minecraft.
    Install { id: String },
    /// Re-download a pack, carrying over player data and keeping the previous install.
    Update { id: String },
    /// Install as needed, then launch. Prism prompts for Microsoft sign-in if needed.
    Play {
        id: String,
        #[arg(long)]
        profile: Option<String>,
    },
    /// Validate and extract a Prism ZIP in a temporary directory.
    Validate { zip: PathBuf },
}

fn main() -> Result<()> {
    // Double-clicking opens just the GUI. CLI invocations retain their parent terminal.
    #[cfg(target_os = "windows")]
    if std::env::args_os().len() > 1 {
        unsafe {
            windows_sys::Win32::System::Console::AttachConsole(
                windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
            );
        }
    }
    let args = Args::parse();
    let paths = Paths::new(args.data_dir)?;
    let source = match args.catalog {
        Some(value) => Source::parse(&value)?,
        None => catalog::default_source()?,
    };
    let progress = |message: String, _: Option<f32>| eprintln!("{message}");
    match args.command {
        None => ui::run(paths, source),
        Some(Action::Validate { zip }) => {
            let temp = tempfile::tempdir()?;
            archive::extract_zip(&zip, temp.path(), &progress)?;
            archive::instance_root(temp.path())?;
            println!("Valid Prism export: {}", zip.display());
            Ok(())
        }
        Some(Action::Setup) => {
            let _lock = paths.lock()?;
            let executable = prism::ensure_installed(&paths, &progress)?;
            prism::prepare_data(&paths)?;
            println!("Ready: {}", executable.display());
            Ok(())
        }
        Some(Action::Login) => {
            let lock = paths.lock()?;
            let executable = prism::ensure_installed(&paths, &progress)?;
            let mut child = prism::start(&paths, &executable, None, None, None)?;
            drop(lock);
            println!(
                "Sign in with Microsoft in Prism. Existing users: Accounts → Manage Accounts → Add Microsoft."
            );
            anyhow::ensure!(
                child.wait()?.success(),
                "Prism exited with an error; see prism-output.log"
            );
            Ok(())
        }
        Some(action) => {
            let (catalog, warning) = catalog::load(&source, &paths)?;
            if let Some(warning) = warning {
                eprintln!("{warning}");
            }
            if matches!(action, Action::List) {
                for pack in catalog.packs {
                    println!(
                        "{}\t{}\t{}\t{}",
                        pack.id, pack.name, pack.version, pack.loader
                    );
                }
                return Ok(());
            }
            let (id, play, profile, force) = match action {
                Action::Install { id } => (id, false, None, false),
                Action::Update { id } => (id, false, None, true),
                Action::Play { id, profile } => (id, true, profile, false),
                _ => unreachable!(),
            };
            let pack = catalog
                .packs
                .iter()
                .find(|p| p.id == id)
                .with_context(|| format!("Unknown pack: {id}"))?;
            let lock = paths.lock()?;
            let root = if force {
                launcher::update(&paths, pack, &source, &progress)?
            } else {
                launcher::install(&paths, pack, &source, &progress)?
            };
            println!("Installed: {}", root.display());
            if play {
                let executable = prism::ensure_installed(&paths, &progress)?;
                let mut child = prism::start(
                    &paths,
                    &executable,
                    Some(
                        root.file_name()
                            .and_then(|n| n.to_str())
                            .context("Invalid installed instance name")?,
                    ),
                    profile.as_deref(),
                    pack.server.as_deref(),
                )?;
                drop(lock);
                let status = child.wait()?;
                if !status.success() {
                    bail!(
                        "Prism exited with {status}; see {}",
                        paths.root.join("prism-output.log").display()
                    );
                }
            }
            Ok(())
        }
    }
}

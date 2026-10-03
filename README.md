# NeelemaNet Launcher

A Rust desktop launcher for a curated list of Minecraft modpacks. Players open NeelemaNet, sign in with Microsoft, and click **Install & play**. NeelemaNet downloads its own copy of Prism Launcher; players do not need to install Prism, Java, or a mod loader themselves.

Prism handles Minecraft authentication and game launch. Its first-run screen asks for Microsoft sign-in. An existing account can be managed through **Accounts → Manage Accounts** in Prism. NeelemaNet displays the Minecraft profile names and lets players select an account. Credentials and refresh tokens stay in Prism's account store.

## Run locally

Requires Rust 1.95 or newer to build. Players using a prebuilt download do not need Rust.

```sh
cargo run
```

The repository's `packs.toml` is configured for the supplied `duck craft.zip`: Minecraft 1.21.1, NeoForge 21.1.252, and an 8 GiB memory limit. Keep the ZIP alongside `packs.toml` for local testing. Large ZIPs are intentionally ignored by Git.

On first opening, NeelemaNet downloads the official Prism 11.1.1 runtime into its private application data directory. Linux uses the official AppImage with extraction mode, so FUSE and administrator privileges are unnecessary. Windows uses the MinGW portable build, avoiding a separate Visual C++ runtime installation. macOS uses the official application ZIP. x86-64 and ARM64 runtime downloads are supported on these platforms. Native graphics/desktop libraries are still required on Linux.

## Publish packs for players

1. Upload each Prism export ZIP to a public HTTPS host, such as a GitHub Release asset. The sample ZIP is too large for a normal Git commit.
2. Edit the root `packs.toml`, replacing local paths with direct HTTPS downloads. Set each pack's SHA-256 to the exported ZIP's hash (`sha256sum 'duck craft.zip'` on Linux).
3. Commit `packs.toml` to this repository's default branch. Pack list changes then reach players when they open the launcher or press **Refresh**.
4. Run the **Build player downloads** GitHub Actions workflow, or push a `v*` tag. It embeds this repository's raw default-branch `packs.toml` URL into each binary and produces downloadable archives. Download the workflow artifacts and attach the archives to your release.

The release workflow refuses local pack paths. This checkout does not yet have a Git remote or a public ZIP URL configured. Set those before distributing builds. Public distribution requires a publicly readable catalog and ZIP host; private GitHub authentication is not implemented.

You can also build manually with a catalog URL embedded:

```sh
NEELEMANET_CATALOG_URL=https://raw.githubusercontent.com/OWNER/REPO/main/packs.toml cargo build --release
```

At runtime, `--catalog` or the `NEELEMANET_CATALOG_URL` environment variable overrides the embedded URL. Development builds look for `packs.toml` in the working directory, beside the executable, then the source directory. The GUI's settings offer a temporary catalog override.

The workflow packages Windows x64, Linux x64, and macOS Intel/Apple Silicon builds. Windows builds are unsigned; macOS bundles are ad-hoc signed, not notarized. Production distribution should use your own code-signing/notarization credentials to avoid operating system trust prompts. No signing credentials are included.

## Catalog format

```toml
schema_version = 1
title = "NeelemaNet"

[[packs]]
id = "duck-craft"
name = "Duck Craft"
version = "1.0.0"
description = "Our modded survival pack."
minecraft = "1.21.1"
loader = "NeoForge 21.1.252"
url = "https://your-host.example/duck-craft-1.0.0.zip"
# sha256 = "64 hexadecimal characters from the ZIP's SHA-256"
memory_mb = 8192
# server = "play.your-server.example"
```

Add another `[[packs]]` entry for each pack. `id` must be unique and use lowercase letters, digits, or hyphens. `minecraft` and `loader` are display labels; the actual versions come from the export's `mmc-pack.json`. `sha256` is optional but recommended. Memory defaults to 4096 MiB. `server` makes Play join that server automatically. Remote catalogs can use relative HTTPS URLs; local catalogs can use paths relative to the TOML file.

Exports must contain `instance.cfg` and `mmc-pack.json`, either directly in the ZIP root or together in one top-level folder. The game files (`minecraft` or `.minecraft`), mods, configs, and component patches are retained. Machine-specific Java settings, account selection, environment variables, and launch commands from the export are replaced with portable defaults.

## Updates, storage, and recovery

Downloads stream to temporary files and verify SHA-256 when supplied. The official Prism runtime's GitHub release asset digest is required and verified before execution. Pack extraction rejects traversal, symlinks, duplicate paths, and oversized archives. Incomplete operations do not become installed packs and can be retried. A process lock prevents concurrent installers from changing the same data directory.

Pack installs are identified by catalog source, pack ID, version, URL, and checksum. Bump the version whenever publishing changed pack contents. A new version gets its own instance; previous worlds and settings are preserved in the old instance. Worlds are **not automatically copied** between versions. Open **Settings → Open data folder → prism/instances** to copy a world's `minecraft/saves` folder after checking mod compatibility. The launcher never removes old instances automatically.

The last valid remote catalog is cached for temporary network outages. Already-installed packs are reused without downloading their ZIP again. Actual offline Minecraft availability depends on Prism's cached authentication and game files.

Use **Settings → Open data folder** to inspect storage. The platform default comes from `directories::ProjectDirs`; it includes `prism/` (accounts, Java, game assets, instances), `runtime/` (managed Prism), `cache/`, and `prism-output.log`. `--data-dir PATH` or `NEELEMANET_DATA_DIR` overrides it. Treat the data directory as private because Prism stores authentication tokens there. NeelemaNet does not modify a separately installed Prism profile.

If setup or launch fails, the UI shows the error. Retry the action after addressing it. `prism-output.log` records runtime output; Prism's own logs are under `prism/logs`. Downloads have connection and overall timeouts. Closing NeelemaNet during a download interrupts it; a later attempt starts again (partial-download resume is not implemented).

## Developer commands

```sh
cargo run -- list
cargo run -- setup
cargo run -- login
cargo run -- install duck-craft
cargo run -- play duck-craft
cargo run -- play duck-craft --profile YourMinecraftName
cargo run -- validate 'duck craft.zip'
cargo run -- --catalog https://your-host.example/packs.toml list
cargo run -- --data-dir .neelemanet-test install duck-craft
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
```

`install` only imports a pack. `setup` downloads Prism and prepares Java defaults without opening a window. `play` performs both automatically, then launches. `validate` extracts into a temporary directory and checks the Prism metadata without launching anything. The test suite uses small generated ZIPs and does not require the large sample export, a Minecraft account, or a network connection.

Prism's supported [launch arguments](https://prismlauncher.org/wiki/getting-started/command-line-interface/) and [automatic Java setup](https://prismlauncher.org/wiki/getting-started/installing-java/) provide the launch engine. NeelemaNet downloads unmodified official Prism releases; it is a separate application, not an official Prism or Minecraft product.

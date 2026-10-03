# NeelemaNet Launcher

A Rust desktop launcher for a curated list of Minecraft modpacks. Players open NeelemaNet, sign in with Microsoft, and click **Install & play**. NeelemaNet downloads its own copy of Prism Launcher; players do not need to install Prism, Java, or a mod loader themselves.

Prism handles Minecraft authentication and game launch. Its first-run screen asks for Microsoft sign-in. An existing account can be managed through **Accounts → Manage Accounts** in Prism. NeelemaNet displays the Minecraft profile names and lets players select an account. Credentials and refresh tokens stay in Prism's account store.

Each pack has a **Minecraft RAM** control. Uncheck **Use pack default**, choose 0.5–64 GiB, and click **Save RAM**. The limit applies the next time Minecraft starts, including for already-installed packs and CLI launches. Choices are saved per pack/catalog in `preferences.json` in the data folder and survive launcher restarts and pack updates. Re-enable **Use pack default** and save to follow the catalog's RAM setting again. This controls Minecraft's maximum Java heap; the game and launcher can also use memory outside that heap.

## Run locally

Requires Rust 1.95 or newer to build. Players using a prebuilt download do not need Rust.

```sh
cargo run
```

The launcher fetches its pack list from [this repository's `packs.toml`](https://raw.githubusercontent.com/icanthink42/neelemanet_launcher/main/packs.toml) by default, including when running from the source checkout. Duck Craft downloads from `https://neelemanet-cdn.s3.us-east-1.amazonaws.com/duck+craft.zip`: Minecraft 1.21.1, NeoForge 21.1.252, and an 8 GiB memory limit. No local ZIP is required. Large ZIPs are intentionally ignored by Git.

To test unpublished catalog changes, run `cargo run -- --catalog packs.toml`. Local catalogs also accept ZIP paths relative to the TOML file.

On first opening, NeelemaNet downloads the official Prism 11.1.1 runtime into its private application data directory. Linux uses the official AppImage with extraction mode, so FUSE and administrator privileges are unnecessary. Windows uses the MinGW portable build, avoiding a separate Visual C++ runtime installation. macOS uses the official application ZIP. x86-64 and ARM64 runtime downloads are supported on these platforms. Native graphics/desktop libraries are still required on Linux.

## Publish packs for players

1. Upload each Prism export ZIP to a public HTTPS host, such as a GitHub Release asset. The sample ZIP is too large for a normal Git commit.
2. Edit the root `packs.toml` with each pack's direct HTTPS download URL and version. No checksum is needed.
3. Commit and push `packs.toml` to this repository's `main` branch. Pack list changes then reach players when they open the launcher or press **Refresh**.
4. Players receive catalog updates without a launcher release. To distribute a new version of the launcher itself, use the release workflow below.

The release workflow refuses local pack paths. The default catalog and Duck Craft download are configured for public access; private GitHub authentication is not implemented.

You can also build manually with a catalog URL embedded:

```sh
NEELEMANET_CATALOG_URL=https://raw.githubusercontent.com/OWNER/REPO/main/packs.toml cargo build --release
```

At runtime, `--catalog` or the `NEELEMANET_CATALOG_URL` environment variable overrides the embedded URL. Both development and release builds otherwise fetch the GitHub catalog; a local `packs.toml` does not override it automatically. The GUI's settings offer a temporary catalog override.

## Build and publish launcher releases

**Push to `main` to publish a launcher release.** No manual tag or version bump is required. GitHub Actions runs formatting, Clippy, and tests, then builds Windows, Linux, and both Mac architectures. After every build succeeds, it creates the tag and publishes all four downloads to a [GitHub Release](https://github.com/icanthink42/neelemanet_launcher/releases) with generated release notes.

Tags combine the version in `Cargo.toml` with an automatic build number, for example `v0.1.0+build.42`. Each new workflow run gets a unique tag, even when `Cargo.toml` stays unchanged. The source files are not rewritten or committed by the workflow. The automatic build suffix is release metadata; normal builds remain regular releases, while a base version such as `0.2.0-beta.1` produces a prerelease.

For `v0.1.0+build.42`, the release assets are:

| Platform | Download |
| --- | --- |
| Windows x64 | `NeelemaNet-windows-x64-v0.1.0+build.42.exe` — double-click to run |
| Linux x64 | `NeelemaNet-linux-x64-v0.1.0+build.42.AppImage` — allow execution in file properties, then double-click |
| macOS Apple Silicon | `NeelemaNet-macos-arm64-v0.1.0+build.42.dmg` — open and drag NeelemaNet to Applications |
| macOS Intel | `NeelemaNet-macos-x64-v0.1.0+build.42.dmg` — open and drag NeelemaNet to Applications |

These launcher downloads require no manual archive extraction. The Linux AppImage targets x64 desktop Linux with glibc 2.35 or newer (such as Ubuntu 22.04+). It uses the desktop's installed graphics and keyboard libraries so they remain compatible with its drivers and locale data. X11 needs the matching `libxkbcommon-x11` system library (`libxkbcommon-x11-0` on Ubuntu). On systems without FUSE, it can also be started with `APPIMAGE_EXTRACT_AND_RUN=1 ./NeelemaNet-*.AppImage`. GitHub also lists automatic **Source code** archives; players should use the platform downloads above. Modpack exports remain ZIP files and are handled automatically by the launcher.

Rerun a failed workflow to resume the same tag/release, or choose **Actions → Build and publish release → Run workflow** to start a new build without entering a tag. Each platform builds the exact triggering commit. No tag is created if a build fails; existing tags are never moved to another commit. New releases remain drafts until all downloads are uploaded. Reruns can replace uploaded assets unless repository release immutability is enabled.

Branch pushes and pull requests also run the separate **Check** workflow. Pack-only changes still reach existing launchers through the catalog; pushing them to `main` also triggers a new launcher release.

Before publishing, the Linux AppImage must open a real window on both X11 and Wayland in an Ubuntu 24.04 test environment, separate from its Ubuntu 22.04 build environment. These checks exercise graphics and keyboard initialization, which a `--version` check does not cover.

Publishing uses GitHub's built-in `GITHUB_TOKEN`, with `contents: write` granted only to the publishing job; no personal access token is needed. Enable GitHub Actions in the repository if it is disabled. Pack URLs must be public HTTPS downloads. The workflow updates the Mac application's version metadata from the release version.

Windows builds are unsigned; macOS bundles are ad-hoc signed, not notarized. Production distribution should use your own code-signing/notarization credentials to avoid operating system trust prompts. No signing credentials are included.

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
memory_mb = 8192
# server = "play.your-server.example"
```

Add another `[[packs]]` entry for each pack. `id` must be unique and use lowercase letters, digits, or hyphens. `minecraft` and `loader` are display labels; the actual versions come from the export's `mmc-pack.json`. Old `sha256` fields are accepted for compatibility and ignored. Memory defaults to 4096 MiB. `server` makes Play join that server automatically. Remote catalogs can use relative HTTPS URLs; local catalogs can use paths relative to the TOML file.

Exports must contain `instance.cfg` and `mmc-pack.json`, either directly in the ZIP root or together in one top-level folder. The game files (`minecraft` or `.minecraft`), mods, configs, and component patches are retained. Machine-specific Java settings, account selection, environment variables, and launch commands from the export are replaced with portable defaults.

## Updates, storage, and recovery

Pack downloads stream to temporary files without computing or checking SHA-256. The official Prism runtime's GitHub release asset digest is required and verified before execution. Pack extraction rejects traversal, symlinks, duplicate paths, and oversized archives. Incomplete operations do not become installed packs and can be retried. A process lock prevents concurrent installers from changing the same data directory.

**Play checks for updates automatically.** It refreshes the GitHub catalog, then compares the installed version and URL. For an unchanged URL, it checks the server's ETag, Last-Modified, and Content-Length headers. Replacing the S3 ZIP updates its ETag, so the same URL can deliver a new pack without a checksum or version change. Bumping `version` is still recommended so players can see which release they are using. **Refresh** reloads the catalog and shows **Update & play** for a changed version or URL. **Update pack** (or `cargo run -- update duck-craft`) forces a fresh download, including when a host has no usable change headers.

Each update is extracted into a fresh instance. Worlds (`saves`), screenshots, game settings (`options.txt`, `optionsof.txt`, `optionsshaders.txt`), saved servers, Xaero maps/waypoints, and JourneyMap data are copied from the active installation, including between `minecraft` and `.minecraft` layouts. Mods and pack configs come from the new export, so removed mods stay removed and new pack configs take effect. Other custom mod configs and extra user-installed mods/resources remain in the backup; they are not automatically merged into the new pack.

Only after download, extraction, and player-data copying succeed does the launcher atomically select the new instance. Failed updates keep the old instance active. The previous instance remains in `prism/instances` as a backup, and active revision records live in `installed/`. Old launcher installations are detected and migrated automatically. Close the pack's Minecraft process before updating; detected running games block updates to avoid copying a world while it is being saved. Backups are never automatically deleted; they can consume substantial disk space, especially for large worlds.

The last valid remote catalog is cached for temporary network outages. Already-installed packs are reused when unchanged. If checking download metadata fails and the catalog version/URL is unchanged, Play uses the installed copy; explicit updates report download failures and leave the old install intact. Hosts without usable change headers require a version bump or **Update pack**. Actual offline Minecraft availability depends on Prism's cached authentication and game files.

Use **Settings → Open data folder** to inspect storage. The platform default comes from `directories::ProjectDirs`; it includes `prism/` (accounts, Java, game assets, instances), `runtime/` (managed Prism), `cache/`, `installed/` (active pack revisions), and `prism-output.log`. `--data-dir PATH` or `NEELEMANET_DATA_DIR` overrides it. Treat the data directory as private because Prism stores authentication tokens there. NeelemaNet does not modify a separately installed Prism profile.

If setup or launch fails, the UI shows the error. Retry the action after addressing it. `prism-output.log` records runtime output; Prism's own logs are under `prism/logs`. Downloads have connection and overall timeouts. Closing NeelemaNet during a download interrupts it; a later attempt starts again (partial-download resume is not implemented).

## Developer commands

```sh
cargo run -- list
cargo run -- setup
cargo run -- login
cargo run -- install duck-craft
cargo run -- update duck-craft
cargo run -- play duck-craft
cargo run -- play duck-craft --profile YourMinecraftName
cargo run -- validate 'duck craft.zip'
cargo run -- --catalog https://your-host.example/packs.toml list
cargo run -- --data-dir .neelemanet-test install duck-craft
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
```

`install` installs or updates a pack without launching it. `update` forces a fresh download while preserving player data. `setup` downloads Prism and prepares Java defaults without opening a window. `play` performs both automatically, then launches. `validate` extracts into a temporary directory and checks the Prism metadata without launching anything. The test suite uses small generated ZIPs and does not require the large sample export, a Minecraft account, or a network connection.

Prism's supported [launch arguments](https://prismlauncher.org/wiki/getting-started/command-line-interface/) and [automatic Java setup](https://prismlauncher.org/wiki/getting-started/installing-java/) provide the launch engine. NeelemaNet downloads unmodified official Prism releases; it is a separate application, not an official Prism or Minecraft product.

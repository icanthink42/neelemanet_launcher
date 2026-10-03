use anyhow::{Context, Result};
use eframe::egui::{self, Color32, RichText};
use neelemanet_launcher::{
    Paths,
    catalog::{self, Catalog, Source},
    launcher, prism,
};
use std::{
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

const GREEN: Color32 = Color32::from_rgb(160, 232, 145);
const MUTED: Color32 = Color32::from_rgb(152, 162, 157);
const SURFACE: Color32 = Color32::from_rgb(26, 34, 30);

enum Event {
    Progress(String, Option<f32>),
    Catalog(Catalog, Option<String>),
    Done(String),
    Error(String),
    ProcessError(String),
}
enum Job {
    Initialize,
    Refresh,
    Login,
    Play(usize),
    Install(usize),
    Update(usize),
}

struct App {
    paths: Paths,
    source: Source,
    catalog: Option<Catalog>,
    selected: usize,
    accounts: Vec<String>,
    profile: String,
    status: String,
    warning: Option<String>,
    error: Option<String>,
    progress: Option<f32>,
    busy: bool,
    sender: Sender<Event>,
    receiver: Receiver<Event>,
    last_account_poll: Instant,
    search: String,
    settings: bool,
    catalog_input: String,
}

pub fn run(paths: Paths, source: Source) -> Result<()> {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1060.0, 700.0])
            .with_min_inner_size([820.0, 580.0]),
        ..Default::default()
    };
    // Wayland can withhold frame callbacks for covered/off-workspace windows.
    // Blocking in OpenGL's vsync wait then prevents winit from answering desktop
    // pings, causing an "Application Not Responding" dialog. Egui already
    // schedules repaints on demand, so Linux does not need a blocking swap.
    if cfg!(target_os = "linux") {
        options.glow_options.vsync = false;
    }
    eframe::run_native(
        "NeelemaNet",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            let mut style = (*cc.egui_ctx.global_style()).clone();
            style.visuals = egui::Visuals::dark();
            style.visuals.panel_fill = Color32::from_rgb(16, 23, 19);
            style.visuals.window_fill = SURFACE;
            style.visuals.selection.bg_fill = Color32::from_rgb(58, 89, 60);
            style.spacing.item_spacing = egui::vec2(12.0, 12.0);
            style.spacing.button_padding = egui::vec2(16.0, 10.0);
            cc.egui_ctx.set_global_style(style);
            let (sender, receiver) = mpsc::channel();
            let mut app = App {
                catalog_input: source.label(),
                paths,
                source,
                catalog: None,
                selected: 0,
                accounts: Vec::new(),
                profile: String::new(),
                status: "Getting things ready…".into(),
                warning: None,
                error: None,
                progress: None,
                busy: false,
                sender,
                receiver,
                last_account_poll: Instant::now() - Duration::from_secs(5),
                search: String::new(),
                settings: false,
            };
            app.start(Job::Initialize, &cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| anyhow::anyhow!("Could not open the launcher window: {error}"))
}

impl App {
    fn start(&mut self, job: Job, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        self.progress = None;
        self.status = "Getting things ready…".into();
        let paths = self.paths.clone();
        let source = self.source.clone();
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        let profile = self.profile.clone();
        let mut pack = match &job {
            Job::Play(i) | Job::Install(i) | Job::Update(i) => {
                self.catalog.as_ref().and_then(|c| c.packs.get(*i)).cloned()
            }
            _ => None,
        };
        thread::spawn(move || {
            let send = |event| {
                let _ = tx.send(event);
                ctx.request_repaint();
            };
            let progress = |message, fraction| send(Event::Progress(message, fraction));
            let result: Result<String> = (|| {
                if let Some(selected) = &pack {
                    let id = selected.id.clone();
                    progress("Checking the latest pack list…".into(), None);
                    let (catalog, warning) = catalog::load(&source, &paths)?;
                    pack = Some(
                        catalog
                            .packs
                            .iter()
                            .find(|p| p.id == id)
                            .context("This pack is no longer in the catalog")?
                            .clone(),
                    );
                    send(Event::Catalog(catalog, warning));
                }
                if matches!(job, Job::Initialize | Job::Refresh) {
                    let (catalog, warning) = catalog::load(&source, &paths)?;
                    send(Event::Catalog(catalog, warning));
                    if matches!(job, Job::Refresh) {
                        return Ok("Pack list refreshed".into());
                    }
                }
                let lock = paths.lock()?;
                let root = if let Some(pack) = &pack {
                    Some(if matches!(job, Job::Update(_)) {
                        launcher::update(&paths, pack, &source, &progress)?
                    } else {
                        launcher::install(&paths, pack, &source, &progress)?
                    })
                } else {
                    None
                };
                if matches!(job, Job::Install(_) | Job::Update(_)) {
                    return Ok("Installed and ready to play".into());
                }
                let executable = prism::ensure_installed(&paths, &progress)?;
                prism::prepare_data(&paths)?;
                if matches!(job, Job::Initialize) {
                    return Ok("Ready when you are. Pick a pack and play.".into());
                }
                let instance = root
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str());
                let mut child = prism::start(
                    &paths,
                    &executable,
                    instance,
                    Some(&profile),
                    pack.as_ref().and_then(|p| p.server.as_deref()),
                )?;
                drop(lock);
                let monitor_tx = tx.clone();
                let monitor_ctx = ctx.clone();
                thread::spawn(move || {
                    let message = match child.wait() {
                        Ok(status) if status.success() => return,
                        Ok(status) => format!(
                            "Prism exited with {status}. Open the data folder and check prism-output.log."
                        ),
                        Err(error) => format!("Could not monitor Prism: {error}"),
                    };
                    let _ = monitor_tx.send(Event::ProcessError(message));
                    monitor_ctx.request_repaint();
                });
                if instance.is_some() {
                    Ok("Launch requested. Complete Microsoft sign-in in Prism if prompted; your game will start afterward.".into())
                } else {
                    Ok("Sign in with Microsoft in the Prism window. To switch accounts: Accounts > Manage Accounts > Add Microsoft. Then return here to play.".into())
                }
            })();
            match result {
                Ok(message) => send(Event::Done(message)),
                Err(error) => send(Event::Error(format!("{error:#}"))),
            }
        });
    }

    fn poll(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                Event::Progress(message, progress) => {
                    self.status = message;
                    self.progress = progress;
                }
                Event::Catalog(catalog, warning) => {
                    let selected_id = self
                        .catalog
                        .as_ref()
                        .and_then(|c| c.packs.get(self.selected))
                        .map(|p| &p.id);
                    self.selected = selected_id
                        .and_then(|id| catalog.packs.iter().position(|p| &p.id == id))
                        .unwrap_or(0);
                    self.catalog = Some(catalog);
                    self.warning = warning;
                }
                Event::Done(message) => {
                    self.busy = false;
                    self.status = message;
                    self.progress = None;
                }
                Event::Error(error) => {
                    self.busy = false;
                    self.status = "Something went wrong. You can retry.".into();
                    self.error = Some(error);
                }
                Event::ProcessError(error) => self.error = Some(error),
            }
        }
        if self.last_account_poll.elapsed() > Duration::from_secs(2) {
            self.accounts = prism::accounts(&self.paths);
            if !self.accounts.contains(&self.profile) {
                self.profile = self.accounts.first().cloned().unwrap_or_default();
            }
            self.last_account_poll = Instant::now();
        }
    }

    fn folder(&mut self, path: &std::path::Path) {
        let result = if cfg!(target_os = "windows") {
            Command::new("explorer").arg(path).spawn()
        } else if cfg!(target_os = "macos") {
            Command::new("open").arg(path).spawn()
        } else {
            Command::new("xdg-open").arg(path).spawn()
        };
        match result {
            Ok(mut child) => {
                thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => self.error = Some(format!("Could not open {}: {e}", path.display())),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(Duration::from_millis(250));
        egui::Panel::top("header").exact_size(88.0).show(ui, |ui| {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("NEELEMANET").size(24.0).strong().color(GREEN));
                    ui.label(
                        RichText::new("A good place to get lost.")
                            .size(12.0)
                            .color(MUTED),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(18.0);
                    if ui.button("Settings").clicked() {
                        self.settings = !self.settings;
                    }
                    let label = if self.accounts.is_empty() {
                        "Sign in with Microsoft"
                    } else {
                        "Manage account"
                    };
                    if ui
                        .add_enabled(!self.busy, egui::Button::new(label))
                        .clicked()
                    {
                        self.start(Job::Login, &ctx);
                    }
                    if !self.accounts.is_empty() {
                        egui::ComboBox::from_id_salt("profile")
                            .selected_text(&self.profile)
                            .show_ui(ui, |ui| {
                                for name in &self.accounts {
                                    ui.selectable_value(&mut self.profile, name.clone(), name);
                                }
                            });
                    }
                });
            });
        });

        egui::Panel::bottom("status").min_size(85.0).show(ui, |ui| {
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(18.0);
                if self.busy {
                    spinner(ui);
                } else {
                    ui.label(RichText::new("Ready").color(GREEN));
                }
                ui.label(&self.status);
            });
            if let Some(progress) = self.progress {
                ui.add(egui::ProgressBar::new(progress.clamp(0.0, 1.0)).show_percentage());
            }
            if let Some(error) = &self.error {
                ui.colored_label(Color32::from_rgb(255, 164, 145), error);
            }
            if let Some(warning) = &self.warning {
                ui.colored_label(Color32::from_rgb(235, 200, 126), warning);
            }
        });

        egui::Panel::left("library")
            .exact_size(248.0)
            .show(ui, |ui| {
                ui.add_space(24.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("YOUR PACKS").size(12.0).color(MUTED).strong());
                    if ui
                        .add_enabled(!self.busy, egui::Button::new("Refresh").small())
                        .clicked()
                    {
                        self.start(Job::Refresh, &ctx);
                    }
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Find a pack…")
                        .desired_width(220.0),
                );
                ui.add_space(8.0);
                if let Some(catalog) = &self.catalog {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let mut found = false;
                        for (i, pack) in catalog.packs.iter().enumerate() {
                            if !pack
                                .name
                                .to_lowercase()
                                .contains(&self.search.to_lowercase())
                            {
                                continue;
                            }
                            found = true;
                            let text =
                                format!("{}\nv{} · {}", pack.name, pack.version, pack.minecraft);
                            if ui
                                .add_sized(
                                    [220.0, 70.0],
                                    egui::Button::new(text).selected(i == self.selected),
                                )
                                .clicked()
                            {
                                self.selected = i;
                            }
                        }
                        if !found {
                            ui.label("No matching packs.");
                        }
                    });
                } else {
                    ui.label("Loading your pack library…");
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(26.0);
                if let Some(pack) = self.catalog.as_ref().and_then(|c| c.packs.get(self.selected)).cloned() {
                    egui::Frame::new().fill(SURFACE).inner_margin(28.0).corner_radius(16.0).show(ui, |ui| {
                        ui.set_min_width((ui.available_width() - 8.0).max(0.0));
                        ui.label(RichText::new(&pack.name).size(44.0).strong());
                        ui.add_space(6.0);
                        ui.label(RichText::new(&pack.description).size(17.0).color(MUTED));
                        ui.add_space(22.0);
                        ui.horizontal_wrapped(|ui| {
                            for tag in [format!("Minecraft {}", pack.minecraft), pack.loader.clone(), format!("v{}", pack.version)] {
                                egui::Frame::new().fill(Color32::from_rgb(39, 49, 42)).inner_margin(8.0).corner_radius(6.0).show(ui, |ui| { ui.label(tag); });
                            }
                        });
                        ui.add_space(28.0);
                        let current = launcher::current_install(&self.paths, &pack, &self.source).ok().flatten();
                        let installed = current.is_some();
                        let update_available = current.as_ref().is_some_and(|i| i.catalog_changed(&pack));
                        ui.horizontal(|ui| {
                            let title = if update_available { "Update & play" } else if installed { "Play now" } else { "Install & play" };
                            let button = egui::Button::new(RichText::new(title).size(18.0).strong().color(Color32::from_rgb(16, 32, 20))).fill(GREEN);
                            if ui.add_enabled(!self.busy, button.min_size(egui::vec2(195.0, 52.0))).clicked() { self.start(Job::Play(self.selected), &ctx); }
                            if !installed && ui.add_enabled(!self.busy, egui::Button::new("Install only")).clicked() { self.start(Job::Install(self.selected), &ctx); }
                            if installed && ui.add_enabled(!self.busy, egui::Button::new("Update pack")).clicked() { self.start(Job::Update(self.selected), &ctx); }
                            if let Some(current) = &current
                                && ui.button("Pack folder").clicked() {
                                self.folder(&current.root(&self.paths));
                            }
                        });
                        ui.add_space(8.0);
                        let detail = if update_available {
                            format!("Update available: {} to {}. Your worlds and personal settings carry over.", current.as_ref().unwrap().pack.version, pack.version)
                        } else if installed { "Play checks for updates automatically. Update pack forces a fresh download.".into() }
                        else { "We'll download the pack and set everything up for you.".into() };
                        ui.label(RichText::new(detail).size(12.0).color(MUTED));
                    });
                    ui.add_space(24.0);
                    ui.label(RichText::new("Less setup. More Minecraft.").size(21.0).strong());
                    ui.add_space(4.0);
                    ui.label("Your launch engine, Java, and mods are handled automatically. Sign in with a Microsoft account that owns Minecraft: Java Edition, then you're ready to go.");
                    ui.add_space(12.0);
                    ui.label(RichText::new(format!("Memory allowance: {} GiB", pack.memory_mb as f32 / 1024.0)).color(MUTED));
                    if let Some(server) = &pack.server { ui.label(RichText::new(format!("Join the server automatically: {server}")).color(GREEN)); }
                } else {
                    ui.heading("Welcome to NeelemaNet");
                    ui.label("Your packs will appear here. Use Refresh to retry, or Settings to change the pack list location.");
                }
            });
        });

        if self.settings {
            egui::Window::new("Launcher settings").resizable(false).default_width(540.0).show(&ctx, |ui| {
                ui.label("Pack list URL or packs.toml path");
                ui.add(egui::TextEdit::singleline(&mut self.catalog_input).desired_width(500.0));
                if ui.add_enabled(!self.busy, egui::Button::new("Use this pack list")).clicked() {
                    match Source::parse(self.catalog_input.trim()) {
                        Ok(source) => { self.source = source; self.catalog = None; self.start(Job::Refresh, &ctx); }
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
                ui.label(RichText::new("This override applies to this session. Release builds use the repository's pack list automatically.").small().color(MUTED));
                ui.separator();
                ui.label(format!("Data folder: {}", self.paths.root.display()));
                if ui.button("Open data folder").clicked() { self.folder(&self.paths.root.clone()); }
                ui.label("Updates carry over your worlds and personal settings. Previous installations stay in prism/instances as backups.");
                ui.hyperlink_to("Powered by Prism Launcher", "https://prismlauncher.org/");
            });
        }
    }
}

/// Like `ui.spinner()`, but repaints at a fixed 20 fps. The stock spinner repaints every
/// frame, which pins a CPU core when vsync is off.
fn spinner(ui: &mut egui::Ui) {
    let size = ui.spacing().interact_size.y;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let time = ui.ctx().input(|i| i.time);
    let start = (time * std::f64::consts::TAU) as f32;
    let radius = size / 2.0 - 2.0;
    let points: Vec<egui::Pos2> = (0..=16)
        .map(|n| {
            let angle = start + n as f32 / 16.0 * std::f32::consts::PI * 1.5;
            rect.center() + radius * egui::vec2(angle.cos(), angle.sin())
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(points, egui::Stroke::new(3.0, GREEN)));
    ui.ctx().request_repaint_after(Duration::from_millis(50));
}

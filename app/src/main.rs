//! ISO2DSD: converts SACD disc images (.iso) into DSD audio files by running the converter program.

mod convert;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::thread;

use eframe::egui::{self, Align2, Color32, RichText, Stroke, Vec2, ViewportCommand};
use serde::{Deserialize, Serialize};

use convert::{build_args, program_name, Channels, Format, Job, Log, Msg, Options, Stop};
use std::ffi::OsStr;
use std::fs;

const ACCENT: Color32 = Color32::from_rgb(31, 92, 176);
const PAGE: Color32 = Color32::from_rgb(241, 244, 249);
const CARD_EDGE: Color32 = Color32::from_rgb(218, 225, 235);
const MUTED: Color32 = Color32::from_rgb(105, 115, 133);
const WARN: Color32 = Color32::from_rgb(176, 96, 20);

/// Where "Check for Updates" opens: the project's website.
const UPDATE_PAGE: &str = "https://www.sonore.us/iso2dsd.html";

/// The screenshot option waits this many frames before taking the picture.
const SCREENSHOT_AFTER_FRAMES: u32 = 120;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
enum InputMode {
    #[default]
    Files,
    Server,
}

/// Remembered between runs.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Settings {
    input_mode: InputMode,
    options: Options,
    octets: [String; 4],
    port: String,
    last_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input_mode: InputMode::Files,
            options: Options::default(),
            octets: std::array::from_fn(|_| "0".to_string()),
            port: "2002".to_string(),
            last_dir: None,
        }
    }
}

struct App {
    s: Settings,
    files: Vec<PathBuf>,
    log: Log,
    rx: Option<Receiver<Msg>>,
    help: Option<String>,
    notice: Option<(String, String)>,
    converter_found: bool,
    frames: u32,
    shot: Option<PathBuf>,
    stop: Stop,
    /// Jobs waiting for the user to confirm they may replace existing files, with those names.
    pending: Option<(Vec<Job>, Vec<String>)>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, shot: Option<PathBuf>) -> Self {
        apply_style(&cc.egui_ctx);
        let s = cc
            .storage
            .and_then(|st| eframe::get_value(st, "settings"))
            .unwrap_or_default();
        Self {
            s,
            files: Vec::new(),
            log: Log::default(),
            rx: None,
            help: None,
            notice: None,
            converter_found: app_dir().join(program_name()).exists(),
            frames: 0,
            shot,
            stop: Stop::default(),
            pending: None,
        }
    }

    fn busy(&self) -> bool {
        self.rx.is_some()
    }

    fn add_files(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            if is_iso(&p) && !self.files.contains(&p) {
                self.files.push(p);
            }
        }
    }

    fn server_address(&self) -> String {
        server_address(&self.s.octets, &self.s.port)
    }

    fn start_convert(&mut self) {
        let opts = self.s.options.clone();
        let jobs: Vec<Job> = match self.s.input_mode {
            InputMode::Files => {
                if self.files.is_empty() {
                    self.notice = Some((
                        "No file selected".into(),
                        "Please select a file(s) to convert.".into(),
                    ));
                    return;
                }
                // Each file's output goes next to the file itself.
                self.files
                    .iter()
                    .map(|f| Job {
                        work_dir: f.parent().map(Path::to_path_buf).unwrap_or_else(app_dir),
                        args: build_args(&opts, f.as_os_str()),
                        source: f
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    })
                    .collect()
            }
            InputMode::Server => {
                let address = self.server_address();
                vec![Job {
                    work_dir: app_dir(),
                    args: build_args(&opts, OsStr::new(&address)),
                    source: address,
                }]
            }
        };
        if self.s.input_mode == InputMode::Files {
            if let Some(message) = space_shortfall(&self.files) {
                self.notice = Some(("Not enough free space".into(), message));
                return;
            }
            let existing = existing_outputs(&self.files);
            if !existing.is_empty() {
                self.pending = Some((jobs, existing));
                return;
            }
        }
        self.launch(jobs);
    }

    fn launch(&mut self, jobs: Vec<Job>) {
        self.log.clear();
        let (tx, rx) = channel();
        let stop = Stop::default();
        self.stop = stop.clone();
        let dir = app_dir();
        thread::spawn(move || convert::run_jobs(dir, jobs, stop, tx));
        self.rx = Some(rx);
    }

    fn start_help(&mut self) {
        let (tx, rx) = channel();
        let dir = app_dir();
        thread::spawn(move || convert::run_help(dir, tx));
        self.rx = Some(rx);
    }

    fn poll_worker(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut finished = false;
        loop {
            match rx.try_recv() {
                Ok(Msg::Output(line)) => self.log.push(&line),
                Ok(Msg::Start { index, total, name }) => self.log.start_file(index, total, &name),
                Ok(Msg::Help(text)) => self.help = Some(text),
                Ok(Msg::Done) => {
                    finished = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            self.rx = None;
            self.log.progress = None;
        }
    }

    fn draw_header(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::none()
                    .fill(ACCENT)
                    .inner_margin(egui::Margin::symmetric(18.0, 12.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    wave_icon(ui);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("ISO2DSD")
                                .size(22.0)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.label(
                            RichText::new("Turn SACD disc images into DSD audio files")
                                .color(Color32::from_rgb(205, 223, 246)),
                        );
                    });
                });
            });
    }

    fn draw_footer(&mut self, ctx: &egui::Context) {
        let busy = self.busy();
        egui::TopBottomPanel::bottom("footer")
            .frame(
                egui::Frame::none()
                    .fill(PAGE)
                    .inner_margin(egui::Margin::symmetric(16.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Help")).clicked() {
                        self.start_help();
                    }
                    if ui.button("Check for Updates").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(UPDATE_PAGE));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Quit").clicked() {
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                        ui.label(
                            RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).color(MUTED),
                        );
                        if self.converter_found {
                            ui.label(RichText::new("Converter ready").color(MUTED));
                        } else {
                            ui.label(
                                RichText::new(format!(
                                    "Converter {} not found next to this program",
                                    program_name().trim_start_matches("./")
                                ))
                                .color(WARN),
                            );
                        }
                    });
                });
            });
    }

    fn draw_options(&mut self, ctx: &egui::Context) {
        let busy = self.busy();
        egui::SidePanel::right("options")
            .exact_width(268.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(PAGE)
                    .inner_margin(egui::Margin::symmetric(14.0, 14.0)),
            )
            .show(ctx, |ui| {
                card(ui, |ui| {
                    ui.label(RichText::new("Options").size(16.0).strong());
                    ui.add_space(8.0);
                    ui.label("Channel mode");
                    egui::ComboBox::from_id_salt("channel_mode")
                        .selected_text(self.s.options.channels.label())
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for c in Channels::ALL {
                                ui.selectable_value(&mut self.s.options.channels, c, c.label());
                            }
                        });
                    ui.add_space(6.0);
                    ui.label("Output format");
                    egui::ComboBox::from_id_salt("output_format")
                        .selected_text(self.s.options.format.label())
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for f in Format::ALL {
                                ui.selectable_value(&mut self.s.options.format, f, f.label());
                            }
                        });
                    ui.add_space(4.0);
                    ui.checkbox(&mut self.s.options.dst_to_dsd, "Convert DST to DSD")
                        .on_hover_text("Converts DST-compressed tracks to plain DSD.");
                    ui.checkbox(&mut self.s.options.cue_sheet, "Write a cue sheet")
                        .on_hover_text("Also writes a cue sheet file for each disc.");
                    ui.checkbox(&mut self.s.options.print, "Print disc information")
                        .on_hover_text("Asks the converter to print disc information.");
                    ui.add_space(14.0);
                    let label = if busy { "Converting..." } else { "Convert" };
                    let button = egui::Button::new(
                        RichText::new(label)
                            .size(16.0)
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(ACCENT)
                    .min_size(Vec2::new(ui.available_width(), 40.0));
                    if ui.add_enabled(!busy, button).clicked() {
                        self.start_convert();
                    }
                    if busy {
                        ui.add_space(6.0);
                        let stop = egui::Button::new(RichText::new("Stop").strong())
                            .min_size(Vec2::new(ui.available_width(), 32.0));
                        if ui
                            .add(stop)
                            .on_hover_text("Stops the current file and skips the rest.")
                            .clicked()
                        {
                            self.stop.request();
                        }
                    }
                });
            });
    }

    fn draw_input(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.s.input_mode, InputMode::Files, "File input");
                ui.selectable_value(&mut self.s.input_mode, InputMode::Server, "Server input");
            });
            ui.add_space(8.0);
            match self.s.input_mode {
                InputMode::Files => self.draw_files(ui),
                InputMode::Server => self.draw_server(ui),
            }
        });
    }

    fn draw_files(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy(), egui::Button::new("Add ISO files..."))
                .clicked()
            {
                let mut dialog = rfd::FileDialog::new().add_filter("ISO image", &["iso"]);
                if let Some(dir) = &self.s.last_dir {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(picked) = dialog.pick_files() {
                    if let Some(parent) = picked.first().and_then(|p| p.parent()) {
                        self.s.last_dir = Some(parent.to_path_buf());
                    }
                    self.add_files(picked);
                }
            }
            if ui
                .add_enabled(!self.files.is_empty(), egui::Button::new("Clear"))
                .clicked()
            {
                self.files.clear();
            }
            ui.label(RichText::new(format!("{} selected", self.files.len())).color(MUTED));
        });
        ui.add_space(6.0);
        if self.files.is_empty() {
            ui.label(RichText::new("Drop .iso files here, or click Add ISO files.").color(MUTED));
            return;
        }
        let mut remove = None;
        egui::ScrollArea::vertical()
            .max_height(120.0)
            .show(ui, |ui| {
                for (i, f) in self.files.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                        let name = f.file_name().map(|n| n.to_string_lossy().into_owned());
                        ui.label(name.unwrap_or_default());
                        let folder = f.parent().map(|p| p.display().to_string());
                        ui.label(
                            RichText::new(folder.unwrap_or_default())
                                .small()
                                .color(MUTED),
                        );
                    });
                }
            });
        if let Some(i) = remove {
            self.files.remove(i);
        }
    }

    fn draw_server(&mut self, ui: &mut egui::Ui) {
        for o in self.s.octets.iter_mut() {
            keep_digits(o, 3);
        }
        keep_digits(&mut self.s.port, 5);
        ui.horizontal(|ui| {
            ui.label("Server address");
            for (i, o) in self.s.octets.iter_mut().enumerate() {
                ui.add(egui::TextEdit::singleline(o).desired_width(38.0));
                if i < 3 {
                    ui.label(".");
                }
            }
            ui.label(":");
            ui.add(egui::TextEdit::singleline(&mut self.s.port).desired_width(56.0));
        });
        ui.label(RichText::new("Each part is 0 to 255. Port 2002 is the default.").color(MUTED));
    }

    fn draw_log(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Output").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Clear").clicked() {
                        self.log.clear();
                    }
                });
            });
            ui.separator();
            if let Some(p) = self.log.progress {
                ui.add(egui::ProgressBar::new(p).show_percentage());
                ui.add_space(4.0);
            }
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if self.log.lines.is_empty() {
                        ui.label(
                            RichText::new("Nothing yet. Choose files and press Convert.")
                                .color(MUTED),
                        );
                    }
                    for line in &self.log.lines {
                        ui.label(RichText::new(line).monospace());
                    }
                });
        });
    }

    fn draw_dialogs(&mut self, ctx: &egui::Context) {
        let mut close_notice = false;
        if let Some((title, text)) = &self.notice {
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(text);
                    if ui.button("OK").clicked() {
                        close_notice = true;
                    }
                });
        }
        if close_notice {
            self.notice = None;
        }

        let mut choice = None;
        if let Some((_, names)) = &self.pending {
            egui::Window::new("Replace existing files?")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("These files may be replaced by this conversion:");
                    for name in names.iter().take(10) {
                        ui.label(RichText::new(name).monospace());
                    }
                    if names.len() > 10 {
                        ui.label(format!("...and {} more", names.len() - 10));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Convert anyway").clicked() {
                            choice = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(false);
                        }
                    });
                });
        }
        match choice {
            Some(true) => {
                if let Some((jobs, _)) = self.pending.take() {
                    self.launch(jobs);
                }
            }
            Some(false) => self.pending = None,
            None => {}
        }

        let mut open = true;
        if let Some(text) = &self.help {
            egui::Window::new("Help")
                .open(&mut open)
                .default_size([640.0, 420.0])
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.label(RichText::new(text).monospace());
                        });
                });
        }
        if !open {
            self.help = None;
        }
    }

    /// Drag and drop: .iso files dropped on the window join the file list.
    fn handle_drops(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.s.input_mode = InputMode::Files;
            self.add_files(dropped);
        }
    }

    /// --screenshot <file.png>: draw a while, save the window as a PNG, then exit.
    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(path) = self.shot.clone() else {
            return;
        };
        self.frames += 1;
        if self.frames == SCREENSHOT_AFTER_FRAMES {
            ctx.send_viewport_cmd(ViewportCommand::Screenshot);
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let saved = image::save_buffer(
                &path,
                image.as_raw(),
                w as u32,
                h as u32,
                image::ColorType::Rgba8,
            );
            match saved {
                Ok(()) => {
                    println!("saved {}", path.display());
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("could not save {}: {e}", path.display());
                    std::process::exit(1);
                }
            }
        }
        if self.frames > SCREENSHOT_AFTER_FRAMES + 600 {
            eprintln!("no picture arrived from the window");
            std::process::exit(1);
        }
        ctx.request_repaint();
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();
        self.handle_drops(ctx);
        self.draw_header(ctx);
        self.draw_footer(ctx);
        self.draw_options(ctx);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(PAGE)
                    .inner_margin(egui::Margin::symmetric(14.0, 14.0)),
            )
            .show(ctx, |ui| {
                self.draw_input(ui);
                ui.add_space(12.0);
                self.draw_log(ui);
            });
        self.draw_dialogs(ctx);
        self.screenshot_step(ctx);
        if self.busy() {
            ctx.request_repaint();
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "settings", &self.s);
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let shot = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([850.0, 650.0])
            .with_min_inner_size([720.0, 540.0])
            .with_title("ISO2DSD"),
        ..Default::default()
    };
    eframe::run_native(
        "ISO2DSD",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, shot)))),
    )
}

fn apply_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = PAGE;
    visuals.window_fill = Color32::WHITE;
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.hyperlink_color = ACCENT;
    for w in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
    ] {
        w.rounding = egui::Rounding::same(5.0);
    }
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(190, 200, 215));
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(232, 238, 247);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(232, 238, 247);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(10.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    ctx.set_style(style);
}

/// A white rounded panel with a thin edge.
fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(Color32::WHITE)
        .rounding(12.0)
        .stroke(Stroke::new(1.0_f32, CARD_EDGE))
        .inner_margin(14.0)
        .show(ui, add);
}

/// A small waveform mark drawn with shapes (no artwork copied).
fn wave_icon(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(44.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 10.0, Color32::WHITE);
    for (i, h) in [10.0_f32, 22.0, 30.0, 16.0, 34.0, 20.0, 12.0]
        .iter()
        .enumerate()
    {
        let x = rect.left() + 8.0 + i as f32 * 4.6;
        let bar = egui::Rect::from_center_size(egui::pos2(x, rect.center().y), Vec2::new(2.6, *h));
        ui.painter().rect_filled(bar, 1.5, ACCENT);
    }
}

/// Keeps only digits, at most `max` of them.
fn keep_digits(s: &mut String, max: usize) {
    s.retain(|c| c.is_ascii_digit());
    s.truncate(max);
}

/// The address the server input uses: four parts clamped to 0..=255, and the port if one is given.
fn server_address(octets: &[String; 4], port: &str) -> String {
    let ip = octets
        .iter()
        .map(|o| o.parse::<u32>().unwrap_or(0).min(255).to_string())
        .collect::<Vec<_>>()
        .join(".");
    if port.is_empty() {
        ip
    } else {
        format!("{ip}:{port}")
    }
}

/// Checks that each output folder has room for the ISO files written into it.
/// The output is about the same size as the input, so the ISO sizes are the estimate.
fn space_shortfall(files: &[PathBuf]) -> Option<String> {
    let mut needed: Vec<(PathBuf, u64)> = Vec::new();
    for f in files {
        let dir = f.parent().map(Path::to_path_buf).unwrap_or_else(app_dir);
        let size = fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        match needed.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, total)) => *total += size,
            None => needed.push((dir, size)),
        }
    }
    for (dir, need) in needed {
        if let Ok(free) = fs2::available_space(&dir) {
            if free < need {
                return Some(format!(
                    "{} needs about {} free for the output, but only {} is available.",
                    dir.display(),
                    gigabytes(need),
                    gigabytes(free)
                ));
            }
        }
    }
    None
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
}

/// Files already in an output folder whose names start with an ISO's name.
/// The converter's output names are not known for certain, so this is a conservative check.
fn existing_outputs(files: &[PathBuf]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for f in files {
        let (Some(dir), Some(stem)) = (f.parent(), f.file_stem()) else {
            continue;
        };
        let stem = stem.to_string_lossy().into_owned();
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_iso(&path) && name.starts_with(&stem) && path.is_file() && !found.contains(&name)
            {
                found.push(name);
            }
        }
    }
    found.sort();
    found
}

fn is_iso(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("iso"))
}

/// The folder this program runs from; the converter is looked for here.
fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_address_clamps_parts_and_adds_the_port() {
        let octets = ["999".to_string(), "0".into(), "0".into(), "1".into()];
        assert_eq!(server_address(&octets, ""), "255.0.0.1");
        assert_eq!(server_address(&octets, "2002"), "255.0.0.1:2002");
        let empty: [String; 4] = std::array::from_fn(|_| String::new());
        assert_eq!(server_address(&empty, "9"), "0.0.0.0:9");
    }

    #[test]
    fn keep_digits_drops_other_characters_and_limits_length() {
        let mut s = "a1b22345".to_string();
        keep_digits(&mut s, 3);
        assert_eq!(s, "122");
    }

    #[test]
    fn only_iso_files_are_accepted() {
        assert!(is_iso(Path::new("Disc.ISO")));
        assert!(!is_iso(Path::new("notes.iso.txt")));
    }
}

use aaoffline::MAX_STEPS;
use egui::{
    Align, Color32, Label, ProgressBar, RichText, ScrollArea, TextFormat, Theme, text::LayoutJob,
};
use log::{error, info};

use crate::{
    args::GuiArgs,
    messenger::{GuiMessenger, ProgressMessage},
};

const ENABLED_CATEGORIES: [&str; 13] = [
    "aaoffline",
    "aaoffline::args",
    "aaoffline::data",
    "aaoffline::data::case",
    "aaoffline::data::player",
    "aaoffline::data::site",
    "aaoffline::download",
    "aaoffline::fs",
    "aaoffline::middleware",
    "aaoffline::transform",
    "aaoffline_gui::messenger",
    "aaoffline_gui::app",
    "aaoffline_gui::args",
];

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum DownloadState {
    /// No download started yet.
    #[default]
    NoneStarted = 0,
    /// Last download succeeded.
    LastSuccess = 1,
    /// Last download failed.
    LastFail = 2,
    /// Download is active right now.
    Active = 3,
}

#[derive(Default, Debug)]
pub(crate) struct AaofflineApp {
    messenger: GuiMessenger,
    args: GuiArgs,
    download_state: DownloadState,
    /// The current step as should be shown to the user (step number, message).
    current_step: Option<(u8, String)>,
    current_progress: Option<Progress>,
    enable_force_quit: bool,
}

#[derive(Debug)]
struct Progress {
    progress: u64,
    max: u64,
}

impl Progress {
    fn new(max: u64) -> Self {
        Self { progress: 0, max }
    }

    #[allow(clippy::cast_precision_loss)]
    fn percent(&self) -> f32 {
        self.progress as f32 / self.max as f32
    }
}

impl AaofflineApp {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(Theme::Dark);

        AaofflineApp {
            args: GuiArgs::new(),
            ..AaofflineApp::default()
        }
    }

    fn download(&mut self) {
        egui_logger::clear_logs();
        self.download_state = DownloadState::Active;
        self.current_step = None;
        match self.args.clone().try_into() {
            Ok(args) => self.messenger.run(args),
            Err(e) => error!("{e}"),
        }
    }

    fn handle_messages(&mut self) {
        if let Some(msg) = self.messenger.receive() {
            match msg {
                crate::messenger::GuiMessage::Done(true) => {
                    self.download_state = DownloadState::LastSuccess;
                }
                crate::messenger::GuiMessage::Done(false) => {
                    self.download_state = DownloadState::LastFail;
                }
                crate::messenger::GuiMessage::NextStep { step, text } => {
                    self.current_step = Some((step, text));
                }
                crate::messenger::GuiMessage::Progress(ProgressMessage::Clear) => {
                    self.current_progress = None;
                }
                crate::messenger::GuiMessage::Progress(ProgressMessage::Inc(delta)) => {
                    self.current_progress
                        .as_mut()
                        .expect("progress must be set up here")
                        .progress += delta;
                }
                crate::messenger::GuiMessage::Progress(ProgressMessage::IncLength(delta)) => {
                    self.current_progress
                        .as_mut()
                        .expect("progress must be set up here")
                        .max += delta;
                }
                crate::messenger::GuiMessage::Progress(ProgressMessage::New(length)) => {
                    self.current_progress = Some(Progress::new(length));
                }
                crate::messenger::GuiMessage::Progress(ProgressMessage::Finish(msg)) => {
                    info!("{msg}");
                    self.current_progress = None;
                }
            }
        }
    }
}

impl eframe::App for AaofflineApp {
    /// Called each time the UI needs repainting, which may be many times per second.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_messages();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                let header = RichText::new("aaoffline")
                    .text_style(egui::TextStyle::Monospace)
                    .size(24.0);
                ui.add(Label::new(header));
                ui.label("Download Ace Attorney Online cases to be playable offline.");
            });

            ui.separator();

            egui::containers::Frame::new()
                .fill(ui.style().visuals.window_fill)
                .show(ui, |ui| {
                    egui::SidePanel::left("arg_panel")
                        .resizable(true)
                        .min_width(200.0)
                        .default_width(250.0)
                        .max_width(310.0)
                        .show_inside(ui, |ui| {
                            ui.set_max_height(ui.available_height() - 150.0);
                            ScrollArea::vertical().show(ui, |ui| {
                                ui.heading("Options");
                                if self.args.clicked_download(
                                    ui,
                                    self.download_state >= DownloadState::Active,
                                ) {
                                    self.download();
                                }
                            });
                        });
                    egui::CentralPanel::default().show_inside(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down(Align::Min), |ui| {
                            if self.download_state <= DownloadState::NoneStarted {
                                ui.disable();
                            }
                            ui.heading("Download Status");
                            match self.download_state {
                                DownloadState::NoneStarted => {
                                    ui.label("Download not yet started.");
                                }
                                DownloadState::LastSuccess => {
                                    ui.label(
                                        RichText::new("✔ Download completed (see log output).")
                                            .strong()
                                            .color(Color32::GREEN),
                                    );
                                }
                                DownloadState::LastFail => {
                                    ui.label(
                                        RichText::new("🗙 Download failed (see log output).")
                                            .strong()
                                            .color(Color32::RED),
                                    );
                                }
                                DownloadState::Active => {
                                    ui.group(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.spinner();
                                            if let Some(step) = self.current_step.as_ref() {
                                                let text = step_text(ui, step.0, &step.1);
                                                ui.vertical(|ui| {
                                                    ui.label(text);
                                                    if let Some(progress) =
                                                        self.current_progress.as_ref()
                                                    {
                                                        ui.add(
                                                            ProgressBar::new(progress.percent())
                                                                .corner_radius(2.0)
                                                                .show_percentage(),
                                                        );
                                                    }
                                                });
                                            } else {
                                                ui.label("Starting download...");
                                            }
                                        });
                                    });
                                }
                            }

                            ui.separator();

                            if self.download_state > DownloadState::NoneStarted {
                                ui.heading("Log Output");
                                let mut logger = egui_logger::logger_ui()
                                    .enable_regex(false)
                                    .enable_ctx_menu(false)
                                    .show_target(false)
                                    .include_target(false)
                                    .log_levels([true, true, true, false, false])
                                    .enable_log_count(false)
                                    .enable_search(false)
                                    .enable_max_log_output(false)
                                    .enable_categories_button(false)
                                    .enable_time_button(false)
                                    .max_log_length(50_000);
                                for category in ENABLED_CATEGORIES {
                                    logger = logger.enable_category(category, true);
                                }
                                logger.show(ui);
                            }
                        });

                        ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
                            latest_release_message(ui);
                            if self.download_state >= DownloadState::Active {
                                if self.enable_force_quit {
                                    if ui.button("Really force quit?").clicked() {
                                        std::process::exit(1);
                                    }
                                } else if ui.button("Force Quit").clicked() {
                                    self.enable_force_quit = true;
                                }
                            }
                        });
                    });
                });
        });
    }
}

fn step_text(ui: &egui::Ui, step: u8, text: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        &format!("[Step {step}/{MAX_STEPS}]"),
        0.0,
        TextFormat {
            ..Default::default()
        },
    );
    job.append(
        text,
        8.0,
        TextFormat {
            color: ui.visuals().strong_text_color(),
            ..Default::default()
        },
    );
    job
}

fn latest_release_message(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(".");
        ui.hyperlink_to(
            "GitHub",
            "https://github.com/falko17/aaoffline/releases/latest",
        );
        ui.add_space(3.0);
        ui.label("Latest aaoffline release available at");
        ui.add_space(4.0);
        egui::warn_if_debug_build(ui);
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{}.", env!("CARGO_PKG_VERSION"))).strong());
        ui.add_space(3.0);
        ui.label("Current version:");
    });
}

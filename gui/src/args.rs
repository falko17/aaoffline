use std::{collections::HashSet, path::PathBuf};

use aaoffline::args::{Args, DownloadSequence, HttpHandling, SequenceErrorHandling, Userscripts};
use egui::{Button, Checkbox, CollapsingHeader, Slider, TextEdit, Widget, vec2};
use egui_form::{
    Form, FormField,
    garde::{GardeReport, field_path},
};
use garde::Validate;
use log::LevelFilter;
use rfd::FileDialog;

/// Arguments that configure how aaoffline runs.
#[derive(Debug, Clone, Default, Validate)]
#[allow(clippy::struct_excessive_bools)]
#[garde(allow_unvalidated)]
pub(crate) struct GuiArgs {
    /// The URLs/IDs of the cases that shall be downloaded.
    #[garde(length(min = 1), inner(length(min = 1), custom(Self::validate_case)))]
    pub(crate) cases: Vec<String>,

    /// The output directory (or filename, if `-1` was used) for the case.
    ///
    /// If this is not passed, will use the title + ID of the case.
    /// It multiple cases are downloaded, they will all be put under this directory (which, by
    /// default, will be the current directory).
    #[garde(custom(Self::validate_directory))]
    pub(crate) output: Option<PathBuf>,

    /// The branch or commit name of Ace Attorney Online that shall be used for the player.
    #[garde(length(min = 1))]
    pub(crate) player_version: String,

    /// The language to download the player in.
    #[garde(length(min = 2))]
    pub(crate) language: String,

    /// Whether to continue when an asset for the case could not be downloaded.
    pub(crate) continue_on_asset_error: bool,

    /// Whether to replace any existing output files.
    pub(crate) replace_existing: bool,

    /// Whether to download all trials contained in a sequence (if the given case is part of a
    /// sequence).
    pub(crate) sequence: DownloadSequence,

    /// Whether to output only a single HTML file, with the assets embedded as data URLs.
    pub(crate) one_html_file: bool,

    /// Whether to apply any userscripts to the downloaded case. Can be passed multiple times.
    ///
    /// Scripts were created by Time Axis, with only the expanded keyboard controls written by me,
    /// building on Time Axis' basic keyboard controls script.
    /// (These options may change in the future when some scripts are consolidated).
    pub(crate) with_userscripts: HashSet<Userscripts>,

    /// How many concurrent downloads to use.
    #[garde(range(min = 1))]
    pub(crate) concurrent_downloads: usize,

    /// How to handle cases in a sequence that aren't accessible.
    pub sequence_error_handling: SequenceErrorHandling,

    /// How many times to retry downloads if they fail.
    ///
    /// Note that this is in addition to the first try, so a value of one will lead to two download
    /// attempts if the first one failed.
    #[garde(range(min = 1))]
    pub(crate) retries: u32,

    /// The maximum time to wait for the connect phase of network requests (in seconds).
    /// A value of 0 means that no timeout will be applied.
    pub(crate) connect_timeout: u64,

    /// The maximum time to wait for the read (i.e., download) phase of network requests
    /// (in seconds).
    /// A value of 0 means that no timeout will be applied.
    pub(crate) read_timeout: u64,

    /// How to handle insecure HTTP requests.
    pub(crate) http_handling: HttpHandling,

    /// Whether to disable the use of HTML5 audio for Howler.js.
    ///
    /// Enabling this will lead to CORS errors appearing in your browser's console when you open
    /// the HTML file locally, since it isn't allowed to access other files. Howler.js will then
    /// switch to HTML5 audio automatically. However, if you plan to use a local web server to
    /// open the player, it is recommended to enable this option, since those errors won't appear
    /// there (and there's a problem with how Firefox handles HTML5 audio, making this the better
    /// option if you plan to use Firefox.)
    pub(crate) disable_html5_audio: bool,

    /// Whether to disable the automatic fixing of photobucket watermarks.
    pub(crate) disable_photobucket_fix: bool,

    /// Partial URL pointing to a proxy that all requests should be routed through.
    ///
    /// The actual request URL will be appended to this parameter.
    /// For example, if this were set to `https://example.com/?proxy=`, then a request for
    /// `https://example.org/sample` would become `https://example.com/?proxy=https://example.org/sample`.
    pub(crate) proxy: String,
}

impl GuiArgs {
    pub(crate) fn new() -> Self {
        Self {
            cases: vec![String::new()],
            player_version: String::from("master"),
            language: String::from("en"),
            continue_on_asset_error: false,
            replace_existing: false,
            one_html_file: false,
            concurrent_downloads: 5,
            retries: 3,
            connect_timeout: 10,
            read_timeout: 30,
            disable_html5_audio: false,
            disable_photobucket_fix: false,
            sequence: DownloadSequence::Every,
            sequence_error_handling: SequenceErrorHandling::Continue,
            ..Default::default()
        }
    }

    #[allow(clippy::ref_option, clippy::trivially_copy_pass_by_ref)] // Generated by garde
    fn validate_case(value: &str, (): &()) -> garde::Result {
        Args::accept_case(value)
            .map(|_| ())
            .map_err(garde::Error::new)
    }

    #[allow(clippy::ref_option, clippy::trivially_copy_pass_by_ref)] // Generated by garde
    fn validate_directory(value: &Option<PathBuf>, (): &()) -> garde::Result {
        if let Some(result) = value
            .as_ref()
            .map(|x| std::fs::metadata(x).map_err(garde::Error::new))
        {
            let dir = result?;
            if !dir.is_dir() {
                Err(garde::Error::new("This is a file. I need a directory."))
            } else if dir.permissions().readonly() {
                Err(garde::Error::new("This directory is read-only."))
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    pub(crate) fn clicked_download(&mut self, ui: &mut egui::Ui, download_active: bool) -> bool {
        if download_active {
            ui.disable();
        }
        let mut form = Form::new().add_report(GardeReport::new(self.validate()));

        ui.vertical(|ui| {
            for (i, case) in self.cases.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    FormField::new(&mut form, field_path!("cases", i))
                        .label("Case ID / URL")
                        .ui(
                            ui,
                            TextEdit::singleline(case)
                                .hint_text("https://aaonline.fr/player.php?trial_id=..."),
                        );
                });
            }
            ui.horizontal(|ui| {
                if ui.button("Add case").clicked() {
                    self.cases.push(String::new());
                }
                if self.cases.len() > 1 && ui.button("Remove case").clicked() {
                    self.cases.pop();
                }
            });
            ui.add_space(10.0);
        });
        FormField::new(&mut form, field_path!("continue_on_asset_error"))
            .ui(
                ui,
                Checkbox::new(
                    &mut self.continue_on_asset_error,
                    "Continue on asset errors",
                ),
            )
            .on_hover_text(
                "Whether to continue anyway when an asset for the case could not be downloaded.",
            );

        FormField::new(&mut form, field_path!("replace_existing"))
            .ui(
                ui,
                Checkbox::new(&mut self.replace_existing, "Replace existing output"),
            )
            .on_hover_text("Whether to replace any existing output files.");

        FormField::new(&mut form, field_path!("one_html_file"))
            .ui(
                ui,
                Checkbox::new(&mut self.one_html_file, "Output single HTML file"),
            )
            .on_hover_text("Whether to output only a single HTML file (instead of a directory), with the assets embedded as data URLs.

WARNING: Browsers may not like HTML files very much that are multiple dozens of megabytes large. Your mileage may vary.");

        FormField::new(&mut form, field_path!("concurrent_downloads"))
            .label("Concurrent downloads")
            .ui(ui, Slider::new(&mut self.concurrent_downloads, 1..=10))
            .on_hover_text("How many parallel downloads to use.");

        ui.group(|ui| {
                ui.label("Sequence handling").on_hover_text("Whether to download all trials contained in a sequence (if the given case is part of a sequence).");
                ui.horizontal_wrapped(|ui| {
                ui.radio_value(
                    &mut self.sequence,
                    DownloadSequence::Every,
                    "Every case",
                ).on_hover_text("Automatically download every case in the sequence.");
                ui.radio_value(
                    &mut self.sequence,
                    DownloadSequence::Single,
                    "Single case",
                ).on_hover_text("Only download the cases that are passed.");
            });
        });

        if self.sequence != DownloadSequence::Single {
            ui.group(|ui| {
                ui.label("Sequence error handling")
                    .on_hover_text("What to do if a case in a sequence isn't accessible.");
                ui.horizontal_wrapped(|ui| {
                    ui.radio_value(
                        &mut self.sequence_error_handling,
                        SequenceErrorHandling::Continue,
                        "Continue",
                    )
                    .on_hover_text("Ignore the error and download the other cases.");
                    ui.radio_value(
                        &mut self.sequence_error_handling,
                        SequenceErrorHandling::Abort,
                        "Abort",
                    )
                    .on_hover_text("Stop the entire download.");
                });
            });
        }

        ui.group(|ui| {
            ui.label("Apply userscripts");
            let mut better_layout = self.with_userscripts.contains(&Userscripts::BetterLayout);
            let mut keyboard_controls = self
                .with_userscripts
                .contains(&Userscripts::KeyboardControls);
            let mut alt_nametag = self.with_userscripts.contains(&Userscripts::AltNametag);
            let mut backlog = self.with_userscripts.contains(&Userscripts::Backlog);
            let mut all = self.with_userscripts.contains(&Userscripts::All)
                || better_layout && keyboard_controls && alt_nametag && backlog;

            ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!all, |ui| {
                ui.checkbox(&mut better_layout, "Better Layout")
                    .on_hover_text(
                        "Improves the layout (e.g., enlarging and centering the main screens).",
                    );
                ui.checkbox(&mut keyboard_controls, "Keyboard Controls")
                    .on_hover_ui(|ui|{

                    ui.label("Adds extensive keyboard controls.");
                    ui.hyperlink_to(
                        "Click here for an overview of available controls.",
                        "https://gist.github.com/falko17/965207b1f1f0496ff5f0cb41d8e827f2#file-aaokeyboard-user-js-L10"
                    );});
                ui.checkbox(&mut alt_nametag, "Alt Nametag").on_hover_text("Changes the fonts of nametags to use a proper pixelized font.");
                ui.checkbox(&mut backlog, "Backlog").on_hover_text("Adds a backlog button to see past dialog.");
            });
            });
            ui.checkbox(&mut all, "All").on_hover_text("Apply all userscripts.");

            self.with_userscripts.clear();
            if all {
                self.with_userscripts.insert(Userscripts::All);
            } else {
                if better_layout {
                    self.with_userscripts.insert(Userscripts::BetterLayout);
                }
                if keyboard_controls {
                    self.with_userscripts.insert(Userscripts::KeyboardControls);
                }
                if alt_nametag {
                    self.with_userscripts.insert(Userscripts::AltNametag);
                }
                if backlog {
                    self.with_userscripts.insert(Userscripts::Backlog);
                }
            }
        });

        let to_single_file = self.one_html_file && self.cases.len() == 1;
        let component = if to_single_file { "file" } else { "directory" };
        ui.horizontal(|ui| {
            ui.label("Output path");
            if ui.button(format!("Select {component}")).clicked() {
                if to_single_file {
                    self.output = FileDialog::new()
                        .add_filter("HTML file", &[".html"])
                        .save_file();
                } else {
                    self.output = FileDialog::new().pick_folder();
                }
            }
        });
        let mut path_text = self.output.as_ref().and_then(|x| x.to_str()).unwrap_or("");
        FormField::new(&mut form, field_path!("output"))
            .ui(
                ui,
                TextEdit::singleline(&mut path_text)
                    .interactive(false)
                    .hint_text(format!("new {component} in current directory")),
            )
            .on_hover_text(format!(
                "The output {component} to which the downloaded case{} shall be written.",
                if self.cases.len() == 1 { "" } else { "s" }
            ));

        let response = ui.vertical_centered(|ui| {
            if download_active {
                ui.disable();
            }
            Button::new("Download")
                .min_size(vec2(ui.available_width(), 40.0))
                .ui(ui)
        });

        ui.add_space(20.0);

        CollapsingHeader::new("Advanced Options").show(ui, |ui| {

                FormField::new(&mut form, field_path!("player_version"))
                    .label("Player Version")
                    .ui(ui, TextEdit::singleline(&mut self.player_version).hint_text("master"))
                .on_hover_text("The branch or commit name of Ace Attorney Online that shall be used for the player.");

                FormField::new(&mut form, field_path!("language"))
                    .label("Player Language")
                    .ui(ui, TextEdit::singleline(&mut self.language).hint_text("en"))
                .on_hover_text("The desired language of the Ace Attorney Online player interface (not of the case itself!)");

                FormField::new(&mut form, field_path!("retries"))
                    .label("Network retries")
                    .ui(ui, Slider::new(&mut self.retries, 0..=10))
                    .on_hover_text("How many times to retry downloads if they fail.

        Note that this is in addition to the first try, so a value of one will lead to two download attempts if the first one failed.");

                FormField::new(&mut form, field_path!("connect_timeout"))
                    .label("Network connect timeout (seconds)")
                    .ui(ui, Slider::new(&mut self.connect_timeout, 0..=100))
                    .on_hover_text("The maximum time to wait for the connect phase of network requests (in seconds). A value of 0 means that no timeout will be applied.");

                FormField::new(&mut form, field_path!("read_timeout"))
                    .label("Network read timeout (seconds)")
                    .ui(ui, Slider::new(&mut self.read_timeout, 0..=300))
                    .on_hover_text("The maximum time to wait for the read (i.e., download) phase of network requests (in seconds). A value of 0 means that no timeout will be applied.");

                ui.group(|ui| {
                    ui.label("Insecure HTTP handling").on_hover_text("How to handle insecure HTTP requests.");
                    ui.horizontal_wrapped(|ui| {
                    ui.radio_value(
                        &mut self.http_handling,
                        HttpHandling::AllowInsecure,
                        "Allow insecure",
                    ).on_hover_text("Allow insecure HTTP requests.");
                    ui.radio_value(
                        &mut self.http_handling,
                        HttpHandling::Disallow,
                        "Only allow secure",
                    ).on_hover_text("Fail when an insecure HTTP request is encountered.");
                    ui.radio_value(&mut self.http_handling, HttpHandling::RedirectToHttps, "Redirect to secure").on_hover_text("Try redirecting insecure HTTP requests to HTTPS.");
                    });
                });


                FormField::new(&mut form, field_path!("disable_html5_audio"))
                    .ui(
                        ui,
                        Checkbox::new(&mut self.disable_html5_audio, "Disable HTML5 audio"),
                    )
                    .on_hover_text(
                        "Whether to disable the use of HTML5 audio for Howler.js.\n\nEnabling this will lead to CORS errors appearing in your browser's console when you open the HTML file locally, since it isn't allowed to access other files. Howler.js will then switch to HTML5 audio automatically. However, if you plan to use a local web server to open the player, it is recommended to enable this option, since those errors won't appear there (and there's a problem with how Firefox handles HTML5 audio, making this the better option if you plan to use Firefox.)",
                    );

                FormField::new(&mut form, field_path!("disable_photobucket_fix"))
                    .ui(
                        ui,
                        Checkbox::new(&mut !self.disable_photobucket_fix, "Enable photobucket fix"),
                    )
                    .on_hover_text("Whether to disable the automatic fixing of photobucket watermarks.");

                FormField::new(&mut form, field_path!("proxy"))
                    .label("HTTP Proxy URL")
                    .ui(
                        ui,
                        TextEdit::singleline(&mut self.proxy).hint_text("Leave empty to disable"),
                    ).on_hover_text("Partial URL pointing to a proxy that all requests should be routed through.\n\nThe actual request URL will be appended to this parameter. For example, if this were set to `https://example.com/?proxy=`, then a request for `https://example.org/sample` would become `https://example.com/?proxy=https://example.org/sample`.");
        });
        matches!(form.handle_submit(&response.inner, ui), Some(Ok(())))
    }
}

impl TryFrom<GuiArgs> for Args {
    type Error = String;

    fn try_from(value: GuiArgs) -> Result<Self, Self::Error> {
        let cases: Result<Vec<u32>, String> =
            value.cases.iter().map(|x| Self::accept_case(x)).collect();
        Ok(Args {
            cases: cases?,
            output: value.output,
            player_version: value.player_version,
            language: value.language,
            continue_on_asset_error: value.continue_on_asset_error,
            replace_existing: value.replace_existing,
            sequence: value.sequence,
            one_html_file: value.one_html_file,
            with_userscripts: value.with_userscripts.into_iter().collect(),
            concurrent_downloads: value.concurrent_downloads,
            retries: value.retries,
            connect_timeout: value.connect_timeout,
            read_timeout: value.read_timeout,
            http_handling: value.http_handling,
            disable_html5_audio: value.disable_html5_audio,
            disable_photobucket_fix: value.disable_photobucket_fix,
            proxy: Some(value.proxy).filter(|x| !x.is_empty()),
            log_level: LevelFilter::Debug,
            sequence_error_handling: value.sequence_error_handling,
        })
    }
}

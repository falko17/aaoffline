#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! A downloader for Ace Attorney Online cases that allows them to be played offline.

pub(crate) mod constants;
pub(crate) mod data;
pub(crate) mod download;
pub(crate) mod transform;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use colored::Colorize;
use constants::re;
use data::case::{Case, Sequence};
use data::player::Player;
use download::AssetDownloader;
use futures::future;
use human_panic::setup_panic;
use indicatif::{MultiProgress, ProgressBar};
use itertools::Itertools;
use log::{debug, error, info, warn, Level};
use reqwest::Client;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::{stdin, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::{fs, io};
use tokio_stream::wrappers::ReadDirStream;
use tokio_stream::StreamExt;

#[cfg(debug_assertions)]
use clap_verbosity_flag::DebugLevel;
#[cfg(not(debug_assertions))]
use clap_verbosity_flag::InfoLevel;

/// How to handle insecure HTTP requests.
#[derive(Debug, ValueEnum, Clone, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum HttpHandling {
    /// Fail when an insecure HTTP request is encountered.
    Disallow,

    /// Allow insecure HTTP requests.
    AllowInsecure,

    /// Try redirecting insecure HTTP requests to HTTPS.
    #[default]
    RedirectToHttps,
}

/// Whether to download every case in a sequence if the given case is part of one.
#[derive(Debug, ValueEnum, Clone, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
enum DownloadSequence {
    /// Automatically download every case in the sequence.
    Every,
    /// Only download the cases that are passed.
    Single,
    /// Ask first (if in an interactive terminal, otherwise don't download sequence).
    #[default]
    Ask,
}

/// Downloads an Ace Attorney Online case to be playable offline.
///
/// Simply pass the URL (i.e., `https://aaonline.fr/player.php?trial_id=YOUR_ID`) to this script.
/// You can also directly pass the ID instead.
#[derive(Parser, Debug, Clone)]
#[command(version, about, arg_required_else_help(true))]
#[allow(clippy::struct_excessive_bools)]
struct Args {
    /// The URL to the case, or its ID. May be passed multiple times.
    #[arg(required=true, num_args = 1.., value_parser = Self::accept_case)]
    cases: Vec<u32>,

    /// The output directory (or filename, if `-1` was used) for the case.
    ///
    /// If this is not passed, will use the title + ID of the case.
    /// It multiple cases are downloaded, they will all be put under this directory (which, by
    /// default, will be the current directory).
    #[arg(short('o'), long)]
    output: Option<PathBuf>,

    /// The branch or commit name of Ace Attorney Online that shall be used for the player.
    #[arg(short, long, default_value_t = String::from("master"))]
    player_version: String,

    /// The language to download the player in.
    #[arg(short, long, default_value_t = String::from("en"))]
    language: String,

    /// Whether to continue when an asset for the case could not be downloaded.
    #[arg(long, default_value_t = false)]
    continue_on_asset_error: bool,

    /// Whether to overwrite any existing output files.
    #[arg(short('r'), long, default_value_t = false)]
    remove_existing: bool,

    /// How many concurrent downloads to use.
    #[arg(short('j'), long, default_value_t = 5)]
    concurrent_downloads: usize,

    /// Whether to download all trials contained in a sequence (if the given case is part of a
    /// sequence).
    #[arg(short('s'), long, value_enum, default_value_t)]
    sequence: DownloadSequence,

    /// Whether to output only a single HTML file, with the assets embedded as data URLs.
    ///
    /// WARNING: Browsers may not like HTML files very much that are
    /// multiple dozens of megabytes large. Your mileage may vary.
    #[arg(short('1'), long, default_value_t = false)]
    one_html_file: bool,

    /// How to handle insecure HTTP requests.
    #[arg(long, value_enum, default_value_t)]
    http_handling: HttpHandling,

    /// Whether to disable the use of HTML5 audio for Howler.js.
    ///
    /// Enabling this will lead to CORS errors appearing in your browser's console when you open
    /// the HTML file locally, since it isn't allowed to access other files. Howler.js will then
    /// switch to HTML5 audio automatically. However, if you plan to use a local web server to
    /// open the player, it is recommended to enable this option, since those errors won't appear
    /// there (and there's a problem with how Firefox handles HTML5 audio, making this the better
    /// option if you plan to use Firefox.)
    #[arg(long)]
    disable_html5_audio: bool,

    #[cfg(not(debug_assertions))]
    #[command(flatten)]
    verbose: clap_verbosity_flag::Verbosity<InfoLevel>,

    #[cfg(debug_assertions)]
    #[command(flatten)]
    verbose: clap_verbosity_flag::Verbosity<DebugLevel>,
}

impl Args {
    /// Parses the given [case] into its ID.
    fn accept_case(case: &str) -> Result<u32, String> {
        if let Ok(id) = case.parse::<u32>() {
            Ok(id)
        } else if let Some(captures) = re::CASE_REGEX.captures(case) {
            captures
                .get(1)
                .expect("No captured content in case URL")
                .as_str()
                .parse()
                .map_err(|_| "Case ID in given URL is not a valid number!".to_string())
        } else {
            Err(format!(
                "Could not parse case ID from input \"{case}\". Please provide a valid case URL or ID."
            ))
        }
    }
}

/// The global context for the program.
#[derive(Debug)]
pub(crate) struct GlobalContext {
    /// The parsed command line arguments.
    args: Args,
    /// The output directory for the case.
    output: PathBuf,
    /// The HTTP client to use for requests.
    client: Client,
    /// Mapping from case ID to output directory.
    case_output_mapping: HashMap<u32, PathBuf>,
}

/// The main context for the program.
#[derive(Debug)]
struct MainContext {
    /// Whether the output directory was empty before we started.
    output_was_empty: bool,
    /// The IDs of the cases to download.
    case_ids: Vec<u32>,
    /// The progress bar for the main step indicator.
    pb: indicatif::ProgressBar,
    /// The multi-progress bar that may contain multiple progress bars.
    multi_progress: MultiProgress,
    /// The player instance.
    player: Option<Player>,
    /// The global context.
    global_ctx: Option<GlobalContext>,
}

impl MainContext {
    fn ctx(&self) -> &GlobalContext {
        // The context always either belongs to us or to the player (or more specifically, its
        // PlayerScripts instance).
        self.global_ctx.as_ref().unwrap_or_else(|| {
            &self
                .player
                .as_ref()
                .expect("either player or MainContext must have GlobalContext")
                .scripts
                .ctx
        })
    }

    fn ctx_mut(&mut self) -> &mut GlobalContext {
        self.global_ctx.as_mut().unwrap_or_else(|| {
            &mut self
                .player
                .as_mut()
                .expect("either player or MainContext must have GlobalContext")
                .scripts
                .ctx
        })
    }

    /// Creates a new main context from the given [args].
    async fn new(args: Args) -> MainContext {
        let case_ids = args.cases.clone();

        let output = args.output.clone().unwrap_or_else(|| {
            if let [single] = case_ids[..] {
                // If there's just a single case, we can just put it in a directory with its name.
                // We don't know its name yet, so we'll change this later once we retrieved it,
                // for now we'll use the ID.
                PathBuf::from(single.to_string())
            } else {
                // If there's more than one, we put them in the current directory.
                PathBuf::from(".")
            }
        });
        let output_was_empty = if let Ok(rd) = tokio::fs::read_dir(&output).await {
            ReadDirStream::new(rd)
                .next()
                .await
                .is_none_or(|x| x.is_err())
        } else {
            true
        };

        let multi_progress = MultiProgress::new();
        let http_handling = args.http_handling.clone();
        MainContext {
            case_ids,
            output_was_empty,
            pb: multi_progress.add(ProgressBar::new_spinner()),
            multi_progress,
            player: None,
            global_ctx: Some(GlobalContext {
                args,
                output,
                client: Client::builder()
                    .user_agent("aaoffline")
                    .https_only(http_handling == HttpHandling::Disallow)
                    .build()
                    .expect("client cannot be built"),
                case_output_mapping: HashMap::new(),
            }),
        }
    }

    /// Shows the current step with the given [text] and [step] number in the progress bar.
    fn show_step(&self, step: u8, text: &str) {
        self.pb
            .set_message(format!("{} {text}", format!("[{step}/7]").dimmed()));
        if !Self::should_hide_pb(&self.ctx().args) {
            self.pb.enable_steady_tick(Duration::from_millis(50));
        }
    }

    /// Whether to hide the progress bar.
    ///
    /// This is the case if the log level is higher than info, since then the progress bar would
    /// just interfere with the many log messages.
    fn should_hide_pb(args: &Args) -> bool {
        args.verbose.log_level().is_some_and(|x| x > Level::Info)
    }

    /// Adds a new progress bar with the given [max] value.
    fn add_progress(&self, max: u64) -> ProgressBar {
        if Self::should_hide_pb(&self.ctx().args) {
            ProgressBar::hidden()
        } else {
            self.multi_progress.add(ProgressBar::new(max))
        }
    }

    /// Finishes the given progress bar with the given [msg].
    fn finish_progress(&self, pb: &ProgressBar, msg: impl Into<Cow<'static, str>>) {
        pb.finish_with_message(msg);
        self.multi_progress.remove(pb);
    }

    /// Removes all data in the output directory.
    ///
    /// If [`only_ours`] is true, the directory will only be removed if it was empty before we started.
    /// For multiple cases, only the individual case directories will be removed.
    async fn cleanup_data(&self, only_ours: bool) {
        let output = &self.ctx().output;
        assert_ne!(output, &PathBuf::from("/"), "We will not remove /!");
        if self.case_ids.len() == 1 {
            // We will simply remove everything under the folder, or the file, if the output is a file.
            if Path::new(&output).is_file() {
                tokio::fs::remove_file(&output).await.unwrap_or_else(|e| {
                    if let io::ErrorKind::NotFound = e.kind() {
                        // Ignore if already deleted.
                    } else {
                        error!(
                            "Could not remove file {}: {e}. Please remove it manually.",
                            output.display()
                        );
                    }
                });
            } else if (!only_ours || self.output_was_empty) && output != &PathBuf::from(".") {
                tokio::fs::remove_dir_all(&output)
                    .await
                    .unwrap_or_else(|e| {
                        if let io::ErrorKind::NotFound = e.kind() {
                            // Ignore if already deleted.
                        } else {
                            error!(
                                "Could not remove directory {}: {e}. Please remove it manually.",
                                output.display()
                            );
                        }
                    });
            } else {
                warn!(
                    "Directory {} already contained files, will not clean up directory.",
                    output.display()
                );
            }
        } else {
            // Otherwise, we will remove the case ID directories.
            for case_id in &self.case_ids {
                let case_dir = output.join(case_id.to_string());
                tokio::fs::remove_dir_all(&case_dir)
                    .await
                    .unwrap_or_else(|e| {
                        if let io::ErrorKind::NotFound = e.kind() {
                            // Ignore if already deleted.
                        } else {
                            error!(
                                "Could not remove directory {}: {e}. Please remove it manually.",
                                case_dir.display()
                            );
                        }
                    });
            }
        }
    }

    /// Cleans up the data if the given [res] is an error, otherwise does nothing.
    async fn clean_on_fail(&self, res: Result<()>) -> Result<()> {
        if res.is_err() {
            self.cleanup_data(true).await;
        }
        res
    }

    /// Retrieves the case information for all cases and possibly their sequences.
    async fn retrieve_case_infos(&mut self) -> Result<HashSet<Case>> {
        // We temporarily move the context out of here to use its client freely.
        let ctx = self.global_ctx.take().expect("context must exist here");
        let client = &ctx.client;
        let mut cases: HashSet<_> =
            Self::download_case_infos_no_sequence(&self.case_ids, client).await?;
        cases.extend(
            Self::download_case_infos_no_sequence(
                &cases
                    .iter()
                    .flat_map(|case| self.additional_cases(case, &ctx))
                    .collect::<Vec<u32>>(),
                client,
            )
            .await?,
        );
        // And then we put it back.
        self.global_ctx = Some(ctx);
        Ok(cases)
    }

    /// Downloads the case information for the given [ids], without downloading the sequences.
    async fn download_case_infos_no_sequence(
        ids: &[u32],
        client: &Client,
    ) -> Result<HashSet<Case>> {
        future::join_all(ids.iter().map(|x| Case::retrieve_from_id(*x, client)))
            .await
            .into_iter()
            .collect()
    }

    /// Retrieves additional cases that should be downloaded if the given [case] is part of a sequence.
    ///
    /// This is dependent on the value of the `sequence` field in the arguments.
    fn additional_cases(&mut self, case: &Case, ctx: &GlobalContext) -> Vec<u32> {
        // Check if the user wants to download the whole sequence this case is contained in.
        if let Some(sequence) = case.case_information.sequence.as_ref() {
            debug!("Sequence detected: {sequence}");
            if match ctx.args.sequence {
                DownloadSequence::Every => true,
                DownloadSequence::Single => false,
                DownloadSequence::Ask => self.ask_sequence(case, sequence),
            } {
                return sequence.entry_ids();
            }
        }
        debug!("Not downloading sequence.");
        vec![]
    }

    /// Asks the user whether they want to download the whole sequence.
    fn ask_sequence(&self, case: &Case, sequence: &Sequence) -> bool {
        if stdin().is_terminal() {
            let result = self.pb.suspend(|| {
                info!(
                    "The case \"{}\" is part of a sequence: {sequence}.",
                    case.case_information.title,
                );
                if sequence.len() <= 1 {
                    info!("However, as there is only entry in this sequence, we will continue normally.");
                    return Some(false);
                }
                let result = dialoguer::Confirm::new()
                    .with_prompt("Do you want to download the other cases in this sequence too?")
                    .default(false)
                    .interact_opt()
                    .unwrap_or(Some(false));
                println!();
                result
            });
            if let Some(choice) = result {
                choice
            } else {
                info!("Cancelling download per user request.");
                std::process::exit(exitcode::OK)
            }
        } else {
            debug!("stdin is not a terminal, not asking whether to download sequence.");
            false
        }
    }

    /// Retrieves the site configuration for Ace Attorney Online.
    async fn retrieve_site_config(&mut self) -> Result<()> {
        // Here, we pass over ownership of the context to the player.
        self.player = Some(Player::new(self.global_ctx.take().expect("ctx must exist")).await?);
        Ok(())
    }

    /// Downloads the case data for the given [cases].
    async fn download_case_data(&mut self, cases: &mut [Case]) -> Result<()> {
        let pb = self.add_progress(0);
        let player = self.player.as_mut().unwrap();
        let site_data = &mut player.site_data;
        let ctx = &player.scripts.ctx;
        let mut handler = AssetDownloader::new(ctx.output.clone(), site_data, ctx);
        let multiple = cases.len() > 1;
        // We need to remember these because we overwrite them while collecting downloads,
        // and we may collect downloads more than once (for multiple cases), in which case we'd
        // try to download the modified paths, which we don't want.
        let original_default_places = site_data.default_data.default_places.clone();
        let mut downloads: Vec<Result<_>> = vec![];
        for case in cases.iter_mut() {
            site_data
                .default_data
                .default_places
                .clone_from(&original_default_places);
            let output = if multiple {
                // Case data needs to be put into the directory of that case.
                ctx.output.join(PathBuf::from(case.filename().to_string()))
            } else {
                ctx.output.clone()
            };
            // May need to create the directory first.
            if !ctx.args.one_html_file {
                fs::create_dir_all(output.join("assets")).await?;
            }
            handler.set_output(output);
            downloads.append(&mut handler.collect_case_data(case, site_data).await?);
        }
        // Then, download all assets at once.
        let result = handler
            .download_collected(&pb, downloads, cases, site_data)
            .await;
        self.finish_progress(&pb, "Case data downloaded.");
        self.clean_on_fail(result).await
    }

    /// Retrieves the player for cases.
    async fn retrieve_player(&mut self) -> Result<()> {
        let result = self.player.as_mut().unwrap().retrieve_player().await;
        self.clean_on_fail(result).await
    }

    /// Retrieves the scripts (i.e., JavaScript modules) for the player.
    async fn retrieve_player_scripts(&mut self) -> Result<()> {
        let pb = self.add_progress(0);
        let result = self.player.as_mut().unwrap().retrieve_scripts(&pb).await;
        self.finish_progress(&pb, "Player scripts retrieved.");
        self.clean_on_fail(result).await
    }

    /// Transforms the player blocks for the given [case] to point to offline assets.
    async fn transform_player_blocks(&mut self, case: &Case) -> Result<()> {
        let result = self.player.as_mut().unwrap().transform_player(case);
        self.clean_on_fail(result).await
    }

    /// Retrieves additional sources for the player.
    ///
    /// This includes things like the CSS and JavaScript sources that are not
    /// part of the player itself and only referenced in the source code.
    async fn retrieve_player_sources(&mut self) -> Result<()> {
        let pb = self.add_progress(0);
        let result = self
            .player
            .as_mut()
            .unwrap()
            .retrieve_player_misc_sources(&pb)
            .await;
        self.finish_progress(&pb, "All player sources downloaded.");
        self.clean_on_fail(result).await
    }

    /// Output the finished player for the case to [`output_path`].
    async fn output_player(&self, output_path: &Path) -> Result<()> {
        fs::create_dir_all(output_path.parent().unwrap()).await?;
        fs::write(
            &output_path,
            self.player.as_ref().unwrap().content.as_ref().unwrap(),
        )
        .await
        .with_context(|| {
            format!(
                "Could not write player to file {}. Please check your permissions.",
                output_path.display()
            )
        })?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_panic!();
    let args = Args::parse();
    let original_output = args.output.clone();
    let one_file = args.one_html_file;
    env_logger::builder()
        .format_timestamp(None)
        .filter_level(args.verbose.log_level_filter())
        .init();
    let mut ctx = MainContext::new(args).await;

    ctx.show_step(1, "Retrieving case information...");
    let mut cases: Vec<_> = ctx.retrieve_case_infos().await?.into_iter().collect();
    let num_cases = cases.len();
    let one_case = num_cases == 1;
    let output = &mut ctx.ctx_mut().output;

    if one_case && original_output.is_none() {
        // We need to update the output name, now that we know the title.
        let mut name = cases.first().unwrap().filename();
        if one_file {
            name += ".html";
        }
        *output = PathBuf::from(name);
    } else if one_case
        && one_file
        && output
            .extension()
            .is_none_or(|x| x.to_ascii_lowercase() != "html")
    {
        if output.is_dir() {
            *output = output.join(cases.first().unwrap().filename());
        }
        output.set_extension("html");
    }
    let output = &ctx.ctx().output;

    // Empty directories are fine if they exist already.
    if output.exists() && output.read_dir().ok().and_then(|mut x| x.next()).is_some() {
        if ctx.ctx().args.remove_existing {
            info!("Output exists already, deleting...");
            ctx.cleanup_data(false).await;
        } else if ctx.ctx().args.cases.len() == 1
            || ctx
                .ctx()
                .args
                .cases
                .iter()
                .any(|x| output.join(x.to_string()).exists())
        {
            error!(
                "Output at {} already exists. Please remove it or use --remove-existing.",
                output.display()
            );
            std::process::exit(exitcode::DATAERR);
        }
    }

    let output = output.clone();
    let cases_output = &mut ctx.ctx_mut().case_output_mapping;
    cases_output.extend(cases.iter().map(|case| {
        (
            case.id(),
            match (one_case, one_file) {
                (true, true) => output.clone(),
                (true, false) => output.join("index.html"),
                (false, true) => output.join(case.filename() + ".html"),
                (false, false) => output.join(case.filename()).join("index.html"),
            },
        )
    }));

    ctx.pb.finish_and_clear();
    info!(
        "Case{} identified as: {}",
        if one_case { "" } else { "s" },
        cases.iter().map(ToString::to_string).join(", ")
    );
    ctx.pb = ctx
        .multi_progress
        .add(indicatif::ProgressBar::new_spinner());

    ctx.show_step(2, "Retrieving site configuration...");
    ctx.retrieve_site_config().await?;

    ctx.show_step(
        3,
        &format!(
            "Downloading case assets{}... (This may take a while)",
            if one_case {
                String::new()
            } else {
                format!(" for {num_cases} cases")
            }
        ),
    );
    ctx.download_case_data(&mut cases).await?;

    ctx.show_step(4, "Retrieving player...");
    ctx.retrieve_player().await?;

    ctx.show_step(5, "Retrieving player scripts...");
    ctx.retrieve_player_scripts().await?;

    ctx.show_step(6, "Retrieving additional external player sources...");
    ctx.retrieve_player_sources().await?;

    let original_state = ctx.player.as_ref().unwrap().save();
    let mut output_path: &PathBuf = &PathBuf::new();
    for case in cases {
        // Need to reset transformed player.
        ctx.show_step(
            7,
            &format!(
                "Writing case \"{}\" to disk...",
                case.case_information.title
            ),
        );
        ctx.player.as_mut().unwrap().restore(original_state.clone());
        ctx.transform_player_blocks(&case).await?;
        output_path = ctx
            .ctx()
            .case_output_mapping
            .get(&case.id())
            .expect("Unhandled case encountered");
        ctx.output_player(output_path).await?;
    }

    let message = if one_case {
        format!("Case successfully written to {}!", &output_path.display())
    } else {
        let output = if ctx.ctx().output == PathBuf::from(".") {
            "current directory"
        } else {
            &ctx.ctx().output.display().to_string()
        };
        format!("{num_cases} cases successfully written to {output}!",)
    };
    ctx.finish_progress(&ctx.pb, message.bold().green().to_string());

    Ok(())
}

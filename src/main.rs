mod constants;
mod data;
mod download;
mod transform;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, ValueEnum};
use colored::Colorize;
use constants::re;
use data::case::Case;
use data::player::Player;
use download::AssetDownloader;
use human_panic::setup_panic;
use indicatif::{MultiProgress, ProgressBar};
use log::{error, info, Level};
use serde::Serialize;
use std::borrow::Cow;
use std::io::{self};
use std::path::Path;
use std::time::Duration;

#[cfg(debug_assertions)]
use clap_verbosity_flag::DebugLevel;
#[cfg(not(debug_assertions))]
use clap_verbosity_flag::InfoLevel;

/// How to handle insecure HTTP requests.
#[derive(Debug, ValueEnum, Clone, Serialize, Default)]
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

/// Downloads an Ace Attorney Online case to be playable offline.
///
/// Simply pass the URL (i.e., `https://aaonline.fr/player.php?trial_id=YOUR_ID`) to this script.
/// You can also directly pass the ID instead.
/// NOTE: Currently, all default sprites are downloaded, which may lead to a high download size.
#[derive(Parser, Debug, Clone)]
#[command(version, about, arg_required_else_help(true))]
struct Args {
    /// The URL to the case, or its ID.
    #[arg()]
    case: String,

    /// The output directory for the case.
    #[arg()]
    output: Option<String>,

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
    #[arg(short, long, default_value_t = false)]
    overwrite_existing: bool,

    /// How many concurrent downloads to allow.
    #[arg(short('j'), long, default_value_t = 5)]
    concurrent_downloads: usize,

    /// How to handle insecure HTTP requests.
    #[arg(long, value_enum, default_value_t)]
    http_handling: HttpHandling,

    #[cfg(not(debug_assertions))]
    #[command(flatten)]
    verbose: clap_verbosity_flag::Verbosity<InfoLevel>,

    #[cfg(debug_assertions)]
    #[command(flatten)]
    verbose: clap_verbosity_flag::Verbosity<DebugLevel>,
    // TODO: Offer option for single HTML file
}

#[derive(Debug)]
struct MainContext {
    args: Args,
    output: String,
    case_id: u32,
    pb: indicatif::ProgressBar,
    multi_progress: MultiProgress,
    player: Option<Player>,
}

impl MainContext {
    fn new(args: Args) -> Result<MainContext> {
        let id: u32 = if let Ok(id) = args.case.parse() {
            id
        } else if let Some(captures) = re::CASE_REGEX.captures(&args.case) {
            captures
                .get(1)
                .expect("No captured content in case URL")
                .as_str()
                .parse()
                .context("Case ID in given URL is not a valid number!")?
        } else {
            return Err(anyhow!(
                "Could not parse case ID from input {}. Please provide a valid case URL or ID.",
                args.case
            ));
        };
        let output = args.output.clone().unwrap_or_else(|| id.to_string());
        let multi_progress = MultiProgress::new();
        Ok(MainContext {
            args,
            case_id: id,
            output,
            pb: multi_progress.add(ProgressBar::new_spinner()),
            multi_progress,
            player: None,
        })
    }

    fn show_step(&self, step: u8, text: &str) {
        self.pb
            .set_message(format!("{} {text}", format!("[{step}/8]").dimmed()));
        self.pb.enable_steady_tick(Duration::from_millis(50));
    }

    fn should_hide_pb(args: &Args) -> bool {
        args.verbose.log_level().is_some_and(|x| x > Level::Info)
    }

    fn add_progress(&self, max: u64) -> ProgressBar {
        if Self::should_hide_pb(&self.args) {
            // The progress bar would just be annoying together with that many log messages.
            ProgressBar::hidden()
        } else {
            self.multi_progress.add(ProgressBar::new(max))
        }
    }

    fn finish_progress(&self, pb: &ProgressBar, msg: impl Into<Cow<'static, str>>) {
        pb.finish_with_message(msg);
        self.multi_progress.remove(pb);
    }

    fn cleanup_data(&self) {
        assert!(self.output != "/", "We will not remove /!");
        // We will simply remove everything under the folder, or the file, if the output is a file.
        if Path::new(&self.output).is_file() {
            std::fs::remove_file(&self.output).unwrap_or_else(|e| {
                if let io::ErrorKind::NotFound = e.kind() {
                    // Ignore if already deleted.
                } else {
                    error!(
                        "Could not remove file {}: {e}. Please remove it manually.",
                        self.output
                    );
                }
            });
        } else {
            std::fs::remove_dir_all(&self.output).unwrap_or_else(|e| {
                if let io::ErrorKind::NotFound = e.kind() {
                    // Ignore if already deleted.
                } else {
                    error!(
                        "Could not remove directory {}: {e}. Please remove it manually.",
                        self.output
                    );
                }
            });
        }
    }

    fn clean_on_fail(&self, res: Result<()>) -> Result<()> {
        res.inspect_err(|_| self.cleanup_data())
    }

    async fn retrieve_case_info(&mut self) -> Result<Case> {
        Case::retrieve_from_id(self.case_id).await
    }

    async fn retrieve_site_config(&mut self, case: Case) -> Result<()> {
        self.player = Some(Player::new(self.args.clone(), case).await?);
        Ok(())
    }

    async fn download_case_data(&mut self) -> Result<()> {
        let pb = self.add_progress(0);
        let player = self.player.as_mut().unwrap();
        let case = &mut player.case;
        let site_data = &mut player.site_data;
        let mut handler = AssetDownloader::new(self.args.clone(), self.output.clone(), site_data);
        let result = handler.download_case_data(case, &pb).await;
        self.finish_progress(&pb, "Case data downloaded.");
        self.clean_on_fail(result)
    }

    async fn retrieve_player(&mut self) -> Result<()> {
        let result = self.player.as_mut().unwrap().retrieve_player().await;
        self.clean_on_fail(result)
    }

    async fn retrieve_player_scripts(&mut self) -> Result<()> {
        let pb = self.add_progress(0);
        let result = self.player.as_mut().unwrap().retrieve_scripts(&pb).await;
        self.finish_progress(&pb, "Player scripts retrieved.");
        self.clean_on_fail(result)
    }

    fn transform_player_blocks(&mut self) -> Result<()> {
        let result = self.player.as_mut().unwrap().transform_player();
        self.clean_on_fail(result)
    }

    async fn retrieve_player_sources(&mut self) -> Result<()> {
        let pb = self.add_progress(0);
        let result = self
            .player
            .as_mut()
            .unwrap()
            .retrieve_player_misc_sources(self.output.clone(), &pb)
            .await;
        self.finish_progress(&pb, "All player sources downloaded.");
        self.clean_on_fail(result)
    }

    fn output_player(&self) -> Result<()> {
        std::fs::write(
            format!("{}/index.html", self.output),
            self.player.as_ref().unwrap().player.as_ref().unwrap(),
        )
        .with_context(|| {
            format!(
                "Could not write player to file {}/index.html. Please check your permissions.",
                self.output
            )
        })?;
        self.finish_progress(
            &self.pb,
            format!("Case successfully written to {}/index.html!", self.output)
                .bold()
                .green()
                .to_string(),
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_panic!();
    let args = Args::parse();
    env_logger::builder()
        .format_timestamp(None)
        .filter_level(args.verbose.log_level_filter())
        .init();
    let mut ctx = MainContext::new(args)?;

    if Path::new(&ctx.output).exists() {
        if ctx.args.overwrite_existing {
            info!("Output exists already, deleting...");
            ctx.cleanup_data();
        } else {
            error!(
                "Output at {} already exists. Please remove it or use --overwrite-existing.",
                ctx.output
            );
            std::process::exit(exitcode::DATAERR);
        }
    }

    ctx.show_step(1, "Retrieving case information...");
    let case = ctx.retrieve_case_info().await?;
    ctx.pb.finish_and_clear();
    info!("Case identified as {case}");
    ctx.pb = ctx
        .multi_progress
        .add(indicatif::ProgressBar::new_spinner());

    ctx.show_step(2, "Retrieving site configuration...");
    ctx.retrieve_site_config(case).await?;

    // TODO: Ask to download whole sequence
    ctx.show_step(3, "Downloading trial assets... (This may take a while)");
    ctx.download_case_data().await?;

    ctx.show_step(4, "Retrieving player...");
    ctx.retrieve_player().await?;

    ctx.show_step(5, "Retrieving player scripts...");
    ctx.retrieve_player_scripts().await?;

    ctx.show_step(6, "Transforming player...");
    ctx.transform_player_blocks()?;

    ctx.show_step(7, "Retrieving additional external player sources...");
    ctx.retrieve_player_sources().await?;

    ctx.show_step(8, "Writing player to file...");
    ctx.output_player()?;

    Ok(())
}

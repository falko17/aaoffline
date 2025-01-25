#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! A downloader for Ace Attorney Online cases that allows them to be played offline.

pub(crate) mod args;
pub(crate) mod constants;
pub(crate) mod data;
pub(crate) mod download;
mod middleware;
pub(crate) mod transform;

use anyhow::{Context, Result};
use args::Userscripts;
use clap::Parser;
use colored::Colorize;
use data::case::{Case, Sequence};
use data::player::Player;
use download::AssetDownloader;
use futures::{StreamExt, TryFutureExt};
use human_panic::setup_panic;
use indicatif::{MultiProgress, ProgressBar};
use itertools::Itertools;
use log::{debug, error, info, warn, Level};
use middleware::AaofflineMiddleware;
use reqwest::Client;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_retry::RetryTransientMiddleware;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::{stdin, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::{fs, io};

/// The global context for the program.
#[derive(Debug)]
pub(crate) struct GlobalContext {
    /// The parsed command line arguments.
    args: args::Args,
    /// The output directory for the case.
    output: PathBuf,
    /// The [reqwest] HTTP client to use for requests.
    client: ClientWithMiddleware,
    /// Mapping from case ID to output file.
    case_output_mapping: HashMap<u32, PathBuf>,
}

/// The main context for the program.
#[derive(Debug)]
struct MainContext {
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
    fn new(args: args::Args) -> MainContext {
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

        let multi_progress = MultiProgress::new();
        let http_handling = args.http_handling.clone();
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(args.retries);
        let mut builder = Client::builder()
            .user_agent("aaoffline")
            .https_only(http_handling == args::HttpHandling::Disallow);
        if args.connect_timeout > 0 {
            builder = builder.connect_timeout(Duration::from_secs(args.connect_timeout));
        }
        if args.read_timeout > 0 {
            builder = builder.read_timeout(Duration::from_secs(args.read_timeout));
        }
        let client = ClientBuilder::new(builder.build().expect("client cannot be built"))
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .with_init(AaofflineMiddleware::from(&args))
            .build();
        MainContext {
            case_ids,
            pb: multi_progress.add(ProgressBar::new_spinner()),
            multi_progress,
            player: None,
            global_ctx: Some(GlobalContext {
                args,
                output,
                client,
                case_output_mapping: HashMap::new(),
            }),
        }
    }

    /// Shows the current step with the given [text] and [step] number in the progress bar.
    fn show_step(&self, step: u8, text: &str) {
        self.show_step_ctx(step, text, self.ctx());
    }

    /// Shows the current step with the given [text] and [step] number in the progress bar,
    /// using the given [ctx] for the arguments.
    fn show_step_ctx(&self, step: u8, text: &str, ctx: &GlobalContext) {
        self.pb
            .set_message(format!("{} {text}", format!("[{step}/8]").dimmed()));
        if !Self::should_hide_pb(&ctx.args) {
            self.pb.enable_steady_tick(Duration::from_millis(50));
        }
    }

    /// Whether to hide the progress bar.
    ///
    /// This is the case if the log level is higher than info, since then the progress bar would
    /// just interfere with the many log messages.
    fn should_hide_pb(args: &args::Args) -> bool {
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

    /// Removes all of our created data in the output directory.
    async fn cleanup_data(&self) {
        let output = &self.ctx().output;
        assert_ne!(output, &PathBuf::from("/"), "We will not remove /!");
        if self.case_ids.len() == 1 {
            Self::delete_case_at(output).await;
        } else {
            // Otherwise, we will remove the cases individually.
            for filepath in self.ctx().case_output_mapping.values() {
                if self.ctx().args.one_html_file {
                    // Only need to delete the single file.
                    Self::delete_case_at(filepath).await;
                } else if let Some(parent) = filepath.parent() {
                    // Need to delete both the assets folder and the index.html from the parent
                    // directory.
                    Self::delete_case_at(parent).await;
                }
            }
        }
    }

    /// Deletes the case at the given [output], which may either be a single file or a directory
    /// containing both an `index.html` file and an `assets` directory.
    async fn delete_case_at(output: &Path) {
        if Path::new(output).is_file() {
            // We will simply remove the file if the output is a file.
            tokio::fs::remove_file(output).await.unwrap_or_else(|e| {
                if let io::ErrorKind::NotFound = e.kind() {
                    // Ignore if already deleted.
                } else {
                    warn!(
                        "Could not remove file {}: {e}. Please remove it manually.",
                        output.display()
                    );
                }
            });
        } else if Path::new(output).is_dir() {
            // We need to remove the assets folder and the index.html file.
            tokio::fs::remove_dir_all(output.join("assets"))
                .and_then(|()| tokio::fs::remove_file(output.join("index.html")))
                .await
                .unwrap_or_else(|e| {
                    if let io::ErrorKind::NotFound = e.kind() {
                        // Ignore if already deleted.
                    } else {
                        warn!(
                            "Could not remove content in {}: {e}. Please remove manually.",
                            output.display()
                        );
                    }
                });
        }
    }

    /// Cleans up the data if the given [res] is an error, otherwise does nothing.
    async fn clean_on_fail(&self, res: Result<()>) -> Result<()> {
        if res.is_err() {
            self.cleanup_data().await;
        }
        res
    }

    /// Retrieves the case information for all cases and possibly their sequences.
    async fn retrieve_case_infos(&mut self) -> Result<HashSet<Case>> {
        let pb = self.add_progress(self.case_ids.len() as u64);
        pb.inc(0);
        // We temporarily move the context out of here to use its client freely.
        let ctx = self.global_ctx.take().expect("context must exist here");
        let client = &ctx.client;
        let concurrent = ctx.args.concurrent_downloads;

        let mut cases: HashSet<_> =
            Self::download_case_infos_no_sequence(&self.case_ids, client, concurrent, &pb).await?;

        let additional = &cases
            .iter()
            .flat_map(|case| self.additional_cases(case, &ctx))
            .collect::<Vec<u32>>();
        pb.inc_length(additional.len() as u64);
        self.show_step_ctx(
            1,
            "Retrieving case information for additional sequence cases...",
            &ctx,
        );
        cases.extend(
            Self::download_case_infos_no_sequence(additional, client, concurrent, &pb).await?,
        );
        // And then we put it back.
        self.global_ctx = Some(ctx);
        self.finish_progress(&pb, "All case information retrieved.");
        Ok(cases)
    }

    /// Downloads the case information for the given [ids], without downloading the sequences.
    async fn download_case_infos_no_sequence(
        ids: &[u32],
        client: &ClientWithMiddleware,
        concurrent_conns: usize,
        pb: &ProgressBar,
    ) -> Result<HashSet<Case>> {
        futures::stream::iter(ids.iter().map(|x| Case::retrieve_from_id(*x, client)))
            .buffer_unordered(concurrent_conns)
            .inspect(|_| pb.inc(1))
            .collect::<Vec<_>>()
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
                args::DownloadSequence::Every => true,
                args::DownloadSequence::Single => false,
                args::DownloadSequence::Ask => self.ask_sequence(case, sequence),
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
        let mut downloads: Vec<_> = vec![];
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
            downloads.append(&mut handler.collect_case_data(case, site_data).await?.collect());
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

    /// Retrieves the userscripts and appends them to the player.
    async fn append_userscripts(&mut self) -> Result<()> {
        let urls = Userscripts::all_urls(&self.ctx().args.with_userscripts);
        if urls.is_empty() {
            return Ok(());
        }
        let pb = self.add_progress(urls.len() as u64);
        let result = self
            .player
            .as_mut()
            .unwrap()
            .retrieve_userscripts(&pb)
            .await;
        self.finish_progress(&pb, "Userscripts retrieved.");
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
        self.clean_on_fail(
            fs::create_dir_all(output_path.parent().unwrap())
                .and_then(|()| {
                    fs::write(
                        &output_path,
                        self.player.as_ref().unwrap().content.as_ref().unwrap(),
                    )
                })
                .await
                .with_context(|| {
                    format!(
                        "Could not write player to file {}. Please check your permissions.",
                        output_path.display()
                    )
                }),
        )
        .await
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_panic!();
    let args = args::Args::parse();
    Userscripts::validate_combination(&args.with_userscripts)?;
    let original_output = args.output.clone();
    let one_file = args.one_html_file;
    env_logger::builder()
        .format_timestamp(None)
        .format_suffix("\n\n") // Otherwise progress bar will overlap with log messages.
        .filter_level(args.verbose.log_level_filter())
        .init();
    let mut ctx = MainContext::new(args);

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
            .is_none_or(|x| !x.eq_ignore_ascii_case("html"))
    {
        if output.is_dir() {
            *output = output.join(cases.first().unwrap().filename());
        }
        output.set_extension("html");
    } else if !one_case
        && original_output.is_none()
        && cases
            .iter()
            .map(|x| &x.case_information.sequence)
            .all_equal_value()
            .is_ok_and(Option::is_some)
    {
        // All downloaded cases are part of a sequence.
        let sequence = cases
            .first()
            .as_ref()
            .unwrap()
            .case_information
            .sequence
            .as_ref()
            .unwrap();
        *output = PathBuf::from(&sequence.title);
    } else if !one_case && original_output.is_none() {
        // Downloaded cases are not part of a single sequence.
        // We'll put them in the current directory.
        *output = PathBuf::from(".");
    }
    let output = &ctx.ctx().output;

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

    // If the user doesn't want to replace anything, check first if there is anything.
    if !ctx.ctx().args.replace_existing {
        for player_file in ctx.ctx().case_output_mapping.values() {
            // Either there's the player file itself...
            if player_file.is_file()
            // ...or, if `-1` is not set, the `assets` directory (only important if it's non-empty).
                || !ctx.ctx().args.one_html_file && player_file
                    .parent()
                    .and_then(|x| x.join("assets").read_dir().ok()).is_some_and(|mut x| x.next().is_some())
            {
                error!(
                    "Output at \"{}\" already exists. Please remove it or use --replace-existing.",
                    player_file.parent().unwrap_or(player_file).display()
                );
                std::process::exit(exitcode::DATAERR);
            }
        }
    }

    ctx.pb.finish_and_clear();
    info!(
        "Case{} identified as:{}{}",
        if one_case { "" } else { "s" },
        if one_case { ' ' } else { '\n' },
        cases
            .iter()
            .map(ToString::to_string)
            .map(|x| format!("• {x}"))
            .join("\n")
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

    ctx.show_step(7, "Applying userscripts...");
    ctx.append_userscripts().await?;

    let original_state = ctx.player.as_ref().unwrap().save();
    let mut output_path: &PathBuf = &PathBuf::new();
    for case in cases {
        // Need to reset transformed player.
        ctx.show_step(
            8,
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

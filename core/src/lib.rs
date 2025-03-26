#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! A downloader for Ace Attorney Online cases that allows them to be played offline.

pub mod args;
pub(crate) mod constants;
pub(crate) mod data;
pub(crate) mod download;
mod middleware;
pub(crate) mod transform;

#[cfg(feature = "fs")]
pub mod fs;

use anyhow::{Context, Result, anyhow};
use args::Userscripts;
use async_trait::async_trait;
use colored::Colorize;
use data::case::{Case, Sequence};
use data::player::Player;
use download::AssetDownloader;
use futures::{StreamExt, TryFutureExt};
use itertools::Itertools;
use log::{Level, debug, info, warn};
use middleware::AaofflineMiddleware;
use reqwest::Client;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::io::{IsTerminal, stdin};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use reqwest_retry::RetryTransientMiddleware;
#[cfg(not(target_arch = "wasm32"))]
use reqwest_retry::policies::ExponentialBackoff;

/// The total number of steps that aaoffline needs to go through.
pub const MAX_STEPS: u8 = 8;

/// The global context for the program.
#[derive(Debug)]
pub(crate) struct GlobalContext {
    /// The parsed command line arguments.
    args: args::Args,
    /// The output directory for the case.
    output: PathBuf,
    /// The [reqwest] HTTP client to use for requests.
    client: ClientWithMiddleware,
    /// The [FileWriter] to use for handling files and directories.
    writer: Box<dyn FileWriter + Sync>,
    /// Mapping from case ID to output file.
    case_output_mapping: HashMap<u32, PathBuf>,
}

/// The main context for the program.
#[derive(Debug)]
pub struct MainContext {
    /// The IDs of the cases to download.
    case_ids: Vec<u32>,
    /// The progress bar for the main step indicator.
    pb: Box<dyn ProgressReporter>,
    /// The player instance.
    player: Option<Player>,
    dialog: RwLock<Box<dyn InteractiveDialog>>,
    /// The global context.
    global_ctx: Option<GlobalContext>,
}

/// An abstraction over writing to the file system.
///
/// On certain platforms, we might not actually write to a disk
/// (e.g., on WASM, we'd write to a ZIP file that's stored in memory.)
#[async_trait]
pub trait FileWriter: Debug + Send + Sync {
    /// Writes the given [content] to the given [path].
    async fn write(&self, path: &Path, content: &[u8]) -> Result<(), std::io::Error>;

    /// Creates a symbolic link from the given [orig] to the given [target].
    async fn symlink(&self, orig: &Path, target: &Path) -> Result<(), std::io::Error>;

    /// Creates a hardlink from the given [orig] to the given [target].
    async fn hardlink(&self, orig: &Path, target: &Path);

    /// Deletes the case at the given [output], which may either be a single file or a directory
    /// containing both an `index.html` file and an `assets` directory.
    async fn delete_case_at(&self, output: &Path);

    /// Recursively creates a directory and all of its parent components if they
    /// are missing.
    async fn create_dir_all(&self, path: &Path) -> Result<(), std::io::Error>;

    /// Writes the given [content] to the given [path] (assumed to be in `assets`).
    async fn write_asset(&self, path: &Path, content: &[u8]) -> Result<()> {
        // Write to file. We may need to create the containing directories first.
        debug!("Writing {}...", path.display());
        let dir = path.parent().expect("no parent directory in path");
        assert!(dir.ends_with("assets"));
        self.create_dir_all(dir).await?;
        self.write(path, content).await?;
        Ok(())
    }

    /// Returns self as the [Any] type.
    fn as_any(&self) -> &dyn Any;
}

/// An abstraction over reporting progress back to the user.
///
/// The structure here is based on the `indicatif` crate.
pub trait ProgressReporter: Debug + Send + Sync {
    /// Advances the position of the progress bar by [delta].
    fn inc(&self, delta: u64);
    /// Increases the total length of the progress bar by [delta].
    fn inc_length(&self, delta: u64);
    /// Shows the current step with the given [text] and [step] number in the progress bar,
    /// displaying a steady progress indicator if [hidden] is false.
    fn next_step(&self, step: u8, text: &str, hidden: bool);

    /// Starts a new progress report, starting from an initial position of 0.
    fn new_progress(&self, max: u64, hidden: bool);
    /// Suspends the progress bar animation (if there is any) while `f` is being run.
    fn suspend(&self, f: &dyn Fn() -> Option<bool>) -> Option<bool>;

    /// Finishes the current progress animation, leaving behind the given [`msg`].
    fn finish_progress(&self, msg: String);
    /// Finishes and completely clears any current progress animation.
    fn finish_and_clear(&self);
}

/// An abstraction over interactively asking the user things.
pub trait InteractiveDialog: Debug + Send + Sync {
    /// Ask the user to confirm the given yes/no [prompt], using [default_value] as the default choice.
    fn confirm(&mut self, prompt: &str, default_value: bool) -> Option<bool>;
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

    /// Consumes self, returning the stored [FileWriter].
    pub fn writer(&self) -> &(dyn FileWriter + Sync) {
        self.ctx().writer.as_ref()
    }

    fn pb(&self) -> &dyn ProgressReporter {
        self.pb.as_ref()
    }

    /// Creates a new main context from the given [args].
    ///
    /// # Panics
    /// This function will panic if the download client cannot be built.
    #[must_use]
    pub fn new(
        args: args::Args,
        writer: Box<dyn FileWriter + Sync>,
        dialog: Box<dyn InteractiveDialog>,
        reporter: Box<dyn ProgressReporter>,
    ) -> MainContext {
        debug!("Arguments: {args:?}");
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

        let http_handling = args.http_handling.clone();
        let mut builder = Client::builder().user_agent("aaoffline");

        #[cfg(not(target_arch = "wasm32"))]
        {
            builder = builder.https_only(http_handling == args::HttpHandling::Disallow);

            if args.connect_timeout > 0 {
                builder = builder.connect_timeout(Duration::from_secs(args.connect_timeout));
            }
            if args.read_timeout > 0 {
                builder = builder.read_timeout(Duration::from_secs(args.read_timeout));
            }
        }
        let mut client_builder =
            ClientBuilder::new(builder.build().expect("client cannot be built"))
                .with_init(AaofflineMiddleware::from(&args));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let retry_policy = ExponentialBackoff::builder().build_with_max_retries(args.retries);
            client_builder =
                client_builder.with(RetryTransientMiddleware::new_with_policy(retry_policy));
        }

        let client = client_builder.build();
        MainContext {
            case_ids,
            pb: reporter,
            player: None,
            global_ctx: Some(GlobalContext {
                args,
                writer,
                output,
                client,
                case_output_mapping: HashMap::new(),
            }),
            dialog: RwLock::new(dialog),
        }
    }

    /// Shows the current step with the given [text] and [step] number in the progress bar.
    fn show_step(&self, step: u8, text: &str) {
        self.show_step_ctx(step, text, self.ctx());
    }

    /// Shows the current step with the given [text] and [step] number in the progress bar,
    /// using the given [ctx] for the arguments.
    fn show_step_ctx(&self, step: u8, text: &str, ctx: &GlobalContext) {
        self.pb()
            .next_step(step, text, Self::should_hide_pb(&ctx.args));
    }

    /// Whether to hide the progress bar.
    ///
    /// This is the case if the log level is higher than info, since then the progress bar would
    /// just interfere with the many log messages.
    fn should_hide_pb(args: &args::Args) -> bool {
        args.log_level > Level::Info
    }

    /// Adds a new progress bar with the given [max] value.
    fn add_progress(&self, max: u64) {
        self.pb()
            .new_progress(max, Self::should_hide_pb(&self.ctx().args));
    }

    /// Finishes the given progress bar with the given [msg].
    fn finish_progress(&self, msg: String) {
        self.pb().finish_progress(msg);
    }

    /// Removes all of our created data in the output directory.
    async fn cleanup_data(&self) {
        let output = &self.ctx().output;
        assert_ne!(output, &PathBuf::from("/"), "We will not remove /!");
        if self.case_ids.len() == 1 {
            self.ctx().writer.delete_case_at(output).await;
        } else {
            // Otherwise, we will remove the cases individually.
            for filepath in self.ctx().case_output_mapping.values() {
                if self.ctx().args.one_html_file {
                    // Only need to delete the single file.
                    self.ctx().writer.delete_case_at(filepath).await;
                } else if let Some(parent) = filepath.parent() {
                    // Need to delete both the assets folder and the index.html from the parent
                    // directory.
                    self.ctx().writer.delete_case_at(parent).await;
                }
            }
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
    async fn retrieve_case_infos(&mut self) -> Result<Vec<Case>> {
        self.add_progress(self.case_ids.len() as u64);
        // We temporarily move the context out of here to use its client freely.
        let ctx = self.global_ctx.take().expect("context must exist here");
        let client = &ctx.client;
        let concurrent = ctx.args.concurrent_downloads;
        //let pb = self.pb.take().unwrap();
        let pb = self.pb();
        pb.inc(0);

        let mut cases: HashSet<_> =
            Self::download_case_infos_no_sequence(&self.case_ids, client, concurrent, pb).await?;

        let additional = &cases
            .iter()
            .map(|case| self.additional_cases(case, &ctx))
            .flatten_ok()
            .collect::<Result<Vec<u32>>>()?;
        let pb = self.pb();
        pb.inc_length(additional.len() as u64);
        self.show_step_ctx(
            1,
            "Retrieving case information for additional sequence cases...",
            &ctx,
        );
        cases.extend(
            Self::download_case_infos_no_sequence(additional, client, concurrent, self.pb())
                .await?,
        );
        // And then we put it back.
        self.global_ctx = Some(ctx);

        let cases = cases.into_iter().collect::<Vec<_>>();

        // We also need to update our output filename(s).
        self.update_output_paths(&cases);

        self.finish_progress("All case information retrieved.".into());
        Ok(cases)
    }

    /// Downloads the case information for the given [ids], without downloading the sequences.
    async fn download_case_infos_no_sequence(
        ids: &[u32],
        client: &ClientWithMiddleware,
        concurrent_conns: usize,
        pb: &dyn ProgressReporter,
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
    fn additional_cases(&mut self, case: &Case, ctx: &GlobalContext) -> Result<Vec<u32>> {
        // Check if the user wants to download the whole sequence this case is contained in.
        if let Some(sequence) = case.case_information.sequence.as_ref() {
            debug!("Sequence detected: {sequence}");
            if match ctx.args.sequence {
                args::DownloadSequence::Every => true,
                args::DownloadSequence::Single => false,
                args::DownloadSequence::Ask => self.ask_sequence(case, sequence)?,
            } {
                return Ok(sequence.entry_ids());
            }
        }
        debug!("Not downloading sequence.");
        Ok(vec![])
    }

    /// Asks the user whether they want to download the whole sequence.
    fn ask_sequence(&self, case: &Case, sequence: &Sequence) -> Result<bool> {
        if stdin().is_terminal() {
            let result = self.pb().suspend(&|| {
                info!(
                    "The case \"{}\" is part of a sequence: {sequence}.",
                    case.case_information.title,
                );
                if sequence.len() <= 1 {
                    info!("However, as there is only entry in this sequence, we will continue normally.");
                    return Some(false);
                }
                let result = self.dialog.write().unwrap().confirm("Do you want to download the other cases in this sequence too?", false);
                println!();
                result
            });
            if let Some(choice) = result {
                Ok(choice)
            } else {
                Err(anyhow!("Download cancelled per user request."))
            }
        } else {
            debug!("stdin is not a terminal, not asking whether to download sequence.");
            Ok(false)
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
        self.add_progress(0);
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
            handler.set_output(output).await?;
            downloads.append(&mut handler.collect_case_data(case, site_data).await?.collect());
        }
        // Then, download all assets at once.
        let result = handler
            .download_collected(self.pb.as_ref(), downloads, cases, site_data)
            .await;
        if result.is_ok() {
            self.pb().finish_progress("Case data downloaded.".into());
        }
        self.clean_on_fail(result).await
    }

    /// Retrieves the player for cases.
    async fn retrieve_player(&mut self) -> Result<()> {
        let result = self.player.as_mut().unwrap().retrieve_player().await;
        self.clean_on_fail(result).await
    }

    /// Retrieves the scripts (i.e., JavaScript modules) for the player.
    async fn retrieve_player_scripts(&mut self) -> Result<()> {
        self.add_progress(0);
        let pb = self.pb.as_ref();
        let result = self.player.as_mut().unwrap().retrieve_scripts(pb).await;
        if result.is_ok() {
            pb.finish_progress("Player scripts retrieved.".into());
        }
        self.clean_on_fail(result).await
    }

    /// Retrieves the userscripts and appends them to the player.
    async fn append_userscripts(&mut self) -> Result<()> {
        let urls = Userscripts::all_urls(&self.ctx().args.with_userscripts);
        if urls.is_empty() {
            return Ok(());
        }
        self.add_progress(urls.len() as u64);
        let pb = self.pb.as_ref();
        let result = self.player.as_mut().unwrap().retrieve_userscripts(pb).await;
        if result.is_ok() {
            self.finish_progress("Userscripts retrieved.".into());
        }
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
        self.add_progress(0);
        let pb = self.pb.as_ref();
        let result = self
            .player
            .as_mut()
            .unwrap()
            .retrieve_player_misc_sources(pb)
            .await;

        if result.is_ok() {
            self.finish_progress("All player sources downloaded.".into());
        }
        self.clean_on_fail(result).await
    }

    /// Output the finished player for the case to [`output_path`].
    async fn output_player(&self, output_path: &Path) -> Result<()> {
        self.clean_on_fail(
            self.ctx()
                .writer
                .create_dir_all(output_path.parent().unwrap())
                .and_then(|()| {
                    self.ctx().writer.write(
                        output_path,
                        self.player
                            .as_ref()
                            .unwrap()
                            .content
                            .as_ref()
                            .unwrap()
                            .as_bytes(),
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

    fn update_output_paths(&mut self, cases: &[Case]) {
        let one_case = cases.len() == 1;
        let original_output = self.ctx().args.output.clone();
        let one_file = self.ctx().args.one_html_file;
        let output = &mut self.ctx_mut().output;

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
            *output = PathBuf::from(&sequence.title.trim());
        } else if !one_case && original_output.is_none() {
            // Downloaded cases are not part of a single sequence.
            // We'll put them in the current directory.
            *output = PathBuf::from(".");
        }

        let output = output.clone();

        let cases_output = &mut self.ctx_mut().case_output_mapping;
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
    }

    /// Runs aaoffline to completion, downloading the cases configured in this instance.
    ///
    /// # Panics
    /// Panics if the player has not been set up correctly (this is done internally).
    ///
    /// # Errors
    /// Since this function runs all steps consecutively, each of the errors that can occur for the
    /// individual steps can also occur here.
    pub async fn run_all_steps(&mut self) -> Result<()> {
        self.show_step(1, "Retrieving case information...");
        let mut cases: Vec<_> = self.retrieve_case_infos().await?;
        let num_cases = cases.len();
        let one_case = num_cases == 1;

        // If the user doesn't want to replace anything, check first if there is anything.
        if !self.ctx().args.replace_existing {
            for player_file in self.ctx().case_output_mapping.values() {
                // Either there's the player file itself...
                if player_file.is_file()
                // ...or, if `-1` is not set, the `assets` directory (only important if it's non-empty).
                || !self.ctx().args.one_html_file && player_file
                    .parent()
                    .and_then(|x| x.join("assets").read_dir().ok()).is_some_and(|mut x| x.next().is_some())
                {
                    return Err(anyhow!(
                        "Output at \"{}\" already exists. Please remove it or use --replace-existing.",
                        player_file.parent().unwrap_or(player_file).display()
                    ));
                }
            }
        }

        self.pb().finish_and_clear();
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

        self.show_step(2, "Retrieving site configuration...");
        self.retrieve_site_config().await?;

        self.show_step(
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
        self.download_case_data(&mut cases).await?;

        self.show_step(4, "Retrieving player...");
        self.retrieve_player().await?;

        self.show_step(5, "Retrieving player scripts...");
        self.retrieve_player_scripts().await?;

        self.show_step(6, "Retrieving additional external player sources...");
        self.retrieve_player_sources().await?;

        self.show_step(7, "Applying userscripts...");
        self.append_userscripts().await?;

        let original_state = self.player.as_ref().unwrap().save();
        let mut output_path: &PathBuf = &PathBuf::new();
        for case in cases {
            // Need to reset transformed player.
            self.show_step(
                8,
                &format!(
                    "Writing case \"{}\" to disk...",
                    case.case_information.title
                ),
            );
            self.player
                .as_mut()
                .unwrap()
                .restore(original_state.clone());
            self.transform_player_blocks(&case).await?;
            output_path = self
                .ctx()
                .case_output_mapping
                .get(&case.id())
                .expect("Unhandled case encountered");
            self.output_player(output_path).await?;
        }

        let message = if one_case {
            format!(
                "Case successfully written to \"{}\"!",
                &output_path.display()
            )
        } else {
            let output = if self.ctx().output == PathBuf::from(".") {
                "current directory"
            } else {
                &format!("directory \"{}\"", &self.ctx().output.display().to_string())
            };
            format!("{num_cases} cases successfully written to {output}!",)
        };
        self.pb()
            .finish_progress(message.bold().green().to_string());
        Ok(())
    }
}

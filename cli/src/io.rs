use std::io::{IsTerminal, stdin};
use std::sync::Arc;
use std::time::Duration;

use aaoffline::{ChallengeHandler, InteractiveDialog, MAX_STEPS, ProgressReporter, Url};
use async_trait::async_trait;
use colored::Colorize;
use indicatif::{MultiProgress, ProgressBar};
use std::sync::RwLock;

#[derive(Debug)]
pub(crate) struct CliProgressBar {
    bar: RwLock<Option<indicatif::ProgressBar>>,
    spinner: indicatif::ProgressBar,
    multi_progress: MultiProgress,
}

#[derive(Debug)]
pub(crate) struct CliInteraction {
    /// The progress bars to hide while we ask the user something.
    multi_progress: MultiProgress,
}

impl InteractiveDialog for CliInteraction {
    fn confirm(&mut self, prompt: &str, default_value: bool) -> Option<bool> {
        dialoguer::Confirm::new()
            .with_prompt(prompt)
            .default(default_value)
            .interact_opt()
            .unwrap_or(Some(default_value))
    }

    fn challenge_handler(&self) -> Option<Arc<dyn ChallengeHandler>> {
        Some(Arc::new(CliChallengeHandler {
            multi_progress: self.multi_progress.clone(),
        }))
    }
}

/// Asks the user on the terminal to solve a bot challenge in the browser.
#[derive(Debug)]
struct CliChallengeHandler {
    /// The progress bars to hide while we ask the user.
    multi_progress: MultiProgress,
}

#[async_trait]
impl ChallengeHandler for CliChallengeHandler {
    async fn solve(&self, url: &Url) -> bool {
        if !stdin().is_terminal() {
            return false;
        }
        let multi_progress = self.multi_progress.clone();
        let url = url.clone();
        tokio::task::spawn_blocking(move || {
            multi_progress.suspend(|| {
                eprintln!(
                    "\n{}\nOpen this link in your browser and solve the captcha:\n\n    {}\n",
                    "Ace Attorney Online wants to verify that you are human.".yellow(),
                    url.as_str().underline()
                );
                dialoguer::Confirm::new()
                    .with_prompt("Did you solve the captcha? Retry now?")
                    .default(true)
                    .interact_opt()
                    .ok()
                    .flatten()
                    .unwrap_or(false)
            })
        })
        .await
        .unwrap_or(false)
    }
}

impl CliProgressBar {
    /// Returns a [`CliInteraction`] that hides these progress bars while it asks the user.
    pub(crate) fn interaction(&self) -> CliInteraction {
        CliInteraction {
            multi_progress: self.multi_progress.clone(),
        }
    }

    pub(crate) fn new() -> CliProgressBar {
        let multi_progress = MultiProgress::new();

        CliProgressBar {
            bar: RwLock::new(None),
            spinner: multi_progress.add(ProgressBar::new_spinner()),
            multi_progress,
        }
    }
}

impl ProgressReporter for CliProgressBar {
    fn inc(&self, delta: u64) {
        self.bar.read().unwrap().as_ref().unwrap().inc(delta)
    }

    fn inc_length(&self, delta: u64) {
        self.bar.read().unwrap().as_ref().unwrap().inc_length(delta)
    }

    fn next_step(&self, step: u8, text: &str, hidden: bool) {
        self.spinner.set_message(format!(
            "{} {text}",
            format!("[{step}/{MAX_STEPS}]").dimmed()
        ));
        if !hidden {
            self.spinner.enable_steady_tick(Duration::from_millis(50));
        }
    }

    fn new_progress(&self, max: u64, hidden: bool) {
        assert!(self.bar.read().unwrap().is_none());
        let new_pb = if hidden {
            ProgressBar::hidden()
        } else {
            self.multi_progress.add(ProgressBar::new(max))
        };
        *self.bar.write().unwrap() = Some(new_pb);
    }

    fn suspend(&self, f: &dyn Fn() -> Option<bool>) -> Option<bool> {
        self.multi_progress.suspend(f)
    }

    fn finish_progress(&self, msg: String) {
        if let Some(pb) = self.bar.write().unwrap().take() {
            pb.finish_with_message(msg);
            self.multi_progress.remove(&pb);
        } else {
            self.spinner.finish_with_message(msg);
        }
    }

    fn finish_and_clear(&self) {
        if let Some(pb) = self.bar.write().unwrap().take() {
            pb.finish_and_clear();
            self.multi_progress.remove(&pb);
        } else {
            self.spinner.finish_and_clear();
            self.spinner.reset();
        }
    }
}

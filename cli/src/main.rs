use aaoffline::args::{Args, Userscripts};
use aaoffline::fs::TokioFsWriter;
use aaoffline::MainContext;
use anyhow::Result;
use args::CliArgs;
use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use human_panic::setup_panic;

use io::{CliInteraction, CliProgressBar};
use log::error;

mod args;
mod io;

#[tokio::main]
async fn main() -> Result<()> {
    setup_panic!();
    let args: Args = CliArgs::parse().into();
    Userscripts::validate_combination(&args.with_userscripts)
        .map_err(|x| CliArgs::command().error(ErrorKind::ArgumentConflict, x))?;
    env_logger::builder()
        .format_timestamp(None)
        .format_suffix("\n\n") // Otherwise progress bar will overlap with log messages.
        .filter_level(args.log_level)
        .init();

    let writer = Box::new(TokioFsWriter);
    let pb = Box::new(CliProgressBar::new());
    let interact = Box::new(CliInteraction);
    let mut ctx = MainContext::new(args, writer, interact, pb);
    ctx.run_all_steps().await.inspect_err(|e| error!("{e}"))
}

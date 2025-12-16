use std::path::PathBuf;

use aaoffline::args::{Args, DownloadSequence, HttpHandling, SequenceErrorHandling, Userscripts};
use clap::{Parser, ValueEnum, command};
#[cfg(debug_assertions)]
use clap_verbosity_flag::DebugLevel;
#[cfg(not(debug_assertions))]
use clap_verbosity_flag::InfoLevel;
use serde::Serialize;

/// Downloads an Ace Attorney Online case to be playable offline.
///
/// Simply pass the URL (i.e., `https://aaonline.fr/player.php?trial_id=YOUR_ID`) to this script.
/// You can also directly pass the ID instead.
#[derive(Debug, Clone, Parser)]
#[command(version, about, arg_required_else_help(true))]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct CliArgs {
    /// The URL to the case, or its ID. May be passed multiple times.
    #[arg(required=true, num_args = 1.., value_parser = Args::accept_case)]
    pub(crate) cases: Vec<u32>,

    /// The output directory (or filename, if `-1` was used) for the case.
    ///
    /// If this is not passed, will use the title + ID of the case.
    /// It multiple cases are downloaded, they will all be put under this directory (which, by
    /// default, will be the current directory).
    #[arg(short, long)]
    pub(crate) output: Option<PathBuf>,

    /// The branch or commit name of Ace Attorney Online that shall be used for the player.
    #[arg(short, long, default_value_t = String::from("master"))]
    pub(crate) player_version: String,

    /// The language to download the player in.
    #[arg(short, long, default_value_t = String::from("en"))]
    pub(crate) language: String,

    /// Whether to continue when an asset for the case could not be downloaded.
    #[arg(short, long, default_value_t = false)]
    pub(crate) continue_on_asset_error: bool,

    /// Whether to replace any existing output files.
    #[arg(short('r'), long, default_value_t = false)]
    pub(crate) replace_existing: bool,

    /// Whether to download all trials contained in a sequence (if the given case is part of a
    /// sequence).
    #[arg(short('s'), long, value_enum, default_value_t)]
    pub(crate) sequence: CliDownloadSequence,

    /// Whether to output only a single HTML file, with the assets embedded as data URLs.
    ///
    /// WARNING: Browsers may not like HTML files very much that are
    /// multiple dozens of megabytes large. Your mileage may vary.
    #[arg(short('1'), long, default_value_t = false)]
    pub(crate) one_html_file: bool,

    /// Whether to apply any userscripts to the downloaded case. Can be passed multiple times.
    ///
    /// Scripts were created by Time Axis, with only the expanded keyboard controls written by me,
    /// building on Time Axis' basic keyboard controls script.
    /// (These options may change in the future when some scripts are consolidated).
    #[arg(
        short('u'),
        long,
        num_args(0..=1),
        default_missing_value("all"),
        require_equals(true),
        value_enum,
    )]
    pub(crate) with_userscripts: Vec<CliUserscripts>,

    /// How many concurrent downloads to use.
    #[arg(short('j'), long, default_value_t = 5)]
    pub(crate) concurrent_downloads: usize,

    /// How to handle cases in a sequence that aren't accessible.
    #[arg(long, value_enum, default_value_t)]
    pub sequence_error_handling: CliSequenceErrorHandling,

    /// How many times to retry downloads if they fail.
    ///
    /// Note that this is in addition to the first try, so a value of one will lead to two download
    /// attempts if the first one failed.
    #[arg(long, default_value_t = 3)]
    pub(crate) retries: u32,

    /// The maximum time to wait for the connect phase of network requests (in seconds).
    /// A value of 0 means that no timeout will be applied.
    #[arg(long, default_value_t = 10)]
    pub(crate) connect_timeout: u64,

    /// The maximum time to wait for the read (i.e., download) phase of network requests
    /// (in seconds).
    /// A value of 0 means that no timeout will be applied.
    #[arg(long, default_value_t = 30)]
    pub(crate) read_timeout: u64,

    /// How to handle insecure HTTP requests.
    #[arg(long, value_enum, default_value_t)]
    pub(crate) http_handling: CliHttpHandling,

    /// Whether to disable the use of HTML5 audio for Howler.js.
    ///
    /// Enabling this will lead to CORS errors appearing in your browser's console when you open
    /// the HTML file locally, since it isn't allowed to access other files. Howler.js will then
    /// switch to HTML5 audio automatically. However, if you plan to use a local web server to
    /// open the player, it is recommended to enable this option, since those errors won't appear
    /// there (and there's a problem with how Firefox handles HTML5 audio, making this the better
    /// option if you plan to use Firefox.)
    #[arg(long)]
    pub(crate) disable_html5_audio: bool,

    /// Whether to disable the automatic fixing of photobucket watermarks.
    #[arg(long)]
    pub(crate) disable_photobucket_fix: bool,

    /// Partial URL pointing to a proxy that all requests should be routed through.
    ///
    /// The actual request URL will be appended to this parameter.
    /// For example, if this were set to `https://example.com/?proxy=`, then a request for
    /// `https://example.org/sample` would become `https://example.com/?proxy=https://example.org/sample`.
    #[arg(long)]
    pub(crate) proxy: Option<String>,

    #[cfg(not(debug_assertions))]
    #[command(flatten)]
    pub(crate) verbose: clap_verbosity_flag::Verbosity<InfoLevel>,

    #[cfg(debug_assertions)]
    #[command(flatten)]
    #[allow(missing_docs)]
    pub(crate) verbose: clap_verbosity_flag::Verbosity<DebugLevel>,
}

/// How to handle insecure HTTP requests.
#[derive(Debug, Clone, Copy, Serialize, Default, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CliHttpHandling {
    /// Fail when an insecure HTTP request is encountered.
    Disallow,

    /// Allow insecure HTTP requests.
    AllowInsecure,

    /// Try redirecting insecure HTTP requests to HTTPS.
    #[default]
    RedirectToHttps,
}

/// Whether to download every case in a sequence if the given case is part of one.
#[derive(Debug, Clone, Copy, Serialize, Default, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CliDownloadSequence {
    /// Automatically download every case in the sequence.
    Every,
    /// Only download the cases that are passed.
    Single,
    /// Ask first (if in an interactive terminal, otherwise don't download sequence).
    #[default]
    Ask,
}

/// Whether to abort the download when a case in a sequence is not found.
#[derive(Debug, Clone, Copy, Serialize, Default, ValueEnum)]
pub(crate) enum CliSequenceErrorHandling {
    /// Abort the download.
    Abort,
    /// Continue with the other, existing cases in the sequence.
    Continue,
    /// Ask first (if in an interactive terminal, otherwise abort).
    #[default]
    Ask,
}

/// Whether to apply any userscripts to the downloaded case.
#[derive(Debug, Clone, Copy, Serialize, Default, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CliUserscripts {
    /// Apply all userscripts.
    All,

    /// Changes the fonts of nametags to use a proper pixelized font.
    AltNametag,
    /// Adds a backlog button to see past dialog.
    Backlog,
    /// Improves the layout (e.g., enlarging and centering the main screens).
    BetterLayout,
    /// Adds extensive keyboard controls. See the top of the file at
    /// <https://gist.github.com/falko17/965207b1f1f0496ff5f0cb41d8e827f2#file-aaokeyboard-user-js>
    /// to get an overview of available controls.
    KeyboardControls,

    /// Apply no userscript.
    #[default]
    None,
}

impl From<CliHttpHandling> for HttpHandling {
    fn from(value: CliHttpHandling) -> Self {
        match value {
            CliHttpHandling::Disallow => HttpHandling::Disallow,
            CliHttpHandling::AllowInsecure => HttpHandling::AllowInsecure,
            CliHttpHandling::RedirectToHttps => HttpHandling::RedirectToHttps,
        }
    }
}

impl From<CliDownloadSequence> for DownloadSequence {
    fn from(value: CliDownloadSequence) -> Self {
        match value {
            CliDownloadSequence::Every => DownloadSequence::Every,
            CliDownloadSequence::Single => DownloadSequence::Single,
            CliDownloadSequence::Ask => DownloadSequence::Ask,
        }
    }
}

impl From<CliSequenceErrorHandling> for SequenceErrorHandling {
    fn from(value: CliSequenceErrorHandling) -> Self {
        match value {
            CliSequenceErrorHandling::Abort => SequenceErrorHandling::Abort,
            CliSequenceErrorHandling::Continue => SequenceErrorHandling::Continue,
            CliSequenceErrorHandling::Ask => SequenceErrorHandling::Ask,
        }
    }
}

impl From<CliUserscripts> for Userscripts {
    fn from(value: CliUserscripts) -> Self {
        match value {
            CliUserscripts::All => Userscripts::All,
            CliUserscripts::AltNametag => Userscripts::AltNametag,
            CliUserscripts::Backlog => Userscripts::Backlog,
            CliUserscripts::BetterLayout => Userscripts::BetterLayout,
            CliUserscripts::KeyboardControls => Userscripts::KeyboardControls,
            CliUserscripts::None => Userscripts::None,
        }
    }
}

impl From<CliArgs> for Args {
    fn from(value: CliArgs) -> Self {
        Args {
            cases: value.cases,
            output: value.output,
            player_version: value.player_version,
            language: value.language,
            continue_on_asset_error: value.continue_on_asset_error,
            replace_existing: value.replace_existing,
            sequence: value.sequence.into(),
            one_html_file: value.one_html_file,
            with_userscripts: value.with_userscripts.into_iter().map(Into::into).collect(),
            concurrent_downloads: value.concurrent_downloads,
            retries: value.retries,
            connect_timeout: value.connect_timeout,
            read_timeout: value.read_timeout,
            http_handling: value.http_handling.into(),
            disable_html5_audio: value.disable_html5_audio,
            disable_photobucket_fix: value.disable_photobucket_fix,
            proxy: value.proxy,
            log_level: value.verbose.log_level_filter(),
            sequence_error_handling: value.sequence_error_handling.into(),
        }
    }
}

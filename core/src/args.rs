//! Contains data structures and methods related to the arguments passed to aaoffline.

use anyhow::Result;
use itertools::Itertools;
use log::LevelFilter;
use serde::Serialize;

use std::path::PathBuf;

use crate::constants::re;

/// Arguments that configure how aaoffline runs.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct Args {
    /// The IDs of the cases that shall be downloaded.
    pub cases: Vec<u32>,

    /// The output directory (or filename, if `-1` was used) for the case.
    ///
    /// If this is not passed, will use the title + ID of the case.
    /// It multiple cases are downloaded, they will all be put under this directory (which, by
    /// default, will be the current directory).
    pub output: Option<PathBuf>,

    /// The branch or commit name of Ace Attorney Online that shall be used for the player.
    pub player_version: String,

    /// The language to download the player in.
    pub language: String,

    /// Whether to continue when an asset for the case could not be downloaded.
    pub continue_on_asset_error: bool,

    /// Whether to replace any existing output files.
    pub replace_existing: bool,

    /// Whether to download all trials contained in a sequence (if the given case is part of a
    /// sequence).
    pub sequence: DownloadSequence,

    /// Whether to output only a single HTML file, with the assets embedded as data URLs.
    pub one_html_file: bool,

    /// Whether to apply any userscripts to the downloaded case. Can be passed multiple times.
    ///
    /// Scripts were created by Time Axis, with only the expanded keyboard controls written by me,
    /// building on Time Axis' basic keyboard controls script.
    /// (These options may change in the future when some scripts are consolidated).
    pub with_userscripts: Vec<Userscripts>,

    /// How many concurrent downloads to use.
    pub concurrent_downloads: usize,

    /// How many times to retry downloads if they fail.
    ///
    /// Note that this is in addition to the first try, so a value of one will lead to two download
    /// attempts if the first one failed.
    pub retries: u32,

    /// The maximum time to wait for the connect phase of network requests (in seconds).
    /// A value of 0 means that no timeout will be applied.
    pub connect_timeout: u64,

    /// The maximum time to wait for the read (i.e., download) phase of network requests
    /// (in seconds).
    /// A value of 0 means that no timeout will be applied.
    pub read_timeout: u64,

    /// How to handle insecure HTTP requests.
    pub http_handling: HttpHandling,

    /// Whether to disable the use of HTML5 audio for Howler.js.
    ///
    /// Enabling this will lead to CORS errors appearing in your browser's console when you open
    /// the HTML file locally, since it isn't allowed to access other files. Howler.js will then
    /// switch to HTML5 audio automatically. However, if you plan to use a local web server to
    /// open the player, it is recommended to enable this option, since those errors won't appear
    /// there (and there's a problem with how Firefox handles HTML5 audio, making this the better
    /// option if you plan to use Firefox.)
    pub disable_html5_audio: bool,

    /// Whether to disable the automatic fixing of photobucket watermarks.
    pub disable_photobucket_fix: bool,

    /// Partial URL pointing to a proxy that all requests should be routed through.
    ///
    /// The actual request URL will be appended to this parameter.
    /// For example, if this were set to `https://example.com/?proxy=`, then a request for
    /// `https://example.org/sample` would become `https://example.com/?proxy=https://example.org/sample`.
    pub proxy: Option<String>,

    /// The minimum level messages have to have to be logged.
    pub log_level: LevelFilter,
}

/// How to handle insecure HTTP requests.
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub enum HttpHandling {
    /// Fail when an insecure HTTP request is encountered.
    Disallow,

    /// Allow insecure HTTP requests.
    AllowInsecure,

    /// Try redirecting insecure HTTP requests to HTTPS.
    #[default]
    RedirectToHttps,
}

/// Whether to download every case in a sequence if the given case is part of one.
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub enum DownloadSequence {
    /// Automatically download every case in the sequence.
    Every,
    /// Only download the cases that are passed.
    Single,
    /// Ask first (if in an interactive terminal, otherwise don't download sequence).
    #[default]
    Ask,
}

/// Whether to apply any userscripts to the downloaded case.
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq, Hash)]
pub enum Userscripts {
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

impl Userscripts {
    /// Returns the URLs pointing to the corresponding userscripts.
    pub(crate) fn urls(&self) -> Vec<&str> {
        match self {
            Self::AltNametag => {
                vec!["https://beyondtimeaxis.github.io/misc/aaoaltnametags.user.js"]
            }
            Self::Backlog => vec!["https://beyondtimeaxis.github.io/misc/aaobacklog.user.js"],
            Self::BetterLayout => {
                vec!["https://beyondtimeaxis.github.io/misc/aaobetterlayout.user.js"]
            }
            Self::KeyboardControls => vec![
                "https://gist.github.com/falko17/965207b1f1f0496ff5f0cb41d8e827f2/raw/aaokeyboard.user.js",
            ],
            Self::All => [
                Self::AltNametag,
                Self::Backlog,
                Self::BetterLayout,
                Self::KeyboardControls,
            ]
            .iter()
            .flat_map(Self::urls)
            .collect(),
            Self::None => vec![],
        }
    }

    /// Returns all URLs belonging to the given collection of [scripts].
    pub(crate) fn all_urls(scripts: &[Self]) -> Vec<&str> {
        scripts.iter().flat_map(|x| x.urls()).unique().collect()
    }

    /// Ensures that the given [scripts] are a valid combination.
    ///
    /// # Errors
    /// When the given [scripts] are not a valid combination.
    pub fn validate_combination(scripts: &[Self]) -> Result<(), String> {
        if (scripts.contains(&Self::All)) && scripts.len() > 1 {
            Err("Can't specify any other scripts when including all of them anyway".into())
        } else if scripts.contains(&Self::None) && scripts.len() > 1 {
            Err("Can't specify any other scripts when including none".into())
        } else {
            Ok(())
        }
    }
}

impl Args {
    /// Parses the given [case] into its ID.
    pub fn accept_case(case: &str) -> Result<u32, String> {
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

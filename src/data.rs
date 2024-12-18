//! Contains models for data structures used in the Ace Attorney Online player.

use std::error::Error;
use std::fmt::Display;

use anyhow::{Context, Result};
use log::trace;
use regex::Regex;
use serde::de::DeserializeOwned;

/// Error that occurs when a regex does not match the input text.
#[derive(Debug)]
pub(crate) struct RegexNotMatched;

impl Display for RegexNotMatched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Regex did not match the input text.")
    }
}

impl Error for RegexNotMatched {}

/// Extracts a JSON value of type [T] from the given [text] using the given [regex].
fn retrieve_escaped_json<T: DeserializeOwned>(regex: &Regex, text: &str) -> Result<T> {
    let extracted = regex
        .captures(text)
        .context("Trial script seemingly changed format, this means the script needs to be updated to work with the newest AAO version.")?
        .get(1)
        .ok_or(RegexNotMatched)?
        .as_str()
        .replace(r"\\", r"\")
        .replace(r#"\""#, "\"")
        .replace(r"\'", "'")
        .replace(r"\/", "/");
    trace!("{extracted}");
    serde_json::from_str(&extracted).context("Could not parse trial data. The script needs to be updated to be able to handle this trial.")
}

pub(crate) mod site;

pub(crate) mod case;

pub(crate) mod player;

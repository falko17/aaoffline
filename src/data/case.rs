//! Contains data models and helper methods related to the case that is being downloaded.

use anyhow::{Context, Result};

use chrono::{DateTime, Utc};

use colored::Colorize;

use const_format::formatcp;
use log::{debug, trace};

use anyhow::anyhow;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::formats::Flexible;
use serde_with::TimestampSeconds;

use std::fmt::Display;

use crate::constants::re;
use crate::constants::AAONLINE_BASE;
use crate::data::RegexNotMatched;

/// Represents the information of a case.
#[serde_with::serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CaseInformation {
    /// The name of the author of the case.
    author: String,
    /// The ID of the author of the case.
    author_id: u32,
    /// Whether the case can be read by the current user.
    can_read: bool,
    /// Whether the case can be written to by the current user.
    can_write: bool,
    /// The format of the case.
    format: String,
    /// The ID of the case.
    id: u32,
    /// The language of the case.
    language: String,
    /// The date the case was last edited.
    #[serde_as(as = "TimestampSeconds<i64, Flexible>")]
    last_edit_date: DateTime<Utc>,
    /// The sequence the case is contained in, if any.
    pub(crate) sequence: Option<Sequence>,
    /// The title of the case.
    pub(crate) title: String,
}

/// A sequence of connected cases.
#[derive(Debug, Serialize, Deserialize)]
pub struct Sequence {
    /// The title of the sequence.
    title: String,
    /// The list of entries in the sequence.
    list: Vec<SequenceEntry>,
}

impl Sequence {
    /// Returns a list of case IDs in this sequence.
    pub(crate) fn entry_ids(&self) -> Vec<u32> {
        self.list.iter().map(|x| x.id).collect()
    }
}

impl Display for Sequence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "\"{}\" with cases {}",
            &self.title.bold(),
            self.list
                .iter()
                .map(|x| format!("\"{x}\""))
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}

/// An entry (case) in a sequence.
#[derive(Debug, Serialize, Deserialize)]
pub struct SequenceEntry {
    /// The ID of the case.
    id: u32,
    /// The title of the case.
    title: String,
}

impl Display for SequenceEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.title)
    }
}

impl Display for CaseInformation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let title: &str = if let Some(sequence) = &self.sequence {
            &format!("\"{}\" (Sequence: \"{}\")", self.title, sequence.title)
        } else {
            &format!("\"{}\"", self.title)
        };
        write!(
            f,
            "{} by {} [last edited on {}]",
            title.bold(),
            self.author.italic(),
            self.last_edit_date
        )
    }
}

/// A case for the Ace Attorney Online player.
#[derive(Debug)]
pub(crate) struct Case {
    /// Metadata about this case.
    pub(crate) case_information: CaseInformation,
    /// The data (i.e., contents) of this case.
    pub(crate) case_data: Value,
}

impl Case {
    /// Returns the ID of this case.
    pub(crate) fn id(&self) -> u32 {
        self.case_information.id
    }

    /// Retrieves a case using the given [`case_id`] from Ace Attorney Online.
    pub(crate) async fn retrieve_from_id(case_id: u32, client: &Client) -> Result<Case> {
        let case_script = client.get(format!(
        "{AAONLINE_BASE}/trial.js.php?trial_id={case_id}",
    )).send().await
    .context(
        formatcp!("Could not download case data from {AAONLINE_BASE}. Please check your internet connection.")
    )?
    .error_for_status()
    .context("Case data seems to be inaccessible.")?
    .text().await?;

        let case_information =
            super::retrieve_escaped_json(&re::TRIAL_INFORMATION_REGEX, &case_script).map_err(
                |x| {
                    if x.root_cause().is::<RegexNotMatched>() {
                        anyhow!("The case with given ID {case_id} could not be found!")
                    } else {
                        x
                    }
                },
            )?;

        let case_data = super::retrieve_escaped_json(&re::TRIAL_DATA_REGEX, &case_script)?;
        debug!("{:?}", case_information);
        trace!("{:?}", case_data);

        Ok(Case {
            case_information,
            case_data,
        })
    }

    /// Returns a list of character and sprite IDs for default sprites used in this case.
    pub(crate) fn get_used_sprites(&self) -> Vec<(i64, i64)> {
        trace!("{}", self.case_data);
        // We are filtering out numbers here because for some reason, the arrays always
        // contain a "0: 0" element.
        self.case_data
            .as_object()
            .expect("Trial data must be object")["frames"]
            .as_array()
            .expect("frames must be array")
            .iter()
            .filter(|x| !x.is_number())
            .flat_map(|x| {
                x.as_object().expect("frame must be object")["characters"]
                    .as_array()
                    .expect("characters in frame must be array")
            })
            .filter(|x| !x.is_number())
            .filter_map(|x| {
                let character = x.as_object().expect("character in frame must be object");
                let profile_id = &character["profile_id"];
                let sprite_id = &character["sprite_id"];
                if profile_id.is_null() || sprite_id.is_null() {
                    // Not sure what it means when this occurs, but it does happen sometimes.
                    // When it does, we just skip this.
                    None
                } else {
                    Some((
                        profile_id.as_i64().expect("profile_id must be integer"),
                        sprite_id.as_i64().expect("sprite_id must be integer"),
                    ))
                }
            })
            .collect()
    }

    /// Serializes this case to JavaScript variables that are picked up by the case player.
    pub(crate) fn serialize_to_js(&self) -> Result<String> {
        // We already retrieved trial information and data.
        // We will reserialize it to JSON to include any changes we made.
        let ser_trial_info = serde_json::to_string(&self.case_information)?;
        let ser_trial_data = serde_json::to_string(&self.case_data)?;
        Ok(format!("var trial_information = {ser_trial_info};\nvar initial_trial_data = {ser_trial_data};\n"))
    }
}

impl Display for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.case_information)
    }
}

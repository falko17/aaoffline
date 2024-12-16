//! Contains data models related to the case that is being downloaded.

use anyhow::{Context, Result};

use chrono::{DateTime, Utc};

use colored::Colorize;

use log::{debug, trace};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::formats::Flexible;
use serde_with::TimestampSeconds;

use std::fmt::Display;

use crate::constants::re;

#[serde_with::serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TrialInformation {
    author: String,
    author_id: u32,
    can_read: bool,
    can_write: bool,
    format: String,
    id: u32,
    language: String,
    #[serde_as(as = "TimestampSeconds<i64, Flexible>")]
    last_edit_date: DateTime<Utc>,
    sequence: Option<Sequence>,
    pub(crate) title: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Sequence {
    title: String,
    list: Vec<SequenceEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SequenceEntry {
    id: u32,
    title: String,
}

impl Display for TrialInformation {
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

#[derive(Debug)]
pub(crate) struct Case {
    pub(crate) trial_information: TrialInformation,
    pub(crate) trial_data: Value,
}

impl Case {
    pub(crate) async fn retrieve_from_id(case_id: u32) -> Result<Case> {
        let trial_script = reqwest::get(format!(
        "https://aaonline.fr/trial.js.php?trial_id={}",
        case_id
    )).await
    .context(
        "Could not download trial data from aaonline.fr. Please check your internet connection."
    )?
    .error_for_status()
    .context("aaonline.fr trial data seems to be inaccessible.")?
    .text().await?;

        let trial_information =
            super::retrieve_escaped_json(&re::TRIAL_INFORMATION_REGEX, &trial_script)?;

        let trial_data = super::retrieve_escaped_json(&re::TRIAL_DATA_REGEX, &trial_script)?;
        debug!("{:?}", trial_information);
        trace!("{:?}", trial_data);

        Ok(Case {
            trial_information,
            trial_data,
        })
    }

    // First element is character ID, second is sprite ID.
    pub(crate) fn get_used_sprites(&self) -> Vec<(i64, i64)> {
        trace!("{}", self.trial_data);
        // NOTE: We are filtering out numbers here because for some reason, the arrays always
        // contain a "0: 0" element.
        self.trial_data
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

    pub(crate) fn serialize_to_js(&self) -> Result<String> {
        // We already retrieved trial information and data.
        // We will reserialize it to JSON to include any changes we made.
        let ser_trial_info = serde_json::to_string(&self.trial_information)?;
        let ser_trial_data = serde_json::to_string(&self.trial_data)?;
        Ok(format!("var trial_information = {ser_trial_info};\nvar initial_trial_data = {ser_trial_data};\n"))
    }
}

impl Display for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.trial_information)
    }
}

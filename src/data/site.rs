//! Contains data models related to the configuration of Ace Attorney Online.

use anyhow::{Context, Result};
use const_format::formatcp;
use log::{error, trace};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use std::collections::HashSet;

use crate::constants::{re, AAONLINE_BASE, BRIDGE_URL};

/// Default data for the Ace Attorney Online player.
///
/// Note that this does not contain all default data, only the data that is relevant to the
/// retrieval and offline transformation process.
#[derive(Debug, Clone)]
pub(crate) struct DefaultData {
    /// Names of default profiles that have a startup animation.
    pub(crate) default_profiles_startup: HashSet<String>,
    /// Deserialized default places.
    pub(crate) default_places: Value,
}

impl DefaultData {
    /// Retrieves the default data from the given [module].
    fn from_default_module(module: &str) -> Result<Self> {
        let startup_value =
            super::retrieve_escaped_json::<Value>(&re::DEFAULT_PROFILES_STARTUP_REGEX, module)?;
        let default_profiles_startup = if let Value::Object(startup_map) = startup_value {
            startup_map.into_iter().map(|x| x.0).collect()
        } else {
            error!("Default profiles startup map is not an object!");
            std::process::exit(exitcode::DATAERR);
        };

        let default_places =
            super::retrieve_escaped_json::<Value>(&re::DEFAULT_PLACES_REGEX, module)?;
        Ok(DefaultData {
            default_profiles_startup,
            default_places,
        })
    }

    /// Writes the default places to the given [module] as a JavaScript variable that is picked up
    /// by the player.
    pub(crate) fn write_default_module(&self, module: &mut String) -> Result<()> {
        let default_places_text = serde_json::to_string(&self.default_places)?;
        let default_places_match = re::DEFAULT_PLACES_REGEX
            .find(module)
            .expect("Default places did not match!");
        module.replace_range(
            default_places_match.range(),
            &format!("var default_places = {default_places_text};"),
        );
        Ok(())
    }
}

/// Paths to various directories on the Ace Attorney Online server.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct SitePaths {
    bg_subdir: String,
    cache_dir: String,
    css_dir: String,
    defaultplaces_subdir: String,
    evidence_subdir: String,
    forum_path: String,
    icon_subdir: String,
    js_dir: String,
    pub(crate) lang_dir: String,
    locks_subdir: String,
    music_dir: String,
    picture_dir: String,
    popups_subdir: String,
    site_name: String,
    sounds_dir: String,
    startup_subdir: String,
    still_subdir: String,
    talking_subdir: String,
    trialdata_backups_dir: String,
    trialdata_deleted_dir: String,
    trialdata_dir: String,
    voices_dir: String,
}

impl SitePaths {
    /// Returns the subdirectory for the given [name].
    pub(crate) fn get_subdir(&self, name: &str) -> &str {
        match name {
            "bg" => &self.bg_subdir,
            "defaultplaces" => &self.defaultplaces_subdir,
            "evidence" => &self.evidence_subdir,
            "icon" => &self.icon_subdir,
            "locks" => &self.locks_subdir,
            "popups" => &self.popups_subdir,
            "startup" => &self.startup_subdir,
            "still" => &self.still_subdir,
            "talking" => &self.talking_subdir,
            _ => panic!("Unknown subdir requested!"),
        }
    }

    /// Returns the default icon for the site.
    pub(crate) fn default_icon(&self) -> String {
        format!(
            "{AAONLINE_BASE}/{}/{}/Inconnu.png",
            self.picture_dir, self.icon_subdir
        )
    }

    /// Returns the path for the default evidence on Ace Attorney Online.
    pub(crate) fn sprite_path<'a>(&'a self, kind: &'a str, base: &'a str) -> Vec<&'a str> {
        vec![&self.picture_dir, self.get_subdir(kind), base]
    }
    /// Returns the path for the default icons on Ace Attorney Online.
    pub(crate) fn icon_path(&self) -> Vec<&str> {
        vec![&self.picture_dir, &self.icon_subdir]
    }
    /// Returns the path for the default evidence on Ace Attorney Online.
    pub(crate) fn evidence_path(&self) -> Vec<&str> {
        vec![&self.picture_dir, &self.evidence_subdir]
    }
    /// Returns the path for the default backgrounds on Ace Attorney Online.
    pub(crate) fn bg_path(&self) -> Vec<&str> {
        vec![&self.picture_dir, &self.bg_subdir]
    }
    /// Returns the path for the default popups on Ace Attorney Online.
    pub(crate) fn popup_path(&self) -> Vec<&str> {
        vec![&self.picture_dir, &self.popups_subdir]
    }
    /// Returns the path for the default music on Ace Attorney Online.
    pub(crate) fn music_path(&self) -> Vec<&str> {
        vec![&self.music_dir]
    }
    /// Returns the path for the default sounds on Ace Attorney Online.
    pub(crate) fn sound_path(&self) -> Vec<&str> {
        vec![&self.sounds_dir]
    }
    /// Returns the path for the voices on Ace Attorney Online.
    pub(crate) fn voice_path(&self) -> Vec<&str> {
        vec![&self.voices_dir]
    }
    /// Returns the path for the psyche locks on Ace Attorney Online.
    pub(crate) fn lock_path(&self) -> Vec<&str> {
        vec![&self.picture_dir, &self.locks_subdir]
    }

    /// Retrieves the site paths from the `bridge.js.php` script.
    pub(crate) async fn retrieve_from_bridge() -> Result<Self> {
        // We only need to retrieve the bridge script because we need to know the configuration of
        // aaonline.fr. We don't need it for the JS module system, as we'll handle that manually.
        let bridge = reqwest::get(BRIDGE_URL).await
    .context(
        "Could not download site configuration from {AAONLINE_BASE}. Please check your internet connection."
    )?
    .error_for_status()
    .context(formatcp!("{AAONLINE_BASE} site configuration seems to be inaccessible."))?
    .text().await?;
        trace!("{}", bridge);
        let config_text = re::CONFIG_REGEX
    .captures(&bridge)
    .context("Bridge script seemingly changed format, this means the script needs to be updated to work with the newest AAO version.")?
    .get(1)
    .expect("No captured content in site configuration")
    .as_str();
        trace!("{}", config_text);
        let config: Self = serde_json::from_str(config_text)
            .context("Could not parse site configuration. The script needs to be updated.")?;
        trace!("{:?}", config);

        Ok(config)
    }
}

/// Data model for the Ace Attorney Online site data.
#[derive(Debug, Clone)]
pub(crate) struct SiteData {
    /// Default data for the Ace Attorney Online player.
    pub(crate) default_data: DefaultData,
    /// Site paths for the Ace Attorney Online server.
    pub(crate) site_paths: SitePaths,
}

impl SiteData {
    /// Initializes the site data from the Ace Attorney Online server, using the given [default_mod]
    /// as the default module.
    pub(crate) async fn from_site_data(default_mod: &str) -> Result<Self> {
        let site_paths = SitePaths::retrieve_from_bridge().await?;
        let default_data = DefaultData::from_default_module(default_mod)?;
        Ok(SiteData {
            default_data,
            site_paths,
        })
    }
}

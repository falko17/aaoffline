//! Contains functions and methods for downloading case data.

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use futures::{stream, FutureExt, StreamExt};
use indicatif::ProgressBar;
use itertools::Itertools;
use log::{debug, error, trace, warn};
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::data::case::Case;
use crate::data::site::SiteData;
use crate::{Args, HttpHandling};

// Returns output file path and file content.
pub(crate) async fn download_url(
    url: &str,
    http_handling: &HttpHandling,
) -> Result<(String, Bytes)> {
    debug!("Downloading {url}...");
    let (target, output) = if url.starts_with("http") {
        let url = if url.starts_with("http://") {
            match http_handling {
                HttpHandling::AllowInsecure => url,
                HttpHandling::RedirectToHttps => &url.replacen("http://", "https://", 1),
                HttpHandling::Disallow => {
                    return Err(anyhow!("Blocking insecure HTTP request to {url}."))
                }
            }
        } else {
            url
        };
        (
            url.to_string(),
            format!("assets/{}", url.split('/').last().unwrap()),
        )
    } else {
        // Assume this is a relative URL.
        let relative = url.trim_start_matches('/').to_string();
        (
            format!("https://aaonline.fr/{relative}"),
            format!("assets/{}", url.split('/').last().unwrap_or(url)),
        )
    };

    // Then, download the file as bytes (since it may be binary).
    let content = reqwest::get(&target)
        .await
        .with_context(|| {
            format!("Could not download file from {target}. Please check your internet connection.")
        })?
        .error_for_status()
        .with_context(|| format!("File at {target} seems to be inaccessible."))?
        .bytes()
        .await?;
    Ok((output, content)) // Relative URL need not contain case prefix.
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
struct AssetDownload {
    url: String,
    path: String,
    ignore_inaccessible: bool,
}

#[derive(Debug)]
pub(crate) struct AssetDownloader<'a> {
    args: Args,
    site_data: &'a mut SiteData,
    collector: AssetCollector,
    output: String,
}

#[derive(Debug)]
struct AssetCollector {
    collected: Vec<Result<AssetDownload>>,
    default_icon_url: String,
}

impl AssetCollector {
    fn new(default_icon_url: String) -> AssetCollector {
        AssetCollector {
            collected: vec![],
            default_icon_url,
        }
    }

    fn hash(value: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn path_exists(&self, path: &str) -> bool {
        self.collected
            .iter()
            .any(|x| x.as_ref().ok().map_or(false, |x| x.path == path))
    }

    fn new_path(&self, url: &str, path: &str) -> String {
        // We first need to split the path into filename and extension.
        let split = path
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|x| x.1)
            .unwrap_or(path)
            .rsplit_once('.');
        let ext = split.map(|x| x.1).unwrap_or("bin");
        let name = split.map(|x| x.0).unwrap_or(path);
        let first_choice = format!("assets/{name}.{ext}");
        // If we used this filename already, we need to append a hash.
        if self.path_exists(&first_choice) {
            format!("assets/{name}-{}.{ext}", Self::hash(url))
        } else {
            first_choice
        }
    }

    fn get_path(&self, url: &str, file: &str) -> String {
        // If we have an existing download for this URL, we need to use the same filename here.
        self.collected
            .iter()
            .filter_map(|x| x.as_ref().ok())
            .find(|x| x.url == url)
            .map_or_else(|| self.new_path(url, file), |x| x.path.clone())
    }

    fn make_download(
        &self,
        file_value: &mut Value,
        path_components: Option<Vec<&str>>,
        external: Option<bool>,
        default_extension: Option<&str>,
        filename: Option<&str>,
        ignore_inaccessible: bool,
    ) -> Result<AssetDownload> {
        let mut file = file_value
            .as_str()
            .unwrap_or(&self.default_icon_url)
            .to_string();
        if !file
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|x| x.1)
            .unwrap_or(&file)
            .contains('.')
        {
            if let Some(ext) = default_extension {
                file = format!("{file}.{ext}");
            }
        }
        let url = if !external.unwrap_or(true) {
            &format!(
                "https://aaonline.fr/{}/{file}",
                path_components
                    .context("Non-external path needs path components!")?
                    .join("/")
            )
        } else if file.starts_with("http") {
            &file
        } else {
            &format!("https://aaonline.fr/{file}")
        };
        let path = self.get_path(url, filename.unwrap_or(&file));
        if filename.is_some_and(|x| path != x) {
            return Err(anyhow!(
                "Filename mismatch: Should be {path}, but was {}",
                filename.unwrap()
            ));
        }
        let url = Regex::new(r"([^:/])/{2,}")
            .unwrap()
            .replace_all(url, "$1/")
            .to_string();
        trace!("{url} to {path}");
        // Reassign icon to contain new name.
        *file_value = Value::String(path.clone());
        Ok(AssetDownload {
            url,
            path,
            ignore_inaccessible,
        })
    }

    fn collect_download(
        &mut self,
        file_value: &mut Value,
        path_components: Option<Vec<&str>>,
        external: Option<bool>,
        default_extension: Option<&str>,
    ) {
        self.collected.push(self.make_download(
            file_value,
            path_components,
            external,
            default_extension,
            None,
            false,
        ))
    }

    fn collect_download_with_name(
        &mut self,
        file_value: &mut Value,
        name: &str,
        path_components: Option<Vec<&str>>,
        external: Option<bool>,
        ignore_inaccessible: bool,
    ) {
        self.collected.push(self.make_download(
            file_value,
            path_components,
            external,
            None,
            Some(name),
            ignore_inaccessible,
        ))
    }

    fn get_unique_downloads(&mut self) -> Vec<Result<AssetDownload>> {
        let mut encountered_downloads: HashSet<AssetDownload> = HashSet::new();
        self.collected
            .drain(0..self.collected.len())
            .filter(|x| {
                x.as_ref()
                    .ok()
                    .is_none_or(|x| encountered_downloads.insert(x.clone()))
            })
            .collect()
    }
}

impl<'a> AssetDownloader<'a> {
    pub(crate) fn new(
        args: Args,
        output: String,
        site_data: &'a mut SiteData,
    ) -> AssetDownloader<'a> {
        let default_icon_path = site_data.site_paths.default_icon();
        AssetDownloader {
            args,
            site_data,
            output,
            collector: AssetCollector::new(default_icon_path),
        }
    }

    // Downloads URLs in parallel with the configured number of concurrent downloads.
    async fn download_assets(
        &self,
        assets: Vec<AssetDownload>,
        pb: &ProgressBar,
    ) -> Vec<(AssetDownload, Result<()>)> {
        stream::iter(assets)
            .map(|asset| async move {
                (
                    asset.clone(),
                    self.download_asset(&asset)
                        .map(|x| {
                            pb.inc(1);
                            x.map(|_| ())
                        })
                        .await,
                )
            })
            .buffer_unordered(self.args.concurrent_downloads)
            .collect()
            .await
    }

    async fn download_asset(&self, asset: &AssetDownload) -> Result<String> {
        let (_, content) = download_url(&asset.url, &self.args.http_handling).await?;
        self.write_asset(&asset.path, &content)
            .map(|_| asset.path.clone())
    }

    pub(crate) async fn download_url(&self, url: &str) -> Result<String> {
        let (path, content) = download_url(url, &self.args.http_handling).await?;
        self.write_asset(&path, &content).map(|_| path)
    }

    fn write_asset(&self, path: &str, content: &[u8]) -> Result<()> {
        // Write to file. We may need to create the containing directories first.
        debug!("Writing {path}...");
        let file_output = format!("{}/{}", self.output, path);
        assert!(path.starts_with("assets/"));
        std::fs::create_dir_all(file_output.rsplit_once('/').unwrap().0).with_context(|| {
            format!("Could not create directory for {path}. Please check your permissions.",)
        })?;
        std::fs::write(&file_output, content).with_context(|| {
            format!("Could not write file to {path}. Please check your permissions.",)
        })?;
        Ok(())
    }

    pub(crate) async fn download_case_data(
        &mut self,
        case: &mut Case,
        pb: &ProgressBar,
    ) -> Result<()> {
        let paths = &self.site_data.site_paths;
        let used_sprites = case.get_used_sprites();
        let data = case
            .trial_data
            .as_object_mut()
            .context("Trial data must be an object")?;
        let cloned_profiles = data["profiles"].clone();
        let id_profiles: HashMap<i64, &str> = cloned_profiles
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_object())
            .map(|x| {
                (
                    x["id"].as_i64().expect("profile ID must be number"),
                    x["base"].as_str().expect("profile base must be string"),
                )
            })
            .collect();

        let used_default_sprites = used_sprites
            .into_iter()
            // Only non-positive sprite IDs are default sprites.
            .filter(|x| x.1 < 0)
            .map(|x| (id_profiles[&x.0], -x.1))
            .unique()
            .collect_vec();
        trace!("{:?}", used_default_sprites);

        const SPRITE_KINDS: [&str; 3] = ["talking", "still", "startup"];

        // Download only the default sprites that ended up actually being used.
        for (base, i) in used_default_sprites {
            for kind in SPRITE_KINDS {
                if kind == "startup"
                    && !self
                        .site_data
                        .default_data
                        .default_profiles_startup
                        .contains(&format!("{base}/{i}"))
                {
                    continue;
                }
                self.collector.collect_download_with_name(
                    &mut Value::String(format!("{i}.gif")),
                    &format!("assets/{base}_{i}_{kind}.gif"),
                    Some(paths.sprite_path(kind, base)),
                    Some(false),
                    false,
                );
            }
        }

        // Download the profiles.
        let profiles = data["profiles"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .filter_map(|x| x.as_object_mut());
        for profile in profiles {
            if profile["icon"].as_str().is_none_or(|x| x.is_empty()) {
                // This does not use an external URL.
                // To avoid too many bookkeeping shenanigans here, we just
                // override icon with the URL to the base AAO asset as if it were external.
                profile["icon"] = profile["base"]
                    .as_str()
                    .map(|x| Value::String(format!("{}/{x}.png", paths.icon_path().join("/"))))
                    .unwrap_or(Value::Null);
            }
            self.collector
                .collect_download(&mut profile["icon"], None, Some(true), Some("png"));
            //profile["base"] = Value::Null;

            // Profiles may also contain custom sprites.
            for custom in profile["custom_sprites"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .map(|x| x.as_object_mut().expect("Custom sprite must be object"))
            {
                for kind in SPRITE_KINDS {
                    if custom[kind].as_str().is_none_or(|x| x.is_empty()) {
                        continue;
                    }
                    self.collector.collect_download(
                        &mut custom[kind],
                        None,
                        Some(true),
                        Some("gif"),
                    );
                }
            }
        }

        // Download the evidence.
        for evidence in data["evidence"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .filter_map(|x| x.as_object_mut())
        {
            // Evidence can contain two types of assets:
            // 1.) Icons.
            let external = evidence["icon_external"].as_bool();
            self.collector.collect_download(
                &mut evidence["icon"],
                Some(paths.evidence_path()),
                external,
                Some("png"),
            );
            evidence["icon_external"] = Value::Bool(true);

            // 2.) "Check button data", which may be an image or a sound.
            // NOTE: It seems like this isn't actually preloaded by the player. Is that intentional?
            for check_data in evidence["check_button_data"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .map(|x| x.as_object_mut().unwrap())
                // If this is just text, we can safely ignore it.
                .filter(|x| x["type"].as_str().unwrap_or("text") != "text")
            {
                self.collector
                    .collect_download(&mut check_data["content"], None, None, None);
            }
        }

        // Download the places.
        // Default places are of the form {id: place,...} where we're only interested in the place.
        let default_places = self
            .site_data
            .default_data
            .default_places
            .as_object_mut()
            .context("Default places must be map")?
            .values_mut();
        for place in data["places"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .chain(default_places)
            .filter_map(|x| x.as_object_mut())
        {
            // Download place background itself.
            if let Some(background) = place["background"].as_object_mut() {
                // This may just be a color instead of ana actual image.
                // (In the case of default places).
                if background.contains_key("image") {
                    let external = background["external"]
                        .as_bool()
                        .or_else(|| background["external"].as_i64().map(|x| x == 1))
                        .context("external must be bool")?;
                    self.collector.collect_download(
                        &mut background["image"],
                        Some(paths.bg_path()),
                        Some(external),
                        Some("jpg"),
                    );
                    background["external"] = Value::Bool(true);
                }
            }

            // Download background objects.
            for bg_object in place["background_objects"]
                .as_array_mut()
                .map(|x| x.iter_mut().filter_map(|y| y.as_object_mut()))
                .context("Background objects must be in an array!")?
            {
                if !bg_object["external"]
                    .as_bool()
                    .or_else(|| bg_object["external"].as_i64().map(|x| x == 1))
                    .unwrap_or(false)
                {
                    warn!("Found non-external background object, even though these should always be external! Skipping.");
                    continue;
                }
                self.collector
                    .collect_download(&mut bg_object["image"], None, Some(true), None);
            }

            // Download foreground objects.
            for fg_object in place["foreground_objects"]
                .as_array_mut()
                .map(|x| x.iter_mut().filter_map(|y| y.as_object_mut()))
                .context("Background objects must be in an array!")?
            {
                if !fg_object["external"]
                    .as_bool()
                    .or_else(|| fg_object["external"].as_i64().map(|x| x == 1))
                    .unwrap_or(false)
                {
                    warn!("Found non-external foreground object, even though these should always be external! Skipping.");
                    continue;
                }
                self.collector
                    .collect_download(&mut fg_object["image"], None, Some(true), None);
            }
        }

        // Download the popups.
        for popup in data["popups"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .filter_map(|x| x.as_object_mut())
        {
            let external = popup["external"]
                .as_bool()
                .context("External must be bool!")?;
            self.collector.collect_download(
                &mut popup["path"],
                Some(paths.popup_path()),
                Some(external),
                Some("gif"),
            );
            popup["external"] = Value::Bool(true);
        }

        // Download the music.
        for music in data["music"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .filter_map(|x| x.as_object_mut())
        {
            let external = music["external"]
                .as_bool()
                .context("External must be bool!")?;
            self.collector.collect_download(
                &mut music["path"],
                Some(paths.music_path()),
                Some(external),
                Some("mp3"),
            );
            music["external"] = Value::Bool(true);
        }
        // Download the sound.
        for sound in data["sounds"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .filter_map(|x| x.as_object_mut())
        {
            let external = sound["external"]
                .as_bool()
                .context("External must be bool!")?;
            self.collector.collect_download(
                &mut sound["path"],
                Some(paths.sound_path()),
                Some(external),
                Some("mp3"),
            );
            sound["external"] = Value::Bool(true);
        }

        // Download the voices. These are not present in the trial data, since there are no custom
        // voices.
        const VOICE_EXT: [&str; 3] = ["opus", "wav", "mp3"];
        for i in 1..=3 {
            for ext in VOICE_EXT {
                self.collector.collect_download(
                    &mut Value::String(format!("voice_singleblip_{i}.{ext}")),
                    Some(paths.voice_path()),
                    Some(false),
                    None,
                );
            }
        }

        let downloads = self.collector.get_unique_downloads();
        pb.inc_length(downloads.len() as u64);
        let (successes, failures): (Vec<_>, Vec<_>) = downloads.into_iter().partition_result();
        for (asset, err) in self
            .download_assets(successes, pb)
            .await
            .into_iter()
            .filter_map(|x| x.1.err().map(|e| (Some(x.0), e)))
            .chain(failures.into_iter().map(|e| (None, e)))
        {
            if asset.as_ref().is_some_and(|x| x.ignore_inaccessible) {
                continue;
            }
            error!(
                "Could not download asset at {}: {err}{}",
                asset
                    .map(|x| x.url)
                    .unwrap_or(String::from("[UNKNOWN URL]")),
                if self.args.continue_on_asset_error {
                    " (continuing anyway)"
                } else {
                    " (set --continue-on-asset-error to ignore this)"
                }
            );
            if !self.args.continue_on_asset_error {
                return Err(anyhow!("Could not download asset: {err}"));
            }
        }
        Ok(())
    }
}

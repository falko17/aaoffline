//! Contains functions and methods for downloading case data.

use anyhow::{anyhow, Context, Result};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use bytes::Bytes;
use futures::{stream, FutureExt, StreamExt};
use indicatif::ProgressBar;
use itertools::Itertools;
use log::{debug, error, trace, warn};
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::data::case::Case;
use crate::data::site::SiteData;
use crate::{Args, HttpHandling};

// Returns output file path and file content.
pub(crate) async fn download_url(
    url: &str,
    http_handling: &HttpHandling,
) -> Result<(PathBuf, Bytes)> {
    debug!("Downloading {url}...");
    let output = PathBuf::from("assets").join(url.split('/').last().unwrap_or(url));
    let target = if url.starts_with("http://") {
        match http_handling {
            HttpHandling::AllowInsecure => url.to_string(),
            HttpHandling::RedirectToHttps => url.replacen("http://", "https://", 1),
            HttpHandling::Disallow => {
                return Err(anyhow!("Blocking insecure HTTP request to {url}."))
            }
        }
    } else if url.starts_with("https://") {
        url.to_string()
    } else {
        // Assume this is a relative URL.
        format!("https://aaonline.fr/{}", url.trim_start_matches('/'))
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
    Ok((output, content))
}

pub(crate) fn make_data_url(data: &Bytes) -> Result<String> {
    let mime = infer::get(data)
        .context("Encountered data with unknown MIME type")?
        .mime_type();
    Ok(format!(
        "data:{mime};base64,{}",
        BASE64_STANDARD.encode(data)
    ))
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub(crate) struct AssetDownload {
    url: String,
    path: PathBuf,
    ignore_inaccessible: bool,
}

#[derive(Debug)]
pub(crate) struct AssetDownloader {
    args: Args,
    collector: AssetCollector,
}

#[derive(Debug)]
struct AssetCollector {
    collected: Vec<Result<AssetDownload>>,
    default_icon_url: String,
    output: PathBuf,
}

impl AssetCollector {
    fn new(default_icon_url: String, output: PathBuf) -> AssetCollector {
        AssetCollector {
            collected: vec![],
            default_icon_url,
            output,
        }
    }

    fn hash(value: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn path_exists(&self, path: &PathBuf) -> bool {
        self.collected
            .iter()
            .any(|x| x.as_ref().ok().map_or(false, |x| x.path == *path))
    }

    fn new_path(&self, url: &str, path: &Path) -> PathBuf {
        let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("bin");
        let name = path.file_stem().unwrap_or(path.as_os_str());
        let first_choice = self.output.join("assets").join(name).with_extension(ext);
        // If we used this filename already, we need to append a hash.
        if self.path_exists(&first_choice) {
            self.output
                .join("assets")
                .join(format!(
                    "{}-{}",
                    name.to_str().expect("Invalid filename encountered"),
                    Self::hash(url)
                ))
                .with_extension(ext)
        } else {
            first_choice
        }
    }

    fn get_path(&self, url: &str, file: &Path) -> PathBuf {
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
        filename: Option<&PathBuf>,
        ignore_inaccessible: bool,
    ) -> Result<AssetDownload> {
        let mut file = PathBuf::from(file_value.as_str().unwrap_or(&self.default_icon_url));
        if file.extension().is_none() {
            if let Some(ext) = default_extension {
                file.set_extension(ext);
            }
        }
        let file_string = file.to_str().expect("Invalid path encountered");
        let url = if !external.unwrap_or(true) {
            &format!(
                "https://aaonline.fr/{}/{file_string}",
                path_components
                    .context("Non-external path needs path components!")?
                    .join("/"),
            )
        } else if file_string.starts_with("http") {
            file_string
        } else {
            &format!("https://aaonline.fr/{file_string}")
        };
        let path = self.get_path(url, filename.unwrap_or(&file));
        if filename.is_some_and(|x| !path.ends_with(x)) {
            return Err(anyhow!(
                "Filename mismatch: Should be {}, but was {}",
                filename.unwrap().display(),
                path.display(),
            ));
        }
        let url = Regex::new(r"([^:/])/{2,}")
            .unwrap()
            .replace_all(url, "$1/")
            .to_string();
        trace!("{url} to {}", path.display());
        // Reassign icon to contain new name.
        *file_value = Value::String(
            // We need to strip the output prefix here, as the URL that's written to the trial data
            // should be relative to where the index.html file lives.
            path.strip_prefix(&self.output)?
                .to_owned()
                .into_os_string()
                .into_string()
                .expect("invalid path encountered"),
        );
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
        name: &PathBuf,
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

impl AssetDownloader {
    pub(crate) fn new(args: Args, output: PathBuf, site_data: &SiteData) -> AssetDownloader {
        let default_icon_path = site_data.site_paths.default_icon();
        AssetDownloader {
            args,
            collector: AssetCollector::new(default_icon_path, output),
        }
    }

    pub(crate) fn set_output(&mut self, output: PathBuf) {
        self.collector.output = output;
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

    async fn download_asset(&self, asset: &AssetDownload) -> Result<PathBuf> {
        let (_, content) = download_url(&asset.url, &self.args.http_handling).await?;
        self.write_asset(&asset.path, &content)
            .map(|_| asset.path.clone())
    }

    fn write_asset(&self, path: &PathBuf, content: &[u8]) -> Result<()> {
        // Write to file. We may need to create the containing directories first.
        debug!("Writing {}...", path.display());
        let dir = path.parent().expect("no parent directory in path");
        assert!(dir.ends_with("assets"));
        std::fs::create_dir_all(dir).with_context(|| {
            format!(
                "Could not create directory {}. Please check your permissions.",
                dir.display()
            )
        })?;
        std::fs::write(path, content).with_context(|| {
            format!(
                "Could not write file to {}. Please check your permissions.",
                path.display()
            )
        })?;
        Ok(())
    }

    fn download_psyche_locks(&mut self, site_data: &SiteData, name: &str, max_locks: u8) {
        let mut file_value = Value::String(name.to_string());
        self.collector.collect_download(
            &mut file_value,
            Some(site_data.site_paths.lock_path()),
            Some(false),
            Some("gif"),
        );
        let last = self.collector.collected.last().unwrap().as_ref();
        // Now we need to create the softlinks to this file.
        if let Ok(asset) = last {
            let original_path = &asset.path;
            let original_name = original_path.file_name().expect("must have file path");
            for i in 1..=max_locks {
                let new_path = original_path.with_file_name(format!("{name}_{i}.gif"));
                if let Err(e) = Self::symlink(original_name, &new_path) {
                    warn!("Could not create symbolic link: {e}. Copying file instead.")
                }
            }
        }
    }

    // TODO: Use async version of file operations too.
    #[cfg(unix)]
    fn symlink<P: AsRef<Path>, Q: AsRef<Path>>(orig: P, target: Q) -> Result<(), std::io::Error> {
        std::os::unix::fs::symlink(orig, target)
    }

    #[cfg(windows)]
    fn symlink<P: AsRef<Path>, Q: AsRef<Path>>(orig: P, target: Q) -> Result<(), std::io::Error> {
        std::os::windows::fs::symlink_file(orig, target)
    }

    pub(crate) fn collect_case_data(
        &mut self,
        case: &mut Case,
        site_data: &mut SiteData,
    ) -> Result<Vec<Result<AssetDownload>>> {
        let paths = &site_data.site_paths;
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
                    && !site_data
                        .default_data
                        .default_profiles_startup
                        .contains(&format!("{base}/{i}"))
                {
                    continue;
                }
                self.collector.collect_download_with_name(
                    &mut Value::String(format!("{i}.gif")),
                    &PathBuf::from("assets")
                        .join(format!("{base}_{i}_{kind}"))
                        .with_extension("gif"),
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
        let default_places = site_data
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
                // This may just be a color instead of an actual image.
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
            } else {
                warn!("Encountered place without background!");
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

        // Finally, we need to download psyche-locks. For this, we first need to determine the
        // maximum number of them.
        let max_locks = data["scenes"]
            .as_array()
            .expect("scenes must be array")
            .iter()
            .filter_map(|x| x.as_object())
            .flat_map(|x| x["dialogues"].as_array().expect("dialogues must be array"))
            .filter_map(|x| x.as_object())
            .map(|x| x["locks"].as_object().expect("locks must be object"))
            .map(|x| {
                x["locks_to_display"]
                    .as_array()
                    .expect("locks_to_display must be array")
            })
            .map(|x| x.len())
            .max()
            .unwrap_or(0) as u8;
        if max_locks > 0 {
            const LOCK_NAMES: [&str; 4] = [
                "fg_chains_appear",
                "jfa_lock_appears",
                "jfa_lock_explodes",
                "fg_chains_disappear",
            ];
            for lock in LOCK_NAMES {
                self.download_psyche_locks(site_data, lock, max_locks);
            }
        }

        Ok(self.collector.get_unique_downloads())
    }

    pub(crate) async fn download_collected(
        &mut self,
        pb: &ProgressBar,
        downloads: Vec<Result<AssetDownload>>,
    ) -> Result<()> {
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

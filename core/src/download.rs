//! Contains data structures and methods for downloading case data.

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use bytes::Bytes;
use futures::future::join_all;
use futures::stream::{AbortHandle, Abortable};
use futures::{FutureExt, StreamExt, stream};
use itertools::Itertools;
use log::{debug, error, trace, warn};
use mime2ext::mime2ext;
use regex::Regex;
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest_middleware::ClientWithMiddleware;
use sanitize_filename::sanitize;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::hash_set::Drain;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::iter;
use std::path::{Path, PathBuf};
use std::string::FromUtf8Error;
use tokio::sync::OnceCell;

use crate::ProgressReporter;
use crate::constants::AAONLINE_BASE;
use crate::constants::re::{CONTENT_DISPOSITION_FILENAME_REGEX, REMOVE_QUERY_PARAMETERS_REGEX};
use crate::data::case::Case;
use crate::data::site::{SiteData, SitePaths};
use crate::{GlobalContext, args::HttpHandling};

/// Downloaded content.
pub(crate) struct Download {
    /// The target URL (i.e., after all redirections) from which the content was downloaded.
    pub(crate) target_url: Url,
    /// The content that was downloaded.
    pub(crate) content: Bytes,
    /// The response headers.
    pub(crate) headers: HeaderMap<HeaderValue>,
}

impl Download {
    /// Downloads a file from the given [url] and returns the output path and file content.
    pub(crate) async fn retrieve_url(
        url: &str,
        http_handling: &HttpHandling,
        client: &ClientWithMiddleware,
    ) -> Result<Download> {
        // TODO: Proper timeout for download (test with long sequence)
        debug!("Downloading {url}...");
        let target = if url.starts_with("http://") {
            match http_handling {
                HttpHandling::AllowInsecure => url.to_string(),
                HttpHandling::RedirectToHttps => url.replacen("http://", "https://", 1),
                HttpHandling::Disallow => {
                    return Err(anyhow!("Blocking insecure HTTP request to {url}."));
                }
            }
        } else if url.starts_with("https://") {
            url.to_string()
        } else {
            // Assume this is a relative URL.
            format!("{AAONLINE_BASE}/{}", url.trim_start_matches('/'))
        };

        // Actually download the file.
        let content = client.get(&target).send().await.with_context(|| {
            format!("Could not download file from {target}. Please check your internet connection.")
        })?;
        // NOTE: We need to use the final URL for the output path since the extension may differ.
        let target_url = content.url().clone();
        let content = content.error_for_status()?;
        let headers = content.headers().clone();
        Ok(Self {
            target_url,
            content: content.bytes().await?,
            headers,
        })
    }

    /// Returns the content of this [Download] as a UTF-8 encoded String.
    pub(crate) fn content_str(&self) -> Result<String, FromUtf8Error> {
        String::from_utf8(self.content.to_vec())
    }

    /// Returns the content type of this [Download], as defined by the Content-Type header.
    pub(crate) fn content_type(&self) -> Option<&str> {
        self.headers
            .get("Content-Type")
            .and_then(|x| x.to_str().ok())
            .map(|x| x.split_once(';').map_or(x, |y| y.0))
    }

    /// Converts this [Download] to a base64 data URL.
    pub(crate) fn make_data_url(&self) -> String {
        let mime = self.mime_type().unwrap_or("application/octet-stream");
        format!(
            "data:{mime};base64,{}",
            BASE64_STANDARD.encode(&self.content)
        )
    }

    pub(crate) fn mime_type(&self) -> Option<&str> {
        self.content_type()
            .or_else(|| infer::get(&self.content).map(|x| x.mime_type()))
    }

    /// Returns the filename under which this download should be saved.
    ///
    /// This will first check if an explicit filename has been set in the Content-Disposition
    /// header, and will otherwise use the final path segment of the URL.
    pub(crate) fn filename(&self) -> String {
        if let Some(disposition) = self
            .headers
            .get("Content-Disposition")
            .and_then(|x| x.to_str().ok())
            .and_then(|x| CONTENT_DISPOSITION_FILENAME_REGEX.captures(x))
            .and_then(|x| x.get(1))
        {
            // If a filename is explicitly set using the Content-Disposition header, we'll use it.
            disposition.as_str().to_string()
        } else {
            // We'll assign a path based on the URL's ending.
            let name = self
                .target_url
                .path_segments()
                .and_then(Iterator::last)
                .unwrap_or(self.target_url.path());
            // Remove any query parameters.
            let mut path =
                PathBuf::from(REMOVE_QUERY_PARAMETERS_REGEX.replace(name, "").to_string());
            if path.extension().is_none() {
                if let Some(mime_ext) = self.mime_type().and_then(mime2ext) {
                    path.set_extension(mime_ext);
                }
            }
            path.file_name()
                .unwrap_or(path.as_os_str())
                .to_str()
                .expect("invalid filename encountered")
                .to_string()
        }
    }
}

/// Hashes the given [value] to a u64.
fn hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// An asset download request.
///
/// Two download requests are considered equal iff their URLs and output paths are the same.
#[derive(Debug, Clone, Eq)]
pub(crate) struct AssetDownload {
    /// The URL to download from.
    url: String,
    /// The path to save the downloaded file to.
    ///
    /// Once this is written to, it must not be changed.
    path: OnceCell<String>,
    /// References to the original JSON containing the path to this asset.
    ///
    /// May be more than one if multiple paths reference this asset.
    json_refs: HashSet<JsonReference>,
    /// The case title for which this asset is downloaded.
    case_title: String,
    /// The path into which this asset should be put.
    output_path: PathBuf,
}

impl PartialEq for AssetDownload {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url && self.output_path == other.output_path
    }
}

impl Hash for AssetDownload {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.url.hash(state);
        self.output_path.hash(state);
    }
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
struct JsonReference {
    /// A JSON pointer (see [RFC 6901](https://datatracker.ietf.org/doc/html/rfc6901)) to the
    /// referenced part of the original JSON.
    pointer: String,
    /// The document from which the JSON originally came from.
    source: JsonSource,
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
enum JsonSource {
    /// An asset in the case data. The inner value represents the case ID.
    CaseData(u32),
    /// A default place.
    DefaultPlaces,
    /// A default voice.
    /// The inner values represent the ID and the extension, respectively.
    DefaultVoices(u64, String),
    /// A default sprite.
    /// The inner values represent the base, sprite ID, and kind, respectively.
    DefaultSprites(String, i64, String),
    /// A psyche-lock file. The inner value represents the filename.
    PsycheLock(String),
}

impl JsonReference {
    fn new(source: JsonSource, pointer: String) -> JsonReference {
        JsonReference { pointer, source }
    }

    fn for_case(case_id: u32, pointer: String) -> JsonReference {
        JsonReference {
            pointer,
            source: JsonSource::CaseData(case_id),
        }
    }

    fn concat_path(&self, path: &str) -> JsonReference {
        JsonReference::new(self.source.clone(), format!("{}/{path}", self.pointer))
    }
}

/// A downloader for case assets.
#[derive(Debug)]
pub(crate) struct AssetDownloader<'a> {
    /// The collector that will remember our asset download requests.
    collector: AssetCollector,
    /// A reference to the global context for the program.
    ctx: &'a GlobalContext,
}

/// Collects asset downloads and assigns unique filenames to them.
#[derive(Debug)]
struct AssetCollector {
    /// The collected asset downloads.
    collected: HashSet<AssetDownload>,
    /// The default icon URL.
    default_icon_url: String,
    /// The output directory for the assets.
    output: PathBuf,
    /// The case title for which assets are currently collected.
    target_case_title: Option<String>,
}

impl AssetCollector {
    /// Creates a new asset collector.
    fn new(default_icon_url: String, output: PathBuf) -> AssetCollector {
        AssetCollector {
            collected: HashSet::new(),
            default_icon_url,
            output,
            target_case_title: None,
        }
    }

    /// Returns the [collected] asset whose URL matches the given [url] and the currently set
    /// [output] path.
    fn find_by_url(&self, url: &str) -> Option<&AssetDownload> {
        self.collected
            .iter()
            .find(|x| x.url == url && x.output_path == self.output)
    }

    /// Creates a new unique path for the given [url] and [path].
    ///
    /// The [url] will be used for hashing only, to ensure a unique output name,
    /// while the [path]'s filename will be used for the output path's filename.
    fn new_path(output: &Path, url: &str, path: &Path) -> PathBuf {
        let ext = if let Some(ext) = path.extension().and_then(|x| x.to_str()) {
            ext
        } else {
            const DEFAULT_EXT: &str = "bin";
            warn!("Unknown extension for {url}! Setting to '.{DEFAULT_EXT}'.",);
            DEFAULT_EXT
        };
        let name = path
            .file_stem()
            .unwrap_or(path.as_os_str())
            .to_str()
            .expect("invalid filename encountered")
            .to_string();
        let name = urlencoding::decode(&name)
            .map(Cow::into_owned)
            .map(|x| x.replace('%', "-"))
            .unwrap_or(name);
        let path = output
            .join("assets")
            .join(sanitize(format!("{name}-{}", hash(url))).to_lowercase())
            .with_extension(ext);
        assert!(
            path.parent()
                .expect("parent dir must exist")
                .ends_with("assets"),
            "must end with assets/ but doesn't: {}",
            path.display()
        );
        path
    }

    /// Creates a new asset download request.
    ///
    /// The [`file_value`] will be replaced with the new path[^1]. The given [`path_components`]
    /// will be put in front of the URL, and the [`external`] flag will determine whether the URL
    /// is hosted on Ace Attorney Online or externally. The [`default_extension`] will be applied
    /// if the given [`file_value`] has no extension and if its host is the Ace Attorney Online
    /// server. If a [`filename`] is given, it will be used (or try to be used, as long as it
    /// hasn't been used yet) as the filename for the asset.
    ///
    /// [^1]: Note that this will be the "case-local" path to the asset, which is distinct from the
    /// "case-global" path to the asset where it will be saved relative to the current directory.
    /// The "case-local" path should, for example, always start with "assets/".
    fn collect_download(
        &mut self,
        file_value: &Value,
        path_components: Option<Vec<&str>>,
        external: Option<bool>,
        default_extension: Option<&str>,
        json_ref: JsonReference,
        force_early_name: bool,
    ) -> Option<&AssetDownload> {
        // First, we need to construct the URL to the target resource out of the given parameters.
        let file_string = file_value.as_str().unwrap_or(&self.default_icon_url).trim();
        let non_aao: bool = external.unwrap_or(true) && file_string.starts_with("http");
        if file_string.is_empty() {
            // We can ignore empty URLs.
            return None;
        }
        let mut file = PathBuf::from(file_string);
        if !non_aao && file.extension().is_none() {
            if let Some(ext) = default_extension {
                file.set_extension(ext);
            }
        }
        let file_string = file.to_str().expect("Invalid path encountered");
        let url = if !external.unwrap_or(true) {
            &format!(
                "{AAONLINE_BASE}/{}/{file_string}",
                path_components
                    .expect("Non-external path needs path components!")
                    .join("/"),
            )
        } else if non_aao {
            file_string
        } else {
            &format!("{AAONLINE_BASE}/{file_string}")
        };

        // Get rid of multiple consecutive slashes.
        let url = Regex::new(r"([^:/])/{2,}")
            .unwrap()
            .replace_all(url, "$1/")
            .to_string();

        trace!("Creating asset for {url}");
        let asset = AssetDownload {
            url: url.clone(),
            path: OnceCell::new(),
            json_refs: HashSet::from([json_ref]),
            case_title: self
                .target_case_title
                .as_ref()
                .expect("case title must be set here")
                .clone(),
            output_path: self.output.clone(),
        };
        let target_asset = if let Some(mut existing) = self.collected.take(&asset) {
            // If an asset with this URL exists already, we'll add our JsonRef to it.
            debug!("Duplicate asset for {url}");
            existing.json_refs.extend(asset.json_refs);
            // Then, re-add to the set.
            existing
        } else {
            asset
        };
        if force_early_name {
            let target_path = Self::new_path(&self.output, &target_asset.url, &file)
                .to_str()
                .expect("invalid path")
                .to_string();
            target_asset
                .path
                .set(target_path)
                .expect("path was already set");
        }
        self.collected.insert(target_asset);
        self.find_by_url(&url)
    }

    /// Makes the given [`path`] relative to the given [`output`] directory.
    fn path_to_relative<'a>(path: &'a Path, output: &Path) -> &'a Path {
        // We need to strip the output prefix here, as the URL that's written to the trial data
        // should be relative to where the index.html file lives.
        path.strip_prefix(output).unwrap_or_else(|_| {
            panic!(
                "could not strip prefix {} of {}",
                path.display(),
                output.display()
            )
        })
    }
}

impl<'a> AssetDownloader<'a> {
    /// Creates a new asset downloader.
    ///
    /// The [`site_data`] is used to determine the default icon path.
    pub(crate) fn new(
        output: PathBuf,
        site_data: &SiteData,
        ctx: &'a GlobalContext,
    ) -> AssetDownloader<'a> {
        let default_icon_path = site_data.site_paths.default_icon();
        AssetDownloader {
            collector: AssetCollector::new(default_icon_path, output),
            ctx,
        }
    }

    /// Sets the output directory for the collected assets, creating it if necessary.
    pub(crate) async fn set_output(&mut self, output: PathBuf) -> Result<(), std::io::Error> {
        // May need to create the directory first.
        if !self.ctx.args.one_html_file {
            self.ctx
                .writer
                .create_dir_all(&output.join("assets"))
                .await?;
        }
        self.collector.output = output;
        Ok(())
    }

    /// Handles the given asset [`err`] that occurred for the given [`case_title`] by
    /// emitting an error message and possibly terminating the program (depending on [`args`]).
    fn handle_asset_error(&self, err: &anyhow::Error, case_title: Option<&str>) {
        error!(
            "Could not download asset for case {}: {err}{}",
            case_title.unwrap_or("[UNKNOWN CASE]"),
            if self.ctx.args.continue_on_asset_error {
                " (continuing anyway)"
            } else {
                " (set --continue-on-asset-error to ignore this)"
            }
        );
    }

    /// Downloads the given [assets] **in parallel** with the configured number of concurrent downloads.
    ///
    /// Returns a list of [assets] that were successfully downloaded.
    async fn download_assets(
        &self,
        assets: Vec<AssetDownload>,
        pb: &dyn ProgressReporter,
    ) -> Result<Vec<AssetDownload>> {
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        let total_assets = assets.len();
        let result = Abortable::new(
            stream::iter(assets).map(|mut asset| async {
                let download = self
                    .download_asset(&mut asset)
                    .map(|x| {
                        pb.inc(1);
                        x
                    })
                    .await;
                download
                    .inspect_err(|e| {
                        self.handle_asset_error(e, Some(&asset.case_title));
                        if !self.ctx.args.continue_on_asset_error {
                            abort_handle.abort();
                        }
                    })
                    .map(|()| asset)
                    .ok()
            }),
            abort_registration,
        )
        .buffer_unordered(self.ctx.args.concurrent_downloads)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

        if result.len() < total_assets {
            if self.ctx.args.continue_on_asset_error {
                let failed = total_assets - result.len();
                warn!(
                    "{failed} asset download{} failed, continuing anyway.",
                    if failed == 1 { "" } else { "s" }
                );
                Ok(result)
            } else {
                Err(anyhow!("Asset download failed, aborting case download."))
            }
        } else {
            Ok(result)
        }
    }

    /// Downloads the given [asset] and writes it to its set path.
    async fn download_asset(&self, asset: &mut AssetDownload) -> Result<()> {
        let download =
            Download::retrieve_url(&asset.url, &self.ctx.args.http_handling, &self.ctx.client)
                .await?;
        if self.ctx.args.one_html_file {
            // No need to write data anywhere but in the data URL.
            asset
                .path
                .set(download.make_data_url())
                .expect("path must not be set already");
        } else if let Some(path) = asset.path.get() {
            // We need to reuse the existing path here.
            self.ctx
                .writer
                .write_asset(&PathBuf::from(path), &download.content)
                .await?;
        } else {
            let path = AssetCollector::new_path(
                &asset.output_path,
                download.target_url.as_str(),
                Path::new(&download.filename()),
            );
            self.ctx
                .writer
                .write_asset(&path, &download.content)
                .await?;
            asset
                .path
                .set(
                    AssetCollector::path_to_relative(&path, &asset.output_path)
                        .components()
                        .map(|x| x.as_os_str().to_str().expect("invalid path"))
                        .join("/"),
                )
                .expect("path must have been none here");
        }
        Ok(())
    }

    /// Collects the psyche lock file with the given [name], assuming a maximum number of
    /// [`max_locks`] for the case.
    ///
    /// This will attempt to create symbolic links for the psyche lock files, but will fall back to
    /// using hard-links if symbolic links are not supported. Panics if that also doesn't work.
    async fn collect_psyche_locks_file(&mut self, name: &str, max_locks: u8, site_data: &SiteData) {
        let file_value = Value::String(name.to_string());
        let asset = self.collector.collect_download(
            &file_value,
            Some(site_data.site_paths.lock_path()),
            Some(false),
            Some("gif"),
            JsonReference::new(JsonSource::PsycheLock(name.to_string()), String::new()),
            // We need to know the filename in advance when we have to download the psyche locks as
            // files, because we'll create symlinks to them here already.
            !self.ctx.args.one_html_file,
        );
        if self.ctx.args.one_html_file {
            // No need to do the symlinking here, as we're not actually creating files.
            return;
        }
        // Now we need to create the symbolic links to this file.
        if let Some(path) = asset.and_then(|x| x.path.get()) {
            let original_path = PathBuf::from(path);
            let original_name = original_path.file_name().expect("must have file path");
            let writer = &self.ctx.writer;
            join_all(
                (1..=max_locks)
                    .map(|i| original_path.with_file_name(format!("{name}_{i}.gif")))
                    .map(|p| async move {
                        (
                            p.clone(),
                            writer.symlink(Path::new(original_name), &p).await,
                        )
                    })
                    .map(|future| async move {
                        let (p, result) = future.await;
                        if let Err(e) = result {
                            warn!(
                                "Could not create symbolic link: {e}. Hard-linking file instead."
                            );
                            writer.hardlink(Path::new(path), &p).await;
                        }
                    }),
            )
            .await;
        }
    }

    /// Collects the case asset download requests for the given [case] and [`site_data`], returning
    /// the (possibly faulty) requests in a vector.
    ///
    /// *Note: This does not start the downloads yet!*
    pub(crate) async fn collect_case_data(
        &mut self,
        case: &mut Case,
        site_data: &mut SiteData,
    ) -> Result<Drain<'_, AssetDownload>> {
        self.collector.target_case_title = Some(case.case_information.title.clone());
        let paths = &site_data.site_paths;
        let used_sprites = case.get_used_sprites();
        let used_places = case.get_used_places();
        let case_id = case.id();
        let data = case
            .case_data
            .as_object_mut()
            .context("Trial data must be an object")?;

        let cloned_profiles = data["profiles"].clone();
        let used_default_sprites = Self::get_used_default_sprites(&cloned_profiles, used_sprites);
        self.collect_profiles(data, &used_default_sprites, site_data, case_id);

        self.collect_evidence(data, paths, case_id);

        let used_default_places =
            Self::get_used_default_places(&mut site_data.default_data.default_places, &used_places);
        self.collect_places(used_default_places, data, paths, case_id)?;

        self.collect_popups(data, paths, case_id)?;

        self.collect_music(data, paths, case_id)?;

        self.collect_sounds(data, paths, case_id)?;

        self.collect_voices(paths);

        self.collect_psyche_locks(data, site_data).await;

        Ok(self.collector.collected.drain())
    }

    /// Collects the profile assets used in the case.
    fn collect_profiles(
        &mut self,
        data: &mut serde_json::Map<String, Value>,
        used_default_sprites: &[(&str, i64)],
        site_data: &SiteData,
        case_id: u32,
    ) {
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
                self.collector.collect_download(
                    &Value::String(format!("{i}.gif")),
                    Some(site_data.site_paths.sprite_path(kind, base)),
                    Some(false),
                    Some("gif"),
                    JsonReference::new(
                        JsonSource::DefaultSprites((*base).to_string(), *i, kind.to_string()),
                        String::new(),
                    ),
                    false,
                );
            }
        }
        let profiles = data["profiles"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
            .filter_map(|x| x.1.as_object_mut().map(|o| (x.0, o)));
        for (i, profile) in profiles {
            if profile["icon"].as_str().is_none_or(str::is_empty) {
                // This does not use an external URL.
                // To avoid too many bookkeeping shenanigans here, we just
                // override icon with the URL to the base AAO asset as if it were external.
                profile["icon"] = profile["base"].as_str().map_or(Value::Null, |x| {
                    Value::String(format!(
                        "{}/{x}.png",
                        site_data.site_paths.icon_path().join("/")
                    ))
                });
            }
            self.collector.collect_download(
                &profile["icon"],
                None,
                Some(true),
                Some("png"),
                JsonReference::for_case(case_id, format!("/profiles/{i}/icon")),
                false,
            );

            // Profiles may also contain custom sprites.
            for (j, custom) in profile["custom_sprites"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .map(|x| x.as_object_mut().expect("Custom sprite must be object"))
                .enumerate()
            {
                for kind in SPRITE_KINDS {
                    if custom[kind].as_str().is_none_or(str::is_empty) {
                        continue;
                    }
                    self.collector.collect_download(
                        &custom[kind],
                        None,
                        Some(true),
                        Some("gif"),
                        JsonReference::for_case(
                            case_id,
                            format!("/profiles/{i}/custom_sprites/{j}/{kind}"),
                        ),
                        false,
                    );
                }
            }
        }
    }

    /// Collects the evidence assets used in the case.
    fn collect_evidence(
        &mut self,
        data: &mut serde_json::Map<String, Value>,
        paths: &SitePaths,
        case_id: u32,
    ) {
        // Download the evidence.
        for (i, evidence) in data["evidence"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
            .filter_map(|x| x.1.as_object_mut().map(|o| (x.0, o)))
        {
            // Evidence can contain two types of assets:
            // 1.) Icons.
            let external = evidence["icon_external"].as_bool();
            self.collector.collect_download(
                &evidence["icon"],
                Some(paths.evidence_path()),
                external,
                Some("png"),
                JsonReference::for_case(case_id, format!("/evidence/{i}/icon")),
                false,
            );
            evidence["icon_external"] = Value::Bool(true);

            // 2.) "Check button data", which may be an image or a sound.
            // NOTE: It seems like this isn't actually preloaded by the player. Is that intentional?
            for (j, check_data) in evidence["check_button_data"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .map(|x| x.as_object_mut().unwrap())
                .enumerate()
                // If this is just text, we can safely ignore it.
                .filter(|x| x.1["type"].as_str().unwrap_or("text") != "text")
            {
                self.collector.collect_download(
                    &check_data["content"],
                    None,
                    None,
                    None,
                    JsonReference::for_case(
                        case_id,
                        format!("/evidence/{i}/check_button_data/{j}/content"),
                    ),
                    false,
                );
            }
        }
    }

    /// Collects the place assets used in the case.
    fn collect_places(
        &mut self,
        used_default_places: HashMap<i64, &mut Value>,
        data: &mut serde_json::Map<String, Value>,
        paths: &SitePaths,
        case_id: u32,
    ) -> Result<()> {
        self.collect_places_for(
            (0i64..).zip(data["places"].as_array_mut().unwrap().iter_mut()),
            paths,
            &JsonReference::for_case(case_id, String::from("/places")),
        )?;
        self.collect_places_for(
            used_default_places.into_iter(),
            paths,
            &JsonReference::new(JsonSource::DefaultPlaces, String::new()),
        )?;
        Ok(())
    }

    fn collect_places_for<'b, I: Iterator<Item = (i64, &'b mut Value)>>(
        &mut self,
        places: I,
        paths: &SitePaths,
        ref_base: &JsonReference,
    ) -> Result<()> {
        for (i, place) in places.filter_map(|x| x.1.as_object_mut().map(|o| (x.0, o))) {
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
                        &background["image"],
                        Some(paths.bg_path()),
                        Some(external),
                        Some("jpg"),
                        ref_base.concat_path(&format!("{i}/background/image")),
                        false,
                    );
                    background["external"] = Value::Bool(true);
                }
            } else {
                warn!("Encountered place without background!");
            }

            // Download background objects.
            self.collect_place_objects(
                &mut place["background_objects"],
                &ref_base.concat_path(&format!("{i}/background_objects")),
            )?;

            // Download foreground objects.
            self.collect_place_objects(
                &mut place["foreground_objects"],
                &ref_base.concat_path(&format!("{i}/foreground_objects")),
            )?;
        }
        Ok(())
    }

    /// Collects the assets used in the given [place] objects.
    fn collect_place_objects(
        &mut self,
        place: &mut Value,
        ref_base: &JsonReference,
    ) -> Result<(), anyhow::Error> {
        for (i, object) in place
            .as_array_mut()
            .map(|x| x.iter_mut().filter_map(|y| y.as_object_mut()))
            .context("Background/foreground objects must be in an array!")?
            .enumerate()
        {
            if !object["external"]
                .as_bool()
                .or_else(|| object["external"].as_i64().map(|x| x == 1))
                .unwrap_or(false)
            {
                warn!(
                    "Found non-external foreground/background object, even though these should always be external! Skipping."
                );
                continue;
            }
            self.collector.collect_download(
                &object["image"],
                None,
                Some(true),
                None,
                ref_base.concat_path(&format!("{i}/image")),
                false,
            );
        }
        Ok(())
    }

    /// Collects the popups used in the case.
    fn collect_popups(
        &mut self,
        data: &mut serde_json::Map<String, Value>,
        paths: &SitePaths,
        case_id: u32,
    ) -> Result<(), anyhow::Error> {
        for (i, popup) in data["popups"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
            .filter_map(|x| x.1.as_object_mut().map(|o| (x.0, o)))
        {
            let external = popup["external"]
                .as_bool()
                .context("External must be bool!")?;
            self.collector.collect_download(
                &popup["path"],
                Some(paths.popup_path()),
                Some(external),
                Some("gif"),
                JsonReference::for_case(case_id, format!("/popups/{i}/path")),
                false,
            );
            popup["external"] = Value::Bool(true);
        }
        Ok(())
    }

    /// Collects the voice assets used in the case (which are just all voice assets, since there
    /// are no custom voices).
    fn collect_voices(&mut self, paths: &SitePaths) {
        const VOICE_EXT: [&str; 3] = ["opus", "wav", "mp3"];
        for i in 1..=3 {
            for ext in VOICE_EXT {
                self.collector.collect_download(
                    &Value::String(format!("voice_singleblip_{i}.{ext}")),
                    Some(paths.voice_path()),
                    Some(false),
                    None,
                    JsonReference::new(
                        JsonSource::DefaultVoices(i, ext.to_string()),
                        String::new(),
                    ),
                    false,
                );
            }
        }
    }

    /// Collects the psyche lock assets used in the case.
    async fn collect_psyche_locks(
        &mut self,
        data: &mut serde_json::Map<String, Value>,
        site_data: &mut SiteData,
    ) {
        // To download psyche locks, we first need to determine the maximum number of them.
        let max_locks = data["scenes"]
            .as_array()
            .expect("scenes must be array")
            .iter()
            .filter_map(|x| x.as_object())
            .flat_map(|x| x["dialogues"].as_array().expect("dialogues must be array"))
            .filter_map(|x| x.as_object())
            .filter_map(|x| x["locks"].as_object())
            .filter_map(|x| x["locks_to_display"].as_array())
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .try_into()
            .expect("Too many psyche locks!");

        if max_locks > 0 {
            const LOCK_NAMES: [&str; 4] = [
                "fg_chains_appear",
                "jfa_lock_appears",
                "jfa_lock_explodes",
                "fg_chains_disappear",
            ];
            for lock in LOCK_NAMES {
                self.collect_psyche_locks_file(lock, max_locks, site_data)
                    .await;
            }
        }
    }

    /// Collects the music assets used in the case.
    fn collect_music(
        &mut self,
        data: &mut serde_json::Map<String, Value>,
        paths: &SitePaths,
        case_id: u32,
    ) -> Result<(), anyhow::Error> {
        for (i, music) in data["music"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
            .filter_map(|x| x.1.as_object_mut().map(|o| (x.0, o)))
        {
            let external = music["external"]
                .as_bool()
                .context("External must be bool!")?;
            self.collector.collect_download(
                &music["path"],
                Some(paths.music_path()),
                Some(external),
                Some("mp3"),
                JsonReference::for_case(case_id, format!("/music/{i}/path")),
                false,
            );
            music["external"] = Value::Bool(true);
        }
        Ok(())
    }

    /// Collects the sound assets used in the case.
    fn collect_sounds(
        &mut self,
        data: &mut serde_json::Map<String, Value>,
        paths: &SitePaths,
        case_id: u32,
    ) -> Result<(), anyhow::Error> {
        for (i, sound) in data["sounds"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
            .filter_map(|x| x.1.as_object_mut().map(|o| (x.0, o)))
        {
            let external = sound["external"]
                .as_bool()
                .context("External must be bool!")?;
            self.collector.collect_download(
                &sound["path"],
                Some(paths.sound_path()),
                Some(external),
                Some("mp3"),
                JsonReference::for_case(case_id, format!("/sounds/{i}/path")),
                false,
            );
            sound["external"] = Value::Bool(true);
        }
        Ok(())
    }

    /// Downloads the given collected (possibly faulty) [downloads] in parallel.
    pub(crate) async fn download_collected(
        &mut self,
        pb: &dyn ProgressReporter,
        downloads: Vec<AssetDownload>,
        cases: &mut [Case],
        site_data: &mut SiteData,
    ) -> Result<()> {
        pb.inc_length(downloads.len() as u64);
        let downloaded = self.download_assets(downloads, pb).await?;

        // We now need to write back the data URLs into the JSON.
        let mut case_map: HashMap<u32, &mut Case> = cases.iter_mut().map(|x| (x.id(), x)).collect();
        for asset in downloaded {
            Self::rewrite_data(&asset, &mut case_map, site_data);
        }
        Ok(())
    }

    pub(crate) fn rewrite_data(
        data_asset: &AssetDownload,
        case_map: &mut HashMap<u32, &mut Case>,
        site_data: &mut SiteData,
    ) {
        let path = data_asset
            .path
            .get()
            .expect("path must be set here")
            .clone();
        for json_ref in &data_asset.json_refs {
            let path = path.clone();
            trace!("Rewriting {json_ref:?} to {path}");
            match &json_ref.source {
                JsonSource::CaseData(id) => {
                    let data = &mut case_map
                        .get_mut(id)
                        .expect("case with ID must be present")
                        .case_data;
                    // Modify pointee to value with data URL.
                    *data
                        .pointer_mut(&json_ref.pointer)
                        .expect("pointer must be valid") = Value::String(path);
                }
                JsonSource::DefaultPlaces => {
                    // We skip the root part of the string (i.e., "/{id}/rest...") that would be empty.
                    let mut pointer_parts = json_ref.pointer.splitn(3, '/').skip(1);
                    let id = pointer_parts.next().unwrap();
                    let pointer = format!("/{}", pointer_parts.next().unwrap());
                    *site_data
                        .default_data
                        .default_places
                        .get_mut(&id.parse().expect("invalid ID encountered"))
                        .expect("default place ID must be valid")
                        .pointer_mut(&pointer)
                        .expect("pointer must be valid") = Value::String(path);
                }
                JsonSource::DefaultVoices(id, ext) => {
                    site_data
                        .default_data
                        .default_voice_urls
                        .insert((*id, ext.clone()), path);
                }
                JsonSource::DefaultSprites(base, sprite_id, kind) => {
                    site_data
                        .default_data
                        .default_sprite_urls
                        .insert((base.clone(), *sprite_id, kind.clone()), path);
                }
                JsonSource::PsycheLock(name) => {
                    site_data
                        .default_data
                        .psyche_lock_urls
                        .insert(name.clone(), path);
                }
            }
        }
    }

    /// Returns the default places that are actually used in the case, along with their index.
    fn get_used_default_places<'b>(
        default_places: &'b mut HashMap<i64, Value>,
        used_places: &HashSet<i64>,
    ) -> HashMap<i64, &'b mut Value> {
        default_places
            .iter_mut()
            .filter(|x| used_places.contains(x.0))
            .map(|x| (*x.0, x.1))
            .collect()
    }

    /// Returns the default sprites that are actually used in the case (as character base and
    /// sprite ID), based on the given [profiles] and [`used_sprites`].
    fn get_used_default_sprites(
        profiles: &Value,
        used_sprites: Vec<(i64, i64)>,
    ) -> Vec<(&str, i64)> {
        let id_profiles: HashMap<i64, &str> = profiles
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
            // Handle default profile
            .chain(iter::once((0, "Juge")))
            .collect();
        let used_default_sprites = used_sprites
            .into_iter()
            // Only non-positive sprite IDs are default sprites.
            .filter(|x| x.1 < 0)
            .map(|x| (id_profiles[&x.0], -x.1))
            .unique()
            .collect_vec();
        trace!("{:?}", used_default_sprites);
        used_default_sprites
    }
}

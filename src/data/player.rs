//! Contains data model related to the case player and its scripts.

use crate::args::Userscripts;
use crate::constants::{re, AAONLINE_BASE, BITBUCKET_URL};
use crate::download::Download;
use crate::transform::php;
use crate::GlobalContext;
use anyhow::{Context, Result};

use const_format::formatcp;
use futures::{stream, StreamExt};
use indicatif::ProgressBar;
use itertools::Itertools;
use log::{debug, trace, warn};

use regex::{Captures, Regex};
use reqwest_middleware::ClientWithMiddleware;
use serde_json::Value;

use std::collections::HashSet;

use std::ops::Range;

use super::case::Case;
use super::site::SiteData;

/// Merges two JSON objects into one.
///
/// Code adapted from <https://stackoverflow.com/a/54118457>.
fn merge(a: &mut Value, b: Value) {
    if let Value::Object(a) = a {
        if let Value::Object(b) = b {
            for (k, v) in b {
                // Keep entries that are not in b undisturbed.
                if !v.is_null() {
                    merge(a.entry(k).or_insert(Value::Null), v);
                }
            }

            return;
        }
    }

    *a = b;
}

type ModuleTransformer = fn(&SiteData, &str, &mut String) -> Result<()>;

/// A collection of JavaScript modules for the case player.
#[derive(Debug)]
pub(crate) struct PlayerScripts {
    /// The concatenated JavaScript modules for the player.
    pub(crate) scripts: Option<String>,
    /// The global context for this program.
    pub(crate) ctx: GlobalContext,
}

/// The target of a transformation.
#[derive(PartialEq, Eq, Debug)]
enum TransformationTarget {
    /// The case player itself.
    Player,
    /// The JavaScript scripts/modules for the player.
    Scripts,
}

/// A transformation to be applied to the player or its scripts.
struct PlayerTransformation {
    /// The target of the transformation.
    target: TransformationTarget,
    /// The range that shall be replaced by the [replacement].
    range: Range<usize>,
    /// The replacement text for the range.
    replacement: String,
}

impl PlayerTransformation {
    /// Initializes a new transformation with the given [target], [range], and [replacement].
    fn new(target: TransformationTarget, range: Range<usize>, replacement: String) -> Self {
        PlayerTransformation {
            target,
            range,
            replacement,
        }
    }
}

/// A JavaScript module for the AAO player.
#[derive(Debug)]
struct JsModule {
    /// The name of the module.
    name: String,
    /// The names of modules that this module depends on.
    deps: HashSet<String>,
    /// The initialization code of the module.
    init: String,
    /// The contents of this module (excluding the initialization code).
    content: String,
}

impl PlayerScripts {
    /// Retrieves the JavaScript text for the module with the given [name].
    async fn retrieve_js_text(
        client: &ClientWithMiddleware,
        name: &str,
        player_version: &str,
    ) -> Result<String> {
        let url = if name == "default_data" {
            // This is a special case—we can unfortunately not use the source code of AAO here
            // and need to access the rendered version from aaonline.fr, since this is a PHP file.
            formatcp!("{AAONLINE_BASE}/default_data.js.php")
        } else if name == "trial" {
            // This one is also a PHP file, but we don't need the PHP-generated data as we already
            // retrieved it previously.
            &format!("{BITBUCKET_URL}/{player_version}/trial.js.php")
        } else {
            &format!("{BITBUCKET_URL}/{player_version}/Javascript/{name}.js")
        };
        client.get(url).send()
            .await
            .with_context(|| {
                "Could not download scripts from AAO repository. Please check your internet connection."
            })?
            .error_for_status()
            .context("AAO script code seems to be inaccessible.")?
            .text()
            .await
            .context("Script could not be decoded as text")
    }

    /// Retrieves the JS code for the AAO player and all of its recursive dependencies.
    async fn retrieve_js_modules(
        &self,
        site_data: &SiteData,
        pb: Option<&ProgressBar>,
        module_transformer: ModuleTransformer,
    ) -> Result<String> {
        let mut targets: Vec<String> = vec![String::from("player")];
        let mut modules: Vec<JsModule> = vec![];
        // First, we download the modules until all dependencies are satisfied.
        while !targets.is_empty() {
            // First, download all modules currently in the queue in parallel.
            let new_modules: Vec<JsModule> = stream::iter(targets.drain(..).filter(|target| {
                target != "dom_loaded"
                    && target != "page_loaded"
                    && modules.iter().all(|x| &x.name != target)
            }))
            .map(|target| async {
                self.retrieve_js_module(site_data, target, pb, module_transformer)
                    .await
            })
            .buffer_unordered(self.ctx.args.concurrent_downloads)
            .collect::<Vec<Result<JsModule>>>()
            .await
            .into_iter()
            .collect::<Result<Vec<JsModule>>>()?;

            let existing_modules: HashSet<&String> = modules.iter().map(|x| &x.name).collect();
            targets.extend(
                new_modules
                    .iter()
                    .flat_map(|x| x.deps.clone())
                    .filter(|x| !existing_modules.contains(x))
                    .unique(),
            );
            trace!("Targets: {targets:?}");

            modules.extend(new_modules);
        }
        // Then, we combine the modules such that all dependencies are satisfied.
        Ok(Self::combine_js_modules(modules))
    }

    /// Combines the given [`modules`] into JavaScript code such that the modules are loaded in the
    /// correct order, with all dependencies satisfied.
    fn combine_js_modules(mut modules: Vec<JsModule>) -> String {
        let mut satisfied: HashSet<String> =
            HashSet::from([String::from("page_loaded"), String::from("dom_loaded")]);
        let mut mod_text = String::new();
        while !modules.is_empty() {
            trace!(
                "Not satisfied: {}\nSatisfied so far: {satisfied:?}",
                modules
                    .iter()
                    .map(|x| format!("{} with {:?}", x.name, x.deps))
                    .collect_vec()
                    .join("\n")
            );
            let previously_unsatisfied_modules = modules.len();
            modules.retain_mut(|x| {
                if satisfied.is_superset(&x.deps) {
                    // All satisfied, can be entered and removed from modules.
                    let JsModule {
                        name,
                        deps: _,
                        init,
                        content,
                    } = x;
                    satisfied.insert(name.clone());
                    trace!("{name} is satisfied");
                    // We start with a comment identifying this module to make debugging easier.
                    mod_text.push_str(&format!("// {name}.js\n\n"));
                    // Then its init function. This needs to be an actual function (or lambda) because it may
                    // contain `return` statements. We will execute it after every script has been loaded.
                    mod_text.push_str(&format!("initScripts.push(() => {{{init}}});\n"));
                    // And finally, the module content itself (without the module declaration).
                    let mod_content = re::MODULE_REGEX
                        .find(content)
                        .expect("should have matched during retrieval already");
                    content.replace_range(mod_content.start()..mod_content.end(), "\n");
                    mod_text
                        .push_str(&content.replace(&format!("Modules.complete('{name}')"), "\n"));
                    // The following is necessary due to some naming conflicts that otherwise occur.
                    mod_text = mod_text.replace("SoundHowler.", "window.SoundHowler.");
                    false
                } else {
                    // Dependencies not yet satisfied, we'll try again later.
                    trace!("{} is not satisfied", x.name);
                    true
                }
            });
            assert!(
                modules.len() != previously_unsatisfied_modules,
                "Downloaded JavaScript module set is missing dependencies."
            );
        }

        mod_text
    }

    /// Retrieves the JS module with the given [`name`].
    async fn retrieve_js_module(
        &self,
        site_data: &SiteData,
        name: String,
        pb: Option<&ProgressBar>,
        module_transformer: ModuleTransformer,
    ) -> Result<JsModule> {
        debug!("Retrieving JS module {name}");

        let mut text =
            Self::retrieve_js_text(&self.ctx.client, &name, &self.ctx.args.player_version).await?;
        if let Some(x) = pb {
            x.inc(1);
        }

        module_transformer(site_data, &name, &mut text)?;

        let captures = re::MODULE_REGEX
            .captures(&text)
            .context("AAO JS script seemingly changed format, this means the script needs to be updated to work with the newest AAO version.")?;
        let mod_name = captures.get(1).unwrap().as_str();
        assert_eq!(name, mod_name);
        let dep_text = captures.get(2).unwrap().as_str().replace('\'', "\"");
        let dep_value =
            serde_json::from_str::<Value>(&dep_text).context("Could not parse dependency array")?;
        let deps = dep_value
            .as_array()
            .context("Dependency array is not actually an array")?
            .iter()
            .map(|y| y.as_str().map(ToString::to_string))
            .collect::<Option<HashSet<String>>>()
            .context("Dependency array contains some non-strings")?;
        let init = captures.get(3).unwrap().as_str().to_string();

        Ok(JsModule {
            name,
            deps,
            init,
            content: text,
        })
    }

    /// Retrieves the player scripts for the given [player] and transforms them.
    ///
    /// Each JavaScript module has three things (AFAICT):
    /// 1. A name.
    /// 2. Depdendencies, as an array of other names that should be loaded before this one.
    /// 3. An init function that should be called after dependencies are loaded.
    ///
    /// We want the case to work fully offline, so we need to handle the dependency resolution
    /// at download time (i.e., now). The entry point for these is `player.js`.
    pub(crate) async fn retrieve_player_scripts(
        &mut self,
        site_data: &SiteData,
        pb: &ProgressBar,
        transform_module: ModuleTransformer,
    ) -> Result<()> {
        pb.inc_length(37);
        let config = serde_json::to_string(&site_data.site_paths)?;
        let common_js = Download::retrieve_url(
            format!(
                "{BITBUCKET_URL}/{}/Javascript/common.js",
                self.ctx.args.player_version
            )
            .as_str(),
            &self.ctx.args.http_handling,
            &self.ctx.client,
        )
        .await?;
        pb.inc(1);
        self.scripts = Some(format!(
            "var cfg = {config};
function getFileVersion(path_components)
{{
    // We are not using file versions here.
		return '';
}}
{}

let initScripts = [];
{}
window.addEventListener('load', function() {{
    // Execute all init functions in order.
    initScripts.forEach((x) => x());
}}, false);\n",
            common_js.content_str()?,
            self.retrieve_js_modules(site_data, Some(pb), transform_module)
                .await?
        ));
        Ok(())
    }
}

/// The player for an Ace Attorney Online case.
#[derive(Debug)]
pub(crate) struct Player {
    /// The site data for the Ace Attorney Online server.
    pub(crate) site_data: SiteData,
    /// The player's code.
    pub(crate) content: Option<String>,
    /// The scripts used by the player.
    pub(crate) scripts: PlayerScripts,
}

#[derive(Debug, Clone)]
pub(crate) struct SavedPlayerState {
    content: Option<String>,
    scripts: Option<String>,
}

impl Player {
    pub(crate) fn save(&self) -> SavedPlayerState {
        SavedPlayerState {
            content: self.content.clone(),
            scripts: self.scripts.scripts.clone(),
        }
    }

    pub(crate) fn restore(&mut self, state: SavedPlayerState) {
        self.content = state.content;
        self.scripts.scripts = state.scripts;
    }

    /// Creates a new player with the given [args].
    pub(crate) async fn new(ctx: GlobalContext) -> Result<Self> {
        let default_text =
            PlayerScripts::retrieve_js_text(&ctx.client, "default_data", &ctx.args.player_version)
                .await?;
        let site_data = SiteData::from_site_data(&default_text, &ctx.client).await?;
        Ok(Player {
            site_data,
            content: None,
            scripts: PlayerScripts {
                scripts: Some(default_text),
                ctx,
            },
        })
    }

    /// Potentially transforms the module with the given [name] and [content].
    fn transform_module(site_data: &SiteData, name: &str, content: &mut String) -> Result<()> {
        if name == "default_data" {
            // Here we need to insert our modified default data, to avoid the default
            // resources being retrieved from the AAO server.
            site_data.default_data.write_default_module(content)
        } else {
            Ok(())
        }
    }

    /// Retrieves the player code from the AAO repository.
    pub(crate) async fn retrieve_player(&mut self) -> Result<()> {
        let mut player = self.scripts.ctx.client.get(format!(
            "{BITBUCKET_URL}/{}/player.php",
            self.scripts.ctx.args.player_version
        ))
        .send()
        .await
        .with_context(|| {
            "Could not download player from AAO repository. Please check your internet connection."
        })?
        .error_for_status()
        .context("AAO player code seems to be inaccessible.")?
        .text()
        .await?;

        player.insert(0, '\n');
        self.content = Some(player);
        Ok(())
    }

    /// Retrieves the player scripts from the AAO repository.
    pub(crate) async fn retrieve_scripts(&mut self, pb: &ProgressBar) -> Result<()> {
        self.scripts
            .retrieve_player_scripts(&self.site_data, pb, Self::transform_module)
            .await
    }

    /// Retrieves the userscripts and appends them to the player scripts.
    pub(crate) async fn retrieve_userscripts(&mut self, pb: &ProgressBar) -> Result<()> {
        const HTML_END: &str = "</html>";
        let urls = Userscripts::all_urls(&self.scripts.ctx.args.with_userscripts);
        let client = &self.scripts.ctx.client;
        let userscripts = stream::iter(urls)
            .map(|url| async move {
                debug!("Downloading userscript {url}...");
                pb.inc(1);
                client
                    .get(url)
                    .send()
                    .await
                    .context("Could not download userscript.")?
                    .error_for_status()
                    .context("Userscript seems to be inaccessible.")?
                    .text()
                    .await
                    .context("Script could not be decoded as text")
            })
            .buffer_unordered(self.scripts.ctx.args.concurrent_downloads)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .flatten()
            .join("\n\n");
        let content = self.content.as_mut().expect("player must be present");
        let html_end = content
            .rfind(HTML_END)
            .expect("end of player must be present");
        let replacement =
            format!("<script type=\"text/javascript\">{userscripts}</script>\n{HTML_END}");
        content.replace_range(html_end..html_end + HTML_END.len(), &replacement);
        Ok(())
    }

    // Transforms the player and its scripts to work offline.
    pub(crate) fn transform_player(&mut self, case: &Case) -> Result<()> {
        // We need to temporarily move the scripts out here, or the borrow checker will complain.
        let mut scripts = self.scripts.scripts.take().unwrap();
        // Important: Trial needs to be modified before player, as the trial will then be inserted
        // as part of the scripts into the player.
        php::transform_trial_blocks(&self.scripts, case, &mut scripts)?;
        self.scripts.scripts = Some(scripts);
        php::transform_player_blocks(self.content.as_mut().unwrap(), &self.scripts, case)
    }

    /// Applies the given [regex] to both the [player] and the [scripts] and returns a list of the
    /// targets it was applied to and their captures.
    fn regex_for_both<'b>(
        regex: &'b Regex,
        player: &'b str,
        scripts: &'b str,
    ) -> Vec<(TransformationTarget, Captures<'b>)> {
        regex
            .captures_iter(player)
            .map(|x| (TransformationTarget::Player, x))
            .chain(
                regex
                    .captures_iter(scripts)
                    .map(|x| (TransformationTarget::Scripts, x)),
            )
            .collect()
    }

    /// Retrieves the player's miscellaneous external sources (e.g., sources mentioned in CSS URLs)
    /// and transforms them to work offline.
    pub(crate) async fn retrieve_player_misc_sources(&mut self, pb: &ProgressBar) -> Result<()> {
        const PRELOAD: &str = "preload: true";

        let mut replacements: Vec<PlayerTransformation> = Vec::new();
        let player = self.content.as_ref().unwrap();
        let scripts = self.scripts.scripts.as_ref().unwrap();
        // We need to remove the Google Analytics tag at the bottom of the page.
        if let Some(m) = re::GOOGLE_ANALYTICS_REGEX.find(player) {
            replacements.push(PlayerTransformation::new(
                TransformationTarget::Player,
                m.range(),
                String::new(),
            ));
        } else {
            warn!("Could not find Google Analytics tag in player, skipping.");
        }

        let lang_dir = self.site_data.site_paths.lang_dir.clone();
        let css_caps: Vec<_> = Self::regex_for_both(&re::CSS_REGEX, player, scripts);
        let style_caps: Vec<_> = Self::regex_for_both(&re::STYLE_INCLUDE_REGEX, player, scripts);
        let src_caps: Vec<_> = Self::regex_for_both(&re::SRC_REGEX, player, scripts);
        let psy_caps: Vec<_> = Self::regex_for_both(&re::PSYCHE_LOCK_REGEX, player, scripts);
        pb.inc_length(
            (css_caps.len() + style_caps.len() + src_caps.len() + psy_caps.len() + 1) as u64,
        );

        for (target, css) in css_caps {
            let whole = css.get(0).unwrap();
            let group = css.get(1).unwrap();
            let result = Download::retrieve_url(
                group.as_str(),
                &self.scripts.ctx.args.http_handling,
                &self.scripts.ctx.client,
            )
            .await;
            pb.inc(1);

            if let Ok(download) = result {
                replacements.push(PlayerTransformation::new(
                    target,
                    whole.range(),
                    format!("<style>{}</style>", download.content_str()?),
                ));
            } else if let Err(e) = result {
                warn!("Could not download CSS file, skipping: {e}");
            }
        }

        // We also need to handle any dynamic CSS inclusions.
        for (target, include) in style_caps {
            let whole = include.get(0).unwrap();
            let group = include.get(1).unwrap();
            let result = Download::retrieve_url(
                &format!("CSS/{}.css", group.as_str()),
                &self.scripts.ctx.args.http_handling,
                &self.scripts.ctx.client,
            )
            .await;
            pb.inc(1);
            if let Ok(download) = result {
                replacements.push(PlayerTransformation::new(
                    target,
                    whole.range(),
                    String::new(),
                ));
                // Now, we need to put the CSS thing into a <style> tag in the <head>.
                let head_position = player.find("</head>").expect("No closing head found!");
                replacements.push(PlayerTransformation::new(
                    TransformationTarget::Player,
                    head_position..head_position,
                    format!("\n<style>{}</style>", download.content_str()?),
                ));
            } else if let Err(e) = result {
                warn!("Could not download CSS file, skipping: {e}");
            }
        }

        // Additionally, we need to download the language data.
        let lang = re::LANGUAGE_INCLUDE_REGEX
            .captures(scripts)
            .context("Could not find language data in source.")?;
        let config_lang = &self.scripts.ctx.args.language;
        let func_end = lang.get(0).unwrap().end();
        let group = lang.get(1).unwrap();
        let callback = lang.get(2).unwrap();
        trace!("{}", &group.as_str());
        let lang_files =
            serde_json::from_str::<Value>(&format!("[{}]", &group.as_str().replace('\'', "\"")))?;
        let lang_files: Vec<_> = lang_files
            .as_array()
            .context("languages must be array")?
            .iter()
            .map(|x| x.as_str().expect("language file must be string"))
            .collect();
        let mut lang_json = Value::Null;
        pb.inc_length(lang_files.len() as u64);
        for lang_file in lang_files {
            let content = Download::retrieve_url(&format!("{lang_dir}/{config_lang}/{lang_file}.js"), &self.scripts.ctx.args.http_handling, &self.scripts.ctx.client).await
                .context(format!("Could not download language files for {lang_file}. Please make sure the given language {} exists.", self.scripts.ctx.args.language))?.content;
            pb.inc(1);
            let lang = serde_json::from_slice::<Value>(&content)?;
            merge(&mut lang_json, lang);
        }

        // First, we need to replace the language object.
        let lang_range = re::LANGUAGE_REGEX
            .find(scripts)
            .context("Could not find language definition in scripts.")?
            .range();
        replacements.push(PlayerTransformation::new(
            TransformationTarget::Scripts,
            lang_range,
            format!("var lang = {};", serde_json::to_string(&lang_json)?),
        ));

        // Then, we need to make sure the script doesn't try to dynamically retrieve languages.
        // We do this by just setting the list of files to retrieve to an empty list.
        replacements.push(PlayerTransformation::new(
            TransformationTarget::Scripts,
            group.range(),
            String::new(),
        ));
        // We also need to call the provided callback immediately.
        // To do this, we first empty the original callback...
        replacements.push(PlayerTransformation::new(
            TransformationTarget::Scripts,
            callback.range(),
            String::new(),
        ));
        // ...then we add the callback code as a normal block directly below.
        #[allow(clippy::range_plus_one)] // We only accept Ranges, not inclusive ones.
        replacements.push(PlayerTransformation::new(
            TransformationTarget::Scripts,
            func_end..func_end + 1,
            String::from(callback.as_str()),
        ));

        // We download any sources present in the JavaScript or CSS.
        for (target, src) in src_caps {
            let group = src.get(1).or_else(|| src.get(2)).unwrap();
            if group.as_str().starts_with("data:") {
                // No need to do anything about data URLs.
                continue;
            }
            let result = Download::retrieve_url(
                group.as_str(),
                &self.scripts.ctx.args.http_handling,
                &self.scripts.ctx.client,
            )
            .await;
            if let Ok(download) = result {
                replacements.push(PlayerTransformation::new(
                    target,
                    group.range(),
                    download.make_data_url(),
                ));
            } else if let Err(e) = result {
                warn!("Could not download asset, skipping: {e}");
            }
            pb.inc(1);
        }

        // We need howler.js for sound effects.
        if let Some(howler) = re::HOWLER_REGEX.captures(scripts) {
            let configuration = howler.get(1).unwrap().as_str();
            let result = Download::retrieve_url(
                &format!(
                    "{BITBUCKET_URL}/{}/Javascript/howler.js/howler.min.js",
                    self.scripts.ctx.args.player_version
                ),
                &self.scripts.ctx.args.http_handling,
                &self.scripts.ctx.client,
            )
            .await;
            pb.inc(1);
            if let Ok(download) = result {
                // We will include Howler directly, as well as its configuration below.
                let output = format!("{}\n{}", download.content_str()?, configuration);
                replacements.push(PlayerTransformation::new(
                    TransformationTarget::Scripts,
                    howler.get(0).unwrap().range(),
                    output,
                ));
                // We need to use the HTML5 audio option for Howler, or we'll run into CORS errors.
                // See #1 for why some users might still want to disable HTML5 audios.
                // We also don't want to do this if we use data URLs for everything—there can't be
                // any CORS errors then.
                if !self.scripts.ctx.args.one_html_file {
                    let preload_pos = scripts
                        .find(PRELOAD)
                        .context("preload option not present")?;
                    replacements.push(PlayerTransformation::new(
                        TransformationTarget::Scripts,
                        preload_pos + PRELOAD.len()..preload_pos + PRELOAD.len(),
                        format!(", html5: {}", !self.scripts.ctx.args.disable_html5_audio),
                    ));
                } else if self.scripts.ctx.args.disable_html5_audio {
                    warn!("--disable-html5-audio has no effect when used together with --one-html-file (-1), as HTML5 audio is disabled anyway.");
                }
            } else if let Err(e) = result {
                warn!("Could not download Howler.js, skipping: {e}");
            }
        } else {
            warn!("Could not find Howler.js in scripts, skipping.");
        }

        // Since the default voices are always retrieved from the server, we need to change the
        // getVoiceUrl function to point to our local files.
        if let Some(voice) = re::VOICE_REGEX.captures(scripts) {
            let group = voice.get(1).unwrap();
            let output = {
                // We actually need to use the paths/data URLs we collected earlier
                // Unfortunately, the voice blips are retrieved dynamically, so we need to write
                // some JavaScript here to statically return one of our URLs:
                let mut voice_js = String::new();
                for ((id, ext), url) in &self.site_data.default_data.default_voice_urls {
                    voice_js +=
                        &format!("if (-voice_id === {id} && ext === '{ext}') return '{url}';\n");
                }
                // Just return an empty audio URL otherwise.
                voice_js += "return 'data:audio/wav;base64,'";
                voice_js
            };
            replacements.push(PlayerTransformation::new(
                TransformationTarget::Scripts,
                group.range(),
                output,
            ));
        } else {
            warn!("Could not find getVoiceUrl in scripts, skipping.");
        }

        // We need to do the same for the default sprites.
        if let Some(default_sprites) = re::DEFAULT_SPRITES_REGEX.captures(scripts) {
            let group = default_sprites.get(1).unwrap();
            let output = {
                // We actually need to use the paths/data URLs we collected earlier.
                // Similar to the voice blips, we need to write some JavaScript here to handle this.
                let mut sprite_js = String::new();
                for ((base, sprite_id, status), url) in
                    &self.site_data.default_data.default_sprite_urls
                {
                    sprite_js +=
                        &format!("if (base === '{base}' && sprite_id === {sprite_id} && status === '{status}') return '{url}';\n");
                }
                sprite_js += "return 'data:image/gif;base64,'";
                sprite_js
            };
            replacements.push(PlayerTransformation::new(
                TransformationTarget::Scripts,
                group.range(),
                output,
            ));
        } else {
            warn!(
                "Could not find default sprites in scripts, skipping. Some sprites may be missing."
            );
        }

        // Psyche locks are slightly more tricky to replace. In the online version, a query
        // parameter "?id=" is appended to the lock request, this is later used to differentiate
        // individual lock images (even though the underyling image is the same). We cannot use
        // query parameters like this for static HTML files.
        // As a workaround, we'll copy (more precisely, symlink) each psyche lock file nine times
        // (assuming there'll never be more than nine psyche locks at the same time).
        if psy_caps.is_empty() {
            warn!("Could not find psyche locks in scripts, skipping.");
        } else {
            for (target, lock) in psy_caps {
                pb.inc(1);
                let name = if lock.name("type").is_some() {
                    &format!("jfa_lock{}", lock.name("name").unwrap().as_str())
                } else {
                    lock.name("name").unwrap().as_str()
                };
                let path = lock.name("path").unwrap();
                // Note that this isn't the pure ID, but rather something like `+ id`.
                let lock_id = lock.name("id").unwrap().as_str();
                let replacement = if self.scripts.ctx.args.one_html_file {
                    // We need to insert our data URLs here.
                    // However, we have a conundrum related to the problem mentioned in the
                    // paragraph above: We cannot copy any files around and give them different
                    // names, as there *are no* files. We need some way to make the data URLs
                    // unique, even though they represent the same data. So what we'll do
                    // here is a very ugly trick: Browsers seem to ignore the MIME type in the data
                    // URLs, so we'll append the ID after the MIME type and thus create a
                    // "unique" "file" for every psyche lock.
                    if let Some(data_url) = &self.site_data.default_data.psyche_lock_urls.get(name)
                    {
                        format!("'{}'", data_url.replace(';', &format!("'{lock_id} + ';")))
                    } else {
                        // Case doesn't use any psyche-locks, this doesn't matter.
                        String::new()
                    }
                } else {
                    // This is the only asset whose file name we know for sure (since we need to
                    // create symlinks too), refer to `collect_psyche_locks_file`.
                    format!("'assets/{name}_'{lock_id} + '.gif'",)
                };
                replacements.push(PlayerTransformation::new(target, path.range(), replacement));
            }
        }

        // We disable preloading, it only leads to some errors (since we do not download all
        // default sprites) and we don't gain anything, since the assets are already local.
        if let Some(default_places) = re::PRELOAD_REGEX.find(scripts) {
            replacements.push(PlayerTransformation::new(
                TransformationTarget::Scripts,
                default_places.range(),
                String::from("return;"),
            ));
        } else {
            warn!("Could not find default place preloading in scripts, skipping.");
        }

        // There is a weird bug that I so far only noticed with Strangers in the
        // Land of Turnabouts (106832). In the trial segments, Albert's and Phoenix'
        // sprites quite frequently only appear after they're done speaking their first
        // line of text, staying invisible until then. This only happens in the offline version.
        // The cause seems to be that the sprite's dimensions are used before they're loaded,
        // leading to them being 0, this is what the replacement below patches. However, this isn't
        // the true root cause—the callback using the image dimensions is only called after the
        // image is done loading, so this should be impossible. Here, it seems to be triggered for
        // the sprite when the background is done loading, for some reason. I think something in
        // the callback handling system works differently for offline versions (perhaps due to
        // near-zero load times?), but I haven't figured this out yet,
        // so the below will have to do for now.
        let mut found_img = false;
        for img_handler in re::GRAPHIC_ELEMENT_REGEX.find_iter(scripts) {
            found_img = true;
            replacements.push(PlayerTransformation::new(
                TransformationTarget::Scripts,
                // Append after the match.
                img_handler.start()..img_handler.start(),
                String::from(
                    "\nif (img.height == 0) img.height = 192; if (img.width == 0) img.width = 256;\n",
                ),
            ));
        }
        if !found_img {
            warn!("Could not find image handling code, skipping.");
        }

        // When ending a case, it's possible that we might be redirected to another entry
        // of the same sequence. To support this (when the user downloads multiple cases),
        // we need to add another ugly hack here to hard-code the resulting paths.
        if let Some(redirection) = re::REDIRECTION_REGEX.captures(scripts) {
            let target = redirection.get(1).unwrap().as_str();
            let save = redirection.get(2).unwrap().as_str();
            let mut new_redirection = format!("switch (Number.parseInt({target})) {{\n");
            for (id, path) in &self.scripts.ctx.case_output_mapping {
                // The path needs to be relative to each case (so that downloaded cases can be moved).
                let mut target_path = path
                    .strip_prefix(&self.scripts.ctx.output)?
                    .components()
                    .map(|x| x.as_os_str().to_str().expect("invalid path"))
                    .join("/");
                if !self.scripts.ctx.args.one_html_file {
                    // We need to go up one directory first.
                    target_path = format!("../{target_path}");
                }
                target_path = target_path.replace('\'', "\\'");

                new_redirection.push_str(&format!(
                    "case {id}: window.location.href = '{target_path}' + '?{save};\nbreak;\n"
                ));
            }
            new_redirection.push_str("default: window.alert('Target case was not downloaded when this case was written. Please download a sequence of cases together (at once). You can, for example, use `-s every` with aaoffline to do this.');\n}");
            replacements.push(PlayerTransformation::new(
                TransformationTarget::Scripts,
                redirection.get(0).unwrap().range(),
                new_redirection,
            ));
        } else {
            warn!("Could not find redirection in scripts, skipping.");
        }

        // Apply the replacements in reverse order to avoid messing up the ranges.
        replacements.sort_by(|a, b| b.range.start.cmp(&a.range.start));
        for transformation in &replacements {
            let receiver = match transformation.target {
                TransformationTarget::Player => self.content.as_mut().unwrap(),
                TransformationTarget::Scripts => self.scripts.scripts.as_mut().unwrap(),
            };
            receiver.replace_range(transformation.range.clone(), &transformation.replacement);
        }

        // We also need to retrieve any dependencies present in the CSS.
        // This needs to be done AFTER already inserting the CSS, because these need to be applied
        // on the CSS itself.
        debug!("Downloading CSS dependencies...");
        let player = self.content.as_ref().unwrap();
        let mut replacements: Vec<(Range<usize>, String)> = Vec::new();
        let css_src_caps: Vec<_> = re::CSS_SRC_REGEX.captures_iter(player).collect();
        pb.inc_length(css_src_caps.len() as u64);

        for src in css_src_caps {
            let group = src.get(1).unwrap();
            if group.as_str().ends_with("/tick.png") {
                // This is actually commented out, so we can skip it.
                pb.inc(1);
                continue;
            }
            let result = Download::retrieve_url(
                &format!("CSS/{}", group.as_str()),
                &self.scripts.ctx.args.http_handling,
                &self.scripts.ctx.client,
            )
            .await;
            if let Ok(download) = result {
                replacements.push((group.range(), download.make_data_url()));
            } else if let Err(e) = result {
                warn!("Could not download CSS dependency, skipping: {e}");
            }
            pb.inc(1);
        }
        // Order replacements by reverse order of position so we can safely replace them.
        replacements.sort_by(|a, b| b.0.start.cmp(&a.0.start));
        for (range, output) in replacements {
            self.content.as_mut().unwrap().replace_range(range, &output);
        }
        Ok(())
    }
}

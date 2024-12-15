//! Contains data model related to the case player and its scripts.

use crate::constants::{re, BITBUCKET_URL};
use crate::download::{download_url, AssetDownloader};
use crate::transform::php;
use crate::Args;
use anyhow::{Context, Result};

use indicatif::ProgressBar;
use log::{debug, trace, warn};

use serde_json::Value;

use std::collections::HashSet;

use std::ops::Range;

use super::case::Case;
use super::site::SiteData;

type ModuleTransformer = fn(&Player, &str, &mut String) -> Result<()>;

#[derive(Debug)]
pub(crate) struct PlayerScripts {
    pub(crate) scripts: Option<String>,
    encountered_modules: HashSet<String>,
    args: Args,
}

impl PlayerScripts {
    async fn retrieve_js_text(name: &str, player_version: &str) -> Result<String> {
        let url = if name == "default_data" {
            // This is a special case—we can unfortunately not use the source code of AAO here
            // and need to access the rendered version from aaonline.fr, since this is a PHP file.
            "https://aaonline.fr/default_data.js.php"
        } else if name == "trial" {
            // This one is also a PHP file, but we don't need the PHP-generated data as we already
            // retrieved it previously.
            &format!("{BITBUCKET_URL}/{player_version}/trial.js.php")
        } else {
            &format!("{BITBUCKET_URL}/{player_version}/Javascript/{name}.js")
        };
        reqwest::get(url).await
    .with_context(|| {
        "Could not download scripts from AAO repository. Please check your internet connection."
    })?
    .error_for_status()
    .context("AAO script code seems to be inaccessible.")?
    .text().await.context("Script could not be decoded as text")
    }

    /// Retrieves the JS module with the given [name] and returns the JS code for it.
    ///
    /// If it has any dependencies, these will be recursively retrieved and put before the code of
    /// the target module with the given [name].
    async fn retrieve_js_module(
        &mut self,
        player: &Player,
        name: &str,
        pb: Option<&ProgressBar>,
        module_transformer: ModuleTransformer,
    ) -> Result<String> {
        if name == "dom_loaded" || name == "page_loaded" || self.encountered_modules.contains(name)
        {
            // Page is already loaded.
            return Ok(String::new());
        }
        debug!("Retrieving JS module {name}");
        self.encountered_modules.insert(name.to_string());

        let mut text = Self::retrieve_js_text(name, &self.args.player_version).await?;
        if let Some(x) = pb {
            x.inc(1)
        }

        module_transformer(player, name, &mut text)?;

        let captures = re::MODULE_REGEX
    .captures(&text)
    .context("AAO JS script seemingly changed format, this means the script needs to be updated to work with the newest AAO version.")?;
        let mod_content = captures.get(0).unwrap();
        let mod_name = captures.get(1).unwrap().as_str();
        assert_eq!(name, mod_name);
        let dep_text = captures.get(2).unwrap().as_str().replace("'", "\"");
        let dep_value =
            serde_json::from_str::<Value>(&dep_text).context("Could not parse dependency array")?;
        let deps: Vec<&str> = dep_value
            .as_array()
            .context("Dependency array is not actually an array")?
            .iter()
            .map(|y| y.as_str())
            .collect::<Option<Vec<&str>>>()
            .context("Dependency array contains some non-strings")?;
        let init = captures.get(3).unwrap().as_str();

        // First, we add any dependencies of this module at the top.
        let mut mod_text = String::new();
        for dependency in deps {
            mod_text.push_str(
                &Box::pin(self.retrieve_js_module(player, dependency, pb, module_transformer))
                    .await?,
            );
        }
        // Then, a comment identifying this module to make debugging easier.
        mod_text.push_str(&format!("// {name}.js\n\n"));
        // Then its init function. This needs to be an actual function (or lambda) because it may
        // contain `return` statements. We will execute it after every script has been loaded.
        mod_text.push_str(&format!("initScripts.push(() => {{{init}}});\n"));
        // And finally, the module content itself (without the module declaration).
        trace!("{:?}", mod_content);
        text.replace_range(mod_content.start()..mod_content.end(), "\n");
        text = text.replace(&format!("Modules.complete('{name}')"), "\n");
        mod_text.push_str(&text);
        // The following is necessary due to some naming conflicts that otherwise occur.
        mod_text = mod_text.replace("SoundHowler.", "window.SoundHowler.");
        Ok(mod_text)
    }

    pub(crate) async fn retrieve_player_scripts(
        &mut self,
        player: &Player,
        pb: &ProgressBar,
        transform_module: ModuleTransformer,
    ) -> Result<()> {
        // Each JavaScript module has three things (AFAICT):
        // 1. A name.
        // 2. Depdendencies, as an array of other names that should be loaded before this one.
        // 3. An init function that should be called after dependencies are loaded.
        //
        // We want the case to work fully offline, so we need to handle the dependency resolution
        // at download time (i.e., now). The entry point for these is 'player.js'.

        pb.inc_length(37);
        let config = serde_json::to_string(&player.site_data.site_paths)?;
        let common_js = download_url(
            format!(
                "{BITBUCKET_URL}/{}/Javascript/common.js",
                self.args.player_version
            )
            .as_str(),
            &self.args.http_handling,
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
            String::from_utf8(common_js.1.to_vec())?,
            self.retrieve_js_module(player, "player", Some(pb), transform_module)
                .await?
        ));
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct Player {
    pub(crate) site_data: SiteData,
    pub(crate) args: Args,
    pub(crate) case: Case,
    pub(crate) player: Option<String>,
    pub(crate) scripts: Option<PlayerScripts>,
}

impl Player {
    pub(crate) async fn new(args: Args, case: Case) -> Result<Self> {
        let default_text =
            PlayerScripts::retrieve_js_text("default_data", &args.player_version).await?;
        let site_data = SiteData::from_site_data(&default_text).await?;
        let mut player = Player {
            site_data,
            args: args.clone(),
            case,
            scripts: None,
            player: None,
        };
        player.scripts = Some(PlayerScripts {
            scripts: Some(default_text),
            encountered_modules: HashSet::new(),
            args: args.clone(),
        });
        Ok(player)
    }

    fn transform_module(&self, name: &str, content: &mut String) -> Result<()> {
        // These modules require special handling:
        if name == "trial" {
            // This is really a PHP script, so we need to replace its blocks first.
            php::transform_trial_blocks(self, content)
        } else if name == "default_data" {
            // And here we need to insert our modified default data, to avoid the default
            // resources being retrieved from the AAO server.
            self.site_data.default_data.write_default_module(content)
        } else {
            Ok(())
        }
    }

    pub(crate) async fn retrieve_player(&mut self) -> Result<()> {
        let mut player = reqwest::get(format!(
            "{BITBUCKET_URL}/{}/player.php",
            self.args.player_version
        ))
        .await
        .with_context(|| {
            "Could not download player from AAO repository. Please check your internet connection."
        })?
        .error_for_status()
        .context("AAO player code seems to be inaccessible.")?
        .text()
        .await?;
        trace!("Player: {player}");

        player.insert(0, '\n');
        self.player = Some(player);
        Ok(())
    }

    // Merge function adapted from https://stackoverflow.com/a/54118457.
    fn merge(a: &mut Value, b: Value) {
        if let Value::Object(a) = a {
            if let Value::Object(b) = b {
                for (k, v) in b {
                    // Keep entries that are not in b undisturbed.
                    if !v.is_null() {
                        Self::merge(a.entry(k).or_insert(Value::Null), v);
                    }
                }

                return;
            }
        }

        *a = b;
    }

    pub(crate) async fn retrieve_scripts(&mut self, pb: &ProgressBar) -> Result<()> {
        let mut scripts = self.scripts.take().expect("Scripts not initialized!");
        let transformer: ModuleTransformer =
            |player, name, content| player.transform_module(name, content);
        scripts
            .retrieve_player_scripts(self, pb, transformer)
            .await?;
        self.scripts = Some(scripts);
        Ok(())
    }

    pub(crate) fn transform_player(&mut self) -> Result<()> {
        php::transform_player_blocks(self)
    }

    pub(crate) async fn retrieve_player_misc_sources(
        &mut self,
        output: String,
        pb: &ProgressBar,
    ) -> Result<()> {
        let mut replacements: Vec<(Range<usize>, String)> = Vec::new();
        let player = self.player.as_ref().unwrap();
        // We need to remove the Google Analytics tag at the bottom of the page.
        if let Some(m) = re::GOOGLE_ANALYTICS_REGEX.find(player) {
            replacements.push((m.range(), String::new()));
        } else {
            warn!("Could not find Google Analytics tag in player, skipping.");
        }

        let lang_dir = self.site_data.site_paths.lang_dir.clone();
        let downloader = AssetDownloader::new(self.args.clone(), output, &mut self.site_data);
        let css_caps: Vec<_> = re::CSS_REGEX.captures_iter(player).collect();
        let style_caps: Vec<_> = re::STYLE_INCLUDE_REGEX.captures_iter(player).collect();
        let src_caps: Vec<_> = re::SRC_REGEX.captures_iter(player).collect();
        pb.inc_length((css_caps.len() + style_caps.len() + src_caps.len() + 1) as u64);

        for css in css_caps {
            let whole = css.get(0).unwrap();
            let group = css.get(1).unwrap();
            let result = download_url(group.as_str(), &self.args.http_handling).await;
            pb.inc(1);

            if let Ok((_, content)) = result {
                replacements.push((
                    whole.range(),
                    format!("<style>{}</style>", String::from_utf8(content.to_vec())?),
                ));
            } else if let Err(e) = result {
                warn!("Could not download CSS file, skipping: {e}");
            }
        }

        // We also need to handle any dynamic CSS inclusions.
        for include in style_caps {
            let whole = include.get(0).unwrap();
            let group = include.get(1).unwrap();
            let result = download_url(
                &format!("CSS/{}.css", group.as_str()),
                &self.args.http_handling,
            )
            .await;
            pb.inc(1);
            if let Ok((_, content)) = result {
                replacements.push((whole.range(), String::new()));
                // Now, we need to put the CSS thing into a <style> tag at the top.
                replacements.push((
                    0..0,
                    format!("\n<style>{}</style>", String::from_utf8(content.to_vec())?),
                ));
            } else if let Err(e) = result {
                warn!("Could not download CSS file, skipping: {e}");
            }
        }

        // Additionally, we need to download the language data.
        // TODO: Make some warnings into errors
        let lang = re::LANGUAGE_INCLUDE_REGEX
            .captures(player)
            .context("Could not find language data in source.")?;
        let config_lang = &self.args.language;
        let func_end = lang.get(0).unwrap().end();
        let group = lang.get(1).unwrap();
        let callback = lang.get(2).unwrap();
        trace!("{}", &group.as_str());
        let lang_files =
            serde_json::from_str::<Value>(&format!("[{}]", &group.as_str().replace("'", "\"")))?;
        let lang_files: Vec<_> = lang_files
            .as_array()
            .context("languages must be array")?
            .iter()
            .map(|x| x.as_str().expect("language file must be string"))
            .collect();
        let mut lang_json = Value::Null;
        pb.inc_length(lang_files.len() as u64);
        for lang_file in lang_files {
            let (_, content) = download_url(&format!("{lang_dir}/{config_lang}/{lang_file}.js"), &self.args.http_handling).await
                .context(format!("Could not download language files for {lang_file}. Please make sure the given language {} exists.", self.args.language))?;
            pb.inc(1);
            let lang = serde_json::from_slice::<Value>(&content)?;
            Self::merge(&mut lang_json, lang);
        }

        // First, we need to replace the langauge object.
        let lang_range = re::LANGUAGE_REGEX
            .find(player)
            .context("Could not find language definition in source.")?
            .range();
        replacements.push((
            lang_range,
            format!("var lang = {};", serde_json::to_string(&lang_json)?),
        ));

        // Then, we need to make sure the script doesn't try to dynamically retrieve languages.
        // We do this by just setting the list of files to retrieve to an empty list.
        replacements.push((group.range(), String::new()));
        // We also need to call the provided callback immediately.
        // To do this, we first empty the original callback...
        replacements.push((callback.range(), String::new()));
        // ...then we add the callback code as a normal block directly below.
        replacements.push((func_end..func_end + 1, String::from(callback.as_str())));

        // And we download any sources present in the JavaScript or CSS.
        for src in src_caps {
            let group = src.get(1).or_else(|| src.get(2)).unwrap();
            if group.as_str().starts_with("data:") {
                // No need to do anything about data URLs.
                continue;
            }
            let result = downloader.download_url(group.as_str()).await;
            if let Ok(path) = result {
                replacements.push((group.range(), path));
            } else if let Err(e) = result {
                warn!("Could not download asset, skipping: {e}");
            }
            pb.inc(1);
        }

        // We need howler.js for sound effects.
        if let Some(howler) = re::HOWLER_REGEX.captures(player) {
            let configuration = howler.get(1).unwrap().as_str();
            let result = download_url(
                &format!(
                    "{BITBUCKET_URL}/{}/Javascript/howler.js/howler.min.js",
                    self.args.player_version
                ),
                &self.args.http_handling,
            )
            .await;
            pb.inc(1);
            if let Ok((_, content)) = result {
                // We will include Howler directly, as well as its configuration below.
                let output = format!(
                    "{}\n{}",
                    String::from_utf8(content.to_vec())?,
                    configuration
                );
                replacements.push((howler.get(0).unwrap().range(), output));
                // We need to use the HTML5 audio option for Howler, or we'll run into CORS errors.
                const PRELOAD: &str = "preload: true";
                let preload_pos = player.find(PRELOAD).context("preload option not present")?;
                replacements.push((
                    preload_pos + PRELOAD.len()..preload_pos + PRELOAD.len(),
                    format!(", html5: {}", !self.args.disable_html5_audio),
                ));
            } else if let Err(e) = result {
                warn!("Could not download Howler.js, skipping: {e}");
            }
        } else {
            warn!("Could not find Howler.js in player, skipping.");
        }

        // Since the default voices are always retrieved from the server, we need to change the
        // getVoiceUrl function to point to our local files.
        if let Some(voice) = re::VOICE_REGEX.captures(player) {
            let group = voice.get(1).unwrap();
            let output =
                String::from("return 'assets/voice_singleblip_' + (-voice_id) + '.' + ext;");
            replacements.push((group.range(), output));
        } else {
            warn!("Could not find getVoiceUrl in player, skipping.");
        }

        // We need to do the same for the default sprites.
        if let Some(default_sprites) = re::DEFAULT_SPRITES_REGEX.captures(player) {
            let group = default_sprites.get(1).unwrap();
            let output =
                String::from("return 'assets/' + base + '_' + sprite_id + '_' + status + '.gif';");
            replacements.push((group.range(), output));
        } else {
            warn!(
                "Could not find default sprites in player, skipping. Some sprites may be missing."
            );
        }

        if let Some(default_places) = re::PRELOAD_PLACES_REGEX.find(player) {
            replacements.push((default_places.range(), String::new()));
        } else {
            warn!("Could not find default place preloading in player, skipping.");
        }

        replacements.sort_by(|a, b| b.0.start.cmp(&a.0.start));
        for (range, output) in replacements {
            self.player.as_mut().unwrap().replace_range(range, &output);
        }

        // We also need to retrieve any dependencies present in the CSS.
        // This needs to be done AFTER already inserting the CSS, because these need to be applied
        // on the CSS itself.
        debug!("Downloading CSS dependencies...");
        let player = self.player.as_ref().unwrap();
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
            let result = downloader
                .download_url(&format!("CSS/{}", group.as_str()))
                .await;
            if let Ok(path) = result {
                replacements.push((group.range(), path));
            } else if let Err(e) = result {
                warn!("Could not download CSS dependency, skipping: {e}");
            }
            pb.inc(1);
        }
        // Order replacements by reverse order of position so we can safely replace them.
        replacements.sort_by(|a, b| b.0.start.cmp(&a.0.start));
        for (range, output) in replacements {
            self.player.as_mut().unwrap().replace_range(range, &output);
        }
        Ok(())
    }
}

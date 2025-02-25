//! Contains constants, such as regular expressions or strings.

use const_format::formatcp;

/// Regular expressions used by this crate.
pub(crate) mod re {
    use std::sync::LazyLock;

    use regex::Regex;

    pub(crate) static PHP_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<\?php(.*?)\?>").unwrap());

    pub(crate) static CASE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        // Old URL format: http://aceattorney.sparklin.org/jeu.php?id_proces=TRIAL_ID  (but it
        // could also use the english `player.php?trial_id` format)
        // New URL format: https://aaonline.fr/player.php?trial_id=TRIAL_ID
        Regex::new(r"https?://(?:(?:www\.)?aaonline\.fr|aceattorney\.sparklin\.org)/(?:player\.php\?trial_id|jeu\.php\?id_proces)=(\d+)").unwrap()
    });

    pub(crate) static TRIAL_INFORMATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)var trial_information(?: = JSON\.parse\("(.*?)"\))?;"#).unwrap()
    });

    pub(crate) static TRIAL_DATA_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)var initial_trial_data = JSON\.parse\("(.*?)"\);"#).unwrap()
    });

    pub(crate) static DEFAULT_PROFILES_STARTUP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)var default_profiles_startup = JSON\.parse\("(.*?)"\);"#).unwrap()
    });

    pub(crate) static DEFAULT_PLACES_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)var default_places = (\{.*?\});").unwrap());

    pub(crate) static CONFIG_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)var cfg = (\{.*?\});").unwrap());

    pub(crate) static MODULE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        // I'm sorry for the mess below. As qntm succinctly put it, the plural of regex is regrets.
        Regex::new(r#"(?sm)Modules\.load\(new Object\(\{\s*name\s*:\s*['"](.*?)['"]\s*,\s*dependencies\s*:\s*(\[.*?\]),\s*init\s*:\s*function\(\)\s*\{(.*?)\}\s*^\}\)\);"#).unwrap()
    });

    pub(crate) static CSS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"<link rel="stylesheet" type="text/css" href="([^"]+\.css)"\s*/>"#).unwrap()
    });

    pub(crate) static STYLE_INCLUDE_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"includeStyle\(['"](.*?)['"]\);"#).unwrap());

    pub(crate) static LANGUAGE_INCLUDE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)Languages\.requestFiles\(\[([^\]]*)\], function\(\)\{\s*(.*?)\s*\}\);")
            .unwrap()
    });

    pub(crate) static LANGUAGE_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"var lang = new Object\(\);").unwrap());

    pub(crate) static CSS_SRC_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"[:\s]url\("?([^")]*)"?\)"#).unwrap());

    pub(crate) static SRC_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?:src=["']([^"']+)["']|\.src\s*=\s*['"]([^'"]*?)['"])"#).unwrap()
    });

    pub(crate) static HOWLER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"includeScript\('howler\.js/howler\.min', false, '', function\(\)\{([^}]*?)\}\);",
        )
        .unwrap()
    });

    pub(crate) static VOICE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)function getVoiceUrl\(voice_id,\s*ext\)\s*\{(.*?)\}").unwrap()
    });

    pub(crate) static DEFAULT_SPRITES_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)getDefaultSpriteUrl\(base, sprite_id, status\)\s*\{(.*?)\}").unwrap()
    });

    pub(crate) static PRELOAD_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"translateNode\(images_loading_label\);").unwrap());

    pub(crate) static GOOGLE_ANALYTICS_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<script>.*?UA-.*?</script>").unwrap());

    pub(crate) static PSYCHE_LOCK_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        // This one may look even worse than the previous ones, but look, I used named capture
        // groups this time! That's an improvement, right? ...Right?
        Regex::new(r#"generateImageElement\((?P<path>cfg\.picture_dir\s*\+\s*cfg\.locks_subdir\s*\+\s*(?P<type>[^+]*?\s*\+\s*)?['"](?P<name>[^'"]*?)\.gif\?id=['"](?P<id>.*?))\);"#).unwrap()
    });

    pub(crate) static REDIRECTION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"window\.location\.href\s*=\s*'\?trial_id='\s*\+\s*([^+\s]+)\s*\+\s*'&(save_data=.*?);").unwrap()
    });

    pub(crate) static REMOVE_QUERY_PARAMETERS_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\?.*").unwrap());

    pub(crate) static CONTENT_DISPOSITION_FILENAME_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"filename="([^"]*?)""#).unwrap());

    pub(crate) static GRAPHIC_ELEMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"\s+element\.style\.height\s*=\s*img\.height\s*\+\s*['"]px['"];"#).unwrap()
    });
}

pub(crate) const AAONLINE_BASE: &str = "https://aaonline.fr";

pub(crate) const UPDATE_MESSAGE: &str =
    "This means a new player has been released and the script needs to be updated.";

pub(crate) const BRIDGE_URL: &str = formatcp!("{AAONLINE_BASE}/bridge.js.php");

pub(crate) const BITBUCKET_URL: &str =
    "https://bitbucket.org/AceAttorneyOnline/aao-game-creation-engine/raw/";

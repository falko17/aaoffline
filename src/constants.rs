//! Contains constants, such as regular expressions or strings.

/// Regular expressions used by this crate.
pub(crate) mod re {
    use std::sync::LazyLock;

    use regex::Regex;

    pub(crate) static PHP_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<\?php(.*?)\?>").unwrap());

    pub(crate) static CASE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"https?://(?:www\.)?aaonline\.fr/player\.php\?trial_id=(\d+)").unwrap()
    });

    pub(crate) static TRIAL_INFORMATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)var trial_information(?: = JSON\.parse\("(.*?)"\))?;"#).unwrap()
    });

    pub(crate) static TRIAL_DATA_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)var initial_trial_data = JSON\.parse\("(.*?)"\);"#).unwrap()
    });

    pub(crate) static DEFAULT_PROFILES_NB_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)var default_profiles_nb = JSON\.parse\("(.*?)"\);"#).unwrap()
    });

    pub(crate) static DEFAULT_PROFILES_STARTUP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)var default_profiles_startup = JSON\.parse\("(.*?)"\);"#).unwrap()
    });

    pub(crate) static DEFAULT_PLACES_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)var default_places = (\{.*?\});"#).unwrap());

    pub(crate) static CONFIG_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)var cfg = (\{.*?\});"#).unwrap());

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
        Regex::new(r#"(?s)Languages\.requestFiles\(\[([^\]]*)\], function\(\)\{\s*(.*?)\s*\}\);"#)
            .unwrap()
    });

    pub(crate) static LANGUAGE_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"var lang = new Object\(\);"#).unwrap());

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

    pub(crate) static PRELOAD_PLACES_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"preloadPlaceImages\(default_places\[i\], img_container\)").unwrap()
    });

    pub(crate) static GOOGLE_ANALYTICS_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<script>.*?UA-.*?</script>"#).unwrap());
}

pub(crate) const UPDATE_MESSAGE: &str =
    "This means a new player has been released and the script needs to be updated.";

pub(crate) const BRIDGE_URL: &str = "https://aaonline.fr/bridge.js.php";

pub(crate) const BITBUCKET_URL: &str =
    "https://bitbucket.org/AceAttorneyOnline/aao-game-creation-engine/raw/";

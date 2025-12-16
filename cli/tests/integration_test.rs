use std::{
    error::Error,
    fmt::Display,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use assert_cmd::Command;
use headless_chrome::{
    Browser, LaunchOptionsBuilder,
    browser::tab::EventListener,
    protocol::cdp::{
        Log::{
            LogEntry, LogEntryLevel,
            events::{EntryAddedEvent, EntryAddedEventParams},
        },
        Page::events::JavascriptDialogOpeningEvent,
        types::Event,
    },
};
use itertools::Itertools;
use maplit::hashmap;
use rstest::{fixture, rstest};
use rstest_reuse::{apply, template};
use tempfile::{TempDir, tempdir};

const GAME_OF_TURNABOUTS: &str = "106140";
// This one is also the smallest of these cases, so we will use it frequently.
const PSYCHE_LOCK_TEST: &str = "89247";
// To test if URLs work:
const THE_TORRENTIAL_TURNABOUT: &str = "https://aaonline.fr/player.php?trial_id=99015";
// To test if old URLs work:
const TURNABOUT_OF_COURAGE: &str = "http://aceattorney.sparklin.org/jeu.php?id_proces=27826";
/// To test if old URLs with english names work (this one additionally uses the default sprite ID 0):
const CASCADE_THEATER_ASCENSION: &str = "http://aceattorney.sparklin.org/player.php?trial_id=53593";
// This one has evidence without an icon, which caused issue #3:
const BROKEN_COMMANDMENTS: &str = "140935";
/// This one is a sequence that immediately redirects to the next entry upon proceeding.
const SEQUENCE_TEST: &str = "148564";
/// This one uses assets whose file extensions change after redirects.
const DRAGON: &str = "83543";
/// This one has a custom PointArea, which caused issue #18:
const AAOFFLINE_TEST: &str = "148576";

// Cases used in multi-download test:
const MULTI_CASES: [&str; 4] = [
    THE_TORRENTIAL_TURNABOUT,
    PSYCHE_LOCK_TEST,
    GAME_OF_TURNABOUTS,
    TURNABOUT_OF_COURAGE,
];

/// A save that jumps immediately to the part where psyche locks appear.
const PSYCHE_LOCK_SAVE: &str = "?save_data=eyJ0cmlhbF9pZCI6ODkyNDcsInNhdmVfZGF0ZSI6MTczNDM2OTM5OCwicGxheWVyX3N0YXR1cyI6eyJjdXJyZW50X2ZyYW1lX2lkIjoyNiwiY3VycmVudF9mcmFtZV9pbmRleCI6MjksIm5leHRfZnJhbWVfaW5kZXgiOjMwLCJsYXN0X2ZyYW1lX21lcmdlZCI6ZmFsc2UsImxhdGVzdF9hY3Rpb25fZnJhbWVfaW5kZXgiOjAsImNvbXB1dGVkX3BhcmFtZXRlcnMiOm51bGwsImdhbWVfZW52Ijp7IlRSVUUiOnRydWUsIkZBTFNFIjpmYWxzZX0sImhlYWx0aCI6MTIwLCJoZWFsdGhfZmxhc2giOjAsImdhbWVfb3Zlcl90YXJnZXQiOjAsInByb2NlZWRfY2xpY2siOnRydWUsInByb2NlZWRfY2xpY2tfbWV0IjpmYWxzZSwicHJvY2VlZF90aW1lciI6ZmFsc2UsInByb2NlZWRfdGltZXJfbWV0IjpmYWxzZSwicHJvY2VlZF90eXBpbmciOnRydWUsInByb2NlZWRfdHlwaW5nX21ldCI6dHJ1ZX0sInRvcF9zY3JlZW5fc3RhdGUiOnsicG9zaXRpb24iOnsiaWQiOi0xLCJuYW1lIjoiY2VudGVyIiwiYWxpZ24iOjAsInNoaWZ0IjowfSwicGxhY2UiOnsicGxhY2VfaWQiOi0xfSwiY2hhcmFjdGVycyI6eyJjdXJyZW50RGVmYXVsdFBvc2l0aW9uIjotMSwiY3VycmVudFBvc2l0aW9ucyI6eyItMSI6eyJpZCI6LTEsIm5hbWUiOiJjZW50ZXIiLCJhbGlnbiI6MCwic2hpZnQiOjB9LCItMiI6eyJpZCI6LTIsIm5hbWUiOiJsZWZ0X2FsaWduIiwiYWxpZ24iOi0xLCJzaGlmdCI6MH0sIi0zIjp7ImlkIjotMywibmFtZSI6InJpZ2h0X2FsaWduIiwiYWxpZ24iOjEsInNoaWZ0IjowfSwiLTQiOnsiaWQiOi00LCJuYW1lIjoiYWFpX3NpbmdsZV9sZWZ0IiwiYWxpZ24iOi0xLCJzaGlmdCI6LTg0fSwiLTUiOnsiaWQiOi01LCJuYW1lIjoiYWFpX3NpbmdsZV9yaWdodCIsImFsaWduIjoxLCJzaGlmdCI6ODR9LCItNiI6eyJpZCI6LTYsIm5hbWUiOiJhYWlfZG91YmxlX2xlZnRfbGVmdG1vc3QiLCJhbGlnbiI6LTEsInNoaWZ0IjotNjV9LCItNyI6eyJpZCI6LTcsIm5hbWUiOiJhYWlfZG91YmxlX2xlZnRfcmlnaHRtb3N0IiwiYWxpZ24iOi0xLCJzaGlmdCI6MzJ9LCItOCI6eyJpZCI6LTgsIm5hbWUiOiJhYWlfZG91YmxlX3JpZ2h0X2xlZnRtb3N0IiwiYWxpZ24iOjEsInNoaWZ0IjotMzJ9LCItOSI6eyJpZCI6LTksIm5hbWUiOiJhYWlfZG91YmxlX3JpZ2h0X3JpZ2h0bW9zdCIsImFsaWduIjoxLCJzaGlmdCI6NjV9LCItMTAiOnsiaWQiOi0xMCwibmFtZSI6ImFhaV9kb3VibGVfY2VudGVyX2xlZnRtb3N0IiwiYWxpZ24iOjAsInNoaWZ0IjotNjV9LCItMTEiOnsiaWQiOi0xMSwibmFtZSI6ImFhaV9kb3VibGVfY2VudGVyX3JpZ2h0bW9zdCIsImFsaWduIjowLCJzaGlmdCI6NjV9fSwiY2hhcmFjdGVycyI6eyIyIjp7Im1pcnJvcl9lZmZlY3QiOmZhbHNlLCJwb3NpdGlvbiI6LTEsInByb2ZpbGVfaWQiOjIsInNwcml0ZV9pZCI6LTYsInN0YXJ0dXBfbW9kZSI6MCwic3luY19tb2RlIjoxLCJ2aXN1YWxfZWZmZWN0X2FwcGVhcnMiOjAsInZpc3VhbF9lZmZlY3RfYXBwZWFyc19tb2RlIjowLCJ2aXN1YWxfZWZmZWN0X2Rpc2FwcGVhcnMiOjAsInZpc3VhbF9lZmZlY3RfZGlzYXBwZWFyc19tb2RlIjowfX0sImNoYXJhY3RlcnNfb3JkZXIiOlsyXX0sImxvY2tzIjp7ImN1cnJlbnRfbG9ja3NfZGlhbG9ndWVfZGVzYyI6eyJzY2VuZV9pZCI6IjEiLCJzY2VuZV90eXBlIjoic2NlbmVzIiwic2VjdGlvbl9pZCI6IjEiLCJzZWN0aW9uX3R5cGUiOiJkaWFsb2d1ZXMifSwiZGlzcGxheWVkX2xvY2tzIjpbeyJpZCI6MSwidHlwZSI6ImpmYV9sb2NrIiwieCI6MTI4LCJ5IjoxMjgsImJyb2tlbiI6ZmFsc2V9LHsiaWQiOjIsInR5cGUiOiJqZmFfbG9jayIsIngiOjIyNCwieSI6NjQsImJyb2tlbiI6ZmFsc2V9LHsiaWQiOjMsInR5cGUiOiJqZmFfbG9jayIsIngiOjMyLCJ5Ijo2NCwiYnJva2VuIjpmYWxzZX0seyJpZCI6NCwidHlwZSI6ImpmYV9sb2NrIiwieCI6MTc2LCJ5IjoxNjAsImJyb2tlbiI6ZmFsc2V9LHsiaWQiOjUsInR5cGUiOiJqZmFfbG9jayIsIngiOjgwLCJ5IjoxNjAsImJyb2tlbiI6ZmFsc2V9XX0sImNyX2ljb25zIjp7ImRpc3BsYXllZF9pY29ucyI6W119LCJwb3B1cHMiOnsicG9wdXBzIjpbXSwibGFzdF9mcmFtZV93YXNfbWVyZ2VkIjpmYWxzZX0sInRleHQiOnsibmFtZSI6IlBob2VuaXgiLCJ0ZXh0cyI6WyJbI3NiXVlvdSBhdGUgTXkgQnVyZ2VyLiJdLCJjb2xvcnMiOlsid2hpdGUiXSwicHJldmlvdXNfZnJhbWVfbWVyZ2VkIjpmYWxzZSwiY3VycmVudF9zcGVha2VyIjoxfSwiZmFkZSI6eyJmYWRlIjpudWxsfX0sImN1cnJlbnRfbXVzaWNfaWQiOjIsInRyaWFsX2RhdGFfZGlmZnMiOnsiODkyNDciOnsiZnJhbWVzIjp7Im9yaWdpbmFsUm93RWRpdHMiOnsiNCI6eyJoaWRkZW4iOmZhbHNlfX0sImJsb2NrRWRpdHMiOlt7InN0YXJ0X2luZGV4IjowLCJlbmRfaW5kZXgiOjg4LCJzaGlmdCI6MH1dLCJsZW5ndGgiOjg5fX19LCJ0cmlhbF9kYXRhX2Jhc2VfZGF0ZXMiOnsiODkyNDciOjE2MDE4ODU4ODd9fQ%3D%3D";

struct Cmd {
    cmd: Command,
    path: TempDir,
}

impl Cmd {
    fn path_as_str(&self) -> &str {
        self.path.path().to_str().unwrap()
    }

    fn with_tmp_output(&mut self, one_file: bool) -> &mut Self {
        let path = self.path_as_str();
        let mut filename = path.to_string();
        if one_file {
            filename += "/index.html";
        }
        self.cmd.args(["-o", &filename]);
        self
    }
}

#[fixture]
fn cmd() -> Cmd {
    let mut cmd = Command::cargo_bin("aaoffline").unwrap();
    cmd.args(["-s", "single"]);
    let path = tempdir().unwrap();
    Cmd { cmd, path }
}

#[template]
#[rstest]
fn example_cases(
    #[values(
        THE_TORRENTIAL_TURNABOUT,
        PSYCHE_LOCK_TEST,
        GAME_OF_TURNABOUTS,
        TURNABOUT_OF_COURAGE,
        BROKEN_COMMANDMENTS,
        CASCADE_THEATER_ASCENSION,
        DRAGON,
        AAOFFLINE_TEST
    )]
    case: &str,
) {
}

#[derive(Debug, PartialEq, Eq)]
enum Source {
    Console,
    Dialog,
}

#[derive(Debug)]
struct JsError {
    errors: Vec<(String, Source)>,
}

impl Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "JavaScript errors: {}",
            self.errors.iter().map(|x| &x.0).join(",")
        )
    }
}

impl Error for JsError {}

#[derive(Debug, Default)]
struct JsListener {
    messages: Arc<Mutex<Vec<LogEntry>>>,
    dialogs: Arc<Mutex<Vec<String>>>,
}

impl JsListener {
    /// Makes sure that there are no error messages or dialogs.
    fn check_errors(&self) -> Result<(), JsError> {
        let msgs = self.messages.lock().unwrap();
        let messages = msgs
            .iter()
            .filter(|x| {
                x.level == LogEntryLevel::Info && x.text.contains("Failed to load")
                    || x.level == LogEntryLevel::Error
                // voice_singleblip_0 never exists, so we can ignore that error.
                && !x.url.as_ref().is_some_and(|x| x.contains("voice_singleblip_0"))
            })
            .map(|x| (x.text.clone(), Source::Console));
        let dlgs = self.dialogs.lock().unwrap();
        let dialogs = dlgs.iter().map(|x| (x.clone(), Source::Dialog));
        let errors = messages.chain(dialogs).collect_vec();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(JsError { errors })
        }
    }
}

impl EventListener<Event> for JsListener {
    fn on_event(&self, event: &Event) {
        if let Event::LogEntryAdded(EntryAddedEvent {
            params: EntryAddedEventParams { entry },
        }) = event
        {
            self.messages.lock().unwrap().push(entry.clone());
        } else if let Event::PageJavascriptDialogOpening(JavascriptDialogOpeningEvent {
            params: x,
        }) = event
        {
            self.dialogs.lock().unwrap().push(x.message.clone());
        }
    }
}

fn verify_with_browser_common(
    path: &str,
    query: Option<&str>,
    with_redirection: bool,
    append_index: bool,
) -> Result<(), Box<dyn Error>> {
    let options = LaunchOptionsBuilder::default().build()?;
    let browser = Browser::new(options)?;
    let tab = browser.new_tab()?;
    // We register a listener to capture any JavaScript errors, and then open the offline case.
    tab.enable_log()?;
    let listener: Arc<JsListener> = Arc::new(JsListener::default());
    tab.add_event_listener(listener.clone())?;
    let url = format!(
        "file://{path}{}.html{}",
        if append_index { "/index" } else { "" },
        query.unwrap_or_default()
    );
    tab.navigate_to(&url)?;
    tab.wait_until_navigated()?;

    // We click the "Start" button and wait a little while.
    tab.find_element("#start")?.click()?;

    let proceeds = tab.wait_for_elements("#proceed")?;
    assert!(proceeds.len() == 1);
    let proceed = proceeds.first();
    proceed.unwrap().click()?;
    if with_redirection {
        std::thread::sleep(Duration::from_millis(500));
        tab.wait_until_navigated()?;
        tab.find_element("#start")?.click()?;
    }
    let weak: Weak<dyn EventListener<Event> + Send + Sync> = Arc::downgrade(&listener) as Weak<_>;
    tab.remove_event_listener(&weak)?;
    listener.check_errors().map_err(Into::into)
}

fn verify_with_browser(path: &str, query: Option<&str>) -> Result<(), Box<dyn Error>> {
    verify_with_browser_common(path, query, false, true)
}

fn verify_with_browser_redirected(
    path: &str,
    query: Option<&str>,
    one_file: bool,
) -> Result<(), Box<dyn Error>> {
    verify_with_browser_common(path, query, true, !one_file)
}

#[rstest]
fn test_invalid_id(
    mut cmd: Cmd,
    #[values(
        "incorrect",
        "",
        "http://",
        "https://",
        "http://example.com/player.php?trial_id=1234",
        "https://example.com/player.php?trial_id=1234",
        "https://aaonline.fr/trial.php?trial_id=1234",
        "http://aaonline.fr/player.php?trial_id=",
        "https://aaonline.fr/player.php?trial_id=",
        "12 34",
        "-1234"
    )]
    case_id: &str,
) {
    cmd.cmd.arg(case_id).assert().failure();
}

#[rstest]
#[timeout(Duration::from_secs(60 * 5))]
fn test_html5_cors_error(mut cmd: Cmd) {
    cmd.with_tmp_output(false)
        .cmd
        .arg("--disable-html5-audio")
        .arg(PSYCHE_LOCK_TEST)
        .assert()
        .success();
    let errors = verify_with_browser(cmd.path_as_str(), None)
        .expect_err("expected CORS errors when not using HTML5 audio");
    if let Some(JsError { errors }) = errors.downcast_ref::<JsError>() {
        assert!(
            errors.iter().all(|x| x.1 == Source::Console
                && (x.0.contains("CORS") || x.0.contains("net::ERR_FAILED")))
        );
    } else {
        panic!("expected JsError, got {errors:?}");
    }
}

fn get_id(case: &str) -> &str {
    case.trim_start_matches(|x: char| !x.is_numeric())
}

#[rstest]
fn test_non_existing(mut cmd: Cmd, #[values("0", "999999", "9999999999999999")] case_id: &str) {
    cmd.cmd.arg(case_id).assert().failure();
}

#[apply(example_cases)]
fn test_single(mut cmd: Cmd, case: &str, #[values(true, false)] one_file: bool) {
    if one_file {
        cmd.cmd.arg("-1");
    }
    cmd.with_tmp_output(one_file)
        .cmd
        .arg(case)
        .assert()
        .success();
    verify_with_browser(cmd.path_as_str(), None).unwrap();
}

// These are tricky to get right, so we'll add a special test for these.
#[rstest]
fn test_psyche_locks(mut cmd: Cmd, #[values(true, false)] one_file: bool) {
    if one_file {
        cmd.cmd.arg("-1");
    }
    cmd.with_tmp_output(one_file)
        .cmd
        .arg(PSYCHE_LOCK_TEST)
        .assert()
        .success();
    verify_with_browser(cmd.path_as_str(), Some(PSYCHE_LOCK_SAVE)).unwrap();
}

#[rstest]
#[timeout(Duration::from_secs(60 * 10))]
fn test_sequence_download() {
    let mut cmd = Command::cargo_bin("aaoffline").unwrap();
    cmd.args(["-s", "every"]);
    cmd.args(["-o", tempdir().unwrap().path().to_str().unwrap()]);
    cmd.arg(GAME_OF_TURNABOUTS).assert().success();
}

#[rstest]
#[timeout(Duration::from_secs(60 * 10))]
fn test_sequence_navigation(#[values(true, false)] one_file: bool) {
    let mut cmd = Command::cargo_bin("aaoffline").unwrap();
    let tmpdir = tempdir().unwrap();
    let tmppath = tmpdir.path().to_str().unwrap();
    cmd.args(["-s", "every"]);
    cmd.args(["-o", tmppath]);
    if one_file {
        cmd.arg("-1");
    }
    cmd.arg(SEQUENCE_TEST).assert().success();
    verify_with_browser_redirected(
        &format!("{tmppath}/Sequence Test1_{SEQUENCE_TEST}"),
        None,
        one_file,
    )
    .unwrap();
}

#[rstest]
fn test_multi(mut cmd: Cmd) {
    cmd.with_tmp_output(false)
        .cmd
        .args(MULTI_CASES)
        .assert()
        .success();
    for case in MULTI_CASES {
        let case_id = get_id(case);
        let path = glob_one(&format!("{}/*_{case_id}/", cmd.path_as_str()));
        verify_with_browser(path.as_os_str().to_str().unwrap(), None).unwrap();
    }
    drop(cmd);
}

#[rstest]
fn test_userscripts(
    mut cmd: Cmd,
    #[values(
        "",
        "=all",
        "=none",
        "=alt-nametag",
        "=backlog",
        "=better-layout",
        "=keyboard-controls"
    )]
    userscripts: &str,
    #[values(true, false)] one_file: bool,
) {
    use regex::Regex;

    cmd.with_tmp_output(one_file);
    if one_file {
        cmd.cmd.arg("-1");
    }
    cmd.cmd.arg(PSYCHE_LOCK_TEST);
    cmd.cmd.arg(String::from("-u") + userscripts);
    println!(
        "Command-line args: {}",
        cmd.cmd.get_args().map(|x| x.to_str().unwrap()).join(" ")
    );
    cmd.cmd.assert().success();
    // We want to both make sure that the userscript is actually present...
    let verify_regexes = hashmap! {
        "=alt-nametag" => r"// @name\s+AAO Alt Nametag Font",
        "=backlog" => r"// @name\s+AAO Backlog Script",
        "=better-layout" => r"// @name\s+AAO Better Layout Script",
        "=keyboard-controls" => r"// @name\s+AAO Keyboard Controls \(Expanded\)",
    };

    let path = format!("{}/index.html", cmd.path_as_str(),);
    let file = fs::read_to_string(path).unwrap();
    if let Some(regextext) = verify_regexes.get(userscripts) {
        let regex = Regex::new(regextext).unwrap();
        assert!(regex.is_match(&file));
    } else if userscripts == "=all" || userscripts.is_empty() {
        for r in verify_regexes.values() {
            let regex = Regex::new(r).unwrap();
            assert!(regex.is_match(&file));
        }
    } else if userscripts == "=none" {
        assert!(!file.contains("==UserScript=="));
    } else {
        panic!("Unknown userscripts setting");
    }
    // ...and that there are no errors in the scripts.
    verify_with_browser(cmd.path_as_str(), None).unwrap();
}

#[rstest]
fn test_invalid_userscript(
    mut cmd: Cmd,
    #[values(
        "-u backlog -u all",
        "-u -u",
        "-u backlog -u none",
        "-u none -u all",
        "-u some"
    )]
    userscripts: &str,
) {
    cmd.cmd
        .arg(PSYCHE_LOCK_TEST)
        .args(userscripts.split(' '))
        .assert()
        .failure();
}

#[rstest]
fn test_output_format(
    #[values(true, false)] one_file: bool,
    #[values(true, false)] one_case: bool,
    #[values(true, false)] existing_dir: bool,
) {
    let mut cmd = Command::cargo_bin("aaoffline").unwrap();
    let testpath = tempdir().unwrap().keep().join("test");
    if existing_dir {
        std::fs::create_dir(&testpath).unwrap();
    }
    let path = testpath.to_str().unwrap();
    cmd.args(["-s", "single", "-o", path]);
    if one_file {
        cmd.arg("-1");
    }
    if one_case {
        cmd.arg(PSYCHE_LOCK_TEST);
    } else {
        cmd.args([PSYCHE_LOCK_TEST, GAME_OF_TURNABOUTS]);
    }

    cmd.assert().success();

    let tmpdir = testpath.parent().unwrap();
    match (one_file, one_case, existing_dir) {
        // Should be put at "test.html" in the tempdir.
        (true, true, false) => {
            assert!(tmpdir.join("test.html").is_file());
            assert!(!tmpdir.join("assets").exists());
        }
        // Should be put at "test/<title>_<case id>.html" in the tempdir.
        (true, true, true) => {
            let file = glob_one(&format!("{path}/*_{PSYCHE_LOCK_TEST}.html"));
            assert!(file.is_file());
            assert!(!testpath.join("assets").exists());
        }
        // Should be put at "test/<title>_<case id>.html" in the tempdir.
        (true, false, _) => {
            let first = glob_one(&format!("{path}/*_{GAME_OF_TURNABOUTS}.html"));
            let second = glob_one(&format!("{path}/*_{PSYCHE_LOCK_TEST}.html"));
            assert!(first.is_file());
            assert!(second.is_file());
            assert!(!testpath.join("assets").exists());
        }
        // Should be at "test/index.html".
        (false, true, _) => {
            assert!(testpath.join("index.html").is_file());
            assert!(testpath.join("assets").is_dir());
        }
        // Should be at "test/<title>_<case id>/index.html".
        (false, false, _) => {
            let first = glob_one(&format!("{path}/*_{GAME_OF_TURNABOUTS}"));
            let second = glob_one(&format!("{path}/*_{PSYCHE_LOCK_TEST}"));
            assert!(first.is_dir());
            assert!(second.is_dir());
            assert!(first.join("index.html").is_file());
            assert!(second.join("index.html").is_file());
            assert!(first.join("assets").is_dir());
            assert!(second.join("assets").is_dir());
        }
    };
}

fn glob_one(pat: &str) -> PathBuf {
    glob::glob(pat).unwrap().exactly_one().unwrap().unwrap()
}

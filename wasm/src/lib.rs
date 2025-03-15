use aaoffline::{
    args::{Args, DownloadSequence, HttpHandling, Userscripts},
    MainContext,
};
use anyhow::{anyhow, Result};
use clap_verbosity_flag::Verbosity;
use log::{info, LevelFilter, Log};
use wasm_bindgen::prelude::*;
use web_sys::console;

use fs::WasmWriter;

mod fs;

#[wasm_bindgen]
extern "C" {
    fn alert(s: &str);
}

struct ConsoleLogger;

impl Log for ConsoleLogger {
    #[inline]
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let message = JsValue::from(format!("{}", record.args()));

        match record.level() {
            log::Level::Error => console::error_1(&message),
            log::Level::Warn => console::warn_1(&message),
            log::Level::Info => console::info_1(&message),
            log::Level::Debug => console::debug_1(&message),
            log::Level::Trace => console::trace_1(&message),
        }
    }

    fn flush(&self) {}
}

static LOGGER: ConsoleLogger = ConsoleLogger;

#[wasm_bindgen]
pub async fn download() -> Result<JsValue, JsValue> {
    run_aaoffline()
        .await
        .map_err(|x| JsValue::from_str(&x.to_string()))
        .map(JsValue::from)
}

pub async fn run_aaoffline() -> Result<Vec<u8>> {
    log::set_logger(&LOGGER).map_err(|x| anyhow!(x.to_string()))?;
    log::set_max_level(LevelFilter::Debug);
    let writer = Box::new(WasmWriter::new());
    let args = Args {
        cases: vec![89247],
        output: None,
        player_version: String::from("master"),
        language: String::from("en"),
        continue_on_asset_error: false,
        replace_existing: false,
        sequence: DownloadSequence::Single,
        one_html_file: false,
        with_userscripts: vec![Userscripts::None],
        concurrent_downloads: 4,
        retries: 3,
        connect_timeout: 10,
        read_timeout: 30,
        http_handling: HttpHandling::AllowInsecure,
        disable_html5_audio: false,
        disable_photobucket_fix: false,
        verbose: Verbosity::default(),
        proxy: Some(String::from("http://localhost:8001/")),
    };
    let mut ctx = MainContext::new(args, writer);
    ctx.run_all_steps().await?;
    info!("Unwrapping next...");
    let writer = ctx.writer();
    info!("Downcasting next...");
    let writer: &WasmWriter = writer.as_any().downcast_ref().unwrap();
    writer.finish()
}

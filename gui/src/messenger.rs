use std::sync::Arc;

use aaoffline::{InteractiveDialog, MainContext, ProgressReporter, args::Args, fs::TokioFsWriter};
use log::{error, info, warn};
use tokio::{
    runtime::Runtime,
    sync::mpsc::{Receiver, Sender},
};

#[derive(Debug)]
pub(crate) struct GuiMessenger {
    sender: Sender<GuiMessage>,
    receiver: Receiver<GuiMessage>,
    runtime: Arc<Runtime>,
}

impl Default for GuiMessenger {
    fn default() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(100);
        GuiMessenger {
            sender,
            receiver,
            runtime: Arc::new(Runtime::new().unwrap()),
        }
    }
}

impl GuiMessenger {
    pub(crate) fn run(&mut self, args: Args) {
        let sender = self.sender.clone();
        let rt = Arc::clone(&self.runtime);
        std::thread::spawn(move || {
            rt.block_on(async {
                let writer = Box::new(TokioFsWriter);
                let helper = GuiMessageSender { sender };
                let num_cases = args.cases.len();
                let mut ctx = MainContext::new(
                    args,
                    writer,
                    Box::new(helper.clone()),
                    Box::new(helper.clone()),
                );
                info!(
                    "Starting download for {num_cases} case{}...",
                    if num_cases == 1 { "" } else { "s" }
                );
                let success = if let Err(e) = ctx.run_all_steps().await {
                    error!("{e}");
                    false
                } else {
                    true
                };
                if let Err(e) = helper.sender.send(GuiMessage::Done(success)).await {
                    error!("Could not notify UI that download is done: {e}");
                }
            });
        });
    }

    pub(crate) fn receive(&mut self) -> Option<GuiMessage> {
        self.receiver.try_recv().ok()
    }
}

#[derive(Debug)]
pub(crate) enum GuiMessage {
    // Bool indicates success.
    Done(bool),
    NextStep { step: u8, text: String },
    Progress(ProgressMessage),
}

#[derive(Debug)]
pub(crate) enum ProgressMessage {
    Inc(u64),
    IncLength(u64),
    New(u64),
    Finish(String),
    Clear,
}

#[derive(Debug, Clone)]
struct GuiMessageSender {
    sender: Sender<GuiMessage>,
}

impl GuiMessageSender {
    fn send(&self, msg: GuiMessage) {
        if let Err(e) = self.sender.try_send(msg) {
            warn!("Couldn't notify UI about progress update: {e}");
        }
    }
}

impl InteractiveDialog for GuiMessageSender {
    fn confirm(&mut self, _: &str, _: bool) -> Option<bool> {
        unimplemented!()
    }
}

impl ProgressReporter for GuiMessageSender {
    fn inc(&self, delta: u64) {
        self.send(GuiMessage::Progress(ProgressMessage::Inc(delta)));
    }

    fn inc_length(&self, delta: u64) {
        self.send(GuiMessage::Progress(ProgressMessage::IncLength(delta)));
    }

    fn next_step(&self, step: u8, text: &str, _: bool) {
        self.send(GuiMessage::NextStep {
            step,
            text: text.into(),
        });
    }

    fn new_progress(&self, max: u64, _: bool) {
        self.send(GuiMessage::Progress(ProgressMessage::New(max)));
    }

    fn suspend(&self, f: &dyn Fn() -> Option<bool>) -> Option<bool> {
        f()
    }

    fn finish_progress(&self, msg: String) {
        self.send(GuiMessage::Progress(ProgressMessage::Finish(msg)));
    }

    fn finish_and_clear(&self) {
        self.send(GuiMessage::Progress(ProgressMessage::Clear));
    }
}

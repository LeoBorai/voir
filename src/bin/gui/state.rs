use cacao::appkit::App;
use std::path::PathBuf;

use super::app::VoirApp;

#[derive(Clone, Debug)]
pub enum Message {
    OpenFile(PathBuf),
}

pub fn dispatch(message: Message) {
    App::<VoirApp, Message>::dispatch_main(message);
}

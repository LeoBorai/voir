mod fs;
mod gui;

use cacao::appkit::App;

use self::gui::app::VoirApp;

const BUNDLE_ID: &str = "com.EstebanBorai.voir";

fn main() {
    App::new(BUNDLE_ID, VoirApp::default()).run();
}

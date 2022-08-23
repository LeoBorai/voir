mod gui;

use cacao::appkit::window::{Window, WindowConfig};
use cacao::appkit::App;

fn main() {
    App::new(
        "com.EstebanBorai.voir",
        gui::Voir {
            window: Window::with(WindowConfig::default(), gui::VoirWindow::new()),
        },
    )
    .run();
}

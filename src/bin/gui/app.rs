use cacao::appkit::{App, AppDelegate};
use cacao::notification_center::Dispatcher;

use super::menu::make_menu_options;
use super::state::Message;
use super::window_manager::WindowManager;

#[derive(Default)]
pub struct VoirApp {
    pub(crate) window_manager: WindowManager,
}

impl AppDelegate for VoirApp {
    fn did_finish_launching(&self) {
        App::set_menu(make_menu_options());
        App::activate();

        self.window_manager.open_main();
    }
}

impl Dispatcher for VoirApp {
    type Message = Message;

    fn on_ui_message(&self, message: Self::Message) {
        self.window_manager.on_ui_message(message);
    }
}

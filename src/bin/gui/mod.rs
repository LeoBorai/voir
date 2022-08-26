mod bytes_table;
mod content_view;
mod menu;
mod toolbar;

use cacao::appkit::toolbar::Toolbar;
use cacao::appkit::window::{Window, WindowDelegate};
use cacao::appkit::{App, AppDelegate};
use cacao::view::ViewController;

use self::content_view::ContentView;
use self::toolbar::AppToolbar;

pub struct Voir {
    pub(crate) window: Window<VoirWindow>,
}

impl AppDelegate for Voir {
    fn did_finish_launching(&self) {
        let menu_options = menu::make_menu_options();

        App::set_menu(menu_options);
        App::activate();
        self.window.show();
    }

    fn should_terminate_after_last_window_closed(&self) -> bool {
        true
    }
}

#[derive(Debug)]
pub struct VoirWindow {
    pub(crate) toolbar: Toolbar<AppToolbar>,
    pub(crate) content: ViewController<ContentView>,
}

impl VoirWindow {
    pub fn new() -> Self {
        Self {
            toolbar: Toolbar::new("com.EstebanBorai.voir.Toolbar", AppToolbar::new()),
            content: ViewController::new(ContentView::default()),
        }
    }
}

impl WindowDelegate for VoirWindow {
    const NAME: &'static str = "WindowDelegate";

    fn did_load(&mut self, window: Window) {
        window.set_title("Voir");
        window.set_minimum_content_size(300., 300.);
        window.set_toolbar(&self.toolbar);
        window.set_content_view_controller(&self.content);
    }
}

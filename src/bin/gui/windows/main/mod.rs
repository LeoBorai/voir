mod bytes_table;
mod content_view;
mod toolbar;

use cacao::appkit::toolbar::Toolbar;
use cacao::appkit::window::{Window, WindowDelegate};
use cacao::view::ViewController;

use crate::gui::state::Message;

use self::content_view::ContentView;
use self::toolbar::AppToolbar;

#[derive(Debug)]
pub struct MainWindow {
    pub(crate) toolbar: Toolbar<AppToolbar>,
    pub(crate) content: ViewController<ContentView>,
}

impl MainWindow {
    pub fn new() -> Self {
        Self {
            toolbar: Toolbar::new("com.EstebanBorai.voir.Toolbar", AppToolbar::new()),
            content: ViewController::new(ContentView::default()),
        }
    }

    pub fn on_message(&self, message: Message) {
        if let Some(delegate) = &self.content.view.delegate {
            delegate.on_message(message);
        }
    }
}

impl WindowDelegate for MainWindow {
    const NAME: &'static str = "WindowDelegate";

    fn did_load(&mut self, window: Window) {
        window.set_title("Voir");
        window.set_minimum_content_size(500., 500.);
        window.set_toolbar(&self.toolbar);
        window.set_content_view_controller(&self.content);
    }
}

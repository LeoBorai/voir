mod menu;
mod toolbar;

use cacao::appkit::toolbar::Toolbar;
use cacao::appkit::window::{Window, WindowDelegate};
use cacao::appkit::{App, AppDelegate};

use cacao::input::{TextField, TextFieldDelegate};
use cacao::layout::{Layout, LayoutConstraint};
use cacao::view::View;

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

#[derive(Debug, Default)]
pub struct ConsoleLogger;

impl TextFieldDelegate for ConsoleLogger {
    const NAME: &'static str = "ConsoleLogger";

    fn text_should_begin_editing(&self, value: &str) -> bool {
        println!("Should begin with value: {}", value);
        true
    }

    fn text_did_change(&self, value: &str) {
        println!("Did change to: {}", value);
    }

    fn text_did_end_editing(&self, value: &str) {
        println!("Ended: {}", value);
    }
}

#[derive(Debug)]
pub struct VoirWindow {
    pub(crate) input: TextField<ConsoleLogger>,
    pub(crate) toolbar: Toolbar<AppToolbar>,
    pub(crate) content: View,
}

impl VoirWindow {
    pub fn new() -> Self {
        Self {
            input: TextField::with(ConsoleLogger),
            toolbar: Toolbar::new("com.EstebanBorai.voir.Toolbar", AppToolbar::new()),
            content: View::new(),
        }
    }
}

impl WindowDelegate for VoirWindow {
    const NAME: &'static str = "WindowDelegate";

    fn did_load(&mut self, window: Window) {
        window.set_title("Voir");
        window.set_minimum_content_size(300., 300.);
        window.set_toolbar(&self.toolbar);

        self.content.add_subview(&self.input);
        window.set_content_view(&self.content);

        LayoutConstraint::activate(&[
            self.input
                .center_x
                .constraint_equal_to(&self.content.center_x),
            self.input
                .center_y
                .constraint_equal_to(&self.content.center_y),
            self.input.width.constraint_equal_to_constant(280.),
        ]);
    }
}

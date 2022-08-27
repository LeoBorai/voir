use cacao::appkit::window::{Window, WindowConfig, WindowDelegate};
use std::sync::RwLock;

use crate::gui::windows::main::MainWindow;

#[derive(Default)]
pub struct WindowManager {
    pub main: RwLock<Option<Window<MainWindow>>>,
}

fn open_or_show<T, F>(window: &RwLock<Option<Window<T>>>, vendor: F)
where
    T: WindowDelegate + 'static,
    F: Fn() -> (WindowConfig, T),
{
    let mut lock = window.write().unwrap();

    if let Some(win) = &*lock {
        win.show();
    } else {
        let (config, delegate) = vendor();
        let win = Window::with(config, delegate);
        win.show();
        *lock = Some(win);
    }
}

impl WindowManager {
    pub fn open_main(&self) {
        open_or_show(&self.main, || (WindowConfig::default(), MainWindow::new()))
    }
}

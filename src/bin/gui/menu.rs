use cacao::appkit::menu::{Menu, MenuItem};

/// Creates menu options for the top bar
pub fn make_menu_options() -> Vec<Menu> {
    vec![
        Menu::new(
            "",
            vec![
                MenuItem::Services,
                MenuItem::Separator,
                MenuItem::Hide,
                MenuItem::HideOthers,
                MenuItem::ShowAll,
                MenuItem::Separator,
                MenuItem::Quit,
            ],
        ),
        Menu::new("File", vec![MenuItem::CloseWindow]),
        Menu::new(
            "Edit",
            vec![
                MenuItem::Undo,
                MenuItem::Redo,
                MenuItem::Separator,
                MenuItem::Cut,
                MenuItem::Copy,
                MenuItem::Paste,
                MenuItem::Separator,
                MenuItem::SelectAll,
            ],
        ),
        Menu::new("View", vec![MenuItem::EnterFullScreen]),
        Menu::new(
            "Window",
            vec![
                MenuItem::Minimize,
                MenuItem::Zoom,
                MenuItem::Separator,
                MenuItem::new("Bring All to Front"),
            ],
        ),
        Menu::new("Help", vec![]),
    ]
}

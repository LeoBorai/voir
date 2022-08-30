use cacao::appkit::toolbar::{
    ItemIdentifier, Toolbar, ToolbarDelegate, ToolbarDisplayMode, ToolbarItem,
};
use cacao::button::Button;
use cacao::filesystem::FileSelectPanel;
use cacao::foundation::NSURL;

use crate::gui::storage::{dispatch, Message};

use libvoir::Voir;

const OPEN_FILE: &str = "OPEN_FILE";

#[derive(Debug)]
pub struct AppToolbar {
    open_file: ToolbarItem,
}

impl AppToolbar {
    pub fn new() -> Self {
        let open_file_button = Button::new("Open");
        let mut open_file_item = ToolbarItem::new(OPEN_FILE);
        open_file_item.set_button(open_file_button);
        open_file_item.set_action(|| FileSelectPanel::new().show(AppToolbar::open_file));

        Self {
            open_file: open_file_item,
        }
    }

    fn open_file(nsurls: Vec<NSURL>) {
        if let Some(first) = nsurls.get(0) {
            let pathbuf = first.pathbuf();

            dispatch(Message::OpenFile(pathbuf));
        }
    }

    pub fn item_identifiers(&self) -> Vec<ItemIdentifier> {
        vec![ItemIdentifier::Custom(OPEN_FILE), ItemIdentifier::Space]
    }
}

impl ToolbarDelegate for AppToolbar {
    const NAME: &'static str = "Toolbar";

    fn did_load(&mut self, toolbar: Toolbar) {
        toolbar.set_display_mode(ToolbarDisplayMode::IconOnly);
    }

    fn allowed_item_identifiers(&self) -> Vec<ItemIdentifier> {
        self.item_identifiers()
    }

    fn default_item_identifiers(&self) -> Vec<ItemIdentifier> {
        self.item_identifiers()
    }

    fn item_for(&self, identifier: &str) -> &ToolbarItem {
        match identifier {
            OPEN_FILE => &self.open_file,
            _ => {
                std::unreachable!();
            }
        }
    }
}

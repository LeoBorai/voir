mod row;

use std::io::Read;

use cacao::{
    color::Color,
    listview::{ListView, ListViewDelegate, ListViewRow, RowAnimation},
};

use crate::fs::open_file;
use crate::gui::storage::{Message, Storage};

const VIEW_ROW_CELL: &str = "FileContentsViewRowCell";
const BYTES_PER_LINE: usize = 16;

#[derive(Debug)]
pub struct BytesTableView {
    pub view: Option<ListView>,
    pub storage: Storage,
}

impl BytesTableView {
    pub fn on_message(&self, message: Message) {
        match message {
            Message::OpenFile(path) => {
                let mut buff = open_file(path);
                let mut bytes: Vec<u8> = Vec::new();

                buff.read_to_end(&mut bytes).expect("failed to read file");

                self.storage.load_bytes(bytes);
                self.view
                    .as_ref()
                    .unwrap()
                    .perform_batch_updates(|listview| {
                        listview.perform_batch_updates(|listview| {
                            let indexes =
                                (0..self.storage.len() / BYTES_PER_LINE).collect::<Vec<usize>>();

                            listview.insert_rows(&indexes, RowAnimation::None);
                        });
                    });
            }
        }
    }
}

impl Default for BytesTableView {
    fn default() -> Self {
        Self {
            view: None,
            storage: Storage::default(),
        }
    }
}

impl ListViewDelegate for BytesTableView {
    const NAME: &'static str = "BytesTableView";

    fn did_load(&mut self, view: ListView) {
        view.register(VIEW_ROW_CELL, row::TableRow::default);
        view.set_row_height(21.);
        view.set_background_color(Color::SystemGreen);
        view.set_uses_alternating_backgrounds(true);
        self.view = Some(view);
    }

    fn number_of_items(&self) -> usize {
        self.storage.len()
    }

    fn item_for(&self, row: usize) -> ListViewRow {
        let mut view = self
            .view
            .as_ref()
            .unwrap()
            .dequeue::<row::TableRow>(VIEW_ROW_CELL);
        let chunks = self.storage.chunks();

        if let Some(chunk) = chunks.get(row) {
            if let Some(view) = &mut view.delegate {
                self.storage.with(row, |_| view.configure_with(chunk));
            }
        }

        view.into_row()
    }
}

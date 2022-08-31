mod row;

use cacao::listview::{ListView, ListViewDelegate, ListViewRow, RowAnimation};

use crate::gui::storage::{Message, Storage};

const VIEW_ROW_CELL: &str = "FileContentsViewRowCell";

#[derive(Debug)]
pub struct BytesTableView {
    pub view: Option<ListView>,
    pub storage: Storage,
}

impl BytesTableView {
    pub fn on_message(&self, message: Message) {
        match message {
            Message::OpenFile(_) => {
                self.storage.load_bytes(vec![122]);

                self.view
                    .as_ref()
                    .unwrap()
                    .perform_batch_updates(|listview| {
                        listview.perform_batch_updates(|listview| {
                            listview.insert_rows(&[0], RowAnimation::SlideDown);
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
        view.set_row_height(16.);
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

        if let Some(view) = &mut view.delegate {
            self.storage.with(row, |byte| view.configure_with(byte));
        }

        view.into_row()
    }
}

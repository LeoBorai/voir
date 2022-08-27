mod row;

use cacao::layout::{Layout, LayoutConstraint};
use cacao::listview::{ListView, ListViewDelegate, ListViewRow};
use cacao::view::{View, ViewDelegate};

const VIEW_ROW_CELL: &str = "FileContentsViewRowCell";

#[derive(Debug)]
pub enum Message {
    OpenFile(Vec<u8>),
}

#[derive(Debug, Default)]
pub struct BytesTableView {
    pub view: ListView,
    pub contents: Vec<u8>,
}

impl BytesTableView {
    pub fn on_message(&self, message: Message) {
        match message {
            Message::OpenFile(bytes) => {
                println!("ON: {:?}", bytes);
            }
        }
    }
}

impl ViewDelegate for BytesTableView {
    const NAME: &'static str = "BytesTableView";

    fn did_load(&mut self, view: View) {
        view.add_subview(&self.view);

        LayoutConstraint::activate(&[
            self.view.top.constraint_equal_to(&view.top),
            self.view.leading.constraint_equal_to(&view.leading),
            self.view.trailing.constraint_equal_to(&view.trailing),
            self.view.bottom.constraint_equal_to(&view.bottom),
        ]);
    }
}

impl ListViewDelegate for BytesTableView {
    const NAME: &'static str = "FileContentsListView";

    fn did_load(&mut self, view: ListView) {
        view.register(VIEW_ROW_CELL, row::TableRow::default);
    }

    fn number_of_items(&self) -> usize {
        self.contents.len()
    }

    fn item_for(&self, row: usize) -> ListViewRow {
        // let mut view = self.view.as_ref().unwrap().dequeue::<row::TableRow>(VIEW_ROW_CELL);

        ListViewRow::default()
    }
}

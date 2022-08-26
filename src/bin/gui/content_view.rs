use cacao::layout::{Layout, LayoutConstraint};
use cacao::listview::ListView;
use cacao::view::{View, ViewDelegate};

use crate::gui::bytes_table::BytesTableView;

#[derive(Debug)]
pub struct ContentView {
    pub bytes_table_view: ListView<BytesTableView>,
}

impl Default for ContentView {
    fn default() -> Self {
        Self {
            bytes_table_view: ListView::with(BytesTableView::default()),
        }
    }
}

impl ViewDelegate for ContentView {
    const NAME: &'static str = "ContentView";

    fn did_load(&mut self, view: View) {
        view.add_subview(&self.bytes_table_view);

        LayoutConstraint::activate(&[
            self.bytes_table_view.top.constraint_equal_to(&view.top),
            self.bytes_table_view
                .leading
                .constraint_equal_to(&view.leading),
            self.bytes_table_view
                .trailing
                .constraint_equal_to(&view.trailing),
            self.bytes_table_view
                .bottom
                .constraint_equal_to(&view.bottom),
        ]);
    }
}

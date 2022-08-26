use cacao::layout::{Layout, LayoutConstraint};
use cacao::text::{Font, Label, LineBreakMode};
use cacao::view::{View, ViewDelegate};

#[derive(Default, Debug)]
pub struct TableRow {
    pub byte: Label,
}

impl ViewDelegate for TableRow {
    const NAME: &'static str = "TableRow";

    /// Called when the view is first created; handles setup of layout and associated styling that
    /// doesn't change.
    fn did_load(&mut self, view: View) {
        view.add_subview(&self.byte);

        let font = Font::system(10.);
        self.byte.set_font(&font);
        self.byte.set_line_break_mode(LineBreakMode::TruncateMiddle);

        LayoutConstraint::activate(&[self.byte.top.constraint_equal_to(&view.top).offset(16.)]);
    }
}

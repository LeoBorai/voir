use cacao::color::Color;
use cacao::layout::{Layout, LayoutConstraint};
use cacao::text::{Font, Label, LineBreakMode};
use cacao::view::{View, ViewDelegate};

#[derive(Default, Debug)]
pub struct TableRow {
    pub byte: Label,
}

impl TableRow {
    pub fn configure_with(&mut self, byte: &u8) {
        self.byte.set_text(format!("TEST: {}", byte));
        self.byte.set_text_color(Color::SystemRed);
    }
}

impl ViewDelegate for TableRow {
    const NAME: &'static str = "TableRow";

    fn did_load(&mut self, view: View) {
        view.add_subview(&self.byte);

        let font = Font::system(10.);
        self.byte.set_font(&font);
        self.byte.set_line_break_mode(LineBreakMode::TruncateMiddle);

        LayoutConstraint::activate(&[
            self.byte.top.constraint_equal_to(&view.top).offset(16.),
            self.byte
                .leading
                .constraint_equal_to(&view.leading)
                .offset(16.),
            self.byte
                .trailing
                .constraint_equal_to(&view.trailing)
                .offset(-16.),
        ]);
    }
}

use cacao::color::Color;
use cacao::layout::{Layout, LayoutConstraint};
use cacao::text::{Font, Label};
use cacao::view::{View, ViewDelegate};

#[derive(Default, Debug)]
pub struct TableRow {
    pub byte: Label,
}

impl TableRow {
    pub fn configure_with(&mut self, byte: &u8) {
        self.byte.set_text(&format!("{:0x}", byte));
        self.byte.set_text_color(Color::SystemRed);
    }
}

impl ViewDelegate for TableRow {
    const NAME: &'static str = "TableRow";

    fn did_load(&mut self, view: View) {
        view.add_subview(&self.byte);
        view.height.constraint_equal_to_constant(120.);
        view.width.constraint_equal_to_constant(120.);

        let font = Font::system(16.);
        self.byte.set_font(font);

        LayoutConstraint::activate(&[
            self.byte.top.constraint_equal_to(&view.top),
            self.byte.leading.constraint_equal_to(&view.leading),
            self.byte.trailing.constraint_equal_to(&view.trailing),
            self.byte.bottom.constraint_equal_to(&view.bottom),
        ]);
    }
}

use cacao::color::Color;
use cacao::layout::{Layout, LayoutConstraint};
use cacao::text::{Font, Label};
use cacao::view::{View, ViewDelegate};

#[derive(Default, Debug)]
pub struct TableRow {
    pub x0: Label,
    pub x1: Label,
    pub x2: Label,
}

impl TableRow {
    pub fn configure_with(&mut self, byte: &Vec<u8>) {
        self.x0.set_text(&format!("{:0x}", byte.get(0).unwrap()));
        self.x0.set_text_color(Color::SystemRed);
        self.x1.set_text(&format!("{:0x}", byte.get(1).unwrap()));
        self.x1.set_text_color(Color::SystemRed);
        self.x2.set_text(&format!("{:0x}", byte.get(2).unwrap()));
        self.x2.set_text_color(Color::SystemRed);
    }
}

impl ViewDelegate for TableRow {
    const NAME: &'static str = "TableRow";

    fn did_load(&mut self, view: View) {
        view.add_subview(&self.x0);
        view.height.constraint_equal_to_constant(16.);
        view.width.constraint_equal_to_constant(16. * 2.);

        view.add_subview(&self.x1);
        view.height.constraint_equal_to_constant(16.);
        view.width.constraint_equal_to_constant(16. * 2.);

        view.add_subview(&self.x2);
        view.height.constraint_equal_to_constant(16.);
        view.width.constraint_equal_to_constant(16. * 2.);

        let font = Font::monospace(16., 1.);

        self.x0.set_font(font.clone());
        self.x1.set_font(font.clone());
        self.x2.set_font(font.clone());

        LayoutConstraint::activate(&[
            self.x0.top.constraint_equal_to(&view.top),
            self.x0.bottom.constraint_equal_to(&view.bottom),
            self.x1.top.constraint_equal_to(&view.top),
            self.x1
                .leading
                .constraint_greater_than_or_equal_to(&self.x0.trailing),
            self.x1.bottom.constraint_equal_to(&view.bottom),
            self.x2.top.constraint_equal_to(&view.top),
            self.x2
                .trailing
                .constraint_greater_than_or_equal_to(&self.x1.trailing),
            self.x2.bottom.constraint_equal_to(&view.bottom),
        ]);
    }
}

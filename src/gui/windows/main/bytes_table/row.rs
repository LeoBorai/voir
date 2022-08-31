use cacao::color::Color;
use cacao::layout::{Layout, LayoutConstraint};
use cacao::text::{Font, Label, LineBreakMode};
use cacao::view::{View, ViewDelegate};

#[derive(Default, Debug)]
pub struct TableRow {
    pub title: Label,
    pub status: Label,
}

impl TableRow {
    pub fn configure_with(&mut self, byte: &u8) {
        self.title.set_text(&format!("TEST: {}", byte));
        self.title.set_text_color(Color::SystemRed);
        self.status.set_text(&format!("TEST: {}", byte));
        self.status.set_text_color(Color::SystemRed);
    }
}

impl ViewDelegate for TableRow {
    const NAME: &'static str = "TableRow";

    fn did_load(&mut self, view: View) {
        view.add_subview(&self.title);
        view.add_subview(&self.status);

        self.title
            .set_line_break_mode(LineBreakMode::TruncateMiddle);

        let font = Font::system(10.);
        self.status.set_font(&font);

        LayoutConstraint::activate(&[
            self.title.top.constraint_equal_to(&view.top).offset(16.),
            self.title
                .leading
                .constraint_equal_to(&view.leading)
                .offset(16.),
            self.title
                .trailing
                .constraint_equal_to(&view.trailing)
                .offset(-16.),
            self.status
                .top
                .constraint_equal_to(&self.title.bottom)
                .offset(8.),
            self.status
                .leading
                .constraint_equal_to(&view.leading)
                .offset(16.),
            self.status
                .trailing
                .constraint_equal_to(&view.trailing)
                .offset(-16.),
            self.status
                .bottom
                .constraint_equal_to(&view.bottom)
                .offset(-16.),
        ]);
    }
}

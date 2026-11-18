use crate::i18n::{Language, Message};
use eframe::egui;

pub(in crate::app) fn show(
    ui: &mut egui::Ui,
    language: Language,
    page: usize,
    has_previous: bool,
    has_next: bool,
) -> Option<usize> {
    ui.horizontal(|ui| {
        let previous = ui
            .add_enabled(
                has_previous && page > 1,
                egui::Button::new(language.text(Message::PreviousPage)),
            )
            .clicked();
        ui.label(page.to_string());
        let next = ui
            .add_enabled(
                has_next,
                egui::Button::new(language.text(Message::NextPage)),
            )
            .clicked();
        if previous {
            Some(page - 1)
        } else if next {
            page.checked_add(1)
        } else {
            None
        }
    })
    .inner
}

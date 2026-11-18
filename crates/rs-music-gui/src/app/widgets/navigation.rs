use super::super::{ACCENT, MusicApp, Page, SURFACE};
use eframe::egui::{self, RichText};

pub(in crate::app) fn show(ctx: &egui::Context, app: &mut MusicApp) {
    let language = app.language();
    egui::SidePanel::left("navigation")
        .resizable(false)
        .exact_width(152.0)
        .frame(egui::Frame::new().fill(SURFACE).inner_margin(16))
        .show(ctx, |ui| {
            ui.label(RichText::new("rs-music").size(24.0).strong().color(ACCENT));
            ui.add_space(30.0);
            for page in [
                Page::Library,
                Page::Hall,
                Page::Search,
                Page::Queue,
                Page::Settings,
                Page::About,
            ] {
                let label = if page == Page::Queue {
                    language.queue_count(app.queue.len())
                } else {
                    language.text(page.message()).into()
                };
                if ui
                    .add_sized(
                        [ui.available_width(), 36.0],
                        egui::Button::selectable(app.page == page, label),
                    )
                    .clicked()
                {
                    app.page = page;
                }
                ui.add_space(4.0);
            }
        });
}

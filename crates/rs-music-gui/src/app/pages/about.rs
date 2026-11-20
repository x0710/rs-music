use super::super::Message;
use crate::i18n::Language;
use eframe::egui::{self, RichText};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const GPL: &str = include_str!("../../../../../LICENSE");

pub(in crate::app) fn show(language: Language, ui: &mut egui::Ui) {
    ui.heading(language.text(Message::About));
    ui.add_space(16.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.set_max_width(660.0);
        egui::Frame::new()
            .fill(ui.visuals().window_fill)
            .corner_radius(8)
            .inner_margin(20)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    RichText::new("rs-music")
                        .color(ui.visuals().selection.stroke.color)
                        .size(30.0)
                        .strong(),
                );
                ui.colored_label(
                    ui.visuals().weak_text_color(),
                    format!(
                        "{} {}",
                        language.text(Message::Version),
                        env!("CARGO_PKG_VERSION")
                    ),
                );
                ui.add_space(10.0);
                ui.label(language.text(Message::AboutDescription));
                ui.add_space(14.0);
                ui.horizontal_wrapped(|ui| {
                    // Official Invertocat from https://brand.github.com/foundations/logo.
                    let icon =
                        egui::Image::new(egui::include_image!("../../../assets/github-mark.svg"))
                            .tint(ui.visuals().text_color())
                            .max_size(egui::vec2(18.0, 18.0));
                    let button = egui::Button::image_and_text(icon, "GitHub");
                    if ui
                        .add(button.min_size(egui::vec2(0.0, 30.0)))
                        .on_hover_text(REPOSITORY)
                        .clicked()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(REPOSITORY));
                    }
                    ui.hyperlink_to(
                        language.text(Message::ReportIssue),
                        format!("{REPOSITORY}/issues"),
                    );
                });
            });
        ui.add_space(16.0);
        ui.heading(language.text(Message::ProjectLicense));
        ui.label(language.text(Message::LicenseSummary));
        egui::CollapsingHeader::new(language.text(Message::LicenseText)).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(GPL).monospace()).wrap());
        });
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                ui.visuals().weak_text_color(),
                language.text(Message::InspiredBy),
            );
            ui.hyperlink_to("LX Music", "https://github.com/lyswhut/lx-music-desktop");
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_github_mark_and_opens_the_project_from_the_button() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let frame = |events| {
            ctx.run(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(530.0, 400.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| show(Language::English, ui));
                },
            )
        };
        let initial = frame(Vec::new());
        assert!(initial.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Rect(rect)
                if rect.brush.as_ref().is_some_and(|brush| brush.fill_texture_id != egui::TextureId::default()))
        }));
        let button = initial
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "GitHub" => {
                    Some(text.galley.rect.translate(text.pos.to_vec2()).center())
                }
                _ => None,
            })
            .unwrap();
        let pointer = |pressed| egui::Event::PointerButton {
            pos: button,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(button), pointer(true)]);
        let clicked = frame(vec![pointer(false)]);
        assert!(clicked.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::OpenUrl(url) if url.url == REPOSITORY)
        }));
    }
}

use super::Message;
use crate::i18n::Language;
use eframe::egui::{self, Color32, Theme};
use rs_music_storage::ThemePreference;

fn rgb([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub(super) fn configure(ctx: &egui::Context, preference: ThemePreference) {
    for theme in [Theme::Dark, Theme::Light] {
        let mut visuals = theme.default_visuals();
        let (background, surface, muted, accent, selection) = match theme {
            Theme::Dark => (
                [18, 21, 25],
                [26, 30, 35],
                [160, 172, 183],
                [137, 221, 184],
                [42, 81, 68],
            ),
            Theme::Light => (
                [242, 245, 247],
                [255, 255, 255],
                [80, 94, 104],
                [19, 100, 72],
                [192, 231, 211],
            ),
        };
        visuals.panel_fill = rgb(background);
        visuals.window_fill = rgb(surface);
        visuals.weak_text_color = Some(rgb(muted));
        visuals.selection.stroke.color = rgb(accent);
        visuals.selection.bg_fill = rgb(selection);
        ctx.set_visuals_of(theme, visuals);
    }
    apply(ctx, preference);
}

fn apply(ctx: &egui::Context, preference: ThemePreference) {
    let (ui_theme, native_theme) = match preference {
        ThemePreference::System => (
            egui::ThemePreference::System,
            egui::SystemTheme::SystemDefault,
        ),
        ThemePreference::Light => (egui::ThemePreference::Light, egui::SystemTheme::Light),
        ThemePreference::Dark => (egui::ThemePreference::Dark, egui::SystemTheme::Dark),
    };
    ctx.set_theme(ui_theme);
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
    ctx.request_repaint();
}

pub(super) fn selector(
    ui: &mut egui::Ui,
    language: Language,
    preference: &mut ThemePreference,
) -> bool {
    let before = *preference;
    ui.horizontal_wrapped(|ui| {
        ui.label(language.text(Message::Theme));
        for (value, label) in [
            (ThemePreference::System, Message::System),
            (ThemePreference::Light, Message::Light),
            (ThemePreference::Dark, Message::Dark),
        ] {
            ui.radio_value(preference, value, language.text(label));
        }
    });
    if before == *preference {
        return false;
    }
    apply(ui.ctx(), *preference);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_changes_switch_both_panels_and_controls_and_respect_manual_choice() {
        let ctx = egui::Context::default();
        configure(&ctx, ThemePreference::System);
        let frame = |system, expected: Theme| {
            let _ = ctx.run(
                egui::RawInput {
                    system_theme: Some(system),
                    ..Default::default()
                },
                |ctx| {
                    let visuals = &ctx.style().visuals;
                    assert_eq!(ctx.theme(), expected);
                    assert_eq!(visuals.dark_mode, expected == Theme::Dark);
                    assert_eq!(
                        visuals.window_fill,
                        ctx.style_of(expected).visuals.window_fill
                    );
                    assert_eq!(
                        visuals.widgets.inactive.bg_fill,
                        expected.default_visuals().widgets.inactive.bg_fill
                    );
                },
            );
        };
        frame(Theme::Light, Theme::Light);
        frame(Theme::Dark, Theme::Dark);
        apply(&ctx, ThemePreference::Dark);
        frame(Theme::Light, Theme::Dark);
        apply(&ctx, ThemePreference::Light);
        frame(Theme::Dark, Theme::Light);
    }
}

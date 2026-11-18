use super::super::{ACCENT, MUTED, Message, MusicApp, now_playing::active_line, widgets::artwork};
use eframe::egui;
use rs_music_core::lyrics::Lyrics;

pub(in crate::app) fn show(app: &mut MusicApp, ui: &mut egui::Ui) {
    let language = app.language();
    let back = ui
        .horizontal(|ui| {
            let back = ui.button(language.text(Message::Back)).clicked();
            ui.heading(language.text(Message::SubtitleMode));
            back
        })
        .inner;
    if back {
        app.toggle_subtitles();
        ui.ctx().request_repaint();
        return;
    }
    ui.add_space(16.0);
    let Some((title, artist)) = app.now_playing.title_and_artist() else {
        ui.label(language.text(Message::NoTrack));
        return;
    };

    let position = app.audio.sink().map(|_| app.audio.position());
    let can_seek = app.audio.can_seek() && app.audio.duration().is_some();
    let active = match app.now_playing.lyrics() {
        Some(Ok(Some(Lyrics::Timed(lines)))) => {
            position.and_then(|position| active_line(lines, position))
        }
        _ => None,
    };
    let follow = app.now_playing.followed_line != active;

    let mut seek = None;
    let mut retry = false;
    let edge = (ui.available_width() * 0.36).clamp(96.0, 280.0);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(edge, edge),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                artwork::show(ui, app.now_playing.artwork(), edge);
                ui.add_space(8.0);
                ui.add(egui::Label::new(egui::RichText::new(title.as_ref()).heading()).truncate())
                    .on_hover_text(title.as_ref());
                if !artist.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(artist).color(MUTED)).truncate())
                        .on_hover_text(artist);
                }
            },
        );
        ui.add_space(16.0);
        ui.vertical(|ui| {
            let loaded = match app.now_playing.lyrics() {
                Some(Ok(Some(loaded))) => loaded,
                Some(Ok(None)) => {
                    ui.label(language.text(Message::NoLyrics));
                    return;
                }
                Some(Err(error)) => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        language.text(Message::LyricsLoadFailed),
                    )
                    .on_hover_text(error);
                    retry = ui.button(language.text(Message::Refresh)).clicked();
                    return;
                }
                None => {
                    ui.spinner();
                    ui.label(language.text(Message::LoadingLyrics));
                    return;
                }
            };
            egui::ScrollArea::vertical()
                .id_salt(("lyrics", app.now_playing.generation))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| match loaded {
                        Lyrics::Plain(text) => {
                            ui.label(text);
                        }
                        Lyrics::Timed(lines) => {
                            for (index, line) in lines.iter().enumerate() {
                                let current = active == Some(index);
                                let mut text = egui::RichText::new(line.text.as_ref()).size(17.0);
                                if current {
                                    text = text.color(ACCENT).strong();
                                }
                                let mut label = egui::Label::new(text);
                                if can_seek {
                                    label = label.sense(egui::Sense::click());
                                }
                                let response = ui.add(label);
                                if current && follow {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                }
                                if response.clicked() {
                                    seek = Some(line.at);
                                }
                                ui.add_space(7.0);
                            }
                        }
                    });
                });
        });
    });
    app.now_playing.followed_line = active;
    if retry {
        app.now_playing.retry_lyrics();
        ui.ctx().request_repaint();
    }
    if let Some(position) = seek {
        let position = app
            .audio
            .duration()
            .map_or(position, |duration| position.min(duration));
        app.seek(position);
    }
}

use super::super::widgets::track_row::{Action, TrackRow};
use super::super::{MUTED, Message, MusicApp, Page, title};
use eframe::egui::{self, RichText};
use rs_music_storage::QueueTrack;

pub(in crate::app) fn show(app: &mut MusicApp, ui: &mut egui::Ui) {
    let language = app.language();
    ui.heading(RichText::new(language.text(app.page.message())).size(30.0));
    ui.add_space(12.0);
    if app.page == Page::Library {
        ui.horizontal(|ui| {
            if ui.button(language.text(Message::ImportFiles)).clicked()
                && let Some(paths) = rfd::FileDialog::new()
                    .add_filter(
                        language.text(Message::AudioFiles),
                        rs_music_core::library::EXTENSIONS,
                    )
                    .pick_files()
            {
                app.import(ui.ctx(), paths);
            }
            if ui.button(language.text(Message::ImportFolder)).clicked()
                && let Some(path) = rfd::FileDialog::new().pick_folder()
            {
                app.import(ui.ctx(), vec![path]);
            }
            ui.add(
                egui::TextEdit::singleline(&mut app.query)
                    .return_key(None)
                    .hint_text(language.text(Message::Search)),
            );
        });
    }
    if !app.imports.is_empty() {
        ui.label(language.text(Message::Scanning));
    }
    if let Some((message, detail)) = &app.storage_error {
        ui.colored_label(ui.visuals().error_fg_color, language.text(*message))
            .on_hover_text(detail);
    }
    ui.add_space(12.0);

    let query = app.query.trim().to_lowercase();
    let in_queue = app.page == Page::Queue;
    let rows: Vec<_> = match app.page {
        Page::Library => (0..app.tracks.len())
            .filter(|&id| title(&app.tracks[id]).to_lowercase().contains(&query))
            .map(QueueTrack::Local)
            .collect(),
        Page::Queue => app.queue.clone(),
        Page::Hall | Page::Search | Page::Settings | Page::About | Page::Lyrics => return,
    };
    if rows.is_empty() {
        let message = match app.page {
            Page::Hall | Page::Search | Page::Settings | Page::About | Page::Lyrics => return,
            Page::Queue => language.text(Message::EmptyQueue),
            Page::Library if app.tracks.is_empty() => language.text(Message::EmptyLibrary),
            Page::Library => language.text(Message::NoResults),
        };
        ui.colored_label(MUTED, message);
    }

    let action = egui::ScrollArea::vertical()
        .show(ui, |ui| {
            let mut selected = None;
            for entry in rows {
                let mut actions = vec![(Action::Play, true)];
                if in_queue {
                    actions.push((Action::RemoveFromQueue, true));
                } else {
                    actions.push((Action::AddToQueue, !app.queue.contains(&entry)));
                    actions.push((Action::RemoveFromLibrary, true));
                }
                let row = TrackRow::new(&entry, &app.tracks, app.current.as_ref() == Some(&entry));
                if let Some(action) = row.show(ui, language, &actions) {
                    selected = Some((entry, action));
                }
            }
            selected
        })
        .inner;
    if let Some((entry, action)) = action {
        app.apply_track_action(action, entry, ui.ctx());
    }
}

use crate::{app::Message, i18n::Language};
use eframe::egui;
use rs_music_storage::PlaybackMode;

pub(in crate::app) fn show(ui: &mut egui::Ui, language: Language, mode: &mut PlaybackMode) -> bool {
    let before = *mode;
    egui::ComboBox::from_id_salt("playback_mode")
        .width(88.0)
        .selected_text(language.text(mode_message(before)))
        .show_ui(ui, |ui| {
            for option in [
                PlaybackMode::Sequential,
                PlaybackMode::RepeatAll,
                PlaybackMode::RepeatOne,
                PlaybackMode::Shuffle,
            ] {
                ui.selectable_value(mode, option, language.text(mode_message(option)));
            }
        })
        .response
        .on_hover_text(language.text(Message::PlaybackMode));
    before != *mode
}

fn mode_message(mode: PlaybackMode) -> Message {
    match mode {
        PlaybackMode::Sequential => Message::Sequential,
        PlaybackMode::RepeatAll => Message::RepeatAll,
        PlaybackMode::RepeatOne => Message::RepeatOne,
        PlaybackMode::Shuffle => Message::Shuffle,
    }
}

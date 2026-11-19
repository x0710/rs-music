use super::super::tasks::{Task, finished, spawn};
use super::super::{Message, MusicApp, PlaybackMode, SURFACE};
use super::search;
use crate::i18n::{LANGUAGE_OPTIONS, Language, preference_label};
use eframe::egui;
use rs_music_storage::CloseAction;
use std::time::Duration;

const CACHE_RANGES: [(Message, Option<Duration>); 6] = [
    (Message::CacheFiveMinutes, Some(Duration::from_secs(5 * 60))),
    (Message::CacheTenMinutes, Some(Duration::from_secs(10 * 60))),
    (
        Message::CacheThirtyMinutes,
        Some(Duration::from_secs(30 * 60)),
    ),
    (Message::CacheOneHour, Some(Duration::from_secs(60 * 60))),
    (
        Message::CacheOneDay,
        Some(Duration::from_secs(24 * 60 * 60)),
    ),
    (Message::CacheAllTime, None),
];

#[derive(Default)]
pub(in crate::app) struct CacheCleanup {
    range: usize,
    pending: Option<Task<usize>>,
    result: Option<Result<usize, String>>,
}

impl CacheCleanup {
    fn show(&mut self, ui: &mut egui::Ui, language: Language) {
        if let Some(result) = self.pending.as_ref().and_then(finished) {
            self.pending = None;
            if let Err(error) = &result {
                tracing::warn!(%error, "Could not clear application cache");
            }
            self.result = Some(result);
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(self.pending.is_none(), |ui| {
                egui::ComboBox::from_id_salt("cache_cleanup_range")
                    .selected_text(language.text(CACHE_RANGES[self.range].0))
                    .show_ui(ui, |ui| {
                        for (index, &(label, _)) in CACHE_RANGES.iter().enumerate() {
                            ui.selectable_value(&mut self.range, index, language.text(label));
                        }
                    });
                if ui.button(language.text(Message::ClearCache)).clicked() {
                    let range = CACHE_RANGES[self.range].1;
                    self.result = None;
                    self.pending = Some(spawn(ui.ctx(), move || {
                        rs_music_storage::clear_recent_cache(range)
                            .map_err(|error| rs_music_core::error_details(&error))
                    }));
                }
            });
            if self.pending.is_some() {
                ui.spinner();
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
        });
        match &self.result {
            Some(Ok(count)) => {
                ui.label(language.cache_cleared(*count));
            }
            Some(Err(error)) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    language.text(Message::CacheClearFailed),
                )
                .on_hover_text(error);
            }
            None => {}
        }
    }
}

pub(in crate::app) fn show(app: &mut MusicApp, ui: &mut egui::Ui) {
    let language = app.language();
    ui.heading(language.text(Message::Settings));
    ui.add_space(16.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(660.0);
            section(ui, language.text(Message::Interface), |ui| {
                ui.horizontal(|ui| {
                    ui.label(language.text(Message::Zoom));
                    if ui.button("−").clicked() {
                        egui::gui_zoom::zoom_out(ui.ctx());
                    }
                    if ui
                        .button(format!("{:.0}%", ui.ctx().zoom_factor() * 100.0))
                        .clicked()
                    {
                        ui.ctx().set_zoom_factor(1.0);
                    }
                    if ui.button("+").clicked() {
                        egui::gui_zoom::zoom_in(ui.ctx());
                    }
                });
                let before = app.language_preference;
                ui.horizontal_wrapped(|ui| {
                    ui.label(language.text(Message::Language));
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(preference_label(before, language))
                        .show_ui(ui, |ui| {
                            for preference in LANGUAGE_OPTIONS {
                                ui.selectable_value(
                                    &mut app.language_preference,
                                    preference,
                                    preference_label(preference, language),
                                );
                            }
                        });
                });
                if before != app.language_preference {
                    app.dirty = true;
                    ui.ctx().request_repaint();
                }
                let before = app.font_family.clone();
                ui.horizontal_wrapped(|ui| {
                    ui.label(language.text(Message::Font));
                    egui::ComboBox::from_id_salt("font_family")
                        .selected_text(
                            before
                                .as_deref()
                                .unwrap_or(language.text(Message::SystemFont)),
                        )
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(
                                    app.font_family.is_none(),
                                    language.text(Message::SystemFont),
                                )
                                .clicked()
                            {
                                app.font_family = None;
                            }
                            for name in app.fonts.names() {
                                if ui
                                    .selectable_label(
                                        app.font_family.as_deref() == Some(name),
                                        name,
                                    )
                                    .clicked()
                                {
                                    app.font_family = Some(name.into());
                                }
                            }
                        });
                });
                if before != app.font_family {
                    app.fonts.install(ui.ctx(), app.font_family.as_deref());
                    app.dirty = true;
                }
                let before = app.close_action;
                ui.horizontal_wrapped(|ui| {
                    ui.label(language.text(Message::CloseAction));
                    egui::ComboBox::from_id_salt("close_action")
                        .selected_text(language.text(close_message(before)))
                        .show_ui(ui, |ui| {
                            for action in [CloseAction::HideToTray, CloseAction::Exit] {
                                ui.selectable_value(
                                    &mut app.close_action,
                                    action,
                                    language.text(close_message(action)),
                                );
                            }
                        });
                });
                app.dirty |= before != app.close_action;
            });
            section(ui, language.text(Message::PlaybackSettings), |ui| {
                let before = app.playback_mode;
                ui.horizontal_wrapped(|ui| {
                    ui.label(language.text(Message::PlaybackMode));
                    egui::ComboBox::from_id_salt("playback_mode")
                        .selected_text(language.text(mode_message(before)))
                        .show_ui(ui, |ui| {
                            for mode in [
                                PlaybackMode::Sequential,
                                PlaybackMode::RepeatAll,
                                PlaybackMode::RepeatOne,
                                PlaybackMode::Shuffle,
                            ] {
                                ui.selectable_value(
                                    &mut app.playback_mode,
                                    mode,
                                    language.text(mode_message(mode)),
                                );
                            }
                        });
                });
                app.dirty |= before != app.playback_mode;
            });
            section(ui, language.text(Message::Downloads), |ui| {
                let before = app.downloads.clone();
                ui.checkbox(
                    &mut app.downloads.audio,
                    language.text(Message::DownloadAudio),
                );
                ui.checkbox(
                    &mut app.downloads.lyrics,
                    language.text(Message::DownloadLyrics),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(language.text(Message::DownloadDirectory));
                    if ui.button(language.text(Message::ChooseDirectory)).clicked() {
                        let mut dialog = rfd::FileDialog::new();
                        if let Some(directory) = app
                            .downloads
                            .directory
                            .as_ref()
                            .cloned()
                            .or_else(rs_music_storage::default_music_directory)
                            .filter(|directory| directory.is_dir())
                        {
                            dialog = dialog.set_directory(directory);
                        }
                        if let Some(path) = dialog.pick_folder() {
                            app.downloads.directory = Some(path);
                        }
                    }
                    if ui
                        .add_enabled(
                            app.downloads.directory.is_some(),
                            egui::Button::new(language.text(Message::DefaultDirectory)),
                        )
                        .clicked()
                    {
                        app.downloads.directory = None;
                    }
                });
                let directory = app
                    .downloads
                    .directory
                    .clone()
                    .or_else(rs_music_storage::default_music_directory);
                let path = directory
                    .as_deref()
                    .map_or_else(|| "Music".into(), std::path::Path::to_string_lossy);
                ui.add(egui::Label::new(path.as_ref()).truncate())
                    .on_hover_text(path.as_ref());
                app.dirty |= before != app.downloads;
            });
            section(ui, language.text(Message::Cache), |ui| {
                app.cache_cleanup.show(ui, language);
            });
            section(ui, language.text(Message::ManageSources), |ui| {
                search::show_source_settings(app, ui);
            });
            if let Some((message, detail)) = &app.storage_error {
                ui.colored_label(ui.visuals().error_fg_color, language.text(*message))
                    .on_hover_text(detail);
            }
        });
}

fn section(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(SURFACE)
        .corner_radius(8)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.heading(title);
            ui.add_space(8.0);
            contents(ui);
        });
    ui.add_space(12.0);
}

fn mode_message(mode: PlaybackMode) -> Message {
    match mode {
        PlaybackMode::Sequential => Message::Sequential,
        PlaybackMode::RepeatAll => Message::RepeatAll,
        PlaybackMode::RepeatOne => Message::RepeatOne,
        PlaybackMode::Shuffle => Message::Shuffle,
    }
}

fn close_message(action: CloseAction) -> Message {
    match action {
        CloseAction::HideToTray => Message::HideToTray,
        CloseAction::Exit => Message::ExitApplication,
    }
}

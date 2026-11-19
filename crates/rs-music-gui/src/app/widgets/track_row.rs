use super::super::{ACCENT, MUTED, Message, SURFACE, title};
use crate::i18n::Language;
use eframe::egui::{self, RichText};
use rs_music_storage::QueueTrack;
use std::{borrow::Cow, path::PathBuf};

#[derive(Clone, Copy)]
pub(in crate::app) enum Action {
    Play,
    Search,
    AddToQueue,
    RemoveFromQueue,
    RemoveFromLibrary,
}

impl Action {
    fn message(self) -> Message {
        match self {
            Self::Play => Message::Play,
            Self::Search => Message::SearchOnline,
            Self::AddToQueue => Message::AddToQueue,
            Self::RemoveFromQueue => Message::RemoveFromQueue,
            Self::RemoveFromLibrary => Message::RemoveFromLibrary,
        }
    }
}

pub(in crate::app) struct TrackRow<'a> {
    title: Cow<'a, str>,
    artist: &'a str,
    detail: Cow<'a, str>,
    pub rank: Option<usize>,
    pub play_unavailable: Option<Message>,
    selected: bool,
}

impl<'a> TrackRow<'a> {
    pub fn new(entry: &'a QueueTrack, local: &'a [PathBuf], selected: bool) -> Self {
        let (title, artist, detail) = match entry {
            QueueTrack::Local(id) => {
                let path = &local[*id];
                (title(path).into(), "", path.to_string_lossy())
            }
            QueueTrack::Online(online) => {
                let track = &online.track;
                (
                    track.title.as_str().into(),
                    track.artist.as_str(),
                    track.title.as_str().into(),
                )
            }
        };
        Self {
            title,
            artist,
            detail,
            rank: None,
            play_unavailable: None,
            selected,
        }
    }

    pub fn show(
        self,
        ui: &mut egui::Ui,
        language: Language,
        actions: &[(Action, bool)],
    ) -> Option<Action> {
        let action = egui::Frame::new()
            .fill(SURFACE)
            .corner_radius(6)
            .inner_margin(7)
            .show(ui, |ui| {
                let width = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 36.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let mut picked = None;
                        for &(action, enabled) in actions {
                            let label = language.text(action.message());
                            let button = match action {
                                Action::AddToQueue if enabled => egui::Button::new(
                                    RichText::new(label).color(egui::Color32::WHITE),
                                )
                                .fill(egui::Color32::from_rgb(32, 104, 83)),
                                Action::RemoveFromQueue => egui::Button::new(
                                    RichText::new(label).color(egui::Color32::WHITE),
                                )
                                .fill(egui::Color32::from_rgb(141, 62, 49)),
                                _ => egui::Button::new(label),
                            };
                            let mut response = ui.add_enabled(enabled, button.truncate());
                            if let Action::Play = action
                                && let Some(reason) = self.play_unavailable
                            {
                                response = response.on_disabled_hover_text(language.text(reason));
                            }
                            if response.clicked() {
                                picked = Some(action);
                            }
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            if let Some(rank) = self.rank {
                                ui.add_sized(
                                    [24.0, 20.0],
                                    egui::Label::new(RichText::new(rank.to_string()).color(MUTED)),
                                );
                            }
                            ui.vertical(|ui| {
                                let mut title = RichText::new(self.title.as_ref()).strong();
                                if self.selected {
                                    title = title.color(ACCENT);
                                }
                                ui.add(egui::Label::new(title).truncate())
                                    .on_hover_text(self.detail.as_ref());
                                if !self.artist.is_empty() {
                                    ui.add(
                                        egui::Label::new(RichText::new(self.artist).color(MUTED))
                                            .truncate(),
                                    )
                                    .on_hover_text(self.artist);
                                }
                            });
                        });
                        picked
                    },
                )
                .inner
            })
            .inner;
        ui.add_space(3.0);
        action
    }
}

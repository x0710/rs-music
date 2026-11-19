use super::super::{Message, title};
use crate::i18n::Language;
use eframe::egui::{self, RichText};
use rs_music_storage::QueueTrack;
use std::{borrow::Cow, path::PathBuf};

#[derive(Clone, Copy)]
pub(in crate::app) enum Action {
    Play,
    QueueAndPlay,
    Search,
    AddToQueue,
    RemoveFromQueue,
    RemoveFromLibrary,
}

impl Action {
    fn message(self) -> Message {
        match self {
            Self::Play | Self::QueueAndPlay => Message::Play,
            Self::Search => Message::SearchOnline,
            Self::AddToQueue => Message::AddToQueue,
            Self::RemoveFromQueue => Message::RemoveFromQueue,
            Self::RemoveFromLibrary => Message::RemoveFromLibrary,
        }
    }
}

/// 子按钮优先处理点击，双击行内文字或空白才把曲目加入队列并播放。
pub(in crate::app) fn interactive_row(
    ui: &mut egui::Ui,
    playable: bool,
    contents: impl FnOnce(&mut egui::Ui) -> Option<Action>,
) -> Option<Action> {
    let row = ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
        ui.style_mut().interaction.selectable_labels = false;
        contents(ui)
    });
    row.inner
        .or_else(|| (playable && row.response.double_clicked()).then_some(Action::QueueAndPlay))
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
        let playable = actions
            .iter()
            .any(|(action, enabled)| matches!(action, Action::Play) && *enabled);
        let action = interactive_row(ui, playable, |ui| {
            egui::Frame::new()
                .fill(ui.visuals().window_fill)
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
                                    response =
                                        response.on_disabled_hover_text(language.text(reason));
                                }
                                if response.clicked() {
                                    picked = Some(action);
                                }
                            }
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    if let Some(rank) = self.rank {
                                        ui.add_sized(
                                            [24.0, 20.0],
                                            egui::Label::new(
                                                RichText::new(rank.to_string())
                                                    .color(ui.visuals().weak_text_color()),
                                            ),
                                        );
                                    }
                                    ui.vertical(|ui| {
                                        let mut title = RichText::new(self.title.as_ref()).strong();
                                        if self.selected {
                                            title =
                                                title.color(ui.visuals().selection.stroke.color);
                                        }
                                        ui.add(egui::Label::new(title).truncate())
                                            .on_hover_text(self.detail.as_ref());
                                        if !self.artist.is_empty() {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(self.artist)
                                                        .color(ui.visuals().weak_text_color()),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(self.artist);
                                        }
                                    });
                                },
                            );
                            picked
                        },
                    )
                    .inner
                })
                .inner
        });
        ui.add_space(3.0);
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_clicking_title_queues_but_clicking_play_only_plays() {
        for (target, enabled) in [("Song.wav", true), ("Play", true), ("Song.wav", false)] {
            let ctx = egui::Context::default();
            let frame = |events| {
                let mut action = None;
                let output = ctx.run(
                    egui::RawInput {
                        events,
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(480.0, 100.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            action =
                                TrackRow::new(&QueueTrack::Local(0), &["Song.wav".into()], false)
                                    .show(ui, Language::English, &[(Action::Play, enabled)]);
                        });
                    },
                );
                (output, action)
            };
            let pos = frame(vec![])
                .0
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == target => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()).center())
                    }
                    _ => None,
                })
                .unwrap();
            let event = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            for second in [false, true] {
                frame(vec![egui::Event::PointerMoved(pos), event(true)]);
                let action = frame(vec![event(false)]).1;
                if target == "Play" {
                    assert!(matches!(action, Some(Action::Play)));
                } else if enabled && second {
                    assert!(matches!(action, Some(Action::QueueAndPlay)));
                } else {
                    assert!(action.is_none());
                }
            }
        }
    }
}

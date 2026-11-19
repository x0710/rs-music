use super::super::{Message, MusicApp, Page};
use super::artwork;
use crate::{
    audio::{ArchiveState, DownloadProgress},
    i18n::Language,
};
use eframe::egui::{self, RichText};
use rs_music_storage::QueueTrack;
use std::time::Duration;

pub(in crate::app) fn show(app: &mut MusicApp, ui: &mut egui::Ui) {
    let language = app.language();
    let track = app.playback_title();
    ui.horizontal(|ui| {
        if app.current.is_some() {
            let cover = artwork::show(ui, app.now_playing.artwork(), 48.0)
                .interact(egui::Sense::click())
                .on_hover_text(language.text(Message::SubtitleMode));
            if cover.clicked() && app.page != Page::Lyrics {
                app.toggle_subtitles();
            }
        }
        let lyric = app
            .audio
            .sink()
            .and_then(|_| app.now_playing.current_line(app.audio.position()));
        let title = egui::Label::new(RichText::new(&track).strong()).truncate();
        if let Some(lyric) = lyric {
            ui.scope(|ui| {
                ui.set_max_width(ui.available_width() * 0.4);
                ui.add(title).on_hover_text(&track);
            });
            ui.add(
                egui::Label::new(RichText::new(lyric).color(ui.visuals().selection.stroke.color))
                    .truncate(),
            )
            .on_hover_text(lyric);
        } else {
            ui.add(title).on_hover_text(track);
        }
    });
    if matches!(app.current, Some(QueueTrack::Online(_)))
        && let Some(source) = &app.hall.fallback_source
    {
        let notice = format!("{} · {source}", language.text(Message::AutoSwitchedSource));
        ui.add(
            egui::Label::new(RichText::new(&notice).color(ui.visuals().selection.stroke.color))
                .truncate(),
        )
        .on_hover_text(notice);
    }
    if app.audio.sink().is_some() {
        show_progress(app, ui);
    }
    if let Some(progress) = app.buffering_progress() {
        let decision = ui
            .horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let decision = match app.audio.archive_state().filter(|_| progress.complete) {
                        Some(ArchiveState::Pending(_)) => {
                            let skip = ui.button(language.text(Message::SkipSave)).clicked();
                            let save = ui.button(language.text(Message::SaveToMusic)).clicked();
                            if save {
                                Some(true)
                            } else if skip {
                                Some(false)
                            } else {
                                None
                            }
                        }
                        Some(ArchiveState::Requested) => {
                            ui.colored_label(
                                ui.visuals().weak_text_color(),
                                language.text(Message::SaveRequested),
                            );
                            None
                        }
                        Some(ArchiveState::Skipped) => {
                            ui.colored_label(
                                ui.visuals().weak_text_color(),
                                language.text(Message::SaveSkipped),
                            );
                            None
                        }
                        None => None,
                    };
                    show_buffer_progress(progress, language, ui);
                    decision
                })
                .inner
            })
            .inner;
        if let Some(save) = decision {
            app.choose_audio_save(save, ui.ctx());
        }
    }
    ui.horizontal_wrapped(|ui| {
        if let Some(id) = app.previous_track().cloned()
            && ui.button(language.text(Message::Previous)).clicked()
        {
            app.play(id, ui.ctx());
        }
        let button = if app.audio.playing() {
            language.text(Message::Pause)
        } else {
            language.text(Message::Play)
        };
        let can_play = app.hall.download.is_none() && app.has_playback_selection();
        if ui
            .add_enabled(can_play, egui::Button::new(button))
            .clicked()
        {
            app.toggle_playback(ui.ctx());
        }
        if ui
            .add_enabled(
                app.can_go_next(),
                egui::Button::new(language.text(Message::Next)),
            )
            .clicked()
            && let Some(id) = app.next_track(false).cloned()
        {
            app.play(id, ui.ctx());
        }

        app.dirty |= super::playback_mode::show(ui, language, &mut app.playback_mode);

        if app.hall.download.is_some() {
            ui.spinner();
            ui.colored_label(
                ui.visuals().weak_text_color(),
                language.text(Message::LoadingAudio),
            );
        } else if app.audio.buffering() {
            ui.colored_label(
                ui.visuals().weak_text_color(),
                language.text(Message::Buffering),
            );
        }
        if app.now_playing.has_track() || app.subtitle_window_open {
            ui.toggle_value(
                &mut app.subtitle_window_open,
                language.text(Message::Subtitles),
            );
        }
        ui.label(language.text(Message::Volume));
        let mut volume = app.volume;
        if ui
            .add(egui::Slider::new(&mut volume, 0..=100).suffix("%"))
            .changed()
        {
            app.set_volume(volume);
        }
    });
    if let Some((message, detail)) = &app.playback_error {
        ui.colored_label(ui.visuals().error_fg_color, language.text(*message))
            .on_hover_text(detail);
    }
}

fn show_buffer_progress(progress: DownloadProgress, language: Language, ui: &mut egui::Ui) {
    let label = language.text(Message::Buffered);
    let received = byte_size(progress.received);
    if !progress.complete {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
    if let Some(fraction) = progress.fraction() {
        let percent = (fraction * 100.0).floor() as u8;
        let amount = progress.total.map_or(received.clone(), |total| {
            format!("{received} / {}", byte_size(total))
        });
        ui.add(
            egui::ProgressBar::new(fraction)
                .desired_width(ui.available_width())
                .text(format!("{label} {percent}% · {amount}")),
        );
    } else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.colored_label(
                ui.visuals().weak_text_color(),
                format!("{label} {received}"),
            );
        });
    }
}

fn byte_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn show_progress(app: &mut MusicApp, ui: &mut egui::Ui) {
    let current = app.audio.position();
    let duration = app.audio.duration().filter(|duration| !duration.is_zero());
    ui.horizontal(|ui| {
        ui.colored_label(
            ui.visuals().weak_text_color(),
            time(app.seek_preview.unwrap_or(current)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.colored_label(
                ui.visuals().weak_text_color(),
                duration.map_or_else(|| "--:--".into(), time),
            );
            let width = ui.available_width();
            if let Some(duration) = duration
                && app.audio.can_seek()
            {
                ui.spacing_mut().slider_width = width;
                if let Some(position) = seek_slider(ui, current, duration, &mut app.seek_preview) {
                    app.seek(position);
                }
            } else {
                app.seek_preview = None;
                let progress = duration.map_or(0.0, |duration| {
                    (current.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0) as f32
                });
                ui.add(
                    egui::ProgressBar::new(progress)
                        .desired_width(width)
                        .animate(duration.is_none() && app.audio.playing()),
                );
            }
        });
    });
}

fn seek_slider(
    ui: &mut egui::Ui,
    current: Duration,
    duration: Duration,
    preview: &mut Option<Duration>,
) -> Option<Duration> {
    let previous = preview.take();
    let mut position = previous.unwrap_or(current).min(duration).as_secs_f64();
    let response =
        ui.add(egui::Slider::new(&mut position, 0.0..=duration.as_secs_f64()).show_value(false));
    let position = Duration::from_secs_f64(position);
    if response.drag_started() || response.dragged() {
        *preview = Some(position);
        None
    } else if response.changed() || (response.drag_stopped() && previous.is_some()) {
        Some(position)
    } else {
        None
    }
}

fn time(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitles_button_opens_closes_and_reopens_the_window() {
        let ctx = egui::Context::default();
        let mut app = MusicApp::from_state(&ctx, Default::default(), None, None);
        let label = app.language().text(Message::Subtitles);
        app.now_playing.local("song.wav".into());
        let frame = |app: &mut MusicApp, events| {
            ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| show(app, ui));
                },
            )
        };
        for open in [true, false, true, false] {
            let output = frame(&mut app, vec![]);
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()).center())
                    }
                    _ => None,
                })
                .expect("subtitle toggle visible");
            let click = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(&mut app, vec![egui::Event::PointerMoved(pos), click(true)]);
            frame(&mut app, vec![click(false)]);
            assert_eq!(app.subtitle_window_open, open);
        }
    }

    #[test]
    fn dragging_playback_progress_seeks_only_on_release() {
        let ctx = egui::Context::default();
        let mut preview = None;
        let mut frame = |events| {
            let mut result = None;
            let mut slider = egui::Rect::NOTHING;
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.spacing_mut().slider_width = 200.0;
                        slider = egui::Rect::from_min_size(
                            ui.next_widget_position(),
                            egui::vec2(200.0, ui.spacing().interact_size.y),
                        );
                        result =
                            seek_slider(ui, Duration::ZERO, Duration::from_secs(100), &mut preview);
                    });
                },
            );
            (result, slider)
        };
        let (_, slider) = frame(Vec::new());
        let press = slider.left_center() + egui::vec2(50.0, 0.0);
        let pointer_button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        assert_eq!(
            frame(vec![
                egui::Event::PointerMoved(press),
                pointer_button(press, true)
            ])
            .0,
            None
        );
        let target = slider.left_center() + egui::vec2(150.0, 0.0);
        assert_eq!(frame(vec![egui::Event::PointerMoved(target)]).0, None);
        assert!(
            frame(vec![pointer_button(target, false)])
                .0
                .is_some_and(|position| (70..=85).contains(&position.as_secs()))
        );
        assert_eq!(frame(Vec::new()).0, None);
    }
}

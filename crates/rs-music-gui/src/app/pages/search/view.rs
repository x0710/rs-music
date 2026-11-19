use super::Hall;
use crate::{
    app::{
        Message, MusicApp, Page,
        widgets::{
            pagination,
            track_row::{Action, interactive_row},
        },
    },
    i18n::Language,
};
use eframe::egui;
use rs_music_core::{Source, Track};

fn show_search_history(hall: &mut Hall, language: Language, ui: &mut egui::Ui) -> bool {
    if hall.search_history.is_empty() {
        return false;
    }

    let can_search = hall.catalog().is_some() && hall.search.is_none();
    let mut selected = None;
    let clear = ui
        .horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let clear = ui
                    .small_button(language.text(Message::ClearSearchHistory))
                    .clicked();
                egui::ScrollArea::horizontal()
                    .id_salt("recent_searches")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            for query in &hall.search_history {
                                let response = ui.scope(|ui| {
                                    ui.set_max_width(180.0);
                                    ui.add_enabled(can_search, egui::Button::new(query).truncate())
                                        .on_hover_text(query)
                                });
                                if response.inner.clicked() {
                                    selected = Some(query.clone());
                                }
                            }
                        });
                    });
                clear
            })
            .inner
        })
        .inner;
    if clear {
        hall.search_history.clear();
        true
    } else if let Some(query) = selected {
        hall.query = query;
        hall.submit_search(ui.ctx())
    } else {
        false
    }
}

pub(in crate::app) fn find_track(app: &mut MusicApp, track: &Track, ctx: &egui::Context) {
    let hall = &mut app.hall;
    hall.query = format!("{} {}", track.title, track.artist)
        .trim()
        .to_owned();
    if hall.submit_search(ctx) {
        hall.save_search_history(app.store.as_ref());
    }
    app.page = Page::Search;
}

pub(super) fn show_search_toolbar(hall: &mut Hall, language: Language, ui: &mut egui::Ui) -> bool {
    let has_catalog = hall.catalog().is_some();
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Source::Lx(source) = &hall.source
                && let Some(qualities) = source.platforms().get(&hall.platform)
            {
                egui::ComboBox::from_id_salt("playback_quality")
                    .width(112.0)
                    .truncate()
                    .selected_text(format!(
                        "{} · {}",
                        language.text(Message::Quality),
                        hall.quality
                    ))
                    .show_ui(ui, |ui| {
                        for quality in qualities {
                            ui.selectable_value(&mut hall.quality, quality.clone(), quality);
                        }
                    });
            }
            let can_search = has_catalog && hall.search.is_none() && !hall.query.trim().is_empty();
            let submit = ui
                .add_enabled(
                    can_search,
                    egui::Button::new(language.text(Message::SearchOnline))
                        .min_size(egui::vec2(0.0, 36.0)),
                )
                .clicked();
            let field = ui.add_enabled(
                has_catalog,
                egui::TextEdit::singleline(&mut hall.query)
                    .id(egui::Id::new("online_search_box"))
                    .return_key(None)
                    .font(egui::FontId::proportional(18.0))
                    .margin(egui::Margin::symmetric(12, 8))
                    .vertical_align(egui::Align::Center)
                    .min_size(egui::vec2(0.0, 44.0))
                    .desired_width(f32::INFINITY)
                    .hint_text(language.text(Message::OnlineQuery)),
            );
            let enter = if field.has_focus() {
                ui.input(|input| search_enter(&input.events, &mut hall.search_composing))
            } else {
                hall.search_composing = false;
                false
            };
            (submit || enter)
                && has_catalog
                && hall.search.is_none()
                && !hall.query.trim().is_empty()
                && hall.submit_search(ui.ctx())
        })
        .inner
    })
    .inner
}

pub(super) fn search_enter(events: &[egui::Event], composing: &mut bool) -> bool {
    // Some input methods emit an Enter key alongside the candidate confirmation.
    let mut ime_this_frame = *composing;
    let mut enter = false;
    for event in events {
        match event {
            egui::Event::Ime(egui::ImeEvent::Preedit(text)) => {
                *composing = !text.is_empty();
                ime_this_frame |= *composing;
            }
            egui::Event::Ime(egui::ImeEvent::Commit(_)) => {
                ime_this_frame = true;
                *composing = false;
            }
            egui::Event::Ime(egui::ImeEvent::Disabled) => *composing = false,
            egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } if *modifiers == egui::Modifiers::NONE => enter = true,
            _ => {}
        }
    }
    enter && !ime_this_frame
}

fn show_platform_tabs(hall: &mut Hall, language: Language, ui: &mut egui::Ui) {
    const PLATFORMS: [(&str, Message); 5] = [
        ("tx", Message::PlatformQQ),
        ("kg", Message::PlatformKugou),
        ("kw", Message::PlatformKuwo),
        ("mg", Message::PlatformMigu),
        ("wy", Message::PlatformNetEase),
    ];
    let has_default = !matches!(hall.source, Source::Lx(_));
    let tabs = has_default
        .then_some(("", Message::DefaultCatalog))
        .into_iter()
        .chain(PLATFORMS);
    ui.columns(PLATFORMS.len() + usize::from(has_default), |columns| {
        for (column, (platform, message)) in columns.iter_mut().zip(tabs) {
            let active = hall.platform == platform;
            if column
                .add_sized(
                    [column.available_width(), 32.0],
                    egui::Button::selectable(active, language.text(message)).truncate(),
                )
                .clicked()
                && !active
            {
                hall.select_platform(platform, column.ctx());
            }
        }
    });
}

pub(in crate::app) fn show(app: &mut MusicApp, ui: &mut egui::Ui) {
    let language = app.language();
    let before = app.hall.selection();
    let hall = &mut app.hall;
    ui.heading(language.text(Message::MusicSearch));
    ui.add_space(12.0);
    let history_changed = show_search_history(hall, language, ui);
    let history_changed = show_search_toolbar(hall, language, ui) || history_changed;
    ui.add_space(8.0);
    show_platform_tabs(hall, language, ui);
    ui.separator();
    if history_changed {
        hall.save_search_history(app.store.as_ref());
    }
    if hall.catalog().is_none() {
        ui.label(language.text(Message::ImportSearchCatalog));
    }
    if let Some((message, error)) = &hall.search_history_error {
        ui.colored_label(ui.visuals().error_fg_color, language.text(*message))
            .on_hover_text(error);
    }
    if hall.search.is_some() {
        ui.label(language.text(Message::Searching));
    }
    if let Some((message, error)) = &hall.error {
        ui.colored_label(ui.visuals().error_fg_color, language.text(*message))
            .on_hover_text(error);
    }
    let picked = results(hall, language, ui);
    let idle = hall.search.is_none();
    if !hall.submitted_query.is_empty()
        && let Some(page) = pagination::show(ui, language, hall.page, idle, idle && hall.has_next)
    {
        hall.search_page(page, ui.ctx());
    }
    if let Some((track, action)) = picked {
        app.apply_track_action(action, app.hall.online_track(track).into(), ui.ctx());
    }
    app.dirty |= before != app.hall.selection();
}

fn results(hall: &Hall, language: Language, ui: &mut egui::Ui) -> Option<(Track, Action)> {
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt((&hall.platform, &hall.submitted_query, hall.page))
        .max_height((ui.available_height() - 36.0).max(24.0))
        .show(ui, |ui| {
            for track in &hall.tracks {
                let playable = hall.can_play_search_result(track);
                let enabled = playable && hall.download.is_none();
                let action = interactive_row(ui, enabled, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let reason = if playable {
                            Message::LoadingAudio
                        } else if track.id.is_none() && !hall.platform.is_empty() {
                            Message::MissingPlaybackId
                        } else {
                            Message::NoCompatiblePlaybackSource
                        };
                        let play = ui
                            .add_enabled(enabled, egui::Button::new(language.text(Message::Play)))
                            .on_disabled_hover_text(language.text(reason))
                            .clicked();
                        if !playable && let Some(url) = &track.store_url {
                            ui.hyperlink_to(language.text(Message::ViewOnITunes), url.as_str());
                        }
                        ui.strong(&track.title);
                        ui.label(&track.artist);
                        play.then_some(Action::Play)
                    })
                    .inner
                });
                if let Some(action) = action {
                    picked = Some((track.clone(), action));
                }
                ui.separator();
            }
        });
    picked
}

mod covers;
mod queue_import;
use super::super::tasks::{Task, finished, spawn};
use super::super::widgets::{
    artwork, pagination,
    track_row::{Action, TrackRow},
};
use super::super::{MUTED, Message, MusicApp, SURFACE};
use crate::i18n::Language;
use covers::{Cover, playlist_texture};
use eframe::egui;
pub(in crate::app) use queue_import::poll_queue_imports;
use queue_import::{QueueImport, show_queue_import};
use rs_music_core::{
    DiscoveryData, DiscoveryPlatform, Playlist, PlaylistPage, PlaylistTracks, Track, Url,
    public_error,
};
use rs_music_storage::{OnlineTrack, QueueTrack};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

enum Load<T> {
    Pending(Task<DiscoveryData<T>>),
    Complete(Result<DiscoveryData<T>, String>),
}

impl<T> Load<T> {
    fn poll(&mut self, ctx: &egui::Context) {
        if let Self::Pending(task) = self {
            if let Some(result) = finished(task) {
                *self = Self::Complete(result);
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    fn expired(&self, ctx: &egui::Context) -> bool {
        if let Self::Complete(Ok(data)) = self {
            return expired(data.valid_until, ctx);
        }
        false
    }
}

fn expired(valid_until: Instant, ctx: &egui::Context) -> bool {
    let remaining = valid_until.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        true
    } else {
        ctx.request_repaint_after(remaining);
        false
    }
}

struct Feeds {
    hot: Trending,
    playlists: Load<PlaylistPage>,
    page: usize,
}

impl Feeds {
    fn load(platform: DiscoveryPlatform, ctx: &egui::Context) -> Self {
        Self {
            hot: Trending {
                page: 1,
                data: None,
                pending: Some((1, load_trending(platform, 1, false, ctx))),
                error: None,
            },
            playlists: load_playlists(platform, 1, false, ctx),
            page: 1,
        }
    }
}

struct Trending {
    page: usize,
    data: Option<DiscoveryData<PlaylistTracks>>,
    pending: Option<(usize, Task<DiscoveryData<PlaylistTracks>>)>,
    error: Option<String>,
}

impl Trending {
    fn request(
        &mut self,
        platform: DiscoveryPlatform,
        page: usize,
        refresh: bool,
        ctx: &egui::Context,
    ) {
        self.pending = Some((page, load_trending(platform, page, refresh, ctx)));
        self.error = None;
    }

    fn poll(&mut self, platform: DiscoveryPlatform, ctx: &egui::Context) {
        if let Some((page, task)) = &self.pending {
            match finished(task) {
                Some(Ok(data)) => {
                    self.page = *page;
                    self.data = Some(data);
                    self.pending = None;
                }
                Some(Err(error)) => {
                    self.error = Some(error);
                    self.pending = None;
                }
                None => ctx.request_repaint_after(Duration::from_millis(100)),
            }
        } else if self.error.is_none()
            && self
                .data
                .as_ref()
                .is_some_and(|data| expired(data.valid_until, ctx))
        {
            self.request(platform, self.page, false, ctx);
        }
    }
}

fn load_trending(
    platform: DiscoveryPlatform,
    page: usize,
    refresh: bool,
    ctx: &egui::Context,
) -> Task<DiscoveryData<PlaylistTracks>> {
    spawn(ctx, move || {
        platform
            .cached_popular_track_page(page, refresh)
            .map_err(public_error)
    })
}

fn load_playlists(
    platform: DiscoveryPlatform,
    page: usize,
    refresh: bool,
    ctx: &egui::Context,
) -> Load<PlaylistPage> {
    Load::Pending(spawn(ctx, move || {
        platform
            .community_playlist_page(page, refresh)
            .map_err(public_error)
    }))
}

fn load_playlist(
    platform: DiscoveryPlatform,
    id: u64,
    page: usize,
    refresh: bool,
    ctx: &egui::Context,
) -> Load<PlaylistTracks> {
    Load::Pending(spawn(ctx, move || {
        platform
            .cached_playlist_tracks(id, page, refresh)
            .map_err(public_error)
    }))
}

struct PlaylistView {
    playlist: Playlist,
    page: usize,
    tracks: Load<PlaylistTracks>,
}
#[derive(Default)]
struct PlatformPage {
    feeds: Option<Feeds>,
    playlist: Option<PlaylistView>,
    covers: HashMap<Url, Cover>,
}

#[derive(Default)]
pub(in crate::app) struct Discovery {
    platform: DiscoveryPlatform,
    pages: HashMap<DiscoveryPlatform, PlatformPage>,
    queue_imports: HashMap<(DiscoveryPlatform, u64), QueueImport>,
}
#[cfg(target_os = "linux")]
impl Discovery {
    pub(in crate::app) fn clear_textures(&mut self) {
        for page in self.pages.values_mut() {
            page.covers
                .retain(|_, cover| !matches!(cover, Cover::Ready(_)));
        }
    }
}

pub(in crate::app) fn show(app: &mut MusicApp, ui: &mut egui::Ui) {
    let language = app.language();
    ui.heading(language.text(Message::Hall));
    ui.add_space(8.0);
    ui.columns(DiscoveryPlatform::ALL.len(), |columns| {
        for (column, platform) in columns.iter_mut().zip(DiscoveryPlatform::ALL) {
            let label = language.text(match platform {
                DiscoveryPlatform::NetEase => Message::PlatformNetEase,
                DiscoveryPlatform::QQ => Message::PlatformQQ,
                DiscoveryPlatform::Kugou => Message::PlatformKugou,
                DiscoveryPlatform::Kuwo => Message::PlatformKuwo,
            });
            if column
                .add_sized(
                    [column.available_width(), 32.0],
                    egui::Button::selectable(app.discovery.platform == platform, label).truncate(),
                )
                .clicked()
            {
                app.discovery.platform = platform;
            }
        }
    });
    ui.add_space(8.0);
    let platform = app.discovery.platform;
    let rows = TrackRows {
        platform,
        playable: app.hall.can_play_lx(platform.code()),
        queue: &app.queue,
        current: app.current.as_ref(),
    };
    let discovery = app.discovery.pages.entry(platform).or_default();
    let mut picked_track = None;
    if let Some(view) = &mut discovery.playlist {
        if ui.button(language.text(Message::Back)).clicked() {
            discovery.playlist = None;
            return;
        }
        ui.add_space(8.0);
        view.tracks.poll(ui.ctx());
        if let Load::Complete(Ok(data)) = &view.tracks
            && let Some(description) = &data.value.description
            && view.playlist.description.as_ref() != Some(description)
        {
            view.playlist.description = Some(description.clone());
        }
        show_playlist_header(view, platform, &mut discovery.covers, language, ui);
        ui.add_space(8.0);
        let idle = matches!(view.tracks, Load::Complete(_));
        let refresh = ui
            .horizontal_wrapped(|ui| {
                let refresh = ui
                    .add_enabled(idle, egui::Button::new(language.text(Message::Refresh)))
                    .clicked();
                let id = view.playlist.id;
                show_queue_import(&mut app.discovery.queue_imports, platform, id, language, ui);
                refresh
            })
            .inner;
        if refresh || view.tracks.expired(ui.ctx()) {
            view.tracks = load_playlist(platform, view.playlist.id, view.page, refresh, ui.ctx());
        }
        let has_next = matches!(&view.tracks, Load::Complete(Ok(data)) if data.value.has_next);
        if let Some(page) = pagination::show(ui, language, view.page, idle, has_next) {
            view.page = page;
            view.tracks = load_playlist(platform, view.playlist.id, page, false, ui.ctx());
        }
        egui::ScrollArea::vertical()
            .id_salt((platform, view.playlist.id, view.page))
            .show(ui, |ui| {
                if let Some(detail) = loaded(&view.tracks, language, ui) {
                    let total = detail.total.max(view.playlist.total.unwrap_or(0));
                    if detail.tracks.len() < total {
                        ui.colored_label(
                            MUTED,
                            language.playlist_track_count(detail.tracks.len(), total),
                        );
                    }
                    picked_track = rows.show(&detail.tracks, None, language, ui);
                }
            });
    } else {
        let feeds = discovery
            .feeds
            .get_or_insert_with(|| Feeds::load(platform, ui.ctx()));
        feeds.hot.poll(platform, ui.ctx());
        feeds.playlists.poll(ui.ctx());
        if feeds.playlists.expired(ui.ctx()) {
            feeds.playlists = load_playlists(platform, feeds.page, false, ui.ctx());
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    feeds.hot.pending.is_none() && matches!(feeds.playlists, Load::Complete(_)),
                    egui::Button::new(language.text(Message::Refresh)),
                )
                .clicked()
            {
                feeds.hot.request(platform, feeds.hot.page, true, ui.ctx());
                feeds.playlists = load_playlists(platform, feeds.page, true, ui.ctx());
                discovery
                    .covers
                    .retain(|_, cover| !matches!(cover, Cover::Unavailable(_)));
            }
        });
        ui.add_space(12.0);
        let mut picked_playlist = None;
        egui::ScrollArea::vertical()
            .id_salt(("discovery", platform))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(language.text(Message::PopularTracks));
                    if feeds.hot.pending.is_some() {
                        ui.add(egui::Spinner::new().size(14.0))
                            .on_hover_text(language.text(Message::LoadingDiscovery));
                    } else if let Some(error) = &feeds.hot.error {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            language.text(Message::DiscoveryLoadFailed),
                        )
                        .on_hover_text(error);
                    }
                });
                ui.add_space(6.0);
                let idle = feeds.hot.pending.is_none();
                let has_next = feeds
                    .hot
                    .data
                    .as_ref()
                    .is_some_and(|data| data.value.has_next);
                if let Some(page) =
                    pagination::show(ui, language, feeds.hot.page, idle, idle && has_next)
                {
                    feeds.hot.request(platform, page, false, ui.ctx());
                }
                if let Some(data) = &feeds.hot.data {
                    let offset =
                        (feeds.hot.page - 1).saturating_mul(platform.popular_track_page_size());
                    picked_track = rows.show(&data.value.tracks, Some(offset), language, ui);
                }
                ui.add_space(16.0);
                ui.heading(language.text(Message::CommunityPlaylists));
                ui.add_space(8.0);
                let has_next =
                    matches!(&feeds.playlists, Load::Complete(Ok(data)) if data.value.has_next);
                let idle = matches!(feeds.playlists, Load::Complete(_));
                if let Some(page) = pagination::show(ui, language, feeds.page, idle, has_next) {
                    feeds.page = page;
                    feeds.playlists = load_playlists(platform, page, false, ui.ctx());
                }
                if let Some(page) = loaded(&feeds.playlists, language, ui) {
                    discovery.covers.retain(|url, _| {
                        page.playlists
                            .iter()
                            .any(|playlist| playlist.cover_url.as_ref() == Some(url))
                    });
                    picked_playlist = show_playlists(
                        platform,
                        &page.playlists,
                        &mut discovery.covers,
                        language,
                        ui,
                    );
                }
            });
        if let Some(playlist) = picked_playlist {
            let id = playlist.id;
            discovery.playlist = Some(PlaylistView {
                playlist,
                page: 1,
                tracks: load_playlist(platform, id, 1, false, ui.ctx()),
            });
            ui.ctx().request_repaint();
        }
    }
    if let Some((entry, action)) = picked_track {
        app.apply_track_action(action, entry, ui.ctx());
    }
}
fn loaded<'a, T>(load: &'a Load<T>, language: Language, ui: &mut egui::Ui) -> Option<&'a T> {
    match load {
        Load::Pending(_) => {
            ui.label(language.text(Message::LoadingDiscovery));
            None
        }
        Load::Complete(Err(error)) => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                language.text(Message::DiscoveryLoadFailed),
            )
            .on_hover_text(error);
            None
        }
        Load::Complete(Ok(data)) => Some(&data.value),
    }
}

struct TrackRows<'a> {
    platform: DiscoveryPlatform,
    playable: bool,
    queue: &'a [QueueTrack],
    current: Option<&'a QueueTrack>,
}

impl TrackRows<'_> {
    fn show(
        &self,
        tracks: &[Track],
        rank_offset: Option<usize>,
        language: Language,
        ui: &mut egui::Ui,
    ) -> Option<(QueueTrack, Action)> {
        if tracks.is_empty() {
            ui.label(language.text(Message::NoResults));
            return None;
        }
        let mut selected = None;
        let columns = if rank_offset.is_some() && ui.available_width() >= 600.0 {
            2
        } else {
            1
        };
        ui.columns(columns, |columns| {
            let chunk_size = tracks.len().div_ceil(columns.len());
            for (column, (column_index, chunk)) in columns
                .iter_mut()
                .zip(tracks.chunks(chunk_size).enumerate())
            {
                for (index, track) in chunk.iter().enumerate() {
                    let entry = QueueTrack::from(OnlineTrack {
                        platform: self.platform.code().into(),
                        track: track.clone(),
                        source: None,
                    });
                    let can_play =
                        track.audio_url.is_some() || (track.id.is_some() && self.playable);
                    let actions = [
                        (Action::Play, can_play),
                        (Action::AddToQueue, !self.queue.contains(&entry)),
                        (Action::Search, true),
                    ];
                    let mut row = TrackRow::new(&entry, &[], self.current == Some(&entry));
                    row.rank =
                        rank_offset.map(|offset| offset + column_index * chunk_size + index + 1);
                    if !can_play {
                        row.play_unavailable = Some(if track.id.is_none() {
                            Message::MissingPlaybackId
                        } else {
                            Message::NoCompatiblePlaybackSource
                        });
                    }
                    let actions = if can_play {
                        &actions[..2]
                    } else {
                        &actions[..]
                    };
                    if let Some(action) = row.show(column, language, actions) {
                        selected = Some((entry, action));
                    }
                }
            }
        });
        selected
    }
}

fn show_playlists(
    platform: DiscoveryPlatform,
    playlists: &[Playlist],
    covers: &mut HashMap<Url, Cover>,
    language: Language,
    ui: &mut egui::Ui,
) -> Option<Playlist> {
    if playlists.is_empty() {
        ui.label(language.text(Message::NoResults));
        return None;
    }
    let mut selected = None;
    let columns = ((ui.available_width() / 150.0) as usize).clamp(2, 5);
    for row in playlists.chunks(columns) {
        ui.columns(columns, |columns| {
            for (column, playlist) in columns.iter_mut().zip(row) {
                egui::Frame::new()
                    .fill(SURFACE)
                    .corner_radius(8)
                    .inner_margin(8)
                    .show(column, |ui| {
                        let size = egui::vec2(ui.available_width(), ui.available_width());
                        let visible = ui.is_rect_visible(egui::Rect::from_min_size(
                            ui.next_widget_position(),
                            size,
                        ));
                        let texture = visible
                            .then(|| playlist_texture(platform, playlist, covers, ui.ctx()))
                            .flatten()
                            .map(egui::TextureHandle::id);
                        let response = match texture {
                            Some(texture) => ui.add(
                                egui::Image::new((texture, size))
                                    .corner_radius(6)
                                    .sense(egui::Sense::click()),
                            ),
                            None => ui.add_sized(size, egui::Button::new("♫")),
                        };
                        let title =
                            ui.add(egui::Button::new(&playlist.title).truncate().frame(false));
                        if response.clicked() || title.on_hover_text(&playlist.title).clicked() {
                            selected = Some(playlist.clone());
                        }
                        if !playlist.author.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&playlist.author).color(MUTED),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&playlist.author);
                        }
                    });
            }
        });
        ui.add_space(8.0);
    }
    selected
}

fn show_playlist_header(
    view: &PlaylistView,
    platform: DiscoveryPlatform,
    covers: &mut HashMap<Url, Cover>,
    language: Language,
    ui: &mut egui::Ui,
) {
    let playlist = &view.playlist;
    let edge = (ui.available_width() * 0.3).clamp(56.0, 144.0);
    let texture = playlist_texture(platform, playlist, covers, ui.ctx());
    ui.horizontal_top(|ui| {
        artwork::show(ui, texture, edge);
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.heading(&playlist.title);
            if !playlist.author.is_empty() {
                ui.colored_label(MUTED, &playlist.author);
            }
            ui.hyperlink_to(language.text(Message::BrowseOnPlatform), playlist.web_url());
            if let Some(description) = &playlist.description {
                egui::ScrollArea::vertical()
                    .id_salt(("playlist-description", platform, playlist.id))
                    .max_height(104.0)
                    .show(ui, |ui| {
                        ui.label(description);
                    });
            } else if matches!(view.tracks, Load::Complete(_)) {
                ui.colored_label(MUTED, language.text(Message::NoPlaylistDescription));
            }
        });
    });
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_playlist_tracks_follow_existing_entries_in_playlist_order() {
        let ctx = egui::Context::default();
        let mut app = MusicApp::from_state(&ctx, rs_music_storage::State::default(), None, None);
        let track = |id: &str, title: &str| {
            let mut track = Track::default();
            track.id = Some(id.into());
            track.title = title.into();
            track
        };
        app.enqueue(
            OnlineTrack {
                platform: "tx".into(),
                track: track("2", "Existing"),
                source: None,
            }
            .into(),
        );

        let key = (DiscoveryPlatform::QQ, 100);
        let (sender, task) = std::sync::mpsc::channel();
        app.discovery
            .queue_imports
            .insert(key, QueueImport::Pending(task));
        sender
            .send(Ok(vec![
                track("1", "First"),
                track("2", "Duplicate"),
                track("3", "Third"),
                track("1", "Repeated"),
            ]))
            .unwrap();
        poll_queue_imports(&mut app, &ctx);
        let titles: Vec<_> = app
            .queue
            .iter()
            .filter_map(|entry| match entry {
                QueueTrack::Online(entry) => Some(entry.track.title.as_str()),
                QueueTrack::Local(_) => None,
            })
            .collect();
        assert_eq!(titles, ["Existing", "First", "Third"]);
        assert!(app.current.is_none());
        assert!(matches!(
            app.discovery.queue_imports.get(&key),
            Some(QueueImport::Complete(Ok(2)))
        ));
    }

    #[test]
    fn trending_replaces_visible_page_only_after_a_successful_load() {
        let result = |title: &str| {
            let mut track = Track::default();
            track.title = title.into();
            DiscoveryData {
                value: PlaylistTracks {
                    tracks: vec![track],
                    total: 24,
                    has_next: true,
                    description: None,
                },
                valid_until: Instant::now() + Duration::from_secs(60),
            }
        };
        let ctx = egui::Context::default();
        let platform = DiscoveryPlatform::NetEase;
        let (sender, task) = std::sync::mpsc::channel();
        let mut trending = Trending {
            page: 1,
            data: Some(result("First page")),
            pending: Some((2, task)),
            error: None,
        };
        trending.poll(platform, &ctx);
        assert_eq!(trending.page, 1);
        assert!(trending.pending.is_some());
        assert_eq!(
            trending.data.as_ref().unwrap().value.tracks[0].title,
            "First page"
        );

        sender.send(Err("Offline".into())).unwrap();
        trending.poll(platform, &ctx);
        assert_eq!(trending.page, 1);
        assert_eq!(trending.error.as_deref(), Some("Offline"));
        assert_eq!(
            trending.data.as_ref().unwrap().value.tracks[0].title,
            "First page"
        );

        let (sender, task) = std::sync::mpsc::channel();
        trending.pending = Some((2, task));
        sender.send(Ok(result("Second page"))).unwrap();
        trending.poll(platform, &ctx);
        assert_eq!(trending.page, 2);
        assert_eq!(
            trending.data.as_ref().unwrap().value.tracks[0].title,
            "Second page"
        );
    }

    #[test]
    fn trending_rows_stay_compact_and_keep_rank_across_pages() {
        let tracks: Vec<_> = (1..=12)
            .map(|index| {
                let mut track = Track::default();
                track.title = format!("Track {index}");
                track.artist = "Artist".into();
                track
            })
            .collect();
        for width in [520.0, 840.0] {
            let ctx = egui::Context::default();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            TrackRows {
                                platform: DiscoveryPlatform::NetEase,
                                playable: true,
                                queue: &[],
                                current: None,
                            }
                            .show(
                                &tracks,
                                Some(12),
                                Language::English,
                                ui,
                            );
                        });
                    });
                },
            );
            let rows: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.fill == SURFACE => Some(rect.rect.height()),
                    _ => None,
                })
                .collect();
            assert_eq!(rows.len(), tracks.len(), "row heights at {width}: {rows:?}");
            assert!(rows.iter().all(|height| *height <= 60.0), "{rows:?}");
            let ranks: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => text.galley.job.text.parse::<usize>().ok(),
                    _ => None,
                })
                .collect();
            assert_eq!(ranks, (13..=24).collect::<Vec<_>>());
        }
    }
}

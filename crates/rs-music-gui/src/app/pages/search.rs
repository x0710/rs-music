use super::super::{
    Message, MusicApp, Page,
    tasks::{Task, finished, spawn},
    widgets::pagination,
};
use crate::audio::{DownloadMonitor, DownloadProgress, OnlineAudio};
use crate::i18n::Language;
use eframe::egui;
use rs_music_core::lyrics::{self, Lyrics};
use rs_music_core::{self as music_source, SearchPage, Source, Track};
use rs_music_storage::{
    AudioCache, CachedTrack, DownloadSettings, OnlineTrack, SavedSource, SourceSelection, Store,
};
use std::{
    cmp::Reverse,
    collections::BTreeMap,
    io::ErrorKind,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

enum PlaybackEvent {
    Saved(Message, Result<PathBuf, String>),
    SourceSucceeded(String),
}
const SEARCH_HISTORY_LIMIT: usize = 50;

pub(in crate::app) struct Playback {
    title: String,
    duration: Option<Duration>,
    lyric_target: Option<(String, Track)>,
    lyric_load: Option<Task<Option<Lyrics>>>,
    lyrics_permission: Option<mpsc::Sender<()>>,
    task: Task<(OnlineAudio, Option<String>)>,
    cancelled: Arc<AtomicBool>,
    monitor: DownloadMonitor,
}

impl Playback {
    pub(in crate::app) fn progress(&self) -> Option<DownloadProgress> {
        self.monitor.progress()
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

pub(in crate::app) struct Hall {
    query: String,
    search_composing: bool,
    source: Source,
    tracks: Vec<Track>,
    importing: Option<Task<(Source, SavedSource, bool)>>,
    pub(in crate::app) saved: Vec<SavedSource>,
    pub(in crate::app) auto_switch_source: bool,
    pub(in crate::app) source_successes: BTreeMap<String, u64>,
    managed_source: Option<usize>,
    catalog: Option<Source>,
    source_index: Option<usize>,
    catalog_index: Option<usize>,
    preferred_selection: Option<SourceSelection>,
    platform: String,
    quality: String,
    search: Option<Task<SearchPage>>,
    search_history: Vec<String>,
    search_history_error: Option<(Message, String)>,
    submitted_query: String,
    page: usize,
    has_next: bool,
    pub(in crate::app) download: Option<Playback>,
    audio_cache: Option<AudioCache>,
    playback_events: mpsc::Receiver<PlaybackEvent>,
    playback_sender: mpsc::Sender<PlaybackEvent>,
    pub(in crate::app) fallback_source: Option<String>,
    error: Option<(Message, String)>,
}

impl Hall {
    pub(in crate::app) fn new(
        saved: Vec<SavedSource>,
        selection: Option<SourceSelection>,
        auto_switch_source: bool,
        ctx: &egui::Context,
    ) -> Self {
        let (playback_sender, playback_events) = mpsc::channel();
        let mut hall = Self {
            query: String::new(),
            search_composing: false,
            source: Source::ITunes,
            tracks: Vec::new(),
            importing: None,
            saved,
            auto_switch_source,
            source_successes: BTreeMap::new(),
            managed_source: None,
            catalog: None,
            source_index: None,
            catalog_index: None,
            preferred_selection: None,
            platform: String::new(),
            quality: String::new(),
            search: None,
            search_history: Vec::new(),
            search_history_error: None,
            submitted_query: String::new(),
            page: 1,
            has_next: false,
            download: None,
            audio_cache: AudioCache::system(),
            playback_events,
            playback_sender,
            fallback_source: None,
            error: None,
        };
        // Snapshots from before source selection was stored already contain the imported files.
        let selection = selection.unwrap_or_else(|| SourceSelection {
            source: hall
                .saved
                .iter()
                .rposition(|entry| entry.script)
                .or_else(|| hall.saved.len().checked_sub(1)),
            ..SourceSelection::default()
        });
        let index = selection.source;
        if let Some(index) = index.filter(|&index| index < hall.saved.len()) {
            hall.load_saved(index, false, ctx);
            hall.preferred_selection = Some(selection);
        } else if index.is_none() {
            hall.restore_selection(selection);
        }
        hall
    }

    pub(in crate::app) fn selection(&self) -> SourceSelection {
        if let Some(selection) = &self.preferred_selection {
            return selection.clone();
        }
        SourceSelection {
            source: self.source_index,
            catalog: self.catalog_index,
            platform: self.platform.clone(),
            quality: self.quality.clone(),
        }
    }

    pub(in crate::app) fn load_search_history(&mut self, store: &Store) {
        match store.load_search_history() {
            Ok(mut history) => {
                history.truncate(SEARCH_HISTORY_LIMIT);
                self.search_history = history;
            }
            Err(error) => {
                let error = rs_music_core::error_details(&error);
                tracing::warn!(%error, "Could not load search history");
                self.search_history_error = Some((Message::SearchHistoryLoadFailed, error));
            }
        }
    }
}

pub(in crate::app) fn finish_online(app: &mut MusicApp, ctx: &egui::Context) -> bool {
    if let Some(error) = app.audio.stream_error() {
        let title = app.playback_title();
        tracing::warn!(track = %title, %error, "Online audio download interrupted");
        app.playback_error = Some((Message::PlaybackFailed, format!("{title}\n{error}")));
        return false;
    }
    if app.downloads.audio {
        save_audio(app, true, true, ctx);
    }
    true
}

pub(in crate::app) fn choose_save(app: &mut MusicApp, save: bool, ctx: &egui::Context) {
    save_audio(app, save, false, ctx);
}

fn save_audio(app: &mut MusicApp, save: bool, automatic: bool, ctx: &egui::Context) {
    let Some(archive) = app.audio.choose_archive(save) else {
        return;
    };
    let sender = app.hall.playback_sender.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = match archive.save(automatic) {
            Ok(None) => return,
            Ok(Some(path)) => Ok(path),
            Err(error) => Err(error),
        };
        let _ = sender.send(PlaybackEvent::Saved(Message::ArchiveFailed, result));
        ctx.request_repaint();
    });
}

pub(in crate::app) fn poll(app: &mut MusicApp, ctx: &egui::Context) -> bool {
    let mut changed = false;
    let hall = &mut app.hall;
    for event in hall.playback_events.try_iter() {
        changed = true;
        let (message, result) = match event {
            PlaybackEvent::Saved(message, result) => (message, result),
            PlaybackEvent::SourceSucceeded(id) => {
                let successes = hall.source_successes.entry(id).or_default();
                *successes = successes.saturating_add(1);
                app.dirty = true;
                continue;
            }
        };
        let kind = if matches!(message, Message::LyricsArchiveFailed) {
            "lyrics"
        } else {
            "audio"
        };
        match result {
            Ok(path) => {
                tracing::info!(kind, path = %path.display(), "Download saved");
            }
            Err(error) => {
                tracing::warn!(kind, %error, "Could not save download");
                app.playback_error.get_or_insert((message, error));
            }
        }
    }
    if let Some(result) = app.hall.importing.as_ref().and_then(finished) {
        changed = true;
        app.hall.importing = None;
        match result {
            Ok((source, document, pair)) => {
                let index = match app.hall.saved.iter().position(|saved| saved == &document) {
                    Some(index) => index,
                    None => {
                        let index = app.hall.saved.len();
                        app.hall.saved.push(document);
                        app.dirty = true;
                        index
                    }
                };
                tracing::info!(source = source.name(), "Music source imported");
                if pair
                    && let (Source::Lx(script), Some(platform)) =
                        (&app.hall.source, source.platform())
                {
                    if let Some(qualities) = script.platforms().get(platform) {
                        app.hall.platform = platform.to_owned();
                        app.hall.quality = qualities[0].clone();
                        app.hall.catalog = Some(source);
                        app.hall.catalog_index = Some(index);
                        app.dirty = true;
                        app.hall.search = None;
                        app.hall.tracks.clear();
                        app.hall.error = None;
                        app.hall.search_first_page(ctx);
                    } else {
                        app.hall.error = Some((
                            Message::SourceFailed,
                            format!("Unsupported LX platform: {platform}"),
                        ));
                    }
                } else {
                    let previous_platform = app.hall.platform.clone();
                    let preferred = app
                        .hall
                        .preferred_selection
                        .take()
                        .filter(|selection| selection.source == Some(index));
                    app.hall.select(source);
                    app.hall.source_index = Some(index);
                    if let Some(selection) = preferred {
                        app.hall.restore_selection(selection);
                    } else if matches!(app.hall.source, Source::Lx(_)) {
                        app.hall.restore_selection(SourceSelection {
                            platform: previous_platform,
                            ..SourceSelection::default()
                        });
                    }
                    app.hall.search_first_page(ctx);
                    app.dirty = true;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "Music source import failed");
                app.hall.error = Some((Message::SourceFailed, error));
            }
        }
    }
    if let Some(result) = app.hall.search.as_ref().and_then(finished) {
        changed = true;
        app.hall.search = None;
        match result {
            Ok(page) => {
                if page.tracks.is_empty() {
                    app.hall.error = Some((Message::NoResults, String::new()));
                }
                app.hall.tracks = page.tracks;
                app.hall.has_next = page.has_next;
            }
            Err(error) => {
                app.hall.error = Some((Message::SearchFailed, error));
            }
        }
    }

    if let Some(result) = app
        .hall
        .download
        .as_ref()
        .and_then(|playback| finished(&playback.task))
        && let Some(mut playback) = app.hall.download.take()
    {
        changed = true;
        match result {
            Ok((mut audio, fallback_source)) => {
                if let Some(permission) = playback.lyrics_permission.take() {
                    audio.enable_lyrics_archive(permission);
                }
                let paused = std::mem::take(&mut app.pause_on_load);
                match app
                    .audio
                    .play_online(audio, playback.duration, app.volume, paused)
                {
                    Ok(()) => {
                        tracing::info!("Online audio started");
                        if let Some((platform, track)) = playback.lyric_target.take() {
                            app.now_playing
                                .online(platform, track, playback.lyric_load.take());
                        }
                        app.hall.fallback_source = fallback_source;
                    }
                    Err(error) => {
                        let error = rs_music_core::error_details(error.as_ref());
                        tracing::warn!(track = %playback.title, %error, "Could not decode or output online audio");
                        app.playback_error = Some((
                            Message::PlaybackFailed,
                            format!("{}\n{error}", playback.title),
                        ));
                    }
                }
            }
            Err(error) => {
                tracing::warn!(track = %playback.title, %error, "Online playback failed");
                app.playback_error = Some((
                    Message::PlaybackFailed,
                    format!("{}\n{error}", playback.title),
                ));
            }
        }
    }
    changed
}

impl Hall {
    fn cached_track(
        &self,
        online: &OnlineTrack,
        document: Option<&SavedSource>,
        title: &str,
        quality: &str,
    ) -> Option<CachedTrack> {
        let track = &online.track;
        let platform = online.platform.as_str();
        let cache = self.audio_cache.as_ref()?;
        let filename = if track.artist.is_empty() {
            title.to_owned()
        } else {
            format!("{} - {title}", track.artist)
        };
        if online.source.is_none() && !platform.is_empty() && track.audio_url.is_none() {
            return Some(cache.track(&["lx", platform, quality, track.id.as_deref()?], filename));
        }
        if let Some(id) = track.id.as_deref()
            && let Some(document) = document
        {
            return Some(cache.track(&["json", &document.contents, id], filename));
        }
        let url = track.audio_url.as_ref()?;
        Some(cache.track(&["url", url.as_str()], filename))
    }

    pub(in crate::app) fn can_play_lx(&self, platform: &str) -> bool {
        let primary_is_lx = matches!(self.source, Source::Lx(_));
        matches!(&self.source, Source::Lx(source) if source.platforms().contains_key(platform))
            || (self.auto_switch_source
                && !platform.is_empty()
                && self.saved.iter().enumerate().any(|(index, document)| {
                    document.script && (!primary_is_lx || Some(index) != self.source_index)
                }))
    }

    fn can_play_search_result(&self, track: &Track) -> bool {
        track.audio_url.is_some()
            || (track.id.is_some() && self.can_play_lx(&self.platform))
            || self.search_result_catalog(track).is_some()
            || (self.platform.is_empty() && self.source.can_play(track))
    }

    fn search_result_catalog(&self, track: &Track) -> Option<usize> {
        if self.platform.is_empty() {
            self.source_index
                .filter(|_| matches!(self.source, Source::Json(_)) && self.source.can_play(track))
        } else {
            self.catalog_index.filter(|_| {
                self.catalog.as_ref().is_some_and(|catalog| {
                    catalog.platform() == Some(&self.platform) && catalog.can_play(track)
                })
            })
        }
    }

    fn ranked_lx_alternatives(&self, primary: Option<usize>) -> Vec<(String, SavedSource)> {
        if !self.auto_switch_source {
            return Vec::new();
        }
        let start = primary.map_or(0, |index| index + 1);
        let mut alternatives: Vec<_> = self
            .saved
            .iter()
            .cycle()
            .skip(start)
            .take(self.saved.len() - usize::from(primary.is_some()))
            .filter(|document| document.script)
            .map(|document| (document.fingerprint(), document.clone()))
            .collect();
        // The stable sort preserves the existing order when counts match.
        alternatives.sort_by_cached_key(|(id, _)| {
            Reverse(self.source_successes.get(id).copied().unwrap_or(0))
        });
        alternatives
    }

    pub(in crate::app) fn start_platform_playback(
        &mut self,
        online: OnlineTrack,
        downloads: &DownloadSettings,
        ctx: &egui::Context,
    ) {
        let document = online.source.as_ref().and_then(|key| {
            self.saved
                .iter()
                .find(|document| !document.script && document.fingerprint() == *key)
        });
        let source = if online.source.is_some() {
            document
                .and_then(|document| Source::from_document(false, &document.contents).ok())
                .unwrap_or(Source::ITunes)
        } else if !online.platform.is_empty() && !matches!(self.source, Source::Lx(_)) {
            Source::ITunes
        } else {
            self.source.clone()
        };
        let track = &online.track;
        let platform = &online.platform;
        let title = track.title.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let quality = if *platform == self.platform && !self.quality.is_empty() {
            self.quality.clone()
        } else if let Source::Lx(source) = &source
            && let Some(qualities) = source.platforms().get(platform)
        {
            qualities
                .iter()
                .find(|quality| quality.as_str() == "128k")
                .unwrap_or(&qualities[0])
                .clone()
        } else {
            "128k".into()
        };
        let cached_track = self
            .cached_track(&online, document, &title, &quality)
            .map(|track| track.in_music_directory(downloads.directory.as_deref()));
        let primary = self
            .source_index
            .filter(|&index| index < self.saved.len() && matches!(source, Source::Lx(_)));
        let primary_id = primary
            .and_then(|index| self.saved.get(index))
            .map(SavedSource::fingerprint);
        let alternatives = if online.source.is_none() && !platform.is_empty() {
            self.ranked_lx_alternatives(primary)
        } else {
            Vec::new()
        };
        let playback_sender = self.playback_sender.clone();
        let playback_ctx = ctx.clone();
        let report = move |event| {
            let _ = playback_sender.send(event);
            playback_ctx.request_repaint();
        };
        let lyric_platform = source.platform().unwrap_or(platform).to_owned();
        let (lyric_load, lyrics_permission) =
            if let Some(file) = cached_track.clone().filter(|_| downloads.lyrics) {
                let (sender, permission) = mpsc::channel();
                let platform = lyric_platform.clone();
                let track = track.clone();
                let report = report.clone();
                let task = spawn(ctx, move || {
                    load_and_save_lyrics(&platform, &track, file, permission, move |result| {
                        report(PlaybackEvent::Saved(Message::LyricsArchiveFailed, result))
                    })
                });
                (Some(task), Some(sender))
            } else {
                (None, None)
            };
        let monitor = DownloadMonitor::default();
        self.download = Some(Playback {
            title,
            duration: track.duration,
            lyric_target: Some((lyric_platform, track.clone())),
            lyric_load,
            lyrics_permission,
            cancelled,
            monitor: monitor.clone(),
            task: spawn(ctx, move || {
                let OnlineTrack {
                    track,
                    platform,
                    source: source_key,
                } = online;
                if let Some(track) = &cached_track {
                    match OnlineAudio::open_cached(track, &monitor) {
                        Ok(audio) => return Ok((audio, None)),
                        Err(error) if error.kind() == ErrorKind::NotFound => {}
                        Err(error) => {
                            tracing::warn!(error = %rs_music_core::error_details(&error), "Could not read cached audio")
                        }
                    }
                }
                let lx = match &source {
                    Source::Lx(source) => Some(source),
                    _ => None,
                };
                if track.audio_url.is_none() && (lx.is_some() || !alternatives.is_empty()) {
                    let (audio, used) = music_source::open_lx_with_fallback(
                        &track,
                        lx,
                        alternatives.iter().map(|(_, document)| {
                            (document.name.as_str(), document.contents.as_str())
                        }),
                        &platform,
                        &quality,
                        || worker_cancelled.load(Ordering::Relaxed),
                        |stream| OnlineAudio::open(stream, cached_track.clone(), &monitor),
                    )?;
                    let (id, fallback_name) = if let Some(index) = used {
                        let (id, document) = &alternatives[index];
                        (Some(id.clone()), Some(document.name.clone()))
                    } else {
                        (primary_id, None)
                    };
                    if let Some(id) = id {
                        audio.on_download_complete(move || {
                            report(PlaybackEvent::SourceSucceeded(id))
                        });
                    }
                    return Ok((audio, fallback_name));
                }
                if matches!(source, Source::ITunes) && track.audio_url.is_none() {
                    if source_key.is_some() {
                        return Err(
                            "The imported playback source for this track is unavailable".into()
                        );
                    }
                    if !platform.is_empty() {
                        return Err(format!(
                            "No LX playback source selected for platform {platform}"
                        ));
                    }
                }
                let stream = source
                    .stream_track(track)
                    .map_err(music_source::public_error)?;
                let audio = OnlineAudio::open(stream, cached_track, &monitor)?;
                Ok((audio, None))
            }),
        });
        ctx.request_repaint();
    }

    fn online_track(&self, track: Track) -> OnlineTrack {
        let source = self
            .search_result_catalog(&track)
            .and_then(|index| self.saved.get(index))
            .map(SavedSource::fingerprint);
        OnlineTrack {
            platform: self.platform.clone(),
            track,
            source,
        }
    }

    fn delete_saved(&mut self, index: usize, ctx: &egui::Context) {
        let deleted_source = self.source_index == Some(index);
        let deleted_catalog = self.catalog_index == Some(index);
        let reindex = |selected: Option<usize>| {
            selected
                .filter(|&selected| selected != index)
                .map(|selected| selected - usize::from(selected > index))
        };
        let mut preferred = self
            .preferred_selection
            .take()
            .filter(|selection| selection.source != Some(index));
        if let Some(selection) = &mut preferred {
            selection.source = reindex(selection.source);
            selection.catalog = reindex(selection.catalog);
        }
        let document = self.saved.remove(index);
        if deleted_source || document.script {
            self.download = None;
        }
        if deleted_source {
            let platform = self.platform.clone();
            self.select(Source::ITunes);
            self.platform = platform;
        } else {
            for selected in [&mut self.source_index, &mut self.catalog_index] {
                *selected = reindex(*selected);
            }
            if deleted_catalog {
                self.catalog = None;
            }
        }
        self.preferred_selection = preferred;
        if deleted_source || deleted_catalog {
            self.search_first_page(ctx);
        }
    }

    fn catalog(&self) -> Option<Source> {
        self.catalog
            .as_ref()
            .filter(|catalog| catalog.platform() == Some(self.platform.as_str()))
            .cloned()
            .or_else(|| Source::builtin_catalog(&self.platform))
            .or_else(|| {
                (!matches!(self.source, Source::Lx(_)) && self.platform.is_empty())
                    .then(|| self.source.clone())
            })
    }

    fn search_first_page(&mut self, ctx: &egui::Context) {
        self.submitted_query = self.query.trim().to_owned();
        self.search_page(1, ctx);
    }

    fn submit_search(&mut self, ctx: &egui::Context) -> bool {
        self.search_first_page(ctx);
        if self.search.is_none() || self.search_history.first() == Some(&self.submitted_query) {
            return false;
        }
        self.search_history
            .retain(|entry| entry != &self.submitted_query);
        self.search_history.insert(0, self.submitted_query.clone());
        self.search_history.truncate(SEARCH_HISTORY_LIMIT);
        true
    }

    fn save_search_history(&mut self, store: Option<&Store>) {
        if let Some(store) = store {
            self.search_history_error =
                store
                    .save_search_history(&self.search_history)
                    .err()
                    .map(|error| {
                        let error = rs_music_core::error_details(&error);
                        tracing::warn!(%error, "Could not save search history");
                        (Message::SearchHistorySaveFailed, error)
                    });
        }
    }

    fn search_page(&mut self, page: usize, ctx: &egui::Context) {
        self.page = page;
        self.has_next = false;
        self.tracks.clear();
        self.error = None;
        self.search = self
            .catalog()
            .filter(|_| !self.submitted_query.is_empty())
            .map(|source| {
                let query = self.submitted_query.clone();
                spawn(ctx, move || {
                    source
                        .search_page(&query, page)
                        .map_err(music_source::public_error)
                })
            });
    }

    fn select_platform(&mut self, platform: &str, ctx: &egui::Context) {
        self.platform = platform.to_owned();
        self.catalog = None;
        self.catalog_index = None;
        self.quality = match &self.source {
            Source::Lx(script) => script
                .platforms()
                .get(platform)
                .and_then(|qualities| qualities.first())
                .cloned()
                .unwrap_or_default(),
            _ => String::new(),
        };
        self.search_first_page(ctx);
    }

    fn load_saved(&mut self, index: usize, pair: bool, ctx: &egui::Context) {
        let document = self.saved[index].clone();
        self.error = None;
        if !pair {
            let previous = self.selection();
            self.preferred_selection = Some(if previous.source == Some(index) {
                previous
            } else {
                SourceSelection {
                    source: Some(index),
                    platform: previous.platform,
                    ..SourceSelection::default()
                }
            });
        }
        self.importing = Some(spawn(ctx, move || {
            Source::from_document(document.script, &document.contents)
                .map(|source| (source, document, pair))
                .map_err(music_source::public_error)
        }));
    }

    fn restore_selection(&mut self, selection: SourceSelection) {
        if Source::builtin_catalog(&selection.platform).is_some() {
            self.platform = selection.platform.clone();
            self.quality.clear();
        }
        if let Source::Lx(script) = &self.source {
            if let Some(qualities) = script.platforms().get(&selection.platform) {
                self.platform = selection.platform;
                self.quality = qualities
                    .iter()
                    .find(|quality| **quality == selection.quality)
                    .unwrap_or(&qualities[0])
                    .clone();
            }
            if let Some(index) = selection.catalog
                && let Some(document) = self.saved.get(index)
                && !document.script
                && document.platform.as_deref() == Some(self.platform.as_str())
                && let Ok(source) = Source::from_document(false, &document.contents)
                && source.platform() == Some(self.platform.as_str())
            {
                self.catalog = Some(source);
                self.catalog_index = Some(index);
            }
        }
    }

    fn select(&mut self, source: Source) {
        self.platform.clear();
        self.quality.clear();
        if let Source::Lx(lx) = &source
            && let Some((platform, qualities)) = lx
                .platforms()
                .iter()
                .find(|(platform, _)| Source::builtin_catalog(platform).is_some())
                .or_else(|| lx.platforms().first_key_value())
        {
            self.platform = platform.clone();
            self.quality = qualities[0].clone();
        }
        self.source = source;
        self.source_index = None;
        self.catalog = None;
        self.catalog_index = None;
        self.preferred_selection = None;
        self.search = None;
        self.download = None;
        self.submitted_query.clear();
        self.page = 1;
        self.has_next = false;
        self.tracks.clear();
        self.error = None;
    }
}

fn load_and_save_lyrics(
    platform: &str,
    track: &Track,
    file: CachedTrack,
    permission: mpsc::Receiver<()>,
    report: impl FnOnce(Result<PathBuf, String>) + Send + 'static,
) -> Result<Option<Lyrics>, String> {
    match file
        .lyrics_path()
        .map_err(Into::into)
        .and_then(|path| lyrics::read_lrc(&path))
    {
        Ok(Some(text)) => {
            if let Some(lyrics) = Lyrics::parse(&text) {
                return Ok(Some(lyrics));
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(error = %music_source::public_error(error), "Could not read downloaded lyrics");
        }
    }
    let Some(text) = lyrics::online_lrc(platform, track).map_err(music_source::public_error)?
    else {
        return Ok(None);
    };
    let parsed = Lyrics::parse(&text);
    if parsed.is_some() {
        std::thread::spawn(move || {
            if permission.recv().is_ok() {
                report(
                    file.archive_lyrics(&text)
                        .map_err(|error| music_source::error_details(&error)),
                );
            }
        });
    }
    Ok(parsed)
}

fn load_document(path: &std::path::Path) -> Result<(Source, SavedSource, bool), String> {
    let result = (|| {
        let (script, contents) = Source::read_document(path)?;
        let source = Source::from_document(script, &contents)?;
        let document = SavedSource {
            name: source.name().to_owned(),
            script,
            contents,
            platform: source.platform().map(str::to_owned),
        };
        Ok((source, document, true))
    })();
    result.map_err(music_source::public_error)
}

pub(in crate::app) fn show_source_settings(app: &mut MusicApp, ui: &mut egui::Ui) {
    let language = app.language();
    let hall = &mut app.hall;
    let before = hall.selection();

    ui.horizontal_wrapped(|ui| {
        ui.label(language.text(Message::Source));
        let mut selected = None;
        let mut built_in = false;
        let name = before
            .source
            .and_then(|index| hall.saved.get(index))
            .map_or_else(|| hall.source.name(), |source| source.name.as_str());
        egui::ComboBox::from_id_salt("saved_music_sources")
            .selected_text(name)
            .show_ui(ui, |ui| {
                if ui.button("iTunes").clicked() {
                    built_in = true;
                    ui.close();
                }
                for (index, source) in hall.saved.iter().enumerate() {
                    let kind = if source.script { "LX" } else { "JSON" };
                    if ui.button(format!("{} ({kind})", source.name)).clicked() {
                        selected = Some(index);
                        ui.close();
                    }
                }
            });
        if built_in {
            hall.importing = None;
            hall.select(Source::ITunes);
            hall.search_first_page(ui.ctx());
        }
        if let Some(index) = selected {
            hall.load_saved(index, false, ui.ctx());
        }
    });
    ui.add_space(8.0);
    let mut changed = ui
        .checkbox(
            &mut hall.auto_switch_source,
            language.text(Message::AutoSwitchSource),
        )
        .changed();
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                hall.importing.is_none(),
                egui::Button::new(language.text(Message::ImportSource)),
            )
            .clicked()
        {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("LX JavaScript", &["js"])
                .add_filter("JSON / LX export", &["json"])
                .pick_file()
            {
                tracing::info!("Source file selected, starting import");
                hall.error = None;
                hall.importing = Some(spawn(ui.ctx(), move || load_document(&path)));
            } else {
                tracing::info!("Source file picker returned no file");
            }
        }
        if hall.importing.is_some() {
            ui.spinner();
        }
    });

    if hall.saved.is_empty() {
        ui.label(language.text(Message::NoImportedSources));
    } else {
        for (index, source) in hall.saved.iter().enumerate() {
            let kind = if source.script { "LX" } else { "JSON" };
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(
                    &mut hall.managed_source,
                    Some(index),
                    format!("{} ({kind})", source.name),
                );
                if hall.source_index == Some(index) {
                    ui.label(language.text(Message::ActiveSource));
                }
                if hall.catalog_index == Some(index) {
                    ui.label(language.text(Message::ActiveCatalog));
                }
            });
        }
        if ui
            .add_enabled(
                hall.managed_source.is_some() && hall.importing.is_none(),
                egui::Button::new(language.text(Message::DeleteSource)),
            )
            .clicked()
            && let Some(index) = hall.managed_source.take()
        {
            hall.delete_saved(index, ui.ctx());
            changed = true;
        }
    }

    if matches!(hall.source, Source::Lx(_)) {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.add_enabled_ui(hall.importing.is_none(), |ui| {
            show_catalog_settings(hall, language, ui);
        });
    }
    if let Some((Message::SourceFailed, error)) = &hall.error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            language.text(Message::SourceFailed),
        )
        .on_hover_text(error);
    }
    app.dirty |= changed || before != hall.selection();
}

fn show_catalog_settings(hall: &mut Hall, language: Language, ui: &mut egui::Ui) {
    let builtin_catalog = Source::builtin_catalog(&hall.platform);
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{} · {}",
            language.text(Message::Platform),
            hall.platform
        ));
        let mut selected = None;
        let mut select_builtin = false;
        let has_saved = hall
            .saved
            .iter()
            .any(|source| source.platform.as_deref() == Some(&hall.platform));
        let catalog = hall
            .catalog
            .as_ref()
            .filter(|source| source.platform() == Some(&hall.platform))
            .or(builtin_catalog.as_ref());
        if has_saved || catalog.is_some() {
            egui::ComboBox::from_label(language.text(Message::SearchCatalog))
                .selected_text(
                    catalog.map_or(language.text(Message::NoSearchCatalog), Source::name),
                )
                .show_ui(ui, |ui| {
                    if let Some(source) = &builtin_catalog
                        && ui.button(source.name()).clicked()
                    {
                        select_builtin = true;
                        ui.close();
                    }
                    for (index, source) in hall.saved.iter().enumerate() {
                        if source.platform.as_deref() == Some(&hall.platform)
                            && ui.button(&source.name).clicked()
                        {
                            selected = Some(index);
                            ui.close();
                        }
                    }
                });
        }
        if select_builtin {
            hall.catalog = None;
            hall.catalog_index = None;
            hall.search_first_page(ui.ctx());
        }
        if let Some(index) = selected {
            hall.load_saved(index, true, ui.ctx());
        }
        if ui
            .add_enabled(
                hall.importing.is_none(),
                egui::Button::new(language.text(Message::ImportCatalog)),
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("JSON", &["json"])
                .pick_file()
        {
            let platform = hall.platform.clone();
            hall.importing = Some(spawn(ui.ctx(), move || {
                let result = load_document(&path)?;
                if result.0.platform() != Some(&platform) {
                    return Err(format!(
                        "Expected a JSON search catalog with platform: {platform}"
                    ));
                }
                Ok(result)
            }));
        }
    });
}

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

fn show_search_toolbar(hall: &mut Hall, language: Language, ui: &mut egui::Ui) -> bool {
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

fn search_enter(events: &[egui::Event], composing: &mut bool) -> bool {
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
    if let Some(track) = picked {
        app.play(app.hall.online_track(track).into(), ui.ctx());
    }
    app.dirty |= before != app.hall.selection();
}

fn results(hall: &Hall, language: Language, ui: &mut egui::Ui) -> Option<Track> {
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt((&hall.platform, &hall.submitted_query, hall.page))
        .max_height((ui.available_height() - 36.0).max(24.0))
        .show(ui, |ui| {
            for track in &hall.tracks {
                ui.horizontal_wrapped(|ui| {
                    let playable = hall.can_play_search_result(track);
                    let reason = if playable {
                        Message::LoadingAudio
                    } else if track.id.is_none() && !hall.platform.is_empty() {
                        Message::MissingPlaybackId
                    } else {
                        Message::NoCompatiblePlaybackSource
                    };
                    if ui
                        .add_enabled(
                            playable && hall.download.is_none(),
                            egui::Button::new(language.text(Message::Play)),
                        )
                        .on_disabled_hover_text(language.text(reason))
                        .clicked()
                    {
                        picked = Some(track.clone());
                    }
                    if !playable && let Some(url) = &track.store_url {
                        ui.hyperlink_to(language.text(Message::ViewOnITunes), url.as_str());
                    }
                    ui.strong(&track.title);
                    ui.label(&track.artist);
                });
                ui.separator();
            }
        });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn enter_submits_search_without_intercepting_ime_confirmation() {
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut composing = false;
        assert!(search_enter(std::slice::from_ref(&enter), &mut composing));
        assert!(!search_enter(
            &[egui::Event::Ime(egui::ImeEvent::Preedit("zhong".into()))],
            &mut composing
        ));
        assert!(!search_enter(std::slice::from_ref(&enter), &mut composing));
        assert!(!search_enter(
            &[
                enter.clone(),
                egui::Event::Ime(egui::ImeEvent::Commit("中".into())),
            ],
            &mut composing
        ));
        assert!(search_enter(&[enter], &mut composing));
    }

    fn script(name: &str, platform: &str, handler: &str) -> SavedSource {
        SavedSource {
            name: name.into(),
            script: true,
            contents: format!(
                "/** @name {name} */
                lx.on('request', async ({{ info }}) => {{ {handler} }});
                lx.send('inited', {{ sources: {{
                    {platform}: {{ type: 'music', actions: ['musicUrl'], qualitys: ['128k'] }}
                }} }});"
            ),
            platform: None,
        }
    }

    #[test]
    fn saved_default_survives_failed_initialization_and_can_be_replaced() {
        let ctx = egui::Context::default();
        let working = script("Working source", "kg", "return undefined;");
        let mut unavailable = script("Unavailable source", "kg", "return undefined;");
        unavailable.contents = "/** @name Unavailable source */ throw Error('offline');".into();
        let configured = SourceSelection {
            source: Some(1),
            platform: "kg".into(),
            quality: "320k".into(),
            ..SourceSelection::default()
        };
        let state = rs_music_storage::State {
            sources: vec![working, unavailable],
            source_selection: Some(configured.clone()),
            ..rs_music_storage::State::default()
        };
        let mut app = MusicApp::from_state(&ctx, state, None, None);
        let finish_loading = |app: &mut MusicApp| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while app.hall.importing.is_some() {
                assert!(std::time::Instant::now() < deadline);
                poll(app, &ctx);
                std::thread::sleep(Duration::from_millis(1));
            }
        };

        assert!(app.hall.selection() == configured);
        finish_loading(&mut app);
        assert!(matches!(app.hall.source, Source::ITunes));
        assert!(matches!(app.hall.error, Some((Message::SourceFailed, _))));
        assert!(app.hall.selection() == configured);

        app.hall.load_saved(0, false, &ctx);
        assert_eq!(app.hall.selection().source, Some(0));
        finish_loading(&mut app);
        assert_eq!(app.hall.source.name(), "Working source");
        assert_eq!(app.hall.selection().source, Some(0));
        assert_eq!(app.hall.selection().platform, "kg");
        assert_eq!(app.hall.selection().quality, "128k");
    }

    #[test]
    fn enlarged_search_field_leaves_room_for_controls_in_a_narrow_panel() {
        let ctx = egui::Context::default();
        let mut hall = Hall::new(Vec::new(), Some(SourceSelection::default()), true, &ctx);
        let source = script("Search toolbar", "kg", "return undefined;");
        hall.select(Source::from_document(true, &source.contents).unwrap());

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(540.0, 480.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let panel = ui.available_rect_before_wrap();
                    show_search_toolbar(&mut hall, Language::English, ui);
                    let field = ctx
                        .read_response(egui::Id::new("online_search_box"))
                        .unwrap();
                    assert!(panel.contains_rect(field.rect));
                    assert!(field.rect.height() >= 44.0);
                    assert!(field.rect.width() > panel.width() / 2.0);
                    assert!(panel.right() - field.rect.right() >= 112.0);
                });
            },
        );
    }

    #[test]
    fn platform_search_results_can_use_lx_fallback_with_a_non_lx_primary() {
        let ctx = egui::Context::default();
        let mut hall = Hall::new(
            vec![script("backup", "kg", "return undefined;")],
            Some(SourceSelection::default()),
            true,
            &ctx,
        );
        hall.select_platform("kg", &ctx);
        let mut track = Track::default();
        assert!(!hall.can_play_search_result(&track));
        track.id = Some("test-hash".into());
        assert!(hall.can_play_search_result(&track));
        let online = hall.online_track(track.clone());
        assert!(online.source.is_none());
        hall.start_platform_playback(
            online,
            &DownloadSettings {
                lyrics: false,
                ..DownloadSettings::default()
            },
            &ctx,
        );
        let error = hall
            .download
            .take()
            .unwrap()
            .task
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .err()
            .expect("the test script deliberately returns no URL");
        assert!(
            error.starts_with("backup: LX musicUrl rejected (kg, 128k)"),
            "{error}"
        );

        hall.auto_switch_source = false;
        assert!(!hall.can_play_search_result(&track));
        track.audio_url = Some("https://example.org/song.mp3".parse().unwrap());
        assert!(hall.can_play_search_result(&track));
    }

    #[test]
    fn tries_saved_sources_in_order_when_primary_does_not_support_kugou() {
        let earlier = script(
            "earlier",
            "kg",
            "throw Error('track ' + info.musicInfo.hash + ' at ' + info.type);",
        );
        let primary = script("primary", "wy", "return undefined;");
        let later = script("later", "kg", "return undefined;");
        let source = Source::from_document(true, &primary.contents).unwrap();
        let ctx = egui::Context::default();
        let mut hall = Hall::new(
            vec![earlier, primary, later],
            Some(SourceSelection::default()),
            true,
            &ctx,
        );
        hall.select(source);
        hall.source_index = Some(1);
        hall.select_platform("kg", &ctx);
        assert!(hall.can_play_lx("kg"));
        assert_eq!(hall.catalog().unwrap().platform(), Some("kg"));
        let mut track = Track::default();
        track.id = Some("test-hash".into());
        hall.start_platform_playback(
            hall.online_track(track.clone()),
            &DownloadSettings {
                lyrics: false,
                ..DownloadSettings::default()
            },
            &ctx,
        );
        let error = hall
            .download
            .as_ref()
            .unwrap()
            .task
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .err()
            .expect("all sources failed");
        assert!(
            error.find("later:").unwrap() < error.find("earlier:").unwrap(),
            "{error}"
        );
        assert!(error.contains("track test-hash at 128k"), "{error}");
        assert_eq!(hall.selection().source, Some(1));

        hall.source_successes
            .insert(hall.saved[0].fingerprint(), 12);
        hall.source_successes
            .insert(hall.saved[1].fingerprint(), 30);
        hall.source_successes.insert(hall.saved[2].fingerprint(), 3);
        hall.start_platform_playback(
            hall.online_track(track.clone()),
            &DownloadSettings {
                lyrics: false,
                ..DownloadSettings::default()
            },
            &ctx,
        );
        let error = hall
            .download
            .as_ref()
            .unwrap()
            .task
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .err()
            .expect("all sources failed");
        assert!(error.starts_with("primary:"), "{error}");
        assert!(
            error.find("earlier:").unwrap() < error.find("later:").unwrap(),
            "{error}"
        );

        hall.auto_switch_source = false;
        assert!(!hall.can_play_lx("kg"));
        hall.start_platform_playback(
            hall.online_track(track),
            &DownloadSettings {
                lyrics: false,
                ..DownloadSettings::default()
            },
            &ctx,
        );
        let error = hall
            .download
            .as_ref()
            .unwrap()
            .task
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .err()
            .expect("primary source failed");
        assert_eq!(error, "primary: Unsupported LX platform or quality");
    }

    #[test]
    fn deleting_sources_reindexes_selection_and_restores_builtin_search() {
        let ctx = egui::Context::default();
        let unused = script("unused", "wy", "return undefined;");
        let source = script("player", "kg", "return undefined;");
        let catalog = SavedSource {
            name: "custom".into(),
            script: false,
            platform: Some("kg".into()),
            contents: r#"{
                "name":"custom", "platform":"kg", "search_url":"https://example.org",
                "query_parameter":"q", "results":"/tracks", "title":"/name",
                "artist":"/artist", "track_id":"/hash"
            }"#
            .into(),
        };
        let mut hall = Hall::new(
            vec![unused, source.clone(), catalog.clone()],
            Some(SourceSelection::default()),
            true,
            &ctx,
        );
        hall.select(Source::from_document(true, &source.contents).unwrap());
        hall.source_index = Some(1);
        hall.catalog = Some(Source::from_document(false, &catalog.contents).unwrap());
        hall.catalog_index = Some(2);

        hall.delete_saved(0, &ctx);
        assert_eq!(hall.selection().source, Some(0));
        assert_eq!(hall.selection().catalog, Some(1));
        hall.delete_saved(1, &ctx);
        assert_eq!(hall.selection().catalog, None);
        assert_eq!(hall.catalog().unwrap().platform(), Some("kg"));
        hall.delete_saved(0, &ctx);
        assert!(hall.saved.is_empty());
        assert_eq!(hall.selection().source, None);
        assert!(matches!(hall.source, Source::ITunes));
    }
}

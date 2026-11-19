use eframe::egui;

mod cover;
#[cfg(target_os = "linux")]
pub(crate) mod media;
mod now_playing;
mod pages;
mod shortcuts;
mod subtitle_window;
mod tasks;
mod theme;
mod widgets;

use crate::{
    audio::Audio,
    fonts::SystemFonts,
    i18n::{Language, LanguagePreference, Message},
};
use rand::seq::IteratorRandom;
use rs_music_storage::{CloseAction, DownloadSettings, PlaybackMode, QueueTrack, State, Store};
use std::{collections::HashSet, path::PathBuf, sync::mpsc};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Hall,
    Search,
    Library,
    Queue,
    Settings,
    About,
    Lyrics,
}

impl Page {
    fn message(self) -> Message {
        match self {
            Self::Hall => Message::Hall,
            Self::Search => Message::MusicSearch,
            Self::Library => Message::Library,
            Self::Queue => Message::Queue,
            Self::Settings => Message::Settings,
            Self::About => Message::About,
            Self::Lyrics => Message::SubtitleMode,
        }
    }
}

pub struct MusicApp {
    page: Page,
    subtitle_return: Page,
    subtitle_window_open: bool,
    query: String,
    tracks: Vec<PathBuf>,
    queue: Vec<QueueTrack>,
    current: Option<QueueTrack>,
    audio: Audio,
    pause_on_load: bool,
    #[cfg(target_os = "linux")]
    seeked: Option<std::time::Duration>,
    seek_preview: Option<std::time::Duration>,
    hall: pages::search::Hall,
    discovery: pages::discovery::Discovery,
    now_playing: now_playing::NowPlaying,
    playback_mode: PlaybackMode,
    volume: u8,
    playback_error: Option<(Message, String)>,
    zoom_factor: f32,
    theme: rs_music_storage::ThemePreference,
    language_preference: LanguagePreference,
    font_family: Option<String>,
    pub(crate) close_action: CloseAction,
    downloads: DownloadSettings,
    cache_cleanup: pages::settings::CacheCleanup,
    fonts: SystemFonts,
    system_language: Language,
    store: Option<Store>,
    storage_error: Option<(Message, String)>,
    dirty: bool,
    imports: Vec<mpsc::Receiver<Vec<PathBuf>>>,
}

impl MusicApp {
    pub fn new(ctx: &egui::Context) -> Self {
        let (store, state, storage_error) = match Store::open() {
            Ok((store, state)) => (Some(store), state, None),
            Err(error) => (
                None,
                State::default(),
                Some((
                    Message::StorageUnavailable,
                    rs_music_core::error_details(&error),
                )),
            ),
        };
        if let Some((_, error)) = &storage_error {
            tracing::warn!(%error, "Saved state could not be loaded");
        } else {
            tracing::info!(
                tracks = state.tracks.len(),
                queued = state.queue.len(),
                "Saved state loaded"
            );
        }
        if store.is_some()
            && let Some(cache) = rs_music_storage::AudioCache::system()
            && let Err(error) = cache.prune_expired()
        {
            tracing::warn!(error = %rs_music_core::error_details(&error), "Could not prune expired audio cache");
        }
        let mut app = Self::from_state(ctx, state, store, storage_error);
        app.fonts = SystemFonts::discover();
        app.configure_context(ctx);
        app
    }

    fn configure_context(&self, ctx: &egui::Context) {
        egui_extras::install_image_loaders(ctx);
        ctx.set_zoom_factor(self.zoom_factor);
        theme::configure(ctx, self.theme);
        self.fonts.install(ctx, self.font_family.as_deref());
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn restore_window(&mut self, ctx: &egui::Context) {
        self.discovery.clear_textures();
        self.now_playing.clear_texture();
        self.configure_context(ctx);
    }

    fn from_state(
        ctx: &egui::Context,
        mut state: State,
        store: Option<Store>,
        storage_error: Option<(Message, String)>,
    ) -> Self {
        let mut seen = HashSet::new();
        state.queue.retain(|entry| match entry {
            QueueTrack::Local(id) => *id < state.tracks.len() && seen.insert(*id),
            QueueTrack::Online(_) => true,
        });
        state.current = state.current.filter(|entry| match entry {
            QueueTrack::Local(id) => *id < state.tracks.len(),
            QueueTrack::Online(_) => true,
        });
        let mut hall = pages::search::Hall::new(
            state.sources,
            state.source_selection,
            state.auto_switch_source,
            ctx,
        );
        hall.source_successes = state.source_successes;
        if let Some(store) = &store {
            hall.load_search_history(store);
        }
        Self {
            page: Page::Library,
            subtitle_return: Page::Library,
            subtitle_window_open: false,
            query: String::new(),
            tracks: state.tracks,
            queue: state.queue,
            current: state.current,
            audio: Audio::default(),
            pause_on_load: false,
            #[cfg(target_os = "linux")]
            seeked: None,
            seek_preview: None,
            hall,
            discovery: pages::discovery::Discovery::default(),
            now_playing: now_playing::NowPlaying::default(),
            playback_mode: state.playback_mode,
            volume: state.volume.min(100),
            playback_error: None,
            zoom_factor: state.zoom_factor.clamp(0.2, 5.0),
            theme: state.theme,
            language_preference: state.language,
            font_family: state.font_family,
            close_action: state.close_action,
            downloads: state.downloads,
            cache_cleanup: pages::settings::CacheCleanup::from_minutes(state.cache_cleanup_minutes),
            fonts: SystemFonts::default(),
            system_language: Language::system(),
            store,
            storage_error,
            dirty: false,
            imports: Vec::new(),
        }
    }

    pub(crate) fn language(&self) -> Language {
        Language::resolve(self.language_preference, self.system_language)
    }

    fn toggle_subtitles(&mut self) {
        if self.page == Page::Lyrics {
            self.page = self.subtitle_return;
        } else {
            self.subtitle_return = self.page;
            self.page = Page::Lyrics;
            self.now_playing.followed_line = None;
        }
    }

    fn buffering_progress(&self) -> Option<crate::audio::DownloadProgress> {
        self.hall
            .download
            .as_ref()
            .and_then(|playback| playback.progress())
            .or_else(|| self.audio.download_progress())
    }

    fn choose_audio_save(&mut self, save: bool, ctx: &egui::Context) {
        pages::search::choose_save(self, save, ctx);
    }

    fn apply_track_action(
        &mut self,
        action: widgets::track_row::Action,
        entry: QueueTrack,
        ctx: &egui::Context,
    ) {
        use widgets::track_row::Action;
        match action {
            Action::Play => self.play(entry, ctx),
            Action::QueueAndPlay => {
                self.enqueue(entry.clone());
                self.play(entry, ctx);
            }
            Action::AddToQueue => self.enqueue(entry),
            Action::RemoveFromQueue => self.remove_from_queue(&entry),
            Action::RemoveFromLibrary => {
                if let QueueTrack::Local(id) = entry {
                    self.remove_from_library(id);
                }
            }
            Action::Search => {
                if let QueueTrack::Online(online) = entry {
                    pages::search::find_track(self, &online.track, ctx);
                }
            }
        }
        ctx.request_repaint();
    }

    fn enqueue(&mut self, entry: QueueTrack) {
        if !self.queue.contains(&entry) {
            self.queue.push(entry);
            self.dirty = true;
        }
    }

    fn enqueue_batch(&mut self, entries: Vec<QueueTrack>) -> usize {
        let mut known: HashSet<_> = self.queue.iter().collect();
        let is_new: Vec<_> = entries.iter().map(|entry| known.insert(entry)).collect();
        let before = self.queue.len();
        self.queue.extend(
            entries
                .into_iter()
                .zip(is_new)
                .filter_map(|(entry, is_new)| is_new.then_some(entry)),
        );
        let count = self.queue.len() - before;
        self.dirty |= count != 0;
        count
    }

    fn play(&mut self, entry: QueueTrack, ctx: &egui::Context) {
        self.start_playback(entry, false, ctx);
    }

    fn select_track(&mut self, entry: QueueTrack) {
        self.stop_playback();
        self.dirty |= self.current.as_ref() != Some(&entry);
        self.current = Some(entry);
        self.now_playing.clear();
    }

    fn stop_playback(&mut self) {
        self.hall.download = None;
        self.audio.stop();
        self.pause_on_load = false;
        self.seek_preview = None;
        self.playback_error = None;
        self.hall.fallback_source = None;
        #[cfg(target_os = "linux")]
        {
            self.seeked = None;
        }
    }

    fn start_playback(&mut self, entry: QueueTrack, paused: bool, ctx: &egui::Context) {
        self.select_track(entry.clone());
        match entry {
            QueueTrack::Local(id) => self.play_local(id, paused),
            QueueTrack::Online(online) => {
                self.pause_on_load = paused;
                self.hall
                    .start_platform_playback(*online, &self.downloads, ctx)
            }
        }
        ctx.request_repaint();
    }

    fn pause_playback(&mut self) {
        if let Some(sink) = self.audio.sink() {
            sink.pause();
        }
        if self.hall.download.is_some() {
            self.pause_on_load = true;
        }
    }

    fn has_playback_selection(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty()
    }

    fn resume_playback(&mut self, ctx: &egui::Context) {
        if let Some(sink) = self.audio.sink() {
            sink.play();
        } else if self.hall.download.is_some() {
            self.pause_on_load = false;
        } else if let Some(entry) = self.current.as_ref().or(self.queue.first()).cloned() {
            self.play(entry, ctx);
        }
    }

    fn toggle_playback(&mut self, ctx: &egui::Context) {
        if self.audio.playing() || (self.hall.download.is_some() && !self.pause_on_load) {
            self.pause_playback();
        } else {
            self.resume_playback(ctx);
        }
    }

    fn play_local(&mut self, id: usize, paused: bool) {
        self.playback_error = self
            .audio
            .play(&self.tracks[id], self.volume, paused)
            .err()
            .map(|error| {
                (
                    Message::PlaybackFailed,
                    format!(
                        "{}: {}",
                        title(&self.tracks[id]),
                        rs_music_core::error_details(error.as_ref())
                    ),
                )
            });
        if let Some((_, error)) = &self.playback_error {
            tracing::warn!(track_id = id, path = %self.tracks[id].display(), %error, "Local audio playback failed");
        } else {
            self.now_playing.local(self.tracks[id].clone());
            tracing::info!(track_id = id, "Local audio started");
        }
    }

    fn playback_title(&self) -> String {
        match &self.current {
            Some(QueueTrack::Online(online)) => online.track.title.clone(),
            Some(QueueTrack::Local(id)) => title(&self.tracks[*id]),
            None => self.language().text(Message::NoTrack).into(),
        }
    }

    fn seek(&mut self, position: std::time::Duration) -> bool {
        match self.audio.seek(position, self.volume) {
            Ok(()) => {
                if matches!(self.playback_error, Some((Message::SeekFailed, _))) {
                    self.playback_error = None;
                }
                #[cfg(target_os = "linux")]
                {
                    self.seeked = Some(position);
                }
                true
            }
            Err(error) => {
                let error = rs_music_core::error_details(error.as_ref());
                tracing::warn!(position_secs = position.as_secs_f64(), %error, "Could not seek");
                self.playback_error = Some((Message::SeekFailed, error));
                false
            }
        }
    }

    fn remove_from_library(&mut self, id: usize) {
        self.remove_from_queue(&QueueTrack::Local(id));
        self.tracks.remove(id);
        for entry in self.queue.iter_mut().chain(self.current.iter_mut()) {
            if let QueueTrack::Local(index) = entry
                && *index > id
            {
                *index -= 1;
            }
        }
        self.dirty = true;
    }

    fn remove_from_queue(&mut self, entry: &QueueTrack) {
        self.queue.retain(|item| item != entry);
        if self.current.as_ref() == Some(entry) {
            self.current = None;
            self.stop_playback();
            self.now_playing.clear();
        }
        self.dirty = true;
    }

    fn neighbor(&self, offset: isize) -> Option<&QueueTrack> {
        let current = self.current.as_ref()?;
        let position = self.queue.iter().position(|item| item == current)?;
        self.queue.get(position.checked_add_signed(offset)?)
    }

    fn previous_track(&self) -> Option<&QueueTrack> {
        self.neighbor(-1).or_else(|| {
            (self.playback_mode == PlaybackMode::RepeatAll && self.current.is_some())
                .then(|| self.queue.last())
                .flatten()
        })
    }

    fn next_track(&self, automatic: bool) -> Option<&QueueTrack> {
        let current = self.current.as_ref()?;
        match self.playback_mode {
            PlaybackMode::RepeatOne if automatic => Some(current),
            PlaybackMode::RepeatAll => self.neighbor(1).or_else(|| self.queue.first()),
            PlaybackMode::Shuffle => self
                .queue
                .iter()
                .filter(|entry| *entry != current)
                .choose(&mut rand::rng())
                .or_else(|| (automatic && self.queue.len() == 1).then_some(current)),
            _ => self.neighbor(1),
        }
    }

    fn can_go_next(&self) -> bool {
        self.current.is_some()
            && match self.playback_mode {
                PlaybackMode::RepeatAll => !self.queue.is_empty(),
                PlaybackMode::Shuffle => self.queue.len() > 1,
                _ => self.neighbor(1).is_some(),
            }
    }

    fn set_volume(&mut self, volume: u8) {
        self.volume = volume;
        if let Some(sink) = self.audio.sink() {
            sink.set_volume(f32::from(volume) / 100.0);
        }
        self.dirty = true;
    }

    fn import(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(rs_music_core::library::scan(paths));
            ctx.request_repaint();
        });
        self.imports.push(receiver);
    }

    fn collect_imports(&mut self) {
        let mut imported = Vec::new();
        self.imports.retain(|receiver| match receiver.try_recv() {
            Ok(paths) => {
                imported.extend(paths);
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => false,
        });
        if !imported.is_empty() {
            let mut known: HashSet<_> = self.tracks.iter().cloned().collect();
            let count = self.tracks.len();
            self.tracks.extend(
                imported
                    .into_iter()
                    .filter(|path| known.insert(path.clone())),
            );
            let added = self.tracks.len() - count;
            self.dirty |= added != 0;
            tracing::info!(added, total = self.tracks.len(), "Local import completed");
        }
    }

    fn save_changes(&mut self) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        if let Some(store) = &self.store {
            tracing::debug!("Saving application state");
            let state = State {
                zoom_factor: self.zoom_factor,
                cache_cleanup_minutes: self.cache_cleanup.minutes(),
                theme: self.theme,
                language: self.language_preference,
                font_family: self.font_family.clone(),
                close_action: self.close_action,
                downloads: self.downloads.clone(),
                tracks: self.tracks.clone(),
                queue: self.queue.clone(),
                current: self.current.clone(),
                playback_mode: self.playback_mode,
                volume: self.volume,
                sources: self.hall.saved.clone(),
                source_selection: Some(self.hall.selection()),
                auto_switch_source: self.hall.auto_switch_source,
                source_successes: self.hall.source_successes.clone(),
            };
            self.storage_error = store.save(&state).err().map(|error| {
                let error = rs_music_core::error_details(&error);
                tracing::warn!(%error, "Could not save application state");
                (Message::SaveFailed, error)
            });
        }
    }

    pub(crate) fn tick(&mut self, ctx: &egui::Context) -> bool {
        let pending_imports = self.imports.len();
        self.collect_imports();
        let search_changed = pages::search::poll(self, ctx);
        let queue_changed = pages::discovery::poll_queue_imports(self, ctx);
        let audio_finished = self.audio.finished();
        if audio_finished {
            let completed = !matches!(self.current, Some(QueueTrack::Online(_)))
                || pages::search::finish_online(self, ctx);
            self.audio.stop();
            if completed && let Some(entry) = self.next_track(true).cloned() {
                self.play(entry, ctx);
            }
        }
        self.save_changes();
        audio_finished || search_changed || queue_changed || self.imports.len() != pending_imports
    }
    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        // egui 在下一帧应用缩放。只从真实窗口读取，托盘后台使用独立的 Context。
        let zoom = ctx.zoom_factor();
        if self.zoom_factor != zoom {
            self.zoom_factor = zoom;
            self.dirty = true;
        }
        self.import(
            ctx,
            ctx.input(|input| {
                input
                    .raw
                    .dropped_files
                    .iter()
                    .filter_map(|file| file.path.clone())
                    .collect::<Vec<_>>()
            }),
        );
        if self.audio.sink().is_some_and(|sink| !sink.is_paused()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        if shortcuts::take_playback_toggle(ctx) {
            self.toggle_playback(ctx);
        }
        self.now_playing.update(ctx);
        egui::TopBottomPanel::bottom("player")
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.window_fill)
                    .inner_margin(16),
            )
            .show(ctx, |ui| widgets::player_bar::show(self, ui));
        widgets::navigation::show(ctx, self);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(24),
            )
            .show(ctx, |ui| match self.page {
                Page::Hall => pages::discovery::show(self, ui),
                Page::Search => pages::search::show(self, ui),
                Page::Library | Page::Queue => pages::library::show(self, ui),
                Page::Settings => pages::settings::show(self, ui),
                Page::About => pages::about::show(self.language(), ui),
                Page::Lyrics => pages::lyrics::show(self, ui),
            });
        if self.subtitle_window_open {
            self.subtitle_window_open = subtitle_window::show(self, ctx);
        }
        self.save_changes();
    }
}

#[cfg(not(target_os = "linux"))]
impl eframe::App for MusicApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.tick(ctx);
        self.show(ctx);
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn prepare_ime_commits(events: &mut Vec<egui::Event>) {
    use egui::{Event, ImeEvent};

    // On Linux, winit can send a commit without any preedit. In egui 0.33 the
    // commit needs an Enabled event to capture the current caret or selection.
    for index in (0..events.len()).rev() {
        if matches!(&events[index], Event::Ime(ImeEvent::Commit(text)) if !text.is_empty()) {
            events.insert(index, Event::Ime(ImeEvent::Enabled));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_restore_zoom_and_cleanup_range_before_the_first_frame() {
        let ctx = egui::Context::default();
        let saved = State {
            zoom_factor: 1.4,
            cache_cleanup_minutes: Some(60),
            ..State::default()
        };
        let mut app = MusicApp::from_state(&ctx, saved, None, None);
        app.configure_context(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            assert_eq!(ctx.zoom_factor(), 1.4);
            app.show(ctx);
        });
        assert_eq!(app.cache_cleanup.minutes(), Some(60));

        ctx.set_zoom_factor(1.1);
        let _ = ctx.run(Default::default(), |ctx| app.show(ctx));
        assert_eq!(app.zoom_factor, 1.1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn text_edit_accepts_consecutive_ime_commits_without_preedit() {
        use egui::{Event, ImeEvent};

        let ctx = egui::Context::default();
        let mut text = String::new();
        let field = egui::Id::new("test_ime");
        let mut frame = |mut events| {
            prepare_ime_commits(&mut events);
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut text)
                                .id(field)
                                .return_key(None),
                        );
                    });
                },
            );
        };
        frame(Vec::new());
        ctx.memory_mut(|memory| memory.request_focus(field));
        frame(vec![
            Event::Ime(ImeEvent::Enabled),
            Event::Ime(ImeEvent::Preedit("zhong".to_owned())),
            Event::Ime(ImeEvent::Disabled),
            Event::Ime(ImeEvent::Commit("中".to_owned())),
            Event::Ime(ImeEvent::Disabled),
        ]);
        frame(vec![
            Event::Ime(ImeEvent::Disabled),
            Event::Ime(ImeEvent::Commit("文".to_owned())),
            Event::Ime(ImeEvent::Disabled),
        ]);
        assert_eq!(text, "中文");
    }

    #[test]
    fn advances_the_queue_without_a_window_frame() {
        let ctx = egui::Context::default();
        let state = State {
            tracks: vec!["first.wav".into(), "second.wav".into()],
            queue: vec![QueueTrack::Local(0), QueueTrack::Local(1)],
            current: Some(QueueTrack::Local(0)),
            ..State::default()
        };
        let mut app = MusicApp::from_state(&ctx, state, None, None);
        app.audio = Audio::with_drained_sink();

        assert!(app.tick(&ctx));
        assert!(matches!(app.current, Some(QueueTrack::Local(1))));
        assert!(!app.tick(&ctx));
    }

    #[test]
    fn playback_title_tracks_online_selection_even_when_loading_fails() {
        use std::time::{Duration, Instant};

        let ctx = egui::Context::default();
        let state = State {
            tracks: vec!["local.mp3".into()],
            queue: vec![QueueTrack::Local(0)],
            current: Some(QueueTrack::Local(0)),
            ..State::default()
        };
        let mut app = MusicApp::from_state(&ctx, state, None, None);
        assert_eq!(app.playback_title(), "local.mp3");

        let mut track = rs_music_core::Track::default();
        track.title = "New online song".into();
        let online = QueueTrack::from(rs_music_storage::OnlineTrack {
            platform: "kg".into(),
            track,
            source: None,
        });
        app.play(online.clone(), &ctx);
        assert_eq!(app.playback_title(), "New online song");

        // The built-in default source cannot play a track with no audio URL.
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.hall.download.is_some() && Instant::now() < deadline {
            pages::search::poll(&mut app, &ctx);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(matches!(
            app.playback_error,
            Some((Message::PlaybackFailed, _))
        ));
        assert_eq!(app.playback_title(), "New online song");
        assert!(app.current.as_ref() == Some(&online));
        assert!(app.previous_track().is_none());
        assert!(app.queue == [QueueTrack::Local(0)]);
        app.remove_from_library(0);
        assert!(app.current.as_ref() == Some(&online));
        assert!(app.queue.is_empty());
        assert!(app.has_playback_selection());
    }
}

fn title(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

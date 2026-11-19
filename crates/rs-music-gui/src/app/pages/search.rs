mod lyrics;
mod playback;
mod settings;
mod sources;
#[cfg(test)]
mod tests;
mod view;

use super::super::{
    Message, MusicApp,
    tasks::{Task, finished, spawn},
};
use eframe::egui;
use playback::{Playback, PlaybackEvent};
pub(in crate::app) use playback::{choose_save, finish_online};
use rs_music_core::{self as music_source, SearchPage, Source, Track};
use rs_music_storage::{AudioCache, SavedSource, SourceSelection, Store};
pub(in crate::app) use settings::show_source_settings;
use std::{collections::BTreeMap, sync::mpsc};
pub(in crate::app) use view::{find_track, show};

const SEARCH_HISTORY_LIMIT: usize = 50;

pub(in crate::app) struct Hall {
    query: String,
    search_composing: bool,
    source: Source,
    tracks: Vec<Track>,
    source_url: String,
    importing: Option<Task<(Source, SavedSource, bool)>>,
    pub(in crate::app) saved: Vec<SavedSource>,
    pub(in crate::app) auto_switch_source: bool,
    pub(in crate::app) source_successes: BTreeMap<String, u64>,
    catalog: Option<Source>,
    source_index: Option<usize>,
    catalog_index: Option<usize>,
    // Retain the saved choice while its source initializes, including after a failed import.
    preferred_selection: Option<SourceSelection>,
    platform: String,
    quality: String,
    search: Option<Task<SearchPage>>,
    search_history: Vec<String>,
    search_history_error: Option<(Message, String)>,
    // Pagination uses the last submission even while the user edits the search box.
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
            source_url: String::new(),
            importing: None,
            saved,
            auto_switch_source,
            source_successes: BTreeMap::new(),
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

/// 消费本帧已完成的后台任务和播放事件。返回真表示至少处理了一项结果，包括失败结果。
pub(in crate::app) fn poll(app: &mut MusicApp, ctx: &egui::Context) -> bool {
    let mut changed = playback::poll_events(app);
    changed |= sources::poll_import(app, ctx);
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
    if playback::poll_download(app) {
        changed = true;
    }
    changed
}

impl Hall {
    fn search_first_page(&mut self, ctx: &egui::Context) {
        self.submitted_query = self.query.trim().to_owned();
        self.search_page(1, ctx);
    }

    /// 提交当前输入。返回值表示搜索历史已更新，调用方据此决定是否保存历史。
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
}

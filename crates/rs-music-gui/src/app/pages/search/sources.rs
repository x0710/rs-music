use super::Hall;
use crate::app::{
    Message, MusicApp,
    tasks::{finished, spawn},
};
use eframe::egui;
use rs_music_core::{self as music_source, Source};
use rs_music_storage::{SavedSource, SourceSelection};

pub(super) fn poll_import(app: &mut MusicApp, ctx: &egui::Context) -> bool {
    let Some(result) = app.hall.importing.as_ref().and_then(finished) else {
        return false;
    };
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
                && let (Source::Lx(script), Some(platform)) = (&app.hall.source, source.platform())
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
    true
}

impl Hall {
    /// 移除有效下标对应的文档，同时修正当前选择及等待恢复的选择，避免下标指向相邻音源。
    pub(super) fn delete_saved(&mut self, index: usize, ctx: &egui::Context) {
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

    pub(super) fn catalog(&self) -> Option<Source> {
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

    pub(super) fn select_platform(&mut self, platform: &str, ctx: &egui::Context) {
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

    /// 后台加载 `saved[index]`。传入 `pair` 表示尝试与当前 LX 源配对为搜索目录；
    /// 加载结果由 `poll_import` 在界面线程应用。
    pub(super) fn load_saved(&mut self, index: usize, pair: bool, ctx: &egui::Context) {
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

    pub(super) fn restore_selection(&mut self, selection: SourceSelection) {
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

    pub(super) fn select(&mut self, source: Source) {
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

pub(super) fn load_document(
    input: Result<(bool, String), Box<dyn std::error::Error>>,
) -> Result<(Source, SavedSource, bool), String> {
    let result = (|| {
        let (script, contents) = input?;
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

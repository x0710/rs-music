use super::Hall;
use super::lyrics::load_and_save_lyrics;
use crate::{
    app::{
        Message, MusicApp,
        tasks::{Task, finished, spawn},
    },
    audio::{DownloadMonitor, DownloadProgress, OnlineAudio},
};
use eframe::egui;
use rs_music_core::{self as music_source, Source, Track, lyrics::Lyrics};
use rs_music_storage::{CachedTrack, DownloadSettings, OnlineTrack, SavedSource};
use std::{
    cmp::Reverse,
    io::ErrorKind,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
pub(super) enum PlaybackEvent {
    Saved(Message, Result<PathBuf, String>),
    SourceSucceeded(String),
}
pub(in crate::app) struct Playback {
    title: String,
    duration: Option<Duration>,
    lyric_target: Option<(String, Track)>,
    lyric_load: Option<Task<Option<Lyrics>>>,
    lyrics_permission: Option<mpsc::Sender<()>>,
    pub(super) task: Task<(OnlineAudio, Option<String>)>,
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
pub(super) fn poll_events(app: &mut MusicApp) -> bool {
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
    changed
}
pub(super) fn poll_download(app: &mut MusicApp) -> bool {
    let mut changed = false;
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
    /// 按可重现的身份生成缓存位置。LX 使用平台、音质和曲目 ID，JSON 源可使用文档内容和 ID，
    /// 直链使用完整 URL。返回值只描述缓存位置，实际文件可能尚未下载。
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

    /// 用于决定是否启用播放按钮。开启自动换源时，保存列表中存在备用脚本即可尝试；
    /// 备用脚本是否支持目标平台，要等该脚本初始化后才能确认。
    pub(in crate::app) fn can_play_lx(&self, platform: &str) -> bool {
        let primary_is_lx = matches!(self.source, Source::Lx(_));
        matches!(&self.source, Source::Lx(source) if source.platforms().contains_key(platform))
            || (self.auto_switch_source
                && !platform.is_empty()
                && self.saved.iter().enumerate().any(|(index, document)| {
                    document.script && (!primary_is_lx || Some(index) != self.source_index)
                }))
    }

    pub(super) fn can_play_search_result(&self, track: &Track) -> bool {
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

    pub(super) fn online_track(&self, track: Track) -> OnlineTrack {
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
}

use crate::{
    app::{
        MUTED, Message, MusicApp,
        tasks::{Task, finished, spawn},
    },
    i18n::Language,
};
use eframe::egui;
use rs_music_core::{DiscoveryPlatform, Track, public_error};
use rs_music_storage::OnlineTrack;
use std::{collections::HashMap, time::Duration};
pub(super) enum QueueImport {
    Pending(Task<Vec<Track>>),
    Complete(Result<usize, String>),
}
/// 按各歌单内的原始顺序追加已加载曲目，再将任务标记为完成，避免下一帧重复入队。
/// 返回真表示本帧有任务结束，失败的任务也计入。
pub(in crate::app) fn poll_queue_imports(app: &mut MusicApp, ctx: &egui::Context) -> bool {
    let mut finished_imports = Vec::new();
    let mut pending = false;
    for (&key, state) in &app.discovery.queue_imports {
        if let QueueImport::Pending(task) = state {
            if let Some(result) = finished(task) {
                finished_imports.push((key, result));
            } else {
                pending = true;
            }
        }
    }
    let changed = !finished_imports.is_empty();
    for ((platform, id), result) in finished_imports {
        let result = result.map(|tracks| {
            let entries = tracks
                .into_iter()
                .map(|track| {
                    OnlineTrack {
                        platform: platform.code().into(),
                        track,
                        source: None,
                    }
                    .into()
                })
                .collect();
            app.enqueue_batch(entries)
        });
        if let Err(error) = &result {
            tracing::warn!(platform = platform.code(), playlist_id = id, %error, "Could not add playlist to queue");
        }
        app.discovery
            .queue_imports
            .insert((platform, id), QueueImport::Complete(result));
    }
    if pending {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    changed
}
pub(super) fn show_queue_import(
    imports: &mut HashMap<(DiscoveryPlatform, u64), QueueImport>,
    platform: DiscoveryPlatform,
    id: u64,
    language: Language,
    ui: &mut egui::Ui,
) {
    let key = (platform, id);
    let pending = matches!(imports.get(&key), Some(QueueImport::Pending(_)));
    if ui
        .add_enabled(
            !pending,
            egui::Button::new(language.text(Message::QueuePlaylist)),
        )
        .clicked()
    {
        imports.insert(
            key,
            QueueImport::Pending(spawn(ui.ctx(), move || {
                platform
                    .cached_all_playlist_tracks(id)
                    .map_err(public_error)
            })),
        );
    }
    match imports.get(&key) {
        Some(QueueImport::Pending(_)) => {
            ui.spinner();
        }
        Some(QueueImport::Complete(Ok(count))) => {
            ui.colored_label(MUTED, language.playlist_queued(*count));
        }
        Some(QueueImport::Complete(Err(error))) => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                language.text(Message::QueuePlaylistFailed),
            )
            .on_hover_text(error);
        }
        None => {}
    }
}

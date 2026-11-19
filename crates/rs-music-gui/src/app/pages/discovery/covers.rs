use super::expired;
use crate::app::{
    cover,
    tasks::{Task, finished, spawn},
};
use eframe::egui;
use rs_music_core::{DiscoveryData, DiscoveryPlatform, Playlist, Url, error_details, public_error};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
pub(super) enum Cover {
    Pending(Task<DiscoveryData<egui::ColorImage>>),
    Ready(DiscoveryData<egui::TextureHandle>),
    Unavailable(Instant),
}
/// 按封面 URL 复用任务和纹理。工作线程解码图片，此处在界面线程创建纹理。
/// 缺少 URL、正在加载或处于失败重试等待期时均返回 `None`。
pub(super) fn playlist_texture<'a>(
    platform: DiscoveryPlatform,
    playlist: &Playlist,
    covers: &'a mut HashMap<Url, Cover>,
    ctx: &egui::Context,
) -> Option<&'a egui::TextureHandle> {
    let url = playlist.cover_url.as_ref()?;
    let cover = covers
        .entry(url.clone())
        .or_insert_with(|| start_cover(platform, url.clone(), ctx));
    let reload = match cover {
        Cover::Ready(data) => expired(data.valid_until, ctx),
        Cover::Unavailable(retry) => expired(*retry, ctx),
        Cover::Pending(_) => false,
    };
    if reload {
        *cover = start_cover(platform, url.clone(), ctx);
    }
    if let Cover::Pending(task) = cover {
        if let Some(result) = finished(task) {
            *cover = match result {
                Ok(data) => Cover::Ready(DiscoveryData {
                    value: ctx.load_texture(
                        format!("playlist-cover-{}-{}", platform.code(), playlist.id),
                        data.value,
                        egui::TextureOptions::LINEAR,
                    ),
                    valid_until: data.valid_until,
                }),
                Err(_) => Cover::Unavailable(Instant::now() + Duration::from_secs(60)),
            };
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    match cover {
        Cover::Ready(data) => Some(&data.value),
        _ => None,
    }
}

fn start_cover(platform: DiscoveryPlatform, url: Url, ctx: &egui::Context) -> Cover {
    Cover::Pending(spawn(ctx, move || load_cover(platform, url)))
}

fn load_cover(
    platform: DiscoveryPlatform,
    url: Url,
) -> Result<DiscoveryData<egui::ColorImage>, String> {
    let host = url.host_str().unwrap_or("unknown").to_owned();
    let data = platform.cached_cover(url, false).map_err(public_error)?;
    let size = data.value.len();
    let value = cover::decode(&data.value, 512).map_err(|error| {
        let error = error_details(error.as_ref());
        tracing::warn!(platform = platform.code(), %host, bytes = size, %error, "Could not decode playlist cover");
        error
    })?;
    Ok(DiscoveryData {
        value,
        valid_until: data.valid_until,
    })
}

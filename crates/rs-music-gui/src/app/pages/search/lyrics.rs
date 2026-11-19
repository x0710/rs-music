use rs_music_core::{
    self as music_source, Track,
    lyrics::{self, Lyrics},
};
use rs_music_storage::CachedTrack;
use std::{path::PathBuf, sync::mpsc};
/// 优先解析已保存的 LRC，否则获取在线歌词。返回解析结果供界面显示。
/// 在线歌词解析成功后，收到 `permission` 才写入文件，并通过 `report` 报告写入结果。
pub(super) fn load_and_save_lyrics(
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
        // Return lyrics for display immediately. Dropping the sender cancels the pending archive.
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

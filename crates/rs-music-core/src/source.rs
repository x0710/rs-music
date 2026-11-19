use crate::{
    AudioStream, LxSource, Result, SEARCH_PAGE_SIZE, SearchPage, Track,
    audio_stream::open_audio,
    diagnostics::ErrorContext,
    http::{client, https_url, lx_client, read_json, read_limited},
    platform_search::PlatformSearch,
    report_failure,
};
use serde::Deserialize;
use std::io::{self, Read};

const SCRIPT_LIMIT: u64 = 2 * 1024 * 1024;
const CATALOG_LIMIT: u64 = 64 * 1024;

/// 在线音源。内置平台提供搜索目录，LX 脚本按平台和音质解析播放地址。
#[derive(Clone)]
pub enum Source {
    ITunes,
    Platform(PlatformSearch),
    Lx(LxSource),
    Json(JsonSource),
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonSource {
    name: String,
    search_url: String,
    #[serde(default = "default_query")]
    query_parameter: String,
    results: String,
    title: String,
    artist: String,
    audio_url: Option<String>,
    track_id: Option<String>,
    platform: Option<String>,
    playback: Option<PlaybackEndpoint>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaybackEndpoint {
    url: String,
    #[serde(default = "default_id_parameter")]
    id_parameter: String,
    audio_url: String,
}

fn default_id_parameter() -> String {
    "id".into()
}

fn default_query() -> String {
    "q".into()
}

impl Source {
    pub fn import(path: &std::path::Path) -> Result<Self> {
        let (is_script, data) = Self::read_document(path)?;
        Self::from_document(is_script, &data)
    }

    /// 按文件名识别音源，返回 `(是否为 LX 脚本或导出包, 原始文本)`。
    /// 读取上限对 `.js`、`.lx.json` 为 2 MiB，对普通 JSON 为 64 KiB。
    pub fn read_document(path: &std::path::Path) -> Result<(bool, String)> {
        let script = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                let name = name.to_ascii_lowercase();
                name.ends_with(".js") || name.ends_with(".lx.json")
            });
        let limit = if script { SCRIPT_LIMIT } else { CATALOG_LIMIT };
        let mut data = String::new();
        std::fs::File::open(path)
            .map_err(|error| {
                ErrorContext::new(
                    format!("Could not open source file ({})", path.display()),
                    error,
                )
            })?
            .take(limit + 1)
            .read_to_string(&mut data)
            .map_err(|error| {
                ErrorContext::new(
                    format!("Could not read source file ({})", path.display()),
                    error,
                )
            })?;
        if data.len() as u64 > limit {
            return Err(io::Error::other(format!("Source exceeds {limit} bytes")).into());
        }
        Ok((script, data))
    }

    /// 下载 HTTP(S) 音源原文。共享链接可能没有扩展名，因此根据内容识别 LX 脚本、导出包和 JSON 目录。
    pub fn download_document(address: &str) -> Result<(bool, String)> {
        let url = crate::Url::parse(address.trim())?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("Expected an HTTP(S) source URL without embedded credentials".into());
        }
        let response = lx_client()?.get(url).send()?;
        classify_download(String::from_utf8(read_limited(response, SCRIPT_LIMIT)?)?)
    }

    /// 从已读取或持久化的文本恢复音源。传入 [`Self::read_document`] 返回的标志；
    /// 当 `script` 为真时，此调用会初始化 LX 运行时，初始化失败会返回错误。
    pub fn from_document(script: bool, data: &str) -> Result<Self> {
        let data = data.trim_start_matches('\u{feff}');
        if script {
            let source = if data.trim_start().starts_with('{') {
                LxSource::load_export(data)?
            } else {
                LxSource::load(data)?
            };
            Ok(Self::Lx(source))
        } else {
            Self::parse(data)
        }
    }

    /// 校验 JSON 音源的 HTTPS 端点和字段映射。搜索结果至少需要提供音频地址或曲目 ID。
    ///
    /// ```
    /// use rs_music_core::Source;
    ///
    /// let source = Source::parse(r#"{
    ///     "name":"Demo", "search_url":"https://example.org/search",
    ///     "results":"/songs", "title":"/title", "artist":"/artist", "audio_url":"/url"
    /// }"#)?;
    /// assert_eq!(source.name(), "Demo");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn parse(data: &str) -> Result<Self> {
        let source: JsonSource = serde_json::from_str(data)?;
        https_url(&source.search_url)?;
        if source.name.trim().is_empty()
            || source.query_parameter.trim().is_empty()
            || [&source.results, &source.title, &source.artist]
                .into_iter()
                .chain(source.audio_url.iter())
                .chain(source.track_id.iter())
                .any(|field| !field.starts_with('/'))
            || (source.audio_url.is_none() && source.track_id.is_none())
            || (source.platform.is_some() && source.track_id.is_none())
        {
            return Err(io::Error::other(
                "Source requires a name, query parameter and JSON pointers",
            )
            .into());
        }
        if let Some(playback) = &source.playback {
            https_url(&playback.url)?;
            if source.track_id.is_none()
                || playback.id_parameter.trim().is_empty()
                || !playback.audio_url.starts_with('/')
            {
                return Err(io::Error::other(
                    "Playback endpoint requires track_id, id_parameter and an audio URL pointer",
                )
                .into());
            }
        }
        Ok(Self::Json(source))
    }

    pub fn name(&self) -> &str {
        match self {
            Self::ITunes => "iTunes",
            Self::Platform(source) => source.name(),
            Self::Lx(source) => source.name(),
            Self::Json(source) => &source.name,
        }
    }

    pub fn builtin_catalog(platform: &str) -> Option<Self> {
        PlatformSearch::find(platform).map(Self::Platform)
    }

    pub fn platform(&self) -> Option<&str> {
        match self {
            Self::Json(source) => source.platform.as_deref(),
            Self::Platform(source) => Some(source.platform()),
            _ => None,
        }
    }

    /// 根据曲目直链或 JSON 源的 ID 解析配置判断是否可尝试播放。此检查不发起网络请求。
    pub fn can_play(&self, track: &Track) -> bool {
        track.audio_url.is_some()
            || (track.id.is_some()
                && matches!(self, Self::Json(source) if source.playback.is_some()))
    }

    pub fn play_track(&self, track: Track) -> Result<Vec<u8>> {
        self.stream_track(track)?.read_all()
    }

    /// 优先打开曲目直链，否则通过 JSON 源的播放端点解析 ID。返回值支持增量读取，
    /// 调用方还需处理读取响应体时发生的网络错误和大小超限错误。
    pub fn stream_track(&self, track: Track) -> Result<AudioStream> {
        report_failure(
            self.name(),
            "online playback",
            self.resolve_and_fetch(track),
        )
    }

    fn resolve_and_fetch(&self, track: Track) -> Result<AudioStream> {
        if let Some(url) = track.audio_url {
            return open_audio(https_url(url.as_str())?);
        }
        if let (Self::Json(source), Some(id)) = (self, track.id)
            && let Some(playback) = &source.playback
        {
            let response = client()?
                .get(https_url(&playback.url)?)
                .query(&[(playback.id_parameter.as_str(), id.as_str())])
                .send()?;
            let body: serde_json::Value = read_json(response)?;
            let url = body
                .pointer(&playback.audio_url)
                .and_then(|value| value.as_str())
                .ok_or_else(|| io::Error::other("Playback response has no audio URL"))?;
            return open_audio(https_url(url)?);
        }
        Err(io::Error::other("Track has no playback address").into())
    }

    /// 页码从 1 开始，传 0 也读取第一页。内置平台负责远端分页，iTunes 和 JSON 源
    /// 从本次请求返回的结果中截取一页。LX 脚本需搭配搜索目录。
    pub fn search_page(&self, query: &str, page: usize) -> Result<SearchPage> {
        let page = page.max(1);
        let source = self.name();
        tracing::info!(source, page, "Starting online search");
        let result = if let Self::Platform(platform) = self {
            platform.search(query, page)
        } else {
            self.search_remote(query).map(|tracks| {
                let start = (page - 1).saturating_mul(SEARCH_PAGE_SIZE);
                SearchPage {
                    has_next: tracks.len() > start.saturating_add(SEARCH_PAGE_SIZE),
                    tracks: tracks
                        .into_iter()
                        .skip(start)
                        .take(SEARCH_PAGE_SIZE)
                        .collect(),
                }
            })
        };
        let results = report_failure(source, "online search", result)?;
        tracing::info!(
            source,
            page,
            results = results.tracks.len(),
            "Online search completed"
        );
        Ok(results)
    }

    fn search_remote(&self, query: &str) -> Result<Vec<Track>> {
        match self {
            Self::Lx(_) | Self::Platform(_) => {
                Err(io::Error::other("Source requires platform search").into())
            }
            Self::ITunes => {
                #[derive(Deserialize)]
                struct Results {
                    results: Vec<Entry>,
                }
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Entry {
                    track_name: String,
                    artist_name: String,
                    track_view_url: Option<String>,
                }
                let response = client()?
                    .get("https://itunes.apple.com/search")
                    .query(&[
                        ("term", query),
                        ("media", "music"),
                        ("entity", "song"),
                        ("limit", "100"),
                    ])
                    .send()?;
                Ok(read_json::<Results>(response)?
                    .results
                    .into_iter()
                    .map(|entry| Track {
                        id: None,
                        lx_metadata: None,
                        artwork: None,
                        title: entry.track_name,
                        artist: entry.artist_name,
                        duration: None,
                        audio_url: None,
                        store_url: entry.track_view_url.and_then(|url| https_url(&url).ok()),
                    })
                    .collect())
            }
            Self::Json(source) => {
                let response = client()?
                    .get(https_url(&source.search_url)?)
                    .query(&[(source.query_parameter.as_str(), query)])
                    .send()?;
                let body: serde_json::Value = read_json(response)?;
                let rows = body
                    .pointer(&source.results)
                    .and_then(|value| value.as_array())
                    .ok_or_else(|| {
                        io::Error::other("Results pointer does not reference an array")
                    })?;
                Ok(rows
                    .iter()
                    .take(100)
                    .filter_map(|row| {
                        let field = |key: &str| row.pointer(key).and_then(|value| value.as_str());
                        Some(Track {
                            duration: None,
                            lx_metadata: None,
                            artwork: None,
                            id: source
                                .track_id
                                .as_deref()
                                .and_then(|key| row.pointer(key))
                                .and_then(|value| match value {
                                    serde_json::Value::String(id) if !id.trim().is_empty() => {
                                        Some(id.clone())
                                    }
                                    serde_json::Value::Number(id) if id.is_i64() || id.is_u64() => {
                                        Some(id.to_string())
                                    }
                                    _ => None,
                                }),
                            title: field(&source.title)?.to_owned(),
                            artist: field(&source.artist).unwrap_or_default().to_owned(),
                            audio_url: source
                                .audio_url
                                .as_deref()
                                .and_then(field)
                                .and_then(|url| https_url(url).ok()),
                            store_url: None,
                        })
                    })
                    .collect())
            }
        }
    }
}

fn classify_download(contents: String) -> Result<(bool, String)> {
    let text = contents.trim_start_matches('\u{feff}').trim_start();
    if text.is_empty() || text.starts_with('<') {
        return Err("Expected source contents. Use a direct link to the JS or JSON file".into());
    }
    let script = if text.starts_with('{') {
        let document: serde_json::Value = serde_json::from_str(text)?;
        document
            .get("script")
            .is_some_and(serde_json::Value::is_string)
    } else {
        true
    };
    if !script && contents.len() as u64 > CATALOG_LIMIT {
        return Err(format!("Source exceeds {CATALOG_LIMIT} bytes").into());
    }
    Ok((script, contents))
}

#[cfg(test)]
mod import_tests {
    use super::*;

    #[test]
    fn classifies_source_contents_without_filename_and_preserves_original_text() {
        for (contents, script) in [
            ("\u{feff}  /** @name Demo */", true),
            (r#"{"name":"Test","script":"gz_example"}"#, true),
            (
                r#"{"name":"Catalog","search_url":"https://example.org"}"#,
                false,
            ),
        ] {
            assert_eq!(
                classify_download(contents.to_owned()).unwrap(),
                (script, contents.to_owned())
            );
        }
        assert!(classify_download("<html>File viewer</html>".into()).is_err());
        assert!(
            classify_download(format!(
                r#"{{"name":"{}"}}"#,
                "a".repeat(CATALOG_LIMIT as usize)
            ))
            .is_err()
        );
    }
}

mod diagnostics;
mod discovery;
mod discovery_cache;
pub mod library;
mod lx;
mod lx_cache;
mod lx_utils;
pub mod lyrics;
mod platform_search;
pub use diagnostics::error_details;
use diagnostics::{ErrorContext, response_details};
pub use discovery::{DiscoveryPlatform, Playlist, PlaylistPage, PlaylistTracks};
pub use discovery_cache::DiscoveryData;
pub use lx::{LxSource, open_lx_with_fallback};
use platform_search::PlatformSearch;
pub use reqwest::Url;
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    error::Error,
    io::{self, Read},
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

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

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Track {
    pub id: Option<String>,
    pub(crate) lx_metadata: Option<serde_json::Value>,
    artwork: Option<(DiscoveryPlatform, Url)>,
    pub title: String,
    pub artist: String,
    pub duration: Option<Duration>,
    pub audio_url: Option<Url>,
    pub store_url: Option<Url>,
}

const SEARCH_PAGE_SIZE: usize = 30;

pub struct SearchPage {
    pub tracks: Vec<Track>,
    pub has_next: bool,
}

impl Source {
    pub fn import(path: &std::path::Path) -> Result<Self> {
        let (is_script, data) = Self::read_document(path)?;
        Self::from_document(is_script, &data)
    }

    pub fn read_document(path: &std::path::Path) -> Result<(bool, String)> {
        let script = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                let name = name.to_ascii_lowercase();
                name.ends_with(".js") || name.ends_with(".lx.json")
            });
        let limit = if script { 2 * 1024 * 1024 } else { 65536 };
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

    pub fn can_play(&self, track: &Track) -> bool {
        track.audio_url.is_some()
            || (track.id.is_some()
                && matches!(self, Self::Json(source) if source.playback.is_some()))
    }

    pub fn play_track(&self, track: Track) -> Result<Vec<u8>> {
        self.stream_track(track)?.read_all()
    }

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

fn https_url(input: &str) -> Result<Url> {
    let url = Url::parse(input)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(io::Error::other("Expected an HTTPS URL without credentials").into());
    }
    Ok(url)
}

fn lx_url(input: &str, stage: &str) -> Result<Url> {
    let input = input.trim();
    let normalized;
    let input = if input.starts_with("//") {
        normalized = format!("https:{input}");
        &normalized
    } else {
        input
    };
    let url = Url::parse(input)
        .map_err(|error| io::Error::other(format!("{stage}: invalid URL ({error})")))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(io::Error::other(format!(
            "{stage}: expected an HTTP(S) URL without embedded credentials"
        ))
        .into());
    }
    if url.scheme() == "http" {
        tracing::debug!(stage, "LX source uses an unencrypted HTTP connection");
    }
    Ok(url)
}

fn lx_client() -> Result<Client> {
    Ok(Client::builder().timeout(Duration::from_secs(20)).build()?)
}

fn client() -> Result<Client> {
    Ok(Client::builder()
        .https_only(true)
        .timeout(Duration::from_secs(20))
        .build()?)
}

fn read_json<T: DeserializeOwned>(response: Response) -> Result<T> {
    let details = response_details(&response);
    let bytes = read_limited(response, 2 * 1024 * 1024)?;
    Ok(serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "Could not parse JSON response ({details}, {} bytes) → JSON {:?} error at line {}, column {}",
            bytes.len(), error.classify(), error.line(), error.column()
        )
    })?)
}

fn read_limited(response: Response, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    check_response(&response, "Unexpected response status")?;
    let mut body = response.take(limit + 1);
    body.read_to_end(&mut bytes).map_err(|error| {
        let details = response_details(body.get_ref());
        ErrorContext::new(format!("Could not read response body ({details})"), error)
    })?;
    if bytes.len() as u64 > limit {
        let details = response_details(body.get_ref());
        return Err(format!("Response exceeds the {limit}-byte limit ({details})").into());
    }
    Ok(bytes)
}

fn check_response(response: &Response, message: &str) -> Result<()> {
    if response.status() != StatusCode::OK {
        return Err(format!("{message} ({})", response_details(response)).into());
    }
    Ok(())
}

pub fn fetch_audio(url: Url) -> Result<Vec<u8>> {
    open_audio(url)?.read_all()
}

fn open_audio(url: Url) -> Result<AudioStream> {
    AudioStream::new(client()?.get(url).send()?)
}

pub struct AudioStream {
    response: Response,
    remaining: u64,
}

impl AudioStream {
    pub const MAX_BYTES: u64 = 40 * 1024 * 1024;

    fn new(response: Response) -> Result<Self> {
        check_response(&response, "Unexpected audio response status")?;
        if let Some(len) = response.content_length()
            && len > Self::MAX_BYTES
        {
            let details = response_details(&response);
            return Err(
                format!("Audio response exceeds 40 MiB ({details}, declared {len} bytes)").into(),
            );
        }
        Ok(Self {
            response,
            remaining: Self::MAX_BYTES,
        })
    }

    pub fn content_length(&self) -> Option<u64> {
        self.response.content_length()
    }

    fn read_all(mut self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn read_body(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.response.read(buffer).map_err(|error| {
            let details = response_details(&self.response);
            let read = Self::MAX_BYTES - self.remaining;
            io::Error::new(
                error.kind(),
                format!(
                    "Could not read audio body ({details}, {read} bytes read) → {}",
                    error_details(&error)
                ),
            )
        })
    }
}

impl Read for AudioStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !buffer.is_empty() && self.remaining == 0 {
            return match self.read_body(&mut [0])? {
                0 => Ok(0),
                _ => Err(io::Error::other(format!(
                    "Audio response exceeds 40 MiB ({})",
                    response_details(&self.response)
                ))),
            };
        }
        let len = buffer.len().min(self.remaining as usize);
        let read = self.read_body(&mut buffer[..len])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

pub fn public_error(error: Box<dyn Error>) -> String {
    error_details(error.as_ref())
}

fn report_failure<T>(source: &str, operation: &str, result: Result<T>) -> Result<T> {
    result.inspect_err(|error| {
        let error = error_details(error.as_ref());
        tracing::warn!(source, operation, %error, "Music source operation failed");
    })
}

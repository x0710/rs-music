mod audio_stream;
mod diagnostics;
mod discovery;
mod discovery_cache;
mod http;
pub mod library;
mod lx;
mod lx_cache;
mod lx_utils;
pub mod lyrics;
mod platform_search;
mod source;
mod track;

pub use audio_stream::{AudioStream, fetch_audio};
pub use diagnostics::error_details;
pub use discovery::{DiscoveryPlatform, Playlist, PlaylistPage, PlaylistTracks};
pub use discovery_cache::DiscoveryData;
use http::{client, https_url, lx_client, lx_url, read_json, read_limited};
pub use lx::{LxSource, open_lx_with_fallback};
pub use reqwest::Url;
pub use source::{JsonSource, Source};
use std::error::Error;
use track::SEARCH_PAGE_SIZE;
pub use track::{SearchPage, Track};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn public_error(error: Box<dyn Error>) -> String {
    error_details(error.as_ref())
}

fn report_failure<T>(source: &str, operation: &str, result: Result<T>) -> Result<T> {
    result.inspect_err(|error| {
        let error = error_details(error.as_ref());
        tracing::warn!(source, operation, %error, "Music source operation failed");
    })
}

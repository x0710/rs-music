use crate::{DiscoveryPlatform, Url};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Track {
    pub id: Option<String>,
    pub(crate) lx_metadata: Option<serde_json::Value>,
    pub(crate) artwork: Option<(DiscoveryPlatform, Url)>,
    pub title: String,
    pub artist: String,
    pub duration: Option<Duration>,
    pub audio_url: Option<Url>,
    pub store_url: Option<Url>,
}

pub(crate) const SEARCH_PAGE_SIZE: usize = 30;

pub struct SearchPage {
    pub tracks: Vec<Track>,
    pub has_next: bool,
}

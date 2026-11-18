pub(super) mod kugou;
pub(super) mod kuwo;
mod migu;
pub(super) mod netease;
pub(super) mod qq;

use crate::{Result, SEARCH_PAGE_SIZE, SearchPage, Track};
use serde_json::Value;
use std::{io, time::Duration};

#[derive(Clone, Copy)]
pub struct PlatformSearch {
    code: &'static str,
    name: &'static str,
    search: fn(&str, usize) -> Result<SearchPage>,
}

const PLATFORMS: &[PlatformSearch] = &[
    PlatformSearch {
        code: "kg",
        name: "酷狗音乐 / Kugou",
        search: kugou::search,
    },
    PlatformSearch {
        code: "kw",
        name: "酷我音乐 / Kuwo",
        search: kuwo::search,
    },
    PlatformSearch {
        code: "mg",
        name: "咪咕音乐 / Migu",
        search: migu::search,
    },
    PlatformSearch {
        code: "tx",
        name: "QQ 音乐 / QQ Music",
        search: qq::search,
    },
    PlatformSearch {
        code: "wy",
        name: "网易云音乐 / NetEase",
        search: netease::search,
    },
];

impl PlatformSearch {
    pub(super) fn find(code: &str) -> Option<Self> {
        PLATFORMS
            .iter()
            .find(|platform| platform.code == code)
            .copied()
    }

    pub(super) fn platform(self) -> &'static str {
        self.code
    }

    pub(super) fn name(self) -> &'static str {
        self.name
    }

    pub(super) fn search(self, query: &str, page: usize) -> Result<SearchPage> {
        (self.search)(query, page)
    }
}

fn singers(value: &Value) -> String {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|singer| singer["name"].as_str())
        .collect::<Vec<_>>()
        .join(" / ")
}

fn rows<'a>(body: &'a Value, pointer: &str) -> Result<&'a [Value]> {
    body.pointer(pointer)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| io::Error::other("Platform search response has no track list").into())
}

fn track(
    id: &str,
    title: &Value,
    artist: String,
    metadata: Value,
    duration: Option<Duration>,
) -> Option<Track> {
    if id.trim().is_empty() {
        return None;
    }
    Some(Track {
        id: Some(id.to_owned()),
        lx_metadata: Some(metadata),
        artwork: None,
        title: title.as_str()?.to_owned(),
        artist,
        duration,
        audio_url: None,
        store_url: None,
    })
}

fn seconds(value: &Value) -> Option<Duration> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|&value| value > 0)
        .map(Duration::from_secs)
}

fn total(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn result_page(
    page: usize,
    received: usize,
    total: Option<usize>,
    tracks: Vec<Track>,
) -> SearchPage {
    SearchPage {
        has_next: received != 0
            && total.map_or(received >= SEARCH_PAGE_SIZE, |total| {
                page.saturating_mul(SEARCH_PAGE_SIZE) < total
            }),
        tracks,
    }
}

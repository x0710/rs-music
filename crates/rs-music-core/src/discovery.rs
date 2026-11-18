use crate::discovery_cache::{self, DiscoveryData};
use crate::platform_search::{kugou, kuwo, netease, qq};
use crate::{Result, Track, Url, client, https_url, read_limited, report_failure};
use serde_json::Value;
use std::{io, time::Duration};

#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize,
)]
pub enum DiscoveryPlatform {
    #[default]
    NetEase,
    QQ,
    Kugou,
    Kuwo,
}

impl DiscoveryPlatform {
    pub const ALL: [Self; 4] = [Self::NetEase, Self::QQ, Self::Kugou, Self::Kuwo];

    pub const fn code(self) -> &'static str {
        match self {
            Self::NetEase => "wy",
            Self::QQ => "tx",
            Self::Kugou => "kg",
            Self::Kuwo => "kw",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::NetEase => "NetEase Music",
            Self::QQ => "QQ Music",
            Self::Kugou => "Kugou",
            Self::Kuwo => "Kuwo",
        }
    }

    pub fn popular_tracks(self) -> Result<Vec<Track>> {
        let mut tracks = self.popular_track_page(1)?.tracks;
        tracks.truncate(12);
        Ok(tracks)
    }

    pub fn community_playlists(self) -> Result<Vec<Playlist>> {
        Ok(self.community_playlist_page(1, false)?.value.playlists)
    }

    pub fn cached_popular_tracks(self, refresh: bool) -> Result<DiscoveryData<Vec<Track>>> {
        let mut data = self.cached_popular_track_page(1, refresh)?;
        data.value.tracks.truncate(12);
        Ok(DiscoveryData {
            value: data.value.tracks,
            valid_until: data.valid_until,
        })
    }

    pub const fn popular_track_page_size(self) -> usize {
        match self {
            Self::Kugou => 30,
            _ => 12,
        }
    }

    pub fn cached_popular_track_page(
        self,
        page: usize,
        refresh: bool,
    ) -> Result<DiscoveryData<PlaylistTracks>> {
        let page = page.max(1);
        discovery_cache::json(&format!("{}:charts:{page}", self.code()), refresh, || {
            self.popular_track_page(page)
        })
    }

    fn popular_track_page(self, page: usize) -> Result<PlaylistTracks> {
        let limit = self.popular_track_page_size();
        let result = match self {
            Self::NetEase => netease::popular_tracks(page, limit),
            Self::QQ => qq::popular_tracks(page, limit),
            Self::Kugou => kugou::popular_tracks(page, limit),
            Self::Kuwo => kuwo::popular_tracks(page, limit),
        };
        report_failure(self.name(), &format!("trending tracks page {page}"), result)
    }

    pub fn community_playlist_page(
        self,
        page: usize,
        refresh: bool,
    ) -> Result<DiscoveryData<PlaylistPage>> {
        let page = page.max(1);
        if self == Self::QQ {
            let data = discovery_cache::json("tx:playlists", refresh, || {
                report_failure(
                    self.name(),
                    "community playlists",
                    qq::community_playlists(),
                )
            })?;
            let start = playlist_offset(page);
            let has_next = data.value.len() > start.saturating_add(PLAYLIST_PAGE_SIZE);
            return Ok(DiscoveryData {
                value: PlaylistPage {
                    playlists: data
                        .value
                        .into_iter()
                        .skip(start)
                        .take(PLAYLIST_PAGE_SIZE)
                        .collect(),
                    has_next,
                },
                valid_until: data.valid_until,
            });
        }
        let catalog = if self == Self::Kugou {
            "playlists:v2"
        } else {
            "playlists"
        };
        discovery_cache::json(
            &format!("{}:{catalog}:{page}", self.code()),
            refresh,
            || {
                let result = match self {
                    Self::NetEase => netease::community_playlists(page),
                    Self::Kugou => kugou::community_playlists(page),
                    Self::Kuwo => kuwo::community_playlists(page),
                    Self::QQ => unreachable!(),
                };
                report_failure(
                    self.name(),
                    &format!("community playlists page {page}"),
                    result,
                )
            },
        )
    }

    pub fn cached_playlist_tracks(
        self,
        id: u64,
        page: usize,
        refresh: bool,
    ) -> Result<DiscoveryData<PlaylistTracks>> {
        let page = page.max(1);
        if self == Self::Kugou {
            let data = cached_kugou_playlist(id, refresh)?;
            return Ok(DiscoveryData {
                value: PlaylistTracks::slice(data.value, page),
                valid_until: data.valid_until,
            });
        }
        discovery_cache::json(
            &format!("{}:playlist:{id}:page:v2:{page}", self.code()),
            refresh,
            || self.playlist_track_page(id, page),
        )
    }

    pub fn cached_all_playlist_tracks(self, id: u64) -> Result<Vec<Track>> {
        if self == Self::Kugou {
            return Ok(cached_kugou_playlist(id, false)?.value);
        }
        collect_playlist_pages(|page| Ok(self.cached_playlist_tracks(id, page, false)?.value))
    }

    pub fn playlist_tracks(self, id: u64) -> Result<PlaylistTracks> {
        self.playlist_track_page(id, 1)
    }

    pub fn playlist_track_page(self, id: u64, page: usize) -> Result<PlaylistTracks> {
        let page = page.max(1);
        let result = match self {
            Self::NetEase => netease::playlist_tracks(id, page),
            Self::QQ => qq::playlist_tracks(id, page),
            Self::Kugou => {
                kugou::playlist_tracks(id).map(|tracks| PlaylistTracks::slice(tracks, page))
            }
            Self::Kuwo => kuwo::playlist_tracks(id, page),
        };
        report_failure(self.name(), &format!("playlist {id} page {page}"), result)
    }

    fn image_host(self, url: &Url) -> bool {
        url.host_str().is_some_and(|host| match self {
            Self::NetEase => host.ends_with(".music.126.net"),
            Self::QQ => {
                host == "qpic.y.qq.com" || host.ends_with(".gtimg.cn") || host.ends_with(".qpic.cn")
            }
            Self::Kugou => host.ends_with(".kugou.com"),
            Self::Kuwo => host.ends_with(".kuwo.cn"),
        })
    }

    pub(crate) fn cover_url(self, input: &str) -> Option<Url> {
        let input = input.trim().replace("{size}", "256");
        let mut url = if input.starts_with("//") {
            Url::parse(&format!("https:{input}"))
        } else {
            Url::parse(&input)
        }
        .ok()?;
        if !self.image_host(&url) {
            return None;
        }
        if url.scheme() == "http" {
            url.set_scheme("https").ok()?;
        }
        https_url(url.as_str()).ok()
    }

    pub fn fetch_cover(self, url: Url) -> Result<Vec<u8>> {
        self.fetch_image(url, "playlist cover")
    }

    pub fn cached_cover(self, url: Url, refresh: bool) -> Result<DiscoveryData<Vec<u8>>> {
        discovery_cache::image(&format!("{}:cover:{url}", self.code()), refresh, || {
            self.fetch_cover(url)
        })
    }

    pub(crate) fn fetch_image(self, url: Url, operation: &str) -> Result<Vec<u8>> {
        report_failure(
            self.name(),
            operation,
            (|| {
                let mut url = self
                    .cover_url(url.as_str())
                    .ok_or_else(|| io::Error::other("Unexpected cover URL"))?;
                if self == Self::NetEase {
                    url.query_pairs_mut().append_pair("param", "256y256");
                }
                read_limited(
                    client()?.get(url).timeout(Duration::from_secs(8)).send()?,
                    512 * 1024,
                )
            })(),
        )
    }
}

fn cached_kugou_playlist(id: u64, refresh: bool) -> Result<DiscoveryData<Vec<Track>>> {
    discovery_cache::json(&format!("kg:playlist:{id}:tracks"), refresh, || {
        report_failure(
            DiscoveryPlatform::Kugou.name(),
            &format!("playlist {id}"),
            kugou::playlist_tracks(id),
        )
    })
}

fn collect_playlist_pages(
    mut load: impl FnMut(usize) -> Result<PlaylistTracks>,
) -> Result<Vec<Track>> {
    let mut tracks = Vec::new();
    let mut page = 1;
    loop {
        let loaded = load(page)?;
        tracks.extend(loaded.tracks);
        if !loaded.has_next {
            return Ok(tracks);
        }
        page += 1;
    }
}

pub(crate) fn artwork(
    platform: DiscoveryPlatform,
    input: &str,
) -> Option<(DiscoveryPlatform, Url)> {
    Some((platform, platform.cover_url(input)?))
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct Playlist {
    platform: DiscoveryPlatform,
    pub id: u64,
    pub title: String,
    pub author: String,
    pub cover_url: Option<Url>,
    pub total: Option<usize>,
    #[serde(default)]
    pub description: Option<String>,
}

impl Playlist {
    pub(crate) fn new(
        platform: DiscoveryPlatform,
        id: u64,
        title: String,
        author: String,
        cover_url: Option<&str>,
    ) -> Self {
        Self {
            platform,
            id,
            title,
            author,
            cover_url: cover_url.and_then(|url| platform.cover_url(url)),
            total: None,
            description: None,
        }
    }

    pub fn web_url(&self) -> String {
        let id = self.id;
        match self.platform {
            DiscoveryPlatform::NetEase => format!("https://music.163.com/#/playlist?id={id}"),
            DiscoveryPlatform::QQ => format!("https://y.qq.com/n/ryqq/playlist/{id}"),
            DiscoveryPlatform::Kugou => {
                format!("https://www.kugou.com/yy/special/single/{id}.html")
            }
            DiscoveryPlatform::Kuwo => format!("https://www.kuwo.cn/playlist_detail/{id}"),
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct PlaylistTracks {
    pub tracks: Vec<Track>,
    pub total: usize,
    pub has_next: bool,
    #[serde(default)]
    pub description: Option<String>,
}

impl PlaylistTracks {
    fn slice(tracks: Vec<Track>, page: usize) -> Self {
        let total = tracks.len();
        let offset = track_offset(page);
        let tracks: Vec<_> = tracks
            .into_iter()
            .skip(offset)
            .take(PLAYLIST_TRACKS_PER_PAGE)
            .collect();
        let received = tracks.len();
        Self::page(
            tracks,
            Some(total),
            offset,
            received,
            PLAYLIST_TRACKS_PER_PAGE,
        )
    }

    pub(crate) fn page(
        tracks: Vec<Track>,
        total: Option<usize>,
        offset: usize,
        received: usize,
        limit: usize,
    ) -> Self {
        let through = offset.saturating_add(received);
        Self {
            tracks,
            total: total.unwrap_or(through),
            has_next: received != 0 && total.map_or(received >= limit, |total| through < total),
            description: None,
        }
    }
}

pub(crate) const PLAYLIST_TRACKS_PER_PAGE: usize = 24;

pub(crate) fn track_offset(page: usize) -> usize {
    page.saturating_sub(1)
        .saturating_mul(PLAYLIST_TRACKS_PER_PAGE)
}

pub const PLAYLIST_PAGE_SIZE: usize = 12;

pub(crate) fn playlist_offset(page: usize) -> usize {
    page.saturating_sub(1).saturating_mul(PLAYLIST_PAGE_SIZE)
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct PlaylistPage {
    pub playlists: Vec<Playlist>,
    pub has_next: bool,
}

pub(crate) fn numeric_id(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

pub(crate) fn playlist_description(text: Option<&str>) -> Option<String> {
    let text = text?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_the_full_playlist_in_page_order() {
        let tracks = collect_playlist_pages(|page| {
            let mut track = Track::default();
            track.title = page.to_string();
            Ok(PlaylistTracks {
                tracks: vec![track],
                total: 3,
                has_next: page < 3,
                description: None,
            })
        })
        .unwrap();
        let titles: Vec<_> = tracks.iter().map(|track| track.title.as_str()).collect();
        assert_eq!(titles, ["1", "2", "3"]);
    }
}

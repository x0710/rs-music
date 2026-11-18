use super::result_page;
use crate::discovery::{
    PLAYLIST_PAGE_SIZE, PLAYLIST_TRACKS_PER_PAGE, artwork, playlist_description, playlist_offset,
};
use crate::{
    DiscoveryPlatform, Playlist, PlaylistPage, PlaylistTracks, Result, SearchPage, Track, client,
    read_json,
};
use serde::Deserialize;
use std::{io, time::Duration};

const HOST: &str = "https://music.163.com";

#[derive(Deserialize)]
struct Song {
    id: u64,
    name: String,
    #[serde(alias = "ar", default)]
    artists: Vec<Artist>,
    #[serde(alias = "al")]
    album: Option<Album>,
    #[serde(alias = "dt")]
    duration: Option<u64>,
}

#[derive(Deserialize)]
struct Album {
    id: u64,
    name: String,
    #[serde(rename = "picUrl")]
    pic_url: Option<String>,
}

#[derive(Deserialize)]
struct Artist {
    name: String,
}

impl From<Song> for Track {
    fn from(song: Song) -> Self {
        let artwork = song
            .album
            .as_ref()
            .and_then(|album| artwork(DiscoveryPlatform::NetEase, album.pic_url.as_deref()?));
        Self {
            id: Some(song.id.to_string()),
            duration: song.duration.filter(|&ms| ms > 0).map(Duration::from_millis),
            lx_metadata: song.album.map(|album| {
                serde_json::json!({"albumId": album.id.to_string(), "albumName": album.name})
            }),
            title: song.name,
            artist: song.artists.into_iter().map(|artist| artist.name).collect::<Vec<_>>().join(" / "),
            artwork,
            audio_url: None,
            store_url: None,
        }
    }
}

pub(crate) fn song_details(id: u64) -> Result<Option<Track>> {
    Ok(songs_by_id(&[id])?
        .into_iter()
        .find(|song| song.id == id)
        .map(Track::from))
}

fn songs_by_id(ids: &[u64]) -> Result<Vec<Song>> {
    #[derive(Deserialize)]
    struct Response {
        code: u32,
        #[serde(default)]
        songs: Vec<Song>,
    }
    let response: Response = read_json(
        client()?
            .get(format!("{HOST}/api/song/detail/"))
            .header("Referer", format!("{HOST}/"))
            .timeout(Duration::from_secs(8))
            .query(&[("ids", serde_json::to_string(ids)?)])
            .send()?,
    )?;
    check_code(response.code)?;
    Ok(response.songs)
}

pub(super) fn search(query: &str, page: usize) -> Result<SearchPage> {
    let client = client()?;
    #[derive(Deserialize)]
    struct Response {
        code: u32,
        result: Option<SearchResult>,
    }
    #[derive(Deserialize)]
    struct SearchResult {
        #[serde(default)]
        songs: Vec<Song>,
        #[serde(rename = "songCount")]
        count: Option<usize>,
    }
    let response = client
        .post("https://music.163.com/api/search/get/web")
        .header("Referer", "https://music.163.com/")
        .form(&[
            ("s", query),
            ("type", "1"),
            (
                "offset",
                &(page - 1)
                    .saturating_mul(crate::SEARCH_PAGE_SIZE)
                    .to_string(),
            ),
            ("limit", "30"),
        ])
        .send()?;
    let response: Response = read_json(response)?;
    if response.code != 200 {
        return Err(
            io::Error::other(format!("NetEase search returned code {}", response.code)).into(),
        );
    }
    let count = response.result.as_ref().and_then(|result| result.count);
    let received = response
        .result
        .as_ref()
        .map_or(0, |result| result.songs.len());
    let tracks = response
        .result
        .into_iter()
        .flat_map(|result| result.songs)
        .take(30)
        .map(Track::from)
        .collect();
    Ok(result_page(page, received, count, tracks))
}

pub(crate) fn popular_tracks(page: usize, limit: usize) -> Result<PlaylistTracks> {
    const TOP_SONGS: u64 = 3778678;
    playlist_track_page(TOP_SONGS, page, limit)
}

pub(crate) fn community_playlists(page: usize) -> Result<PlaylistPage> {
    #[derive(Deserialize)]
    struct Response {
        code: u32,
        #[serde(default)]
        playlists: Vec<RemotePlaylist>,
        #[serde(default)]
        more: bool,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RemotePlaylist {
        id: u64,
        name: String,
        cover_img_url: Option<String>,
        creator: Option<Creator>,
    }
    #[derive(Deserialize)]
    struct Creator {
        nickname: String,
    }
    let response: Response = read_json(
        client()?
            .get(format!("{HOST}/api/playlist/list"))
            .header("Referer", format!("{HOST}/"))
            .query(&[
                ("order", "hot"),
                ("limit", &(PLAYLIST_PAGE_SIZE + 1).to_string()),
                ("offset", &playlist_offset(page).to_string()),
            ])
            .send()?,
    )?;
    check_code(response.code)?;
    let has_next = response.more || response.playlists.len() > PLAYLIST_PAGE_SIZE;
    let playlists = response
        .playlists
        .into_iter()
        .take(PLAYLIST_PAGE_SIZE)
        .map(|playlist| {
            Playlist::new(
                DiscoveryPlatform::NetEase,
                playlist.id,
                playlist.name,
                playlist
                    .creator
                    .map_or_else(String::new, |creator| creator.nickname),
                playlist.cover_img_url.as_deref(),
            )
        })
        .collect();
    Ok(PlaylistPage {
        playlists,
        has_next,
    })
}

pub(crate) fn playlist_tracks(id: u64, page: usize) -> Result<PlaylistTracks> {
    playlist_track_page(id, page, PLAYLIST_TRACKS_PER_PAGE)
}

fn playlist_track_page(id: u64, page: usize, limit: usize) -> Result<PlaylistTracks> {
    #[derive(Deserialize)]
    struct Response {
        code: u32,
        playlist: Option<Details>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Details {
        track_count: usize,
        description: Option<String>,
        #[serde(default)]
        tracks: Vec<Song>,
        #[serde(default)]
        track_ids: Vec<TrackId>,
    }
    #[derive(Deserialize)]
    struct TrackId {
        id: u64,
    }
    let response: Response = read_json(
        client()?
            .get(format!("{HOST}/api/v6/playlist/detail"))
            .header("Referer", format!("{HOST}/"))
            .query(&[
                ("id", id.to_string()),
                ("n", if page == 1 { limit } else { 0 }.to_string()),
            ])
            .send()?,
    )?;
    check_code(response.code)?;
    let playlist = response
        .playlist
        .ok_or_else(|| io::Error::other("Missing playlist data"))?;
    let offset = page.saturating_sub(1).saturating_mul(limit);
    let received = playlist.track_count.saturating_sub(offset).min(limit);
    let tracks =
        if page == 1 && (playlist.tracks.len() >= received || playlist.track_ids.is_empty()) {
            playlist
                .tracks
                .into_iter()
                .take(limit)
                .map(Track::from)
                .collect()
        } else if received == 0 {
            Vec::new()
        } else {
            let ids: Vec<_> = playlist
                .track_ids
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|track| track.id)
                .collect();
            if ids.is_empty() {
                return Err(io::Error::other(
                    "NetEase playlist does not contain track IDs for this page",
                )
                .into());
            }
            let songs: std::collections::HashMap<_, _> = songs_by_id(&ids)?
                .into_iter()
                .map(|song| (song.id, Track::from(song)))
                .collect();
            ids.into_iter()
                .filter_map(|id| songs.get(&id).cloned())
                .collect()
        };
    let mut result =
        PlaylistTracks::page(tracks, Some(playlist.track_count), offset, received, limit);
    result.description = playlist_description(playlist.description.as_deref());
    Ok(result)
}

fn check_code(code: u32) -> Result<()> {
    if code != 200 {
        return Err(io::Error::other(format!("NetEase catalog returned code {code}")).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn retains_cover_from_catalog_and_song_detail() {
        let album = json!({
            "id": 42,
            "name": "Example",
            "picUrl": "http://p1.music.126.net/example.jpg"
        });
        for song in [
            json!({"id": 8, "name": "Song", "al": album.clone()}),
            json!({"id": 8, "name": "Song", "album": album}),
        ] {
            let track = Track::from(serde_json::from_value::<Song>(song).unwrap());
            let (platform, url) = track.artwork.unwrap();
            assert_eq!(platform, DiscoveryPlatform::NetEase);
            assert_eq!(url.as_str(), "https://p1.music.126.net/example.jpg");
        }
    }
}

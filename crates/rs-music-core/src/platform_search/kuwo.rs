use super::{result_page, total};
use super::{rows, track};
use crate::discovery::{PLAYLIST_TRACKS_PER_PAGE, track_offset};
use crate::discovery::{artwork, numeric_id, playlist_description};
use crate::{
    DiscoveryPlatform, Playlist, PlaylistPage, PlaylistTracks, Result, SearchPage, Track, client,
    lx_client, read_json,
};
use serde_json::{Value, json};
use std::io;

pub(super) fn search(query: &str, page: usize) -> Result<SearchPage> {
    let client = lx_client()?;
    let response = client
        .get("http://search.kuwo.cn/r.s")
        .query(&[
            ("all", query),
            ("client", "kt"),
            ("pn", &(page - 1).to_string()),
            ("rn", "30"),
            ("ft", "music"),
            ("encoding", "utf8"),
            ("rformat", "json"),
            ("newver", "1"),
            ("vermerge", "1"),
            ("mobi", "1"),
            ("ver", "kwplayer_ar_9.2.2.1"),
        ])
        .send()?;
    let body: Value = read_json(response)?;
    if body["TOTAL"].as_str() == Some("0") {
        return Ok(result_page(page, 0, Some(0), Vec::new()));
    }
    let rows = rows(&body, "/abslist")?;
    let tracks = rows
        .iter()
        .take(30)
        .filter_map(|row| {
            let id = row["MUSICRID"].as_str()?.strip_prefix("MUSIC_")?;
            let mut track = track(
                id,
                &row["SONGNAME"],
                row["ARTIST"].as_str().unwrap_or_default().to_owned(),
                json!({"albumId": row["ALBUMID"], "albumName": row["ALBUM"]}),
                super::seconds(&row["DURATION"]),
            )?;
            track.artwork = kuwo_artwork(&[&row["ALBUMPIC"], &row["albumpic"], &row["MUSICPIC"]]);
            Some(track)
        })
        .collect();
    Ok(result_page(page, rows.len(), total(&body["TOTAL"]), tracks))
}

fn kuwo_track(row: &Value) -> Option<Track> {
    let id = numeric_id(&row["id"])?.to_string();
    let mut track = track(
        &id,
        &row["name"],
        row["artist"].as_str().unwrap_or_default().to_owned(),
        json!({"albumId": row["albumid"], "albumName": row["album"]}),
        super::seconds(if row["song_duration"].is_null() {
            &row["duration"]
        } else {
            &row["song_duration"]
        }),
    )?;
    track.artwork = kuwo_artwork(&[&row["albumpic"], &row["musicPic"]]);
    Some(track)
}

fn kuwo_artwork(fields: &[&Value]) -> Option<(DiscoveryPlatform, crate::Url)> {
    fields
        .iter()
        .find_map(|field| artwork(DiscoveryPlatform::Kuwo, field.as_str()?))
}

pub(crate) fn popular_tracks(page: usize, limit: usize) -> Result<PlaylistTracks> {
    let result: Value = read_json(
        client()?
            .get("https://kbangserver.kuwo.cn/ksong.s")
            .header("Referer", "https://www.kuwo.cn/")
            .query(&[
                ("from", "pc"),
                ("fmt", "json"),
                ("pn", &page.saturating_sub(1).to_string()),
                ("rn", &limit.to_string()),
                ("type", "bang"),
                ("data", "content"),
                ("id", "93"),
                ("encoding", "utf8"),
            ])
            .send()?,
    )?;
    let entries = rows(&result, "/musiclist")?;
    let tracks = entries.iter().filter_map(kuwo_track).take(limit).collect();
    Ok(PlaylistTracks::page(
        tracks,
        total(&result["num"]),
        page.saturating_sub(1).saturating_mul(limit),
        entries.len().min(limit),
        limit,
    ))
}

pub(crate) fn community_playlists(page: usize) -> Result<PlaylistPage> {
    let result: Value = read_json(
        client()?
            .get("https://wapi.kuwo.cn/api/pc/classify/playlist/getRcmPlayList")
            .header("Referer", "https://www.kuwo.cn/")
            .query(&[
                ("pn", page.to_string().as_str()),
                ("rn", "12"),
                ("order", "hot"),
            ])
            .send()?,
    )?;
    if result["code"].as_u64() != Some(200) {
        return Err(
            io::Error::other(format!("Kuwo playlists returned code {}", result["code"])).into(),
        );
    }
    let entries = rows(&result, "/data/data")?;
    let has_next = total(&result["data"]["total"])
        .map_or(entries.len() >= 12, |total| page.saturating_mul(12) < total);
    let playlists = entries
        .iter()
        .filter_map(|row| {
            let mut playlist = Playlist::new(
                DiscoveryPlatform::Kuwo,
                numeric_id(&row["id"])?,
                row["name"].as_str()?.to_owned(),
                row["uname"].as_str().unwrap_or_default().to_owned(),
                row["img"].as_str(),
            );
            playlist.total = total(&row["total"]);
            Some(playlist)
        })
        .collect();
    Ok(PlaylistPage {
        playlists,
        has_next,
    })
}

pub(crate) fn playlist_tracks(id: u64, page: usize) -> Result<PlaylistTracks> {
    let result: Value = read_json(
        client()?
            .get("https://nplserver.kuwo.cn/pl.svc")
            .header("Referer", "https://www.kuwo.cn/")
            .query(&[
                ("op", "getlistinfo"),
                ("pid", &id.to_string()),
                ("pn", &page.saturating_sub(1).to_string()),
                ("rn", &PLAYLIST_TRACKS_PER_PAGE.to_string()),
                ("encode", "utf8"),
                ("keyset", "pl2012"),
                ("vipver", "MUSIC_8.7.1.0_W4"),
                ("pcmp4", "1"),
            ])
            .send()?,
    )?;
    let entries = rows(&result, "/musiclist")?;
    let tracks = entries
        .iter()
        .filter_map(kuwo_track)
        .take(PLAYLIST_TRACKS_PER_PAGE)
        .collect();
    let mut page = PlaylistTracks::page(
        tracks,
        total(&result["total"]),
        track_offset(page),
        entries.len().min(PLAYLIST_TRACKS_PER_PAGE),
        PLAYLIST_TRACKS_PER_PAGE,
    );
    page.description = playlist_description(result["info"].as_str());
    Ok(page)
}

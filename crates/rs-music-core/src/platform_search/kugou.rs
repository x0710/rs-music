use super::{result_page, total};
use super::{rows, track};
use crate::discovery::{artwork, numeric_id, playlist_description};
use crate::{
    DiscoveryPlatform, Playlist, PlaylistPage, PlaylistTracks, Result, SearchPage, Track, client,
    lx_client, read_json, read_limited,
};
use serde_json::{Value, json};
use std::{io, time::Duration};

pub(super) fn search(query: &str, page: usize) -> Result<SearchPage> {
    let client = lx_client()?;
    let response = client
        .get("https://songsearch.kugou.com/song_search_v2")
        .query(&[
            ("keyword", query),
            ("platform", "AndroidFilter"),
            ("iscorrection", "1"),
            ("hifiquality", "0"),
            ("pagesize", "30"),
            ("PrivilegeFilter", "0"),
            ("page", &page.to_string()),
        ])
        .send()?;
    let body: Value = read_json(response)?;
    if body["error_code"].as_i64() != Some(0) {
        return Err(io::Error::other("Kugou search returned an error").into());
    }
    let rows = rows(&body, "/data/lists")?;
    let tracks = rows
        .iter()
        .take(30)
        .filter_map(|row| {
            let id = row["FileHash"].as_str()?;
            track(
                id,
                &row["OriSongName"],
                row["SingerName"].as_str().unwrap_or_default().to_owned(),
                json!({
                    "songmid": row["Audioid"],
                    "albumId": row["AlbumID"],
                    "albumName": row["AlbumName"],
                    "albumAudioId": row["MixSongID"],
                    "_types": {
                        "128k": {"hash": row["FileHash"]},
                        "320k": {"hash": row["HQFileHash"]},
                        "flac": {"hash": row["SQFileHash"]},
                        "flac24bit": {"hash": row["ResFileHash"]},
                    }
                }),
                super::seconds(&row["Duration"]),
            )
        })
        .collect();
    Ok(result_page(
        page,
        rows.len(),
        total(&body["data"]["total"]),
        tracks,
    ))
}

fn ranked_track(row: &Value, detail: bool) -> Option<Track> {
    let hash = row["hash"].as_str()?;
    let artist = row["singername"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .map_or_else(
            || {
                row["authors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|author| author["author_name"].as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            },
            str::to_owned,
        );
    let duration = if detail {
        numeric_id(&row["duration"])
            .filter(|&ms| ms > 0)
            .map(Duration::from_millis)
    } else {
        super::seconds(&row["duration"])
    };
    let mut track = track(
        hash,
        &row["songname"],
        artist,
        json!({
            "songmid": row["audio_id"],
            "albumId": row["album_id"],
            "albumName": row["album_name"],
            "albumAudioId": row["album_audio_id"],
            "_types": {
                "128k": {"hash":hash},
                "320k": {"hash":row[if detail {"hash_320"} else {"320hash"}]},
                "flac": {"hash":row[if detail {"hash_flac"} else {"sqhash"}]},
                "flac24bit": {"hash":row["hash_high"]},
            }
        }),
        duration,
    )?;
    track.artwork = [&row["album_img"], &row["image"], &row["imgurl"]]
        .into_iter()
        .find_map(|field| artwork(DiscoveryPlatform::Kugou, field.as_str()?));
    Some(track)
}

pub(crate) fn popular_tracks(page: usize, limit: usize) -> Result<PlaylistTracks> {
    let result: Value = read_json(
        client()?
            .get("https://m.kugou.com/rank/info/")
            .header("Referer", "https://www.kugou.com/")
            .query(&[
                ("rankid", "8888"),
                ("page", &page.to_string()),
                ("json", "true"),
            ])
            .send()?,
    )?;
    let entries = rows(&result, "/songs/list")?;
    let tracks = entries
        .iter()
        .filter_map(|song| ranked_track(song, false))
        .take(limit)
        .collect();
    Ok(PlaylistTracks::page(
        tracks,
        total(&result["songs"]["total"]),
        page.saturating_sub(1).saturating_mul(limit),
        entries.len().min(limit),
        limit,
    ))
}

pub(crate) fn community_playlists(page: usize) -> Result<PlaylistPage> {
    let result: Value = read_json(
        client()?
            .get("https://m.kugou.com/plist/index")
            .header("Referer", "https://www.kugou.com/")
            .query(&[("json", "true"), ("page", &page.to_string())])
            .send()?,
    )?;
    let entries = rows(&result, "/plist/list/info")?;
    let last_page = total(&result["plist"]["list"]["totalpage"])
        .or_else(|| total(&result["plist"]["totalpage"]));
    let has_next = last_page.map_or(entries.len() >= 12, |last| page < last);
    let playlists = entries
        .iter()
        .filter_map(|row| {
            let mut playlist = Playlist::new(
                DiscoveryPlatform::Kugou,
                numeric_id(&row["specialid"])?,
                row["specialname"].as_str()?.to_owned(),
                row["username"].as_str().unwrap_or_default().to_owned(),
                row["imgurl"].as_str(),
            );
            playlist.total = total(&row["songcount"]);
            playlist.description = playlist_description(row["intro"].as_str());
            Some(playlist)
        })
        .collect();
    Ok(PlaylistPage {
        playlists,
        has_next,
    })
}

pub(crate) fn playlist_tracks(id: u64) -> Result<Vec<Track>> {
    let bytes = read_limited(
        client()?
            .get(format!(
                "https://pcservice.kugou.com/yueku/v8/special/single/{id}-2-581.html"
            ))
            .header("Referer", "https://www.kugou.com/")
            .send()?,
        8 * 1024 * 1024,
    )?;
    let html = std::str::from_utf8(&bytes)?;
    let songs = playlist_data(html)?;
    Ok(songs
        .iter()
        .filter_map(|song| ranked_track(song, true))
        .collect())
}

fn playlist_data(html: &str) -> Result<Vec<Value>> {
    // The public desktop catalog embeds the full song list as JSON.
    let data = html
        .split("var data")
        .skip(1)
        .find_map(|data| data.trim_start().strip_prefix('='))
        .ok_or_else(|| io::Error::other("Kugou playlist has no track data"))?;
    Ok(serde_json::Deserializer::from_str(data.trim_start())
        .into_iter::<Vec<Value>>()
        .next()
        .ok_or_else(|| io::Error::other("Kugou playlist track data is empty"))??)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_playlist_assignment_with_or_without_spaces() {
        for javascript in [
            r#"var data=[{"hash":"song"}];"#,
            r#"var database=other; var data = [{"hash":"song"}];"#,
        ] {
            assert_eq!(playlist_data(javascript).unwrap()[0]["hash"], "song");
        }
    }
}

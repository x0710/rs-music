use super::{result_page, total};
use super::{rows, singers, track};
use crate::discovery::{PLAYLIST_TRACKS_PER_PAGE, track_offset};
use crate::discovery::{artwork, numeric_id, playlist_description};
use crate::{
    DiscoveryPlatform, Playlist, PlaylistTracks, Result, SearchPage, Track, client, read_json,
};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use rand::Rng;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::io;

const SERVICE: &str = "music.search.SearchCgiService";

pub(super) fn search(query: &str, page: usize) -> Result<SearchPage> {
    let guid = format!("{:032X}", rand::random::<u128>());
    let search_id = format!("{guid}{:05}", rand::rng().random_range(0..100_000));
    let body = json!({
        "comm": {
            "_channelid": "0", "_os_version": "6.2.9200-2", "ct": "19", "cv": "2151",
            "guid": guid, "patch": "118", "psrf_access_token_expiresAt": 0,
            "psrf_qqaccess_token": "", "psrf_qqopenid": "", "psrf_qqunionid": "",
            "tmeAppID": "qqmusic", "tmeLoginType": 0, "uin": "0", "wid": "7223299733393904640"
        },
        (SERVICE): {
            "module": SERVICE,
            "method": "DoSearchForQQMusicDesktop",
            "param": { "grp": 1, "num_per_page": 30, "page_num": page, "query": query,
                "remoteplace": "txt.newclient.top", "search_type": 0, "searchid": search_id }
        }
    })
    .to_string();
    let response = client()?
        .post("https://u.y.qq.com/cgi-bin/musics.fcg")
        .query(&[("sign", signature(body.as_bytes()))])
        .header("User-Agent", "QQMusic 14090508(android 12)")
        .header("Content-Type", "application/json")
        .body(body)
        .send()?;
    let body: Value = read_json(response)?;
    let result = body
        .get(SERVICE)
        .or_else(|| body.get("req"))
        .unwrap_or(&Value::Null);
    if body["code"].as_i64() != Some(0) || result["code"].as_i64() != Some(0) {
        return Err(io::Error::other(format!(
            "QQ Music search returned codes {}/{}",
            body["code"], result["code"]
        ))
        .into());
    }
    let songs = rows(result, "/data/body/song/list")?;
    let tracks = songs.iter().take(30).filter_map(qq_track).collect();
    Ok(result_page(
        page,
        songs.len(),
        total(&result["data"]["meta"]["sum"]),
        tracks,
    ))
}

fn qq_track(row: &Value) -> Option<Track> {
    let mid = row["mid"].as_str()?;
    let media_mid = row.pointer("/file/media_mid").and_then(Value::as_str)?;
    if media_mid.is_empty() {
        return None;
    }
    let mut track = track(
        mid,
        &row["title"],
        singers(&row["singer"]),
        json!({
            "songId": row["id"], "albumId": row["album"]["mid"],
            "albumMid": row["album"]["mid"], "albumName": row["album"]["name"],
            "strMediaMid": media_mid,
        }),
        super::seconds(&row["interval"]),
    )?;
    track.artwork = row["album"]["mid"]
        .as_str()
        .filter(|mid| !mid.is_empty() && mid.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .and_then(|mid| {
            artwork(
                DiscoveryPlatform::QQ,
                &format!("https://y.gtimg.cn/music/photo_new/T002R300x300M000{mid}.jpg"),
            )
        });
    Some(track)
}

fn discovery_request(module: &str, method: &str, param: Value) -> Result<Value> {
    let payload = json!({
        "comm": {"ct": 24, "cv": 0},
        "discovery": {"module": module, "method": method, "param": param},
    })
    .to_string();
    let response: Value = read_json(
        client()?
            .get("https://u.y.qq.com/cgi-bin/musicu.fcg")
            .header("Referer", "https://y.qq.com/")
            .query(&[("format", "json"), ("data", payload.as_str())])
            .send()?,
    )?;
    let result = &response["discovery"];
    let data = &result["data"];
    if response["code"].as_i64() != Some(0)
        || result["code"].as_i64() != Some(0)
        || data["code"].as_i64().is_some_and(|code| code != 0)
        || data["subcode"].as_i64().is_some_and(|code| code != 0)
    {
        return Err(io::Error::other(format!(
            "QQ Music discovery returned codes {}/{}/{}/{}",
            response["code"], result["code"], data["code"], data["subcode"]
        ))
        .into());
    }
    Ok(result["data"].clone())
}

pub(crate) fn popular_tracks(page: usize, limit: usize) -> Result<PlaylistTracks> {
    let offset = page.saturating_sub(1).saturating_mul(limit);
    let result = discovery_request(
        "musicToplist.ToplistInfoServer",
        "GetDetail",
        json!({"topid":26,"offset":offset,"num":limit}),
    )?;
    let entries = rows(&result, "/songInfoList")?;
    let tracks = entries.iter().filter_map(qq_track).take(limit).collect();
    Ok(PlaylistTracks::page(
        tracks,
        total(&result["data"]["totalNum"]),
        offset,
        entries.len().min(limit),
        limit,
    ))
}

pub(crate) fn community_playlists() -> Result<Vec<Playlist>> {
    let result = discovery_request(
        "playlist.HotRecommendServer",
        "get_hot_recommend",
        json!({"async":1,"cmd":2}),
    )?;
    let mut playlists: Vec<_> = rows(&result, "/v_hot")?
        .iter()
        .filter_map(|row| {
            Some(Playlist::new(
                DiscoveryPlatform::QQ,
                numeric_id(&row["content_id"])?,
                row["title"].as_str()?.to_owned(),
                row["username"].as_str().unwrap_or_default().to_owned(),
                row["cover"].as_str(),
            ))
        })
        .collect();
    // The hot feed returns one batch. Extend it with public playlists from the plaza.
    let plaza = crate::report_failure(
        "QQ Music",
        "playlist plaza",
        discovery_request(
            "playlist.PlayListPlazaServer",
            "get_playlist_by_category",
            json!({"id":8,"titleid":8,"curPage":1,"size":48,"order":5}),
        ),
    );
    if let Ok(plaza) = plaza {
        for row in rows(&plaza, "/v_playlist")? {
            let (Some(id), Some(title)) = (numeric_id(&row["tid"]), row["title"].as_str()) else {
                continue;
            };
            if playlists.iter().any(|playlist| playlist.id == id) {
                continue;
            }
            let mut playlist = Playlist::new(
                DiscoveryPlatform::QQ,
                id,
                title.to_owned(),
                row["creator_info"]["nick"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                row["cover_url_medium"].as_str(),
            );
            playlist.total = row["song_ids"].as_array().map(Vec::len);
            playlists.push(playlist);
        }
    }
    Ok(playlists)
}

pub(crate) fn playlist_tracks(id: u64, page: usize) -> Result<PlaylistTracks> {
    let offset = track_offset(page);
    let result = discovery_request(
        "music.srfDissInfo.DissInfo",
        "CgiGetDiss",
        json!({"disstid":id,"song_begin":offset,"song_num":PLAYLIST_TRACKS_PER_PAGE,"userinfo":1,"tag":1}),
    )?;
    let entries = rows(&result, "/songlist")?;
    let tracks = entries
        .iter()
        .filter_map(qq_track)
        .take(PLAYLIST_TRACKS_PER_PAGE)
        .collect();
    let mut page = PlaylistTracks::page(
        tracks,
        total(&result["total_song_num"]),
        offset,
        entries.len().min(PLAYLIST_TRACKS_PER_PAGE),
        PLAYLIST_TRACKS_PER_PAGE,
    );
    page.description = playlist_description(result["dirinfo"]["desc"].as_str()).map(|text| {
        text.replace("<br />", "\n")
            .replace("<br/>", "\n")
            .replace("<br>", "\n")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&amp;", "&")
    });
    Ok(page)
}

fn signature(body: &[u8]) -> String {
    const PREFIX: [usize; 8] = [23, 14, 6, 36, 16, 40, 7, 19];
    const SUFFIX: [usize; 8] = [16, 1, 32, 12, 19, 27, 8, 5];
    const MASK: [u8; 20] = [
        89, 39, 179, 150, 218, 82, 58, 252, 177, 52, 186, 123, 120, 64, 242, 133, 143, 161, 121,
        179,
    ];
    let digest = Sha1::digest(body);
    let hex = hex::encode(digest);
    // The provider's signature skips positions outside the hex digest.
    let pick = |indices: &[usize]| -> String {
        indices
            .iter()
            .filter_map(|&index| hex.as_bytes().get(index).copied())
            .map(char::from)
            .collect()
    };
    let scrambled: Vec<_> = digest
        .iter()
        .zip(MASK)
        .map(|(&value, mask)| value ^ mask)
        .collect();
    let middle = STANDARD_NO_PAD.encode(scrambled).replace(['/', '+'], "");
    format!("zzc{}{middle}{}", pick(&PREFIX), pick(&SUFFIX)).to_ascii_lowercase()
}

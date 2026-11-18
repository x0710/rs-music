use super::{result_page, total};
use super::{singers, track};
use crate::{Result, SearchPage, client, read_json};
use md5::{Digest, Md5};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn search(query: &str, page: usize) -> Result<SearchPage> {
    const DEVICE: &str = "963B7AA0D21511ED807EE5846EC87D20";
    const SIGNATURE_SALT: &str = "6cdc72a439cef99a3418d2a78aa28c73";
    const APP: &str = "yyapp2d16148780a1dcc7408e06336b98cfd50";
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .to_string();
    let signature = format!(
        "{:x}",
        Md5::digest(format!("{query}{SIGNATURE_SALT}{APP}{DEVICE}{timestamp}"))
    );
    let switches = json!({"song":1,"album":0,"singer":0,"tagSong":1,"mvSong":0,"bestShow":1,"songlist":0,"lyricSong":0}).to_string();
    let response = client()?
        .get("https://jadeite.migu.cn/music_search/v3/search/searchAll")
        .query(&[
            ("text", query),
            ("pageNo", &page.to_string()),
            ("pageSize", "30"),
            ("sort", "0"),
            ("isCorrect", "0"),
            ("isCopyright", "1"),
            ("sid", "USS"),
            ("searchSwitch", &switches),
        ])
        .header("uiVersion", "A_music_3.6.1")
        .header("deviceId", DEVICE)
        .header("timestamp", timestamp)
        .header("sign", signature)
        .header("channel", "0146921")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Linux; Android 11) AppleWebKit/534.30 Mobile Safari/534.30",
        )
        .send()?;
    let body: Value = read_json(response)?;
    if body["code"].as_str() != Some("000000") {
        return Err(io::Error::other(format!("Migu search returned code {}", body["code"])).into());
    }
    let mut seen = HashSet::new();
    let groups = body
        .pointer("/songResultData/resultList")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let tracks = groups
        .iter()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|row| {
            let id = row["copyrightId"].as_str()?;
            if row["songId"].is_null() || !seen.insert(id) {
                return None;
            }
            track(
                id,
                &row["name"],
                singers(&row["singerList"]),
                json!({
                    "songmid": row["songId"], "copyrightId": id,
                    "albumId": row["albumId"], "albumName": row["album"],
                }),
                None,
            )
        })
        .take(30)
        .collect();
    Ok(result_page(
        page,
        groups
            .iter()
            .filter_map(Value::as_array)
            .map(Vec::len)
            .sum(),
        total(&body["songResultData"]["totalCount"]),
        tracks,
    ))
}

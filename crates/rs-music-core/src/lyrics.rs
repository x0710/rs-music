use crate::{
    Result, Source, Track, client, diagnostics::ErrorContext, error_details,
    platform_search::netease as catalog, read_json, report_failure,
};
use serde::Deserialize;
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
    sync::Arc,
    time::Duration,
};

use symphonia::core::{
    formats::FormatOptions,
    io::MediaSourceStream,
    meta::{Limit, MetadataOptions, MetadataRevision, StandardVisualKey},
    probe::Hint,
};

const LOCAL_LIMIT: usize = 8 * 1024 * 1024;

pub fn local_cover(path: &Path) -> Result<Option<Vec<u8>>> {
    for extension in ["jpg", "jpeg", "png"] {
        let image = path.with_extension(extension);
        let file = match File::open(&image) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(LOCAL_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > LOCAL_LIMIT {
            return Err(format!("Cover exceeds 8 MiB ({})", image.display()).into());
        }
        return Ok(Some(bytes));
    }

    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }
    let stream = MediaSourceStream::new(Box::new(File::open(path)?), Default::default());
    let options = MetadataOptions {
        limit_metadata_bytes: Limit::Maximum(LOCAL_LIMIT),
        limit_visual_bytes: Limit::Maximum(LOCAL_LIMIT),
    };
    let mut probed = symphonia::default::get_probe().format(
        &hint,
        stream,
        &FormatOptions::default(),
        &options,
    )?;
    let mut metadata = probed.format.metadata();
    let mut tags = probed.metadata.get();
    Ok([
        metadata.skip_to_latest(),
        tags.as_mut().and_then(|tags| tags.skip_to_latest()),
    ]
    .into_iter()
    .flatten()
    .flat_map(MetadataRevision::visuals)
    .filter(|visual| {
        visual.data.len() <= LOCAL_LIMIT
            && matches!(visual.media_type.as_str(), "image/jpeg" | "image/png")
    })
    .max_by_key(|visual| visual.usage == Some(StandardVisualKey::FrontCover))
    .map(|visual| visual.data.to_vec()))
}

pub enum Lyrics {
    Timed(Vec<Line>),
    Plain(String),
}

pub struct Line {
    pub at: Duration,
    pub text: Arc<str>,
}

impl Lyrics {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim_start_matches('\u{feff}');
        let offset = text
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("[offset:")?
                    .strip_suffix(']')?
                    .trim()
                    .parse::<i64>()
                    .ok()
            })
            .unwrap_or(0);
        let mut plain = Vec::new();
        let mut timed = Vec::new();
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let mut rest = line;
            let mut stamps = Vec::new();
            while let Some((tag, tail)) =
                rest.strip_prefix('[').and_then(|rest| rest.split_once(']'))
            {
                let Some(at) = timestamp(tag) else { break };
                stamps.push(at.saturating_add_signed(offset));
                rest = tail.trim_start();
            }
            if stamps.is_empty() {
                if !(line.starts_with('[') && line.ends_with(']') && line.contains(':')) {
                    plain.push(line);
                }
            } else if !rest.is_empty() {
                let text = Arc::<str>::from(rest);
                timed.extend(stamps.into_iter().map(|ms| Line {
                    at: Duration::from_millis(ms),
                    text: Arc::clone(&text),
                }));
            }
        }
        if timed.is_empty() {
            (!plain.is_empty()).then(|| Self::Plain(plain.join("\n")))
        } else {
            timed.sort_by_key(|line| line.at);
            Some(Self::Timed(timed))
        }
    }
}

fn timestamp(tag: &str) -> Option<u64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes = minutes.parse::<u64>().ok()?;
    let (seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let seconds = seconds.parse::<u64>().ok()?;
    if seconds >= 60 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let ms = fraction
        .bytes()
        .take(3)
        .zip([100, 10, 1])
        .map(|(digit, factor)| u64::from(digit - b'0') * factor)
        .sum::<u64>();
    minutes
        .checked_mul(60_000)?
        .checked_add(seconds * 1000 + ms)
}

pub fn local(audio: &Path) -> Result<Option<Lyrics>> {
    Ok(read_lrc(&audio.with_extension("lrc"))?
        .as_deref()
        .and_then(Lyrics::parse))
}

pub fn read_lrc(path: &Path) -> Result<Option<String>> {
    const LIMIT: u64 = 1024 * 1024;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut text = String::new();
    file.take(LIMIT + 1).read_to_string(&mut text)?;
    if text.len() as u64 > LIMIT {
        return Err(io::Error::other("LRC file exceeds 1 MiB").into());
    }
    Ok(Some(text))
}

pub fn online(platform: &str, track: &Track) -> Result<Option<Lyrics>> {
    Ok(online_lrc(platform, track)?
        .as_deref()
        .and_then(Lyrics::parse))
}

pub fn online_lrc(platform: &str, track: &Track) -> Result<Option<String>> {
    let matched;
    let track = if platform == "wy" {
        Some(track)
    } else {
        matched = find_netease_track(track)?;
        matched.as_ref()
    };
    netease_id(track).map_or(Ok(None), |id| {
        report_failure("NetEase Music", "song lyrics", netease(id))
    })
}

pub fn online_cover(platform: &str, track: &Track) -> Result<Option<Vec<u8>>> {
    let cover = match &track.artwork {
        Some(cover) => Some(cover.clone()),
        None => {
            let matched;
            let track = if platform == "wy" {
                Some(track)
            } else {
                matched = find_netease_track(track)?;
                matched.as_ref()
            };
            if let Some(cover) = track.and_then(|track| track.artwork.clone()) {
                Some(cover)
            } else if let Some(id) = netease_id(track) {
                report_failure(
                    "NetEase Music",
                    "song cover metadata",
                    catalog::song_details(id),
                )?
                .and_then(|track| track.artwork)
            } else {
                None
            }
        }
    };
    cover
        .map(|(platform, url)| platform.fetch_image(url, "song cover"))
        .transpose()
}

fn netease_id(track: Option<&Track>) -> Option<u64> {
    track?.id.as_deref()?.parse().ok()
}

fn find_netease_track(track: &Track) -> Result<Option<Track>> {
    if track.title.trim().is_empty() || track.artist.trim().is_empty() {
        return Ok(None);
    }
    let search = Source::builtin_catalog("wy").expect("NetEase search is built in");
    let query = format!("{} {}", track.title, track.artist);
    let title = normalized(&track.title);
    let artist = normalized(&track.artist);
    Ok(search
        .search_page(&query, 1)?
        .tracks
        .into_iter()
        .find(|candidate| {
            normalized(&candidate.title) == title
                && normalized(&candidate.artist) == artist
                && track
                    .duration
                    .zip(candidate.duration)
                    .is_none_or(|(expected, actual)| {
                        expected.abs_diff(actual) <= Duration::from_secs(5)
                    })
        }))
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn netease(id: u64) -> Result<Option<String>> {
    #[derive(Deserialize)]
    struct Response {
        code: u32,
        lrc: Option<Lrc>,
    }
    #[derive(Deserialize)]
    struct Lrc {
        lyric: Option<String>,
    }
    let client = client()?;
    let send = || {
        client
            .get("https://music.163.com/api/song/lyric")
            .header("Referer", "https://music.163.com/")
            .query(&[("id", id.to_string()), ("lv", "-1".into())])
            .send()
    };
    let response = match send() {
        Err(error) if error.is_connect() => {
            let initial = error_details(&error);
            tracing::debug!(%id, error = %initial, "Retrying NetEase lyrics after connection failure");
            std::thread::sleep(Duration::from_millis(250));
            send().map_err(|error| {
                ErrorContext::new(
                    format!("NetEase lyrics retry failed (first attempt: {initial})"),
                    error,
                )
            })?
        }
        result => result?,
    };
    let response: Response = read_json(response)?;
    if response.code != 200 {
        return Err(
            io::Error::other(format!("NetEase lyrics returned code {}", response.code)).into(),
        );
    }
    Ok(response.lrc.and_then(|lrc| lrc.lyric))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_multiple_timestamps_and_applies_offset() {
        let Some(Lyrics::Timed(lines)) = Lyrics::parse(
            "\u{feff}[ar:Artist]\r\n[00:10.7][00:01.50]First\r\n[00:02.001]Second\n[offset:-500]",
        ) else {
            panic!("expected timed lyrics");
        };
        let values: Vec<_> = lines
            .iter()
            .map(|line| (line.at.as_millis(), line.text.as_ref()))
            .collect();
        assert_eq!(
            values,
            [(1000, "First"), (1501, "Second"), (10200, "First")]
        );
        assert!(matches!(
            Lyrics::parse("[ar:Artist]\nUnstamped line"),
            Some(Lyrics::Plain(_))
        ));
        assert!(Lyrics::parse("[ar:Artist]\n[offset:100]").is_none());
    }
}

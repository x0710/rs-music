use crate::{QueueTrack, SavedSource, SourceSelection};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum LanguagePreference {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "zh-CN")]
    Chinese,
    #[serde(rename = "en")]
    English,
}

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    #[default]
    Sequential,
    RepeatAll,
    RepeatOne,
    Shuffle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseAction {
    #[default]
    HideToTray,
    Exit,
}

fn default_zoom_factor() -> f32 {
    1.0
}

fn default_cache_cleanup_minutes() -> Option<u64> {
    Some(5)
}

fn default_volume() -> u8 {
    100
}

fn default_auto_switch_source() -> bool {
    true
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadSettings {
    pub audio: bool,
    pub lyrics: bool,
    pub directory: Option<PathBuf>,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            audio: true,
            lyrics: true,
            directory: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct State {
    #[serde(default = "default_zoom_factor")]
    pub zoom_factor: f32,
    /// 清理时间范围以分钟保存；空值表示全部时间。
    #[serde(default = "default_cache_cleanup_minutes")]
    pub cache_cleanup_minutes: Option<u64>,
    #[serde(default)]
    pub theme: ThemePreference,
    pub language: LanguagePreference,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub close_action: CloseAction,
    #[serde(default)]
    pub downloads: DownloadSettings,
    #[serde(with = "paths")]
    pub tracks: Vec<PathBuf>,
    pub queue: Vec<QueueTrack>,
    pub current: Option<QueueTrack>,
    #[serde(default)]
    pub playback_mode: PlaybackMode,
    #[serde(default = "default_volume")]
    pub volume: u8,
    #[serde(default)]
    pub sources: Vec<SavedSource>,
    #[serde(default)]
    pub source_selection: Option<SourceSelection>,
    #[serde(default = "default_auto_switch_source")]
    pub auto_switch_source: bool,
    #[serde(default)]
    pub source_successes: BTreeMap<String, u64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            zoom_factor: default_zoom_factor(),
            cache_cleanup_minutes: default_cache_cleanup_minutes(),
            theme: ThemePreference::default(),
            language: LanguagePreference::default(),
            font_family: None,
            close_action: CloseAction::default(),
            downloads: DownloadSettings::default(),
            tracks: Vec::new(),
            queue: Vec::new(),
            current: None,
            playback_mode: PlaybackMode::default(),
            volume: default_volume(),
            sources: Vec::new(),
            source_selection: None,
            auto_switch_source: default_auto_switch_source(),
            source_successes: BTreeMap::new(),
        }
    }
}

mod paths {
    use super::*;

    #[cfg(windows)]
    type PathUnit = u16;
    #[cfg(not(windows))]
    type PathUnit = u8;

    pub(super) fn serialize<S: serde::Serializer>(
        paths: &[PathBuf],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        #[cfg(windows)]
        use std::os::windows::ffi::OsStrExt;

        let paths: Vec<Vec<PathUnit>> = paths
            .iter()
            .map(|path| {
                #[cfg(windows)]
                {
                    path.as_os_str().encode_wide().collect()
                }
                #[cfg(not(windows))]
                {
                    path.as_os_str().as_encoded_bytes().to_vec()
                }
            })
            .collect();
        paths.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<PathBuf>, D::Error> {
        let paths = Vec::<Vec<PathUnit>>::deserialize(deserializer)?;
        paths
            .into_iter()
            .map(|units| {
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStringExt;
                    Ok(PathBuf::from(std::ffi::OsString::from_wide(&units)))
                }
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    Ok(PathBuf::from(std::ffi::OsString::from_vec(units)))
                }
                #[cfg(not(any(unix, windows)))]
                {
                    String::from_utf8(units)
                        .map(PathBuf::from)
                        .map_err(serde::de::Error::custom)
                }
            })
            .collect()
    }
}

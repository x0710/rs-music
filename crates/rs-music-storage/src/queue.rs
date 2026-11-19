use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

#[derive(Clone, Serialize, Deserialize)]
/// 判定同一首在线曲目时先匹配平台和音源指纹，再比较曲目 ID。双方均无 ID 时
/// 使用音频地址、标题和艺人区分。修改身份字段时需同时更新 `PartialEq` 和 `Hash`。
pub struct OnlineTrack {
    pub platform: String,
    pub track: rs_music_core::Track,
    /// 提供播放解析的 JSON 音源指纹。可用于从队列恢复该音源。
    pub source: Option<String>,
}

impl PartialEq for OnlineTrack {
    fn eq(&self, other: &Self) -> bool {
        self.platform == other.platform
            && self.source == other.source
            && match (&self.track.id, &other.track.id) {
                (Some(left), Some(right)) => left == right,
                (None, None) => {
                    self.track.audio_url == other.track.audio_url
                        && self.track.title == other.track.title
                        && self.track.artist == other.track.artist
                }
                _ => false,
            }
    }
}

impl Eq for OnlineTrack {}

impl Hash for OnlineTrack {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.platform.hash(state);
        self.source.hash(state);
        self.track.id.hash(state);
        if self.track.id.is_none() {
            self.track.audio_url.hash(state);
            self.track.title.hash(state);
            self.track.artist.hash(state);
        }
    }
}

/// Local entries retain their historical numeric index in snapshots. Online entries serialize as objects.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QueueTrack {
    Local(usize),
    Online(Box<OnlineTrack>),
}

impl From<OnlineTrack> for QueueTrack {
    fn from(track: OnlineTrack) -> Self {
        Self::Online(Box::new(track))
    }
}

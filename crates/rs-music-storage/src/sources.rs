use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSource {
    pub name: String,
    pub script: bool,
    pub contents: String,
    pub platform: Option<String>,
}

impl SavedSource {
    /// 对原始文档内容计算 SHA-256。移动文档在列表中的位置不会改变队列引用和成功次数的键。
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(self.contents.as_bytes()))
    }
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
/// 保存当前选择。`source` 与 `catalog` 均为同一快照中音源列表的下标，删除音源时需同步调整。
pub struct SourceSelection {
    pub source: Option<usize>,
    pub catalog: Option<usize>,
    pub platform: String,
    pub quality: String,
}

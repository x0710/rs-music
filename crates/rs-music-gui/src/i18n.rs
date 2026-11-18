pub(crate) use rs_music_storage::LanguagePreference;

pub(crate) const LANGUAGE_OPTIONS: [LanguagePreference; 3] = [
    LanguagePreference::System,
    LanguagePreference::Chinese,
    LanguagePreference::English,
];

pub(crate) fn preference_label(preference: LanguagePreference, language: Language) -> &'static str {
    match preference {
        LanguagePreference::System => language.text(Message::System),
        LanguagePreference::Chinese => "简体中文",
        LanguagePreference::English => "English",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Language {
    Chinese,
    English,
}

impl Language {
    pub(crate) fn resolve(preference: LanguagePreference, system: Self) -> Self {
        match preference {
            LanguagePreference::System => system,
            LanguagePreference::Chinese => Self::Chinese,
            LanguagePreference::English => Self::English,
        }
    }

    pub(crate) fn system() -> Self {
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|value| !value.is_empty())
            .unwrap_or_default();
        let locale = locale
            .split(['.', '@'])
            .next()
            .unwrap_or_default()
            .replace('_', "-");
        if ["zh", "zh-CN", "zh-Hans", "zh-Hans-CN", "zh-SG"]
            .iter()
            .any(|candidate| locale.eq_ignore_ascii_case(candidate))
        {
            Self::Chinese
        } else {
            Self::English
        }
    }

    pub(crate) fn queue_count(self, count: usize) -> String {
        match self {
            Self::Chinese => format!("队列（{count} 首）"),
            Self::English if count == 1 => "Queue (1 track)".into(),
            Self::English => format!("Queue ({count} tracks)"),
        }
    }

    pub(crate) fn playlist_track_count(self, shown: usize, total: usize) -> String {
        match self {
            Self::Chinese => format!("显示 {shown} 首，共 {total} 首"),
            Self::English => format!("Showing {shown} of {total} tracks"),
        }
    }

    pub(crate) fn playlist_queued(self, count: usize) -> String {
        match self {
            Self::Chinese if count == 0 => "没有新曲目可加入".into(),
            Self::Chinese => format!("已加入 {count} 首"),
            Self::English if count == 0 => "No new tracks to add".into(),
            Self::English if count == 1 => "Added 1 track".into(),
            Self::English => format!("Added {count} tracks"),
        }
    }

    pub(crate) fn cache_cleared(self, count: usize) -> String {
        match self {
            Self::Chinese => format!("已清理 {count} 个缓存文件"),
            Self::English if count == 1 => "Cleared 1 cache file".into(),
            Self::English => format!("Cleared {count} cache files"),
        }
    }
}

macro_rules! messages {
    ($($key:ident => $en:literal, $zh:literal;)*) => {
        #[derive(Clone, Copy)]
        pub(crate) enum Message { $($key,)* }

        impl Language {
            pub(crate) fn text(self, message: Message) -> &'static str {
                match (self, message) {
                    $((Self::English, Message::$key) => $en,
                      (Self::Chinese, Message::$key) => $zh,)*
                }
            }
        }
    };
}

messages! {
    Settings => "Settings", "设置";
    About => "About", "关于";
    AboutDescription => "An open-source desktop music player built with Rust and egui.", "使用 Rust 和 egui 开发的开源桌面音乐播放器。";
    Version => "Version", "版本";
    ReportIssue => "Report an issue", "问题反馈";
    OpenSourceLicenses => "Open-source licenses", "开源许可证";
    InspiredBy => "Inspired by", "使用体验参考";
    Interface => "Interface", "界面";
    PlaybackSettings => "Playback", "播放";
    Language => "Language", "界面语言";
    Font => "Font", "界面字体";
    SystemFont => "System default", "系统默认";
    CloseAction => "When closing the window", "关闭窗口时";
    HideToTray => "Hide to system tray", "隐藏到托盘";
    RestoreWindow => "Show window", "恢复窗口";
    ExitApplication => "Quit application", "退出应用";
    System => "Follow system", "跟随系统";
    Hall => "Music hall", "音乐大厅";
    MusicSearch => "Music search", "音乐搜索";
    PopularTracks => "Trending songs", "热门曲目";
    CommunityPlaylists => "Community playlists", "网友歌单";
    Refresh => "Refresh", "刷新";
    Back => "Back", "返回";
    BrowseOnPlatform => "Open on platform", "在平台查看";
    NoPlaylistDescription => "No playlist description", "暂无歌单介绍";
    QueuePlaylist => "Add all to queue", "全部加入队列";
    QueuePlaylistFailed => "Could not add playlist to queue", "无法将歌单加入队列";
    DiscoveryLoadFailed => "Could not load music", "加载失败";
    LoadingDiscovery => "Loading…", "正在加载…";
    Source => "Source", "音源";
    ImportSource => "Import source…", "导入音源…";
    ManageSources => "Sources", "音源管理";
    NoImportedSources => "No imported sources", "暂无导入的音源";
    AutoSwitchSource => "Automatically switch sources", "自动换源";
    DeleteSource => "Delete selected source", "删除所选音源";
    ActiveSource => "Active", "使用中";
    ActiveCatalog => "Search catalog in use", "用于搜索";
    AutoSwitchedSource => "Switched source automatically", "已自动换源";
    SourceFailed => "Could not import source", "无法导入音源";
    Platform => "Platform", "平台";
    Quality => "Quality", "音质";
    DefaultCatalog => "Default", "默认";
    PlatformQQ => "QQ Music", "QQ 音乐";
    PlatformKugou => "Kugou", "酷狗音乐";
    PlatformKuwo => "Kuwo", "酷我音乐";
    PlatformMigu => "Migu", "咪咕音乐";
    PlatformNetEase => "NetEase", "网易云音乐";
    SearchCatalog => "Search catalog", "搜索音源";
    NoSearchCatalog => "None selected", "未选择";
    ImportCatalog => "Import search catalog…", "导入搜索配置…";
    ImportSearchCatalog => "Keyword search is unavailable for this platform.", "此平台暂无内置搜索。";
    OnlineQuery => "Song or artist", "歌曲或歌手";
    SearchOnline => "Search", "搜索";
    ClearSearchHistory => "Clear history", "清空历史";
    SearchHistoryLoadFailed => "Could not load search history", "无法读取搜索历史";
    SearchHistorySaveFailed => "Could not save search history", "无法保存搜索历史";
    Searching => "Searching…", "正在搜索…";
    SearchFailed => "Search failed", "搜索失败";
    LoadingAudio => "Loading audio…", "正在加载音频…";
    MissingPlaybackId => "This result has no platform track ID", "这首歌缺少平台曲目 ID";
    NoCompatiblePlaybackSource => "No compatible playback source for this result", "这首歌暂无可用的播放音源";
    Subtitles => "Subtitles", "字幕";
    SubtitleMode => "Subtitle mode", "字幕模式";
    LoadingLyrics => "Loading lyrics…", "正在加载歌词…";
    NoLyrics => "No lyrics available", "暂无歌词";
    LyricsLoadFailed => "Could not load lyrics", "歌词加载失败";
    Buffering => "Buffering…", "正在缓冲…";
    Buffered => "Buffered", "已缓冲";
    SaveToMusic => "Save to Music", "保存到本地";
    SkipSave => "Don't save", "不保存";
    SaveRequested => "Save requested", "已提交保存";
    SaveSkipped => "Saving skipped", "本次不保存";
    ArchiveFailed => "Could not save track to Music", "无法将歌曲保存到音乐目录";
    LyricsArchiveFailed => "Could not save lyrics", "无法保存歌词";
    Downloads => "Downloads", "下载";
    Cache => "Cache", "缓存";
    ClearCache => "Clear cache", "清理缓存";
    CacheClearFailed => "Could not clear cache", "清理缓存失败";
    CacheFiveMinutes => "Last 5 minutes", "最近 5 分钟";
    CacheTenMinutes => "Last 10 minutes", "最近 10 分钟";
    CacheThirtyMinutes => "Last 30 minutes", "最近 30 分钟";
    CacheOneHour => "Last hour", "最近 1 小时";
    CacheOneDay => "Last 24 hours", "最近 24 小时";
    CacheAllTime => "All time", "全部时间";
    DownloadAudio => "Save songs automatically", "自动下载歌曲";
    DownloadLyrics => "Save lyrics automatically", "自动下载歌词";
    DownloadDirectory => "Download directory", "下载目录";
    ChooseDirectory => "Choose directory…", "选择目录…";
    DefaultDirectory => "Restore default", "恢复默认";
    ViewOnITunes => "View on iTunes", "在 iTunes 查看";
    RemoveFromLibrary => "Remove from library", "从曲库移除";
    Library => "Library", "曲库";
    Queue => "Queue", "队列";
    ImportFiles => "Import files…", "导入文件…";
    ImportFolder => "Import folder…", "导入文件夹…";
    AudioFiles => "Audio files", "音频文件";
    Scanning => "Scanning for music…", "正在查找音乐文件…";
    Search => "Search file names", "搜索文件名";
    EmptyQueue => "Your queue is empty.", "队列为空。";
    EmptyLibrary => "Import or drop audio files and folders here.", "导入音频文件或文件夹，也可拖放到这里。";
    NoResults => "No matching tracks.", "没有匹配的歌曲。";
    Play => "Play", "播放";
    Pause => "Pause", "暂停";
    AddToQueue => "+ Queue", "加入队列";
    Remove => "Remove", "移除";
    PlaybackMode => "Playback mode", "播放模式";
    Sequential => "In order", "顺序播放";
    RepeatAll => "Repeat queue", "列表循环";
    RepeatOne => "Repeat one", "单曲循环";
    Shuffle => "Shuffle", "随机播放";
    Volume => "Volume", "音量";
    PreviousPage => "Previous page", "上一页";
    NextPage => "Next page", "下一页";
    Previous => "Previous", "上一首";
    Next => "Next", "下一首";
    NoTrack => "No track selected", "未选择歌曲";
    PlaybackFailed => "Playback failed", "播放失败";
    StorageUnavailable => "Saved data unavailable", "无法读取已保存的数据";
    SaveFailed => "Could not save changes", "无法保存更改";
    SeekFailed => "Could not seek", "无法调整播放进度";
}

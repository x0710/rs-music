# 模块与维护指南

程序从 [main.rs](../src/main.rs) 启动 GUI。GUI 组合核心层和存储层，存储层复用核心层的曲目类型。平台响应先在核心层转换为统一的 `Track`，因此调整某个平台的字段映射通常只涉及该平台的适配器。

## 按改动定位代码

| 要修改的行为 | 代码入口 | 相邻模块的分工 |
| --- | --- | --- |
| 某个平台的曲目字段或分页结果 | [平台适配器](../crates/rs-music-core/src/platform_search) | [source.rs](../crates/rs-music-core/src/source.rs) 负责按音源分派搜索和播放请求 |
| HTTP 请求与响应限制 | [http.rs](../crates/rs-music-core/src/http.rs) | [audio_stream.rs](../crates/rs-music-core/src/audio_stream.rs) 在读取音频时执行流大小限制 |
| LX 脚本调用网络或切换备用源 | [request.rs](../crates/rs-music-core/src/lx/request.rs)、[fallback.rs](../crates/rs-music-core/src/lx/fallback.rs) | [lx.rs](../crates/rs-music-core/src/lx.rs) 管理脚本加载和执行 |
| 搜索控件、音源选择 | [view.rs](../crates/rs-music-gui/src/app/pages/search/view.rs)、[settings.rs](../crates/rs-music-gui/src/app/pages/search/settings.rs) | [sources.rs](../crates/rs-music-gui/src/app/pages/search/sources.rs) 应用音源选择、导入和删除 |
| 在线曲目的播放和歌词保存 | [playback.rs](../crates/rs-music-gui/src/app/pages/search/playback.rs)、[lyrics.rs](../crates/rs-music-gui/src/app/pages/search/lyrics.rs) | [online.rs](../crates/rs-music-gui/src/audio/online.rs) 处理音频缓冲和解码 |
| 歌单封面、整张歌单入队 | [covers.rs](../crates/rs-music-gui/src/app/pages/discovery/covers.rs)、[queue_import.rs](../crates/rs-music-gui/src/app/pages/discovery/queue_import.rs) | [discovery.rs](../crates/rs-music-gui/src/app/pages/discovery.rs) 管理页面状态与导航 |
| 新增设置或修改队列的保存格式 | [state.rs](../crates/rs-music-storage/src/state.rs)、[queue.rs](../crates/rs-music-storage/src/queue.rs) | [store.rs](../crates/rs-music-storage/src/store.rs) 管理文件锁、快照版本和写盘 |

## 从提交搜索到开始播放

1. 搜索框保留正在编辑的文本。提交时 [Hall](../crates/rs-music-gui/src/app/pages/search.rs) 记录本次查询，翻页沿用该查询。搜索在工作线程执行，结果通过通道返回，工作线程请求重绘，页面在下一次轮询时接收结果。[后台任务](../crates/rs-music-gui/src/app/tasks.rs)
2. 选择在线曲目时，播放协调代码确定平台、音质、缓存标识和候选音源。GUI 根据本地成功记录排列备用源，核心层按传入顺序尝试支持该平台的源。只有调用方成功打开音频流才算本次尝试成功。[播放协调](../crates/rs-music-gui/src/app/pages/search/playback.rs) · [LX 回退](../crates/rs-music-core/src/lx/fallback.rs)
3. 歌词可以先于音频准备好。歌词任务把解析结果交给显示层，另一个线程等待本次播放的保存许可。发送端被丢弃时，等待结束并跳过写盘。[歌词任务](../crates/rs-music-gui/src/app/pages/search/lyrics.rs)

## 修改持久化格式

`state.json` 使用带版本号的快照。给现有版本增加字段时，检查缺少该字段的旧文件应恢复为何值，并在 Serde 定义中表达这个默认值。队列里的整数表示本地曲目索引，在线条目保存为对象；两者可以混排。搜索历史单独写入 `search-history.json`。[状态定义](../crates/rs-music-storage/src/state.rs) · [兼容性测试](../crates/rs-music-storage/src/store.rs)

改动后可运行 `cargo test -p rs-music-storage` 验证旧快照、混合队列和搜索历史，运行 `cargo test -p rs-music-gui app::pages::search` 验证音源恢复、回退和输入法交互。跨层改动使用 `cargo test --workspace`。

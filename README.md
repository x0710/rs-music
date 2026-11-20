# rs-music

[简体中文](README_CN.md) | English

rs-music is a native desktop music player written in Rust with egui and eframe. Its workflow is inspired by [LX Music](https://github.com/lyswhut/lx-music-desktop).

## Quick start

Install stable Rust. On Linux, install the ALSA development libraries and `pkg-config` before building.

```sh
cargo run --release
```

On Linux, rs-music uses native Wayland by default. Set `RS_MUSIC_WINDOW_BACKEND=x11` for an X11-only session.

On Linux, Fontconfig finds installed system fonts and Chinese glyph fallbacks. Choose an interface font in Settings under Interface. The selection is saved across restarts.

On Linux, closing the window keeps playback running in the system tray by default. Use the tray menu to restore the window or quit. The close action can be changed under Settings → Interface. If a tray host such as the Waybar tray module is unavailable, closing the window quits.

On Linux, media keys and MPRIS clients can control playback in the window or the tray. With `playerctl`, use `playerctl -p rs_music play-pause`, `playerctl -p rs_music next`, or `playerctl -p rs_music stop`.

## Using rs-music

| Page | Features |
| --- | --- |
| Library | Import files, scan folders recursively, or drag and drop music. Filter by file name. Double-click a track to queue and play it. Click Play to listen without changing the queue. |
| Queue | Manage local and online tracks in one queue. Playback modes are available in Settings. |
| Music hall | Browse charts and playlists from NetEase, QQ Music, Kugou, and Kuwo. Open a playlist to add all its tracks to the queue or visit the original playlist. |
| Music search | Search by song or artist and press Enter to submit. Choose a platform and quality, or reuse recent searches above the search box. |
| Settings | Change the interface language and playback mode. Import, select, and remove sources, manage search catalogs, and configure automatic source switching. |

The player bar shows playback and buffering progress separately. It displays a buffered percentage when the total size is known and bytes received otherwise. Online playback starts as data arrives. Seeking becomes available when the download is complete and the duration is known. With automatic source switching enabled, a failed attempt tries compatible LX sources in descending order of successful downloads. Ties keep the existing order. A complete download counts toward the source that supplied it. Local cache hits leave the count unchanged. The player bar identifies the source when the switch succeeds.

### Lyrics

The player bar shows the cover and the current timed lyric beside the song title. Click the cover to view the full lyrics in large type. Choose **Subtitles** for a transparent, always-on-top window with two alternating rows. The upcoming lyric stays in its row as its karaoke sweep begins. Drag the subtitle window with the left mouse button. Under **Settings → Interface**, choose light, dark, or system appearance and adjust zoom, or click the percentage to reset it. Timed lyrics highlight and scroll with playback. When seeking is available, click a timed line to jump to it.

- For local music, place a same-name UTF-8 `.lrc` file beside the audio file. For example, `Song.flac` uses `Song.lrc`.
- For NetEase tracks, lyrics are fetched by track ID. For other online platforms, the app tries to match the song title and artist to a NetEase recording.
- Online tracks use their platform artwork when available, with a NetEase lookup for missing artwork. Local tracks support same-name `.jpg`, `.jpeg`, or `.png` files and embedded JPEG or PNG covers. The player generates default artwork when a cover is unavailable.

### Online sources

In Settings → Source management, paste a direct HTTP(S) link to an LX script, LX export, or JSON source and choose **Import from URL**. The player saves the source contents locally for future sessions.

Music search uses iTunes by default and can open matching store pages. To play inside the app, go to **Settings → Sources → Import source…** and load an LX script or a JSON configuration that provides playback URLs.

LX supports UTF-8 `.js` files up to 2 MiB and `.lx.json` exports. Built-in search covers Kugou (`kg`), Kuwo (`kw`), Migu (`mg`), QQ Music (`tx`), and NetEase (`wy`). Playback availability and quality depend on the selected script and its upstream service. You can also import a JSON search catalog for the current platform.

The following JSON example uses placeholder URLs.

```json
{
  "name": "My music catalog",
  "search_url": "https://music.example.com/search",
  "query_parameter": "q",
  "results": "/tracks",
  "title": "/title",
  "artist": "/artist",
  "audio_url": "/stream"
}
```

Search sends a GET request. `results` points to the result array. Other JSON pointers are relative to each result. If the search returns IDs, set `"track_id": "/id"` and `"playback": {"url": "https://music.example.com/play", "id_parameter": "id", "audio_url": "/url"}` to resolve the audio separately. JSON sources use HTTPS audio URLs. Each online audio response is limited to 40 MiB.

## Cache and automatic saving

- **Audio cache**. A complete online audio download of at least 20 seconds is cached on disk for 14 days. Replaying uses a valid cache entry or saved audio first and can skip source resolution and the audio request.
- **Automatically save audio and lyrics**. Settings → Downloads has independent controls for audio and lyrics, both enabled by default. The default destination is the system music directory, usually `~/Music`. Lyrics are fetched alongside audio and saved as a matching `.lrc` file in the same directory. Once buffering completes, choose Save to Music or Don’t save in the player bar. With no choice, the automatic song download setting applies when playback ends naturally.
- Tracks shorter than 20 seconds skip automatic saving. Use **Save to Music** after buffering to keep a short track and its lyrics if lyric saving is enabled.
- **LX initialization cache**. Cacheable initialization responses have no expiration and can be reused across restarts. Track playback URLs are resolved as needed.
- **Clear cache**. Settings → Cache removes cached audio, music hall data, and LX initialization responses by last write time. Select the last 5, 10, or 30 minutes, the last hour, the last 24 hours, or all time.

Linux defaults are listed below. XDG directory overrides are respected.

| Data | Location |
| --- | --- |
| Settings, sources, library and queue | `~/.local/share/rs-music/state.json` |
| Search history | `~/.local/share/rs-music/search-history.json` |
| Audio cache | `~/.cache/rs-music/audio/` |
| LX initialization cache | `~/.cache/rs-music/lx-init-v1/` |
| Music hall cache | `~/.cache/rs-music/discovery-v1/` |

Popular tracks, community playlists, and the tracks inside each playlist have their own page controls. Open a playlist to see its cover, description when available, and platform link. Chart pages, playlist pages, and playlist details are cached for 30 minutes. Playlist covers are cached for 7 days. Refresh reloads the visible chart and community playlist pages or the open playlist details.

## Terminal logs

Logs go to standard error. Network errors include the operation, destination host, and available causes such as DNS failures, timeouts, or TLS certificate errors. Unexpected responses include the HTTP status or parsing location. Request paths, query parameters, and credentials are hidden. Use the following command to collect debug output.

```sh
RUST_LOG=warn,rs_music_gui=debug,rs_music_core=debug cargo run 2>rs-music.log
```

## Development

See the [module and maintenance guide (Chinese)](docs/architecture.md) for a source map, the online playback flow, and snapshot compatibility.

- `crates/rs-music-core` handles platform catalogs, LX scripts, audio requests, and lyric parsing.
- `crates/rs-music-gui` handles the native window, pages, playback, and lyric synchronization.
- `crates/rs-music-storage` handles settings, search history, audio caching, and saving fully downloaded tracks.

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo build --workspace
```

Planned features include custom playlists, media keys, and word-by-word lyrics.

## License

You may use, modify, and redistribute rs-music under the [GNU General Public License, version 3](LICENSE), or, at your option, any later version published by the Free Software Foundation (`GPL-3.0-or-later`). Third-party dependencies and imported music source scripts are subject to their respective licenses.

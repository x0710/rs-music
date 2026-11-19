use super::{MusicApp, PlaybackMode, QueueTrack};
use eframe::egui;
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::{Arc, Mutex},
    time::Duration,
};
use zbus::{
    blocking::{Connection, connection::Builder},
    fdo,
    object_server::SignalEmitter,
    zvariant::{ObjectPath, Value},
};

const SERVICE: &str = "org.mpris.MediaPlayer2.rs_music";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
type Properties = HashMap<&'static str, Value<'static>>;
type Metadata = HashMap<String, Value<'static>>;

pub(crate) struct MediaControl {
    connection: Connection,
    previous: Properties,
}

impl MediaControl {
    pub(crate) fn start(
        app: Arc<Mutex<Option<MusicApp>>>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> zbus::Result<Self> {
        let connection = Builder::session()?
            .name(SERVICE)?
            .serve_at(PATH, MediaPlayer)?
            .serve_at(
                PATH,
                Player {
                    app,
                    context: egui::Context::default(),
                    repaint: Box::new(repaint),
                },
            )?
            .build()?;
        Ok(Self {
            connection,
            previous: Properties::new(),
        })
    }

    pub(crate) fn update(
        &mut self,
        properties: Properties,
        seeked: Option<i64>,
    ) -> zbus::Result<bool> {
        let changes: Properties = properties
            .iter()
            .filter(|&(name, value)| self.previous.get(name) != Some(value))
            .map(|(&name, value)| (name, value.clone()))
            .collect();
        self.previous = properties;
        if !changes.is_empty() {
            self.connection.emit_signal(
                None::<&str>,
                PATH,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &(PLAYER_INTERFACE, &changes, Vec::<&str>::new()),
            )?;
        }
        if let Some(position) = seeked {
            self.connection.emit_signal(
                None::<&str>,
                PATH,
                PLAYER_INTERFACE,
                "Seeked",
                &(position,),
            )?;
        }
        Ok(!changes.is_empty() || seeked.is_some())
    }
}

pub(crate) fn snapshot(app: &mut MusicApp) -> (Properties, Option<i64>) {
    let properties = Properties::from([
        ("PlaybackStatus", Value::from(status(app))),
        ("LoopStatus", Value::from(loop_status(app))),
        (
            "Shuffle",
            Value::from(app.playback_mode == PlaybackMode::Shuffle),
        ),
        ("Metadata", Value::new(metadata(app))),
        ("Volume", Value::from(f64::from(app.volume) / 100.0)),
        ("CanGoNext", Value::from(app.can_go_next())),
        ("CanGoPrevious", Value::from(app.previous_track().is_some())),
        ("CanPlay", Value::from(app.has_playback_selection())),
        ("CanPause", Value::from(app.has_playback_selection())),
        ("CanSeek", Value::from(can_seek(app))),
    ]);
    (properties, app.seeked.take().map(micros))
}

struct MediaPlayer;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl MediaPlayer {
    fn raise(&self) {}
    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "rs-music"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    app: Arc<Mutex<Option<MusicApp>>>,
    context: egui::Context,
    repaint: Box<dyn Fn() + Send + Sync>,
}

impl Player {
    fn read<T>(&self, read: impl FnOnce(&MusicApp) -> T) -> Option<T> {
        self.app.lock().unwrap().as_ref().map(read)
    }

    fn change<T>(&self, change: impl FnOnce(&mut MusicApp) -> T) -> Option<T> {
        let result = self.app.lock().unwrap().as_mut().map(change);
        if result.is_some() {
            (self.repaint)();
        }
        result
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        self.change(|app| {
            if let Some(entry) = app.next_track(false).cloned() {
                skip(app, entry, &self.context);
            }
        });
    }

    fn previous(&self) {
        self.change(|app| {
            if let Some(entry) = app.previous_track().cloned() {
                skip(app, entry, &self.context);
            }
        });
    }

    fn pause(&self) {
        self.change(MusicApp::pause_playback);
    }

    fn play_pause(&self) {
        self.change(|app| app.toggle_playback(&self.context));
    }

    fn stop(&self) {
        self.change(MusicApp::stop_playback);
    }

    fn play(&self) {
        self.change(|app| app.resume_playback(&self.context));
    }

    async fn seek(
        &self,
        offset: i64,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let position = self
            .change(|app| {
                if can_seek(app)
                    && let Some(duration) = app.audio.duration()
                {
                    let position = micros(app.audio.position()).saturating_add(offset).max(0);
                    if position > micros(duration) {
                        if let Some(entry) = app.next_track(false).cloned() {
                            skip(app, entry, &self.context);
                        }
                    } else if app.seek(Duration::from_micros(position as u64)) {
                        return app.seeked.take().map(micros);
                    }
                }
                None
            })
            .flatten();
        if let Some(position) = position {
            Self::seeked(&emitter, position).await?;
        }
        Ok(())
    }

    async fn set_position(
        &self,
        track_id: ObjectPath<'_>,
        position: i64,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let position = self
            .change(|app| {
                if can_seek(app)
                    && position >= 0
                    && app
                        .current
                        .as_ref()
                        .is_some_and(|entry| id(entry) == track_id)
                    && app
                        .audio
                        .duration()
                        .is_some_and(|duration| position <= micros(duration))
                    && app.seek(Duration::from_micros(position as u64))
                {
                    return app.seeked.take().map(micros);
                }
                None
            })
            .flatten();
        if let Some(position) = position {
            Self::seeked(&emitter, position).await?;
        }
        Ok(())
    }

    fn open_uri(&self, _uri: &str) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported(
            "Opening URIs through MPRIS is unavailable".into(),
        ))
    }

    #[zbus(property)]
    fn playback_status(&self) -> &str {
        self.read(status).unwrap_or("Stopped")
    }

    #[zbus(property)]
    fn loop_status(&self) -> &str {
        self.read(loop_status).unwrap_or("None")
    }

    #[zbus(property)]
    fn set_loop_status(&self, value: &str) -> fdo::Result<()> {
        let mode = match value {
            "None" => PlaybackMode::Sequential,
            "Track" => PlaybackMode::RepeatOne,
            "Playlist" => PlaybackMode::RepeatAll,
            _ => return Err(fdo::Error::InvalidArgs("Invalid loop status".into())),
        };
        self.change(|app| {
            if app.playback_mode != PlaybackMode::Shuffle || mode != PlaybackMode::Sequential {
                app.dirty |= app.playback_mode != mode;
                app.playback_mode = mode;
            }
        });
        Ok(())
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.read(|app| app.playback_mode == PlaybackMode::Shuffle)
            .unwrap_or(false)
    }

    #[zbus(property)]
    fn set_shuffle(&self, value: bool) {
        self.change(|app| {
            if value != (app.playback_mode == PlaybackMode::Shuffle) {
                app.playback_mode = if value {
                    PlaybackMode::Shuffle
                } else {
                    PlaybackMode::Sequential
                };
                app.dirty = true;
            }
        });
    }

    #[zbus(property)]
    fn metadata(&self) -> Metadata {
        self.read(metadata).unwrap_or_default()
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.read(|app| f64::from(app.volume) / 100.0)
            .unwrap_or(1.0)
    }

    #[zbus(property)]
    fn set_volume(&self, value: f64) -> fdo::Result<()> {
        if !value.is_finite() {
            return Err(fdo::Error::InvalidArgs("Invalid volume".into()));
        }
        self.change(|app| app.set_volume((value.clamp(0.0, 1.0) * 100.0).round() as u8));
        Ok(())
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.read(|app| micros(app.audio.position())).unwrap_or(0)
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn set_rate(&self, value: f64) -> fdo::Result<()> {
        match value {
            1.0 => Ok(()),
            0.0 => {
                self.pause();
                Ok(())
            }
            _ => Err(fdo::Error::NotSupported("Playback rate must be 1.0".into())),
        }
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.read(MusicApp::can_go_next).unwrap_or(false)
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.read(|app| app.previous_track().is_some())
            .unwrap_or(false)
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.read(|app| app.has_playback_selection())
            .unwrap_or(false)
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.can_play()
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.read(can_seek).unwrap_or(false)
    }
    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }

    #[zbus(signal)]
    async fn seeked(context: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;
}

fn skip(app: &mut MusicApp, entry: QueueTrack, ctx: &egui::Context) {
    match status(app) {
        "Playing" => app.play(entry, ctx),
        "Paused" => app.start_playback(entry, true, ctx),
        _ => app.select_track(entry),
    }
}

fn status(app: &MusicApp) -> &'static str {
    match app.audio.sink() {
        Some(sink) if sink.is_paused() => "Paused",
        Some(sink) if !sink.empty() => "Playing",
        _ if app.hall.download.is_some() => {
            if app.pause_on_load {
                "Paused"
            } else {
                "Playing"
            }
        }
        _ => "Stopped",
    }
}

fn loop_status(app: &MusicApp) -> &'static str {
    match app.playback_mode {
        PlaybackMode::RepeatOne => "Track",
        PlaybackMode::RepeatAll => "Playlist",
        _ => "None",
    }
}

fn can_seek(app: &MusicApp) -> bool {
    app.audio.sink().is_some() && app.audio.can_seek() && app.audio.duration().is_some()
}

fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

fn id(entry: &QueueTrack) -> ObjectPath<'static> {
    let mut hash = DefaultHasher::new();
    entry.hash(&mut hash);
    ObjectPath::try_from(format!(
        "/org/mpris/MediaPlayer2/Track_{:016x}",
        hash.finish()
    ))
    .expect("generated track ID is a D-Bus object path")
}

fn metadata(app: &MusicApp) -> Metadata {
    let Some(entry) = &app.current else {
        return Metadata::new();
    };
    let mut data = Metadata::from([
        ("mpris:trackid".into(), Value::from(id(entry))),
        ("xesam:title".into(), Value::from(app.playback_title())),
    ]);
    let mut duration = app.audio.duration();
    if let QueueTrack::Online(online) = entry {
        if !online.track.artist.is_empty() {
            data.insert(
                "xesam:artist".into(),
                Value::new(vec![online.track.artist.clone()]),
            );
        }
        duration = duration.or(online.track.duration);
    }
    if let Some(duration) = duration {
        data.insert("mpris:length".into(), Value::from(micros(duration)));
    }
    data
}

use rodio::{Decoder, Source, source::UniformSourceIterator};
use rs_music_core::AudioStream;
use rs_music_storage::CachedTrack;
use std::{
    io::{self, Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
        mpsc,
    },
    thread,
    time::Duration,
};

type Samples = std::vec::IntoIter<f32>;
type CompletedDownload = Box<dyn FnOnce(&Arc<Download>) + Send>;
const MIN_PERSISTED_DURATION: Duration = Duration::from_secs(20);

pub(super) type OnlineDecoder = Decoder<DownloadReader>;

#[derive(Clone, Copy)]
pub(crate) struct DownloadProgress {
    pub received: u64,
    pub total: Option<u64>,
    pub complete: bool,
}

impl DownloadProgress {
    pub(crate) fn fraction(self) -> Option<f32> {
        if self.complete {
            return Some(1.0);
        }
        let total = self.total.filter(|&total| total > 0)?;
        Some((self.received as f32 / total as f32).min(1.0))
    }
}

#[derive(Clone, Default)]
pub(crate) struct DownloadMonitor(Arc<Mutex<Weak<Download>>>);

impl DownloadMonitor {
    pub(crate) fn progress(&self) -> Option<DownloadProgress> {
        let download = self.0.lock().unwrap().upgrade()?;
        download.progress()
    }
}

pub(crate) struct OnlineAudio {
    status: OnlineStatus,
    track: Option<CachedTrack>,
    lyrics_permission: Option<mpsc::Sender<()>>,
    ready: mpsc::Receiver<Vec<f32>>,
    samples: Samples,
    played: u64,
    silence: usize,
    duration: Option<Duration>,
    ended: bool,
}

impl OnlineAudio {
    pub(crate) fn on_download_complete(&self, callback: impl FnOnce() + Send + 'static) {
        self.status
            .download
            .on_complete(Box::new(move |_| callback()));
    }

    pub(crate) fn enable_lyrics_archive(&mut self, permission: mpsc::Sender<()>) {
        self.lyrics_permission = Some(permission.clone());
        self.status.download.on_complete(Box::new(move |download| {
            if download.eligible_for_persistence() {
                let _ = permission.send(());
            }
        }));
    }

    pub(crate) fn open(
        stream: AudioStream,
        track: Option<CachedTrack>,
        monitor: &DownloadMonitor,
    ) -> Result<Self, String> {
        let length = stream.content_length();
        let mut audio = Self::start(stream, length, monitor)?;
        if let Some(track) = track.clone() {
            audio.status.download.on_complete(Box::new(move |download| {
                if download.eligible_for_persistence()
                    && let Err(error) = track.cache(&download.bytes())
                {
                    tracing::warn!(error = %rs_music_core::error_details(&error), "Could not cache online audio");
                }
            }));
        }
        audio.track = track;
        Ok(audio)
    }

    pub(crate) fn open_cached(track: &CachedTrack, monitor: &DownloadMonitor) -> io::Result<Self> {
        let file = track.open()?;
        let length = file.metadata()?.len();
        if length > AudioStream::MAX_BYTES {
            return Err(io::Error::other(format!(
                "Cached audio exceeds 40 MiB ({length} bytes)"
            )));
        }
        let mut audio = Self::start(file, Some(length), monitor).map_err(io::Error::other)?;
        audio.track = Some(track.clone());
        Ok(audio)
    }

    pub(super) fn archive(&mut self) -> Option<OnlineArchive> {
        Some(OnlineArchive {
            track: self.track.take()?,
            download: Arc::clone(&self.status.download),
            lyrics_permission: self.lyrics_permission.take(),
        })
    }

    fn start(
        reader: impl Read + Send + 'static,
        length: Option<u64>,
        monitor: &DownloadMonitor,
    ) -> Result<Self, String> {
        let download = Download::start(reader, length);
        *monitor.0.lock().unwrap() = Arc::downgrade(&download);
        match Self::decode(Arc::clone(&download)) {
            Ok(source) => Ok(source),
            Err(error) => {
                download.cancel();
                Err(download.error().unwrap_or(error))
            }
        }
    }

    fn decode(download: Arc<Download>) -> Result<Self, String> {
        let decoder = match download.decoder(false) {
            Ok(decoder) => decoder,
            Err(_) if download.is_mp4() => {
                // MP4 files with their metadata after the media data require random access.
                download.wait_for_end()?;
                download.decoder(true)?
            }
            Err(error) => return Err(error),
        };
        let channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        let duration = decoder.total_duration();
        let mut decoder = UniformSourceIterator::new(decoder, channels, sample_rate);
        // Prebuffer roughly 80 ms and keep a bounded queue of decoded audio.
        let block = (sample_rate as usize / 12).max(1) * channels as usize;
        let first: Vec<_> = decoder.by_ref().take(block).collect();
        if first.is_empty() {
            return Err("Audio stream contains no decodable samples".into());
        }
        if duration.is_none() {
            let download = Arc::clone(&download);
            thread::spawn(move || {
                if download.wait_for_end().is_ok() {
                    match download.decoder(true) {
                        Ok(decoder) => {
                            download.data.lock().unwrap().duration = decoder.total_duration()
                        }
                        Err(error) => {
                            tracing::warn!(%error, "Could not read duration from downloaded audio")
                        }
                    }
                }
            });
        }
        let (sender, ready) = mpsc::sync_channel(8);
        thread::spawn(move || {
            loop {
                let samples: Vec<_> = decoder.by_ref().take(block).collect();
                if samples.is_empty() || sender.send(samples).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            status: OnlineStatus {
                download,
                played: Arc::new(AtomicU64::new(0)),
                buffering: Arc::new(AtomicBool::new(false)),
                channels,
                sample_rate,
            },
            track: None,
            lyrics_permission: None,
            ready,
            samples: first.into_iter(),
            played: 0,
            silence: 0,
            duration,
            ended: false,
        })
    }

    pub(super) fn status(&self) -> OnlineStatus {
        self.status.clone()
    }

    fn next_sample(&mut self) -> Option<f32> {
        let sample = self.samples.next()?;
        self.played += 1;
        if self.samples.len() % 256 == 0 {
            self.status.played.store(self.played, Relaxed);
        }
        Some(sample)
    }
}

pub(crate) struct OnlineArchive {
    track: CachedTrack,
    download: Arc<Download>,
    lyrics_permission: Option<mpsc::Sender<()>>,
}

impl OnlineArchive {
    pub(crate) fn save(self, automatic: bool) -> Result<Option<PathBuf>, String> {
        self.download.wait_for_end()?;
        if automatic && !self.download.eligible_for_persistence() {
            return Ok(None);
        }
        let path = self
            .track
            .archive(&self.download.bytes())
            .map_err(|error| rs_music_core::error_details(&error))?;
        if let Some(permission) = self.lyrics_permission {
            let _ = permission.send(());
        }
        Ok(Some(path))
    }
}

impl Iterator for OnlineAudio {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if let Some(sample) = self.next_sample() {
            return Some(sample);
        }
        if self.silence > 0 {
            self.silence -= 1;
            return Some(0.0);
        }
        match self.ready.try_recv() {
            Ok(samples) => {
                self.status.buffering.store(false, Relaxed);
                self.samples = samples.into_iter();
                self.next_sample()
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.status.buffering.store(true, Relaxed);
                // Never wait for the network or decoder on the audio output thread.
                let frames = (self.sample_rate() as usize / 100).max(1);
                self.silence = frames * self.channels() as usize - 1;
                Some(0.0)
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status.buffering.store(false, Relaxed);
                self.ended = true;
                None
            }
        }
    }
}

impl Source for OnlineAudio {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.status.channels
    }

    fn sample_rate(&self) -> u32 {
        self.status.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
}

impl Drop for OnlineAudio {
    fn drop(&mut self) {
        if !self.ended {
            self.status.download.cancel();
        }
    }
}

#[derive(Clone)]
pub(super) struct OnlineStatus {
    download: Arc<Download>,
    played: Arc<AtomicU64>,
    buffering: Arc<AtomicBool>,
    channels: u16,
    sample_rate: u32,
}

impl OnlineStatus {
    pub(super) fn progress(&self) -> Option<DownloadProgress> {
        self.download.progress()
    }

    pub(super) fn duration(&self) -> Option<Duration> {
        self.download.data.lock().unwrap().duration
    }

    pub(super) fn position(&self) -> Duration {
        Duration::from_secs_f64(
            self.played.load(Relaxed) as f64
                / (f64::from(self.channels) * f64::from(self.sample_rate)),
        )
    }

    pub(super) fn buffering(&self) -> bool {
        self.buffering.load(Relaxed)
    }

    pub(super) fn complete(&self) -> bool {
        let data = self.download.data.lock().unwrap();
        data.finished && data.error.is_none()
    }

    pub(super) fn decoder(&self) -> Result<OnlineDecoder, String> {
        self.download.decoder(true)
    }

    pub(super) fn error(&self) -> Option<String> {
        self.download.error()
    }
}

#[derive(Default)]
struct DownloadData {
    bytes: Arc<Vec<u8>>,
    duration: Option<Duration>,
    on_complete: Option<CompletedDownload>,
    finished: bool,
    cancelled: bool,
    error: Option<String>,
}

struct Download {
    length: Option<u64>,
    data: Mutex<DownloadData>,
    ready: Condvar,
    eligible: OnceLock<bool>,
}

impl Download {
    fn progress(&self) -> Option<DownloadProgress> {
        let data = self.data.lock().unwrap();
        if data.error.is_some() || (data.cancelled && !data.finished) {
            return None;
        }
        Some(DownloadProgress {
            received: data.bytes.len() as u64,
            total: self.length,
            complete: data.finished,
        })
    }

    fn start(mut reader: impl Read + Send + 'static, length: Option<u64>) -> Arc<Self> {
        let download = Arc::new(Self {
            length,
            data: Mutex::new(DownloadData::default()),
            ready: Condvar::new(),
            eligible: OnceLock::new(),
        });
        let worker = Arc::clone(&download);
        thread::spawn(move || {
            let mut buffer = [0; 16 * 1024];
            loop {
                if worker.data.lock().unwrap().cancelled {
                    break;
                }
                let result = reader.read(&mut buffer);
                let mut data = worker.data.lock().unwrap();
                if data.cancelled {
                    break;
                }
                match result {
                    Ok(0) => {
                        if let Some(length) = length
                            && length != data.bytes.len() as u64
                        {
                            data.error = Some(format!(
                                "Incomplete audio download (expected {length} bytes, received {})",
                                data.bytes.len()
                            ));
                        }
                        data.finished = true;
                    }
                    Ok(read) => {
                        if data.bytes.len() as u64 + read as u64 > AudioStream::MAX_BYTES {
                            data.error = Some("Audio response exceeds 40 MiB".into());
                            data.finished = true;
                        } else {
                            Arc::make_mut(&mut data.bytes).extend_from_slice(&buffer[..read]);
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        data.error = Some(rs_music_core::error_details(&error));
                        data.finished = true;
                    }
                }
                worker.ready.notify_all();
                if data.finished {
                    if let Some(save) = data.on_complete.take().filter(|_| data.error.is_none()) {
                        drop(data);
                        save(&worker);
                    }
                    break;
                }
            }
        });
        download
    }

    fn on_complete(self: &Arc<Self>, callback: CompletedDownload) {
        let mut data = self.data.lock().unwrap();
        if !data.finished {
            if !data.cancelled {
                data.on_complete = Some(if let Some(previous) = data.on_complete.take() {
                    Box::new(move |download| {
                        callback(download);
                        previous(download);
                    })
                } else {
                    callback
                });
            }
        } else if data.error.is_none() {
            let download = Arc::clone(self);
            thread::spawn(move || callback(&download));
        }
    }

    fn cancel(&self) {
        let mut data = self.data.lock().unwrap();
        data.cancelled = true;
        data.on_complete = None;
        self.ready.notify_all();
    }

    fn error(&self) -> Option<String> {
        self.data.lock().unwrap().error.clone()
    }

    fn is_mp4(&self) -> bool {
        self.data.lock().unwrap().bytes.get(4..8) == Some(b"ftyp")
    }

    fn wait_for_end(&self) -> Result<(), String> {
        let data = self
            .ready
            .wait_while(self.data.lock().unwrap(), |data| {
                !data.finished && !data.cancelled
            })
            .unwrap();
        match &data.error {
            Some(error) => Err(error.clone()),
            None if data.finished => Ok(()),
            None => Err("Audio download cancelled".into()),
        }
    }

    fn decoder(self: &Arc<Self>, complete: bool) -> Result<OnlineDecoder, String> {
        let mut builder = Decoder::builder().with_data(DownloadReader {
            download: Arc::clone(self),
            position: 0,
        });
        if complete {
            builder = builder.with_byte_len(self.data.lock().unwrap().bytes.len() as u64);
        }
        builder
            .build()
            .map_err(|error| rs_music_core::error_details(&error))
    }

    fn bytes(&self) -> Arc<Vec<u8>> {
        Arc::clone(&self.data.lock().unwrap().bytes)
    }

    fn eligible_for_persistence(self: &Arc<Self>) -> bool {
        *self.eligible.get_or_init(|| {
            let decoder = match self.decoder(true) {
                Ok(decoder) => decoder,
                Err(error) => {
                    tracing::warn!(%error, "Could not check audio duration for automatic storage");
                    return false;
                }
            };
            // Catalog durations can describe the full song even when the source returns a preview.
            let channels = decoder.channels();
            let rate = decoder.sample_rate();
            let required =
                MIN_PERSISTED_DURATION.as_secs() as usize * rate as usize * channels as usize;
            UniformSourceIterator::new(decoder, channels, rate)
                .take(required)
                .count()
                == required
        })
    }
}

pub(super) struct DownloadReader {
    download: Arc<Download>,
    position: u64,
}

impl Read for DownloadReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let data = self
            .download
            .ready
            .wait_while(self.download.data.lock().unwrap(), |data| {
                self.position >= data.bytes.len() as u64 && !data.finished && !data.cancelled
            })
            .unwrap();
        if let Some(remaining) = data.bytes.get(self.position as usize..)
            && !remaining.is_empty()
        {
            let read = remaining.len().min(buffer.len());
            buffer[..read].copy_from_slice(&remaining[..read]);
            self.position += read as u64;
            Ok(read)
        } else if let Some(error) = &data.error {
            Err(io::Error::other(error.clone()))
        } else {
            Ok(0)
        }
    }
}

impl Seek for DownloadReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let next = match from {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => {
                let data = self.download.data.lock().unwrap();
                let len = if data.finished {
                    Some(data.bytes.len() as u64)
                } else {
                    self.download.length
                };
                len.and_then(|len| len.checked_add_signed(offset))
            }
        };
        self.position = next.ok_or(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid audio seek",
        ))?;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, time::Instant};

    struct GatedReader {
        data: Cursor<Vec<u8>>,
        gate: Option<mpsc::Receiver<()>>,
    }

    impl Read for GatedReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if let Some(gate) = self.gate.take() {
                gate.recv().map_err(io::Error::other)?;
            }
            self.data.read(buffer)
        }
    }

    #[test]
    fn plays_before_download_finishes_and_recovers_from_buffering() {
        let mut wav = b"RIFF\xa4\x3e\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\x80\x3e\0\0".to_vec();
        wav.extend(
            (0..8_000)
                .flat_map(|frame| (if frame < 4_000 { 8192i16 } else { 16384i16 }).to_le_bytes()),
        );
        let byte_len = wav.len() as u64;
        let rest = wav.split_off(4096);
        let (release, gate) = mpsc::channel();
        let input = Cursor::new(wav).chain(GatedReader {
            data: Cursor::new(rest),
            gate: Some(gate),
        });
        let (sender, receiver) = mpsc::channel();
        let monitor = DownloadMonitor::default();
        assert!(monitor.progress().is_none());
        let worker_monitor = monitor.clone();
        thread::spawn(move || {
            let _ = sender.send(OnlineAudio::start(input, Some(byte_len), &worker_monitor));
        });

        let mut audio = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("first samples available without the rest of the download")
            .unwrap();
        let status = audio.status();
        assert!(!status.complete());
        let progress = monitor.progress().unwrap();
        assert_eq!(progress.received, 4096);
        assert_eq!(progress.total, Some(byte_len));
        assert!(
            progress
                .fraction()
                .is_some_and(|value| value > 0.0 && value < 1.0)
        );
        let (save, saved) = mpsc::channel();
        status.download.on_complete(Box::new(move |download| {
            save.send(download.bytes().len()).unwrap();
        }));
        let (report, completed) = mpsc::channel();
        audio.on_download_complete(move || report.send(()).unwrap());
        assert!(matches!(saved.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert_eq!(audio.next(), Some(0.25));
        assert!(audio.by_ref().take(8000).any(|sample| sample == 0.0));
        assert!(status.buffering());
        let position = status.position();
        assert_eq!(audio.next(), Some(0.0));
        assert_eq!(status.position(), position);

        release.send(()).unwrap();
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            saved.recv_timeout(Duration::from_secs(5)).unwrap() as u64,
            byte_len
        );
        assert!(!audio.ended);
        let progress = status.progress().unwrap();
        assert_eq!(progress.received, byte_len);
        assert_eq!(progress.fraction(), Some(1.0));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if audio.next().is_some_and(|sample| sample > 0.0) && status.complete() {
                break;
            }
            assert!(Instant::now() < deadline, "playback did not resume");
            thread::yield_now();
        }
        let mut decoder = status.decoder().unwrap();
        decoder.try_seek(Duration::from_millis(600)).unwrap();
        assert_eq!(decoder.next(), Some(0.5));
    }

    #[test]
    fn incomplete_download_is_never_saved() {
        let download = Download::start(Cursor::new(vec![1]), Some(2));
        let (save, saved) = mpsc::channel();
        download.on_complete(Box::new(move |_| save.send(()).unwrap()));
        assert!(download.wait_for_end().is_err());
        assert!(matches!(
            saved.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn automatic_storage_requires_at_least_twenty_seconds_but_manual_save_does_not() {
        let download = |samples: u32| {
            let data_len = samples * 2;
            let mut wav = b"RIFF".to_vec();
            wav.extend((36 + data_len).to_le_bytes());
            wav.extend(b"WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data");
            wav.extend(data_len.to_le_bytes());
            wav.resize(wav.len() + data_len as usize, 0);
            let length = wav.len() as u64;
            let download = Download::start(Cursor::new(wav), Some(length));
            download.wait_for_end().unwrap();
            download
        };
        let short = download(160_000 - 1);
        assert!(!short.eligible_for_persistence());
        assert!(download(160_000).eligible_for_persistence());

        let cache = rs_music_storage::AudioCache::system().unwrap();
        let directory =
            std::env::temp_dir().join(format!("rs-music-short-test-{}", rand::random::<u64>()));
        let track = cache
            .track(&["short-audio-test"], "Short track".into())
            .in_music_directory(Some(&directory));
        let (sender, permission) = mpsc::channel();
        let archive = || OnlineArchive {
            track: track.clone(),
            download: Arc::clone(&short),
            lyrics_permission: Some(sender.clone()),
        };
        assert!(archive().save(true).unwrap().is_none());
        assert!(matches!(
            permission.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(!directory.exists());
        let saved = archive().save(false).unwrap().unwrap();
        assert_eq!(saved.extension().unwrap(), "wav");
        assert!(permission.try_recv().is_ok());
        std::fs::remove_file(saved).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn explicit_archive_choice_is_consumed_once() {
        use crate::audio::{ArchiveState, Audio};

        let Some(cache) = rs_music_storage::AudioCache::system() else {
            return;
        };
        let archive = || OnlineArchive {
            track: cache.track(&["test-existing-decision"], "Test track".into()),
            download: Download::start(Cursor::new(Vec::new()), Some(0)),
            lyrics_permission: None,
        };
        let mut skipped = Audio {
            archive: Some(ArchiveState::Pending(archive())),
            ..Audio::default()
        };
        assert!(skipped.choose_archive(false).is_none());
        assert!(matches!(
            skipped.archive_state(),
            Some(ArchiveState::Skipped)
        ));
        assert!(skipped.choose_archive(true).is_none());

        let mut chosen = Audio {
            archive: Some(ArchiveState::Pending(archive())),
            ..Audio::default()
        };
        assert!(chosen.choose_archive(true).is_some());
        assert!(matches!(
            chosen.archive_state(),
            Some(ArchiveState::Requested)
        ));
        assert!(chosen.choose_archive(true).is_none());
    }

    #[test]
    fn progress_without_a_content_length_reports_bytes_until_completion() {
        let mut progress = DownloadProgress {
            received: 4096,
            total: None,
            complete: false,
        };
        assert_eq!(progress.fraction(), None);
        progress.complete = true;
        assert_eq!(progress.fraction(), Some(1.0));
    }
}

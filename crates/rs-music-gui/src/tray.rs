use crate::{
    app::{MusicApp, media, prepare_ime_commits},
    i18n::{Language, Message},
    native_options,
};
use eframe::egui;
use ksni::{Tray, blocking::TrayMethods, menu::StandardItem};
use rs_music_storage::CloseAction;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

enum Command {
    Show,
    Quit,
}

#[derive(Default)]
struct WindowSignal {
    available: AtomicBool,
    desktop_theme: Mutex<Option<egui::Theme>>,
    context: Mutex<Option<egui::Context>>,
}

impl WindowSignal {
    fn repaint(&self) {
        if let Some(ctx) = self.context.lock().unwrap().as_ref() {
            ctx.request_repaint();
        }
    }
}

struct Menu {
    commands: mpsc::Sender<Command>,
    signal: Arc<WindowSignal>,
    language: Language,
}

impl Menu {
    fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            self.signal.repaint();
        }
    }
}

impl Tray for Menu {
    fn id(&self) -> String {
        "rs-music".into()
    }

    fn title(&self) -> String {
        "rs-music".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        const SIZE: i32 = 24;
        let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let stem = ((9..=11).contains(&x) && (5..=17).contains(&y))
                    || ((17..=19).contains(&x) && (3..=15).contains(&y));
                let beam = (9..=19).contains(&x) && (4..=7).contains(&(y + (x - 9) / 4));
                let head = [(7, 17), (15, 15)]
                    .into_iter()
                    .any(|(cx, cy)| 4 * (x - cx).pow(2) + 9 * (y - cy).pow(2) <= 36);
                data.extend(if stem || beam || head {
                    [255, 137, 221, 184]
                } else {
                    [0; 4]
                });
            }
        }
        vec![ksni::Icon {
            width: SIZE,
            height: SIZE,
            data,
        }]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Command::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            StandardItem {
                label: self.language.text(Message::RestoreWindow).into(),
                activate: Box::new(|tray: &mut Menu| tray.send(Command::Show)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: self.language.text(Message::ExitApplication).into(),
                activate: Box::new(|tray: &mut Menu| tray.send(Command::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        tracing::warn!(?reason, "System tray became unavailable");
        self.signal.available.store(false, Ordering::Release);
        self.send(Command::Show);
        true
    }

    fn watcher_online(&self) {
        self.signal.available.store(true, Ordering::Release);
    }
}

struct Window<'a> {
    app: &'a Mutex<Option<MusicApp>>,
    hide: &'a mut bool,
    quitting: bool,
    signal: &'a WindowSignal,
    commands: &'a mpsc::Receiver<Command>,
    handle: Option<&'a ksni::blocking::Handle<Menu>>,
}

impl eframe::App for Window<'_> {
    fn raw_input_hook(&mut self, _: &egui::Context, input: &mut egui::RawInput) {
        input.system_theme = self
            .signal
            .desktop_theme
            .lock()
            .unwrap()
            .or(input.system_theme);
        prepare_ime_commits(&mut input.events);
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let mut app = self.app.lock().unwrap();
        let app = app
            .as_mut()
            .expect("application initialized with the window");
        for command in self.commands.try_iter() {
            match command {
                Command::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Command::Quit => {
                    self.quitting = true;
                    *self.hide = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        if ctx.input(|input| input.viewport().close_requested()) {
            let available = self.signal.available.load(Ordering::Acquire)
                && self.handle.is_some_and(|handle| !handle.is_closed());
            *self.hide = hide_on_close(app.close_action, available, self.quitting);
            if app.close_action == CloseAction::HideToTray && !*self.hide && !self.quitting {
                tracing::warn!("System tray unavailable; exiting on window close");
            }
        }

        let language = app.language();
        app.show(ctx);
        let updated_language = app.language();
        if language != updated_language
            && let Some(handle) = self.handle
        {
            handle.update(|tray| tray.language = updated_language);
        }
    }
}

fn hide_on_close(action: CloseAction, tray_available: bool, quitting: bool) -> bool {
    action == CloseAction::HideToTray && tray_available && !quitting
}

pub(crate) fn run() -> eframe::Result {
    let signal = Arc::new(WindowSignal::default());
    let desktop = Arc::clone(&signal);
    std::thread::spawn(move || {
        if let Err(error) = crate::desktop_theme::watch(|theme| {
            *desktop.desktop_theme.lock().unwrap() = theme;
            desktop.repaint();
        }) {
            tracing::debug!(%error, "Desktop color scheme unavailable");
        }
    });
    let app = Arc::new(Mutex::<Option<MusicApp>>::new(None));
    let (stop, stopped) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let signal = &signal;
        let worker_app = Arc::clone(&app);
        scope.spawn(move || {
            let background = egui::Context::default();
            let repaint = Arc::clone(signal);
            let mut media = match media::MediaControl::start(Arc::clone(&worker_app), move || repaint.repaint()) {
                Ok(service) => Some(service),
                Err(error) => {
                    tracing::warn!(error = %rs_music_core::error_details(&error), "Could not start MPRIS media controls");
                    None
                }
            };
            while let Err(mpsc::RecvTimeoutError::Timeout) =
                stopped.recv_timeout(Duration::from_millis(100))
            {
                // Wayland may withhold redraws for an off-screen window.
                let (mut changed, snapshot) = {
                    let mut state = worker_app.lock().unwrap();
                    let Some(app) = state.as_mut() else {
                        continue;
                    };
                    let changed = app.tick(&background);
                    (changed, media.as_ref().map(|_| media::snapshot(app)))
                };
                if let Some((properties, seeked)) = snapshot
                    && let Some(service) = media.as_mut()
                {
                    match service.update(properties, seeked) {
                        Ok(updated) => changed |= updated,
                        Err(error) => {
                            tracing::warn!(error = %rs_music_core::error_details(&error), "Could not publish MPRIS playback state");
                            media = None;
                        }
                    }
                }
                if changed {
                    signal.repaint();
                }
            }
        });
        let result = run_windows(&app, signal);
        drop(stop);
        result
    })
}

fn run_windows(app: &Mutex<Option<MusicApp>>, signal: &Arc<WindowSignal>) -> eframe::Result {
    let (sender, commands) = mpsc::channel();
    let mut handle = None;
    loop {
        let mut hide = false;
        eframe::run_native(
            "rs-music",
            native_options(),
            Box::new(|creation| {
                let ctx = &creation.egui_ctx;
                *signal.context.lock().unwrap() = Some(ctx.clone());
                match &mut *app.lock().unwrap() {
                    Some(app) => {
                        MusicApp::restore_window(app, ctx);
                    }
                    state @ None => {
                        let app = state.insert(MusicApp::new(ctx));
                        signal.available.store(true, Ordering::Release);
                        match (Menu {
                            commands: sender.clone(),
                            signal: Arc::clone(signal),
                            language: app.language(),
                        })
                        .spawn()
                        {
                            Ok(tray) => handle = Some(tray),
                            Err(error) => {
                                signal.available.store(false, Ordering::Release);
                                tracing::warn!(error = %rs_music_core::error_details(&error), "Could not register system tray");
                            }
                        }
                    }
                };
                Ok(Box::new(Window {
                    app,
                    hide: &mut hide,
                    quitting: false,
                    signal,
                    commands: &commands,
                    handle: handle.as_ref(),
                }))
            }),
        )?;
        *signal.context.lock().unwrap() = None;
        if !hide {
            return Ok(());
        }

        // Winit cannot hide a native Wayland window. Close the view and keep playing
        // with the same application state until the tray requests a new window.
        tracing::info!("Window hidden to system tray");
        loop {
            match commands.recv_timeout(Duration::from_millis(100)) {
                Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Ok(Command::Show) => break,
                Err(mpsc::RecvTimeoutError::Timeout)
                    if handle.as_ref().is_some_and(|handle| handle.is_closed()) =>
                {
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        tracing::info!("Restoring window from system tray");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_reachable_tray_can_keep_the_app_running_after_close() {
        assert!(hide_on_close(CloseAction::default(), true, false));
        assert!(!hide_on_close(CloseAction::HideToTray, false, false));
        assert!(!hide_on_close(CloseAction::HideToTray, true, true));
        assert!(!hide_on_close(CloseAction::Exit, true, false));
    }
}

mod app;
mod audio;
mod fonts;
mod i18n;
#[cfg(target_os = "linux")]
mod tray;

pub fn run() -> eframe::Result {
    use std::io::IsTerminal;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("warn,rs_music_gui=info,rs_music_core=info")
    });
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(PortalRequestLog)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(std::io::stderr().is_terminal())
                .with_writer(std::io::stderr),
        )
        .try_init();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Starting rs-music");
    #[cfg(target_os = "linux")]
    let result = tray::run();
    #[cfg(not(target_os = "linux"))]
    let result = eframe::run_native(
        "rs-music",
        native_options(),
        Box::new(|context| Ok(Box::new(app::MusicApp::new(&context.egui_ctx)))),
    );
    match &result {
        Ok(()) => tracing::info!("rs-music closed"),
        Err(error) => {
            tracing::error!(error = %rs_music_core::error_details(error), "Window initialization or event loop failed")
        }
    }
    result
}

fn native_options() -> eframe::NativeOptions {
    #[cfg(target_os = "linux")]
    let x11 = std::env::var("RS_MUSIC_WINDOW_BACKEND").as_deref() == Ok("x11");
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1040.0, 680.0])
            .with_min_inner_size([760.0, 480.0]),
        #[cfg(target_os = "linux")]
        // On Wayland, an occluded surface can stop receiving compositor frame
        // callbacks. Waiting for one in an EGL swap blocks window events too.
        vsync: x11,
        #[cfg(target_os = "linux")]
        event_loop_builder: Some(linux_window_backend(x11)),
        ..Default::default()
    }
}

struct PortalRequestLog;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for PortalRequestLog {
    fn event_enabled(
        &self,
        event: &tracing::Event<'_>,
        _: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        let metadata = event.metadata();
        if metadata.target() != "zbus::proxy" || *metadata.level() != tracing::Level::WARN {
            return true;
        }
        #[derive(Default)]
        struct TransientRequest(bool);
        impl tracing::field::Visit for TransientRequest {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    let message = format!("{value:?}");
                    // The file picker waits for the portal's Response signal.
                    self.0 = message.starts_with("Failed to populate properties cache via GetAll:")
                        && message.contains("org.freedesktop.DBus.Error.UnknownMethod")
                        && message.contains("/org/freedesktop/portal/desktop/request/");
                }
            }
        }
        let mut transient_request = TransientRequest::default();
        event.record(&mut transient_request);
        !transient_request.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    use tracing_subscriber::layer::{Context, SubscriberExt};

    #[test]
    fn filters_only_missing_portal_request_cache_warnings() {
        static SEEN: AtomicUsize = AtomicUsize::new(0);
        struct Count;
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Count {
            fn on_event(&self, _: &tracing::Event<'_>, _: Context<'_, S>) {
                SEEN.fetch_add(1, Relaxed);
            }
        }
        let subscriber = tracing_subscriber::registry()
            .with(PortalRequestLog)
            .with(Count);
        tracing::subscriber::with_default(subscriber, || {
            let path = "/org/freedesktop/portal/desktop/request/1_1/ashpd_test";
            tracing::warn!(target: "zbus::proxy", "Failed to populate properties cache via GetAll: org.freedesktop.DBus.Error.UnknownMethod: Object does not exist at path {path}.");
            tracing::warn!(target: "zbus::proxy", "Failed to populate properties cache via GetAll: org.freedesktop.DBus.Error.AccessDenied for {path}.");
            tracing::error!(target: "zbus::proxy", "Failed to populate properties cache via GetAll: org.freedesktop.DBus.Error.UnknownMethod at {path}.");
        });
        assert_eq!(SEEN.load(Relaxed), 2);
    }
}

#[cfg(target_os = "linux")]
fn linux_window_backend(x11: bool) -> eframe::EventLoopBuilderHook {
    use winit::platform::{wayland::EventLoopBuilderExtWayland, x11::EventLoopBuilderExtX11};

    Box::new(move |builder| {
        if x11 {
            tracing::info!(backend = "x11", "Selecting window backend");
            builder.with_x11();
        } else {
            tracing::info!(backend = "wayland", "Selecting window backend");
            builder.with_wayland();
        }
    })
}

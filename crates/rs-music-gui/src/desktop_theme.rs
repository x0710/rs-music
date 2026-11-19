//! Linux 桌面通过 Settings portal 通知配色变化，X11 或 Wayland 窗口本身可能没有主题信息。
use eframe::egui::Theme;
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::OwnedValue,
};

pub(crate) fn watch(mut changed: impl FnMut(Option<Theme>)) -> zbus::Result<()> {
    const APPEARANCE: &str = "org.freedesktop.appearance";
    const KEY: &str = "color-scheme";
    let connection = Connection::session()?;
    let settings = Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
    )?;
    let changes =
        settings.receive_signal_with_args("SettingChanged", &[(0, APPEARANCE), (1, KEY)])?;
    let initial: OwnedValue = settings.call("ReadOne", &(APPEARANCE, KEY))?;
    changed(theme(initial));
    for signal in changes {
        let (_, _, value): (String, String, OwnedValue) = signal.body().deserialize()?;
        changed(theme(value));
    }
    Ok(())
}

fn theme(value: OwnedValue) -> Option<Theme> {
    match u32::try_from(value).ok()? {
        1 => Some(Theme::Dark),
        2 => Some(Theme::Light),
        _ => None,
    }
}

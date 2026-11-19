use eframe::egui::{self, Event, Key, Modifiers};

/// 在各页面绘制前处理空格。文本框和弹出菜单保留自己的键盘操作。
pub(super) fn take_playback_toggle(ctx: &egui::Context) -> bool {
    let focused = ctx.memory(|memory| memory.focused());
    if focused.is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some())
        || egui::Popup::is_any_open(ctx)
    {
        return false;
    }

    ctx.input_mut(|input| {
        let mut toggle = false;
        input.events.retain(|event| {
            if let Event::Key {
                key: Key::Space,
                pressed: true,
                repeat,
                modifiers: Modifiers::NONE,
                ..
            } = event
            {
                // 吞掉长按产生的重复事件，防止焦点按钮再次响应空格。
                toggle |= !repeat;
                return false;
            }
            true
        });
        toggle
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(repeat: bool) -> Event {
        Event::Key {
            key: Key::Space,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn space_toggles_once_and_remains_available_in_text_fields() {
        let ctx = egui::Context::default();
        let field = egui::Id::new("shortcut_text");
        let mut text = String::new();
        let mut frame = |events, expected| {
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    assert_eq!(take_playback_toggle(ctx), expected);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.add(egui::TextEdit::singleline(&mut text).id(field));
                    });
                },
            );
        };
        frame(vec![space(false)], true);
        frame(vec![space(true)], false);
        ctx.memory_mut(|memory| memory.request_focus(field));
        frame(vec![space(false), Event::Text(" ".into())], false);
        assert_eq!(text, " ");
    }
}

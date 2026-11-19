use super::{Message, MusicApp, now_playing::active_line};
use eframe::egui::{self, Color32, FontId, Vec2};
use rs_music_core::lyrics::{Line, Lyrics};
use std::time::Duration;

fn id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("desktop_subtitles")
}

/// 子窗口只读取当前歌曲和播放时钟，关闭它不会改变主窗口或播放状态。
pub(super) fn show(app: &MusicApp, ctx: &egui::Context) -> bool {
    let language = app.language();
    let position = app.audio.position();
    let lyrics = app.now_playing.lyrics();
    let status = match lyrics {
        Some(Err(_)) => Message::LyricsLoadFailed,
        None if app.now_playing.has_track() => Message::LoadingLyrics,
        None => Message::NoTrack,
        _ => Message::NoLyrics,
    };
    let status = language.text(status);
    let loaded = lyrics
        .and_then(|result| result.as_ref().ok())
        .and_then(Option::as_ref);
    let size = match loaded {
        Some(Lyrics::Timed(lines)) if !lines.is_empty() => {
            let current = active_line(lines, position).unwrap_or(0);
            compact_size(
                ctx,
                lines[current..]
                    .iter()
                    .take(2)
                    .map(|line| line.text.as_ref()),
                2,
            )
        }
        Some(Lyrics::Plain(_)) => egui::vec2(400.0, 76.0),
        _ => compact_size(ctx, [status].into_iter(), 1),
    };
    let builder = egui::ViewportBuilder::default()
        .with_title(language.text(Message::Subtitles))
        .with_app_id("rs-music-subtitles")
        .with_transparent(true)
        .with_always_on_top()
        .with_decorations(false)
        .with_inner_size(size);
    ctx.show_viewport_immediate(id(), builder, |ctx, _| {
        if ctx.input(|input| input.viewport().close_requested()) {
            return false;
        }
        // Wayland 的原生拖动需要本次按下事件的序列号，在按下左键时立即发起。
        if ctx.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(2))
            .show(ctx, |ui| match loaded {
                Some(Lyrics::Timed(lines)) if !lines.is_empty() => {
                    draw_timed(ui, lines, position, app.audio.duration())
                }
                Some(Lyrics::Plain(text)) => {
                    egui::ScrollArea::vertical().show(ui, |ui| draw_text(ui, text));
                }
                _ => draw_text(ui, status),
            });
        if app.audio.playing() {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
        true
    })
}

const FONT_SIZE: f32 = 26.0;

/// 按可见歌词测量客户区。描边占用两侧留白；双行模式保留固定行高，避免换句时上下跳动。
fn compact_size<'a>(
    ctx: &egui::Context,
    lines: impl Iterator<Item = &'a str>,
    rows: usize,
) -> Vec2 {
    let font = FontId::proportional(FONT_SIZE);
    ctx.fonts_mut(|fonts| {
        let mut height = fonts.row_height(&font);
        let mut width = 0.0_f32;
        for text in lines {
            let galley = fonts.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE);
            width = width.max(galley.size().x);
            height = height.max(galley.size().y);
        }
        egui::vec2(
            (width.ceil() + 8.0).clamp(36.0, 520.0),
            (height.ceil() + 4.0) * rows as f32 + 4.0,
        )
    })
}

/// 歌词序号决定行的位置。预显句到点后留在原位，跳转时也能从播放位置直接还原两行。
fn draw_timed(ui: &mut egui::Ui, lines: &[Line], position: Duration, duration: Option<Duration>) {
    let active = active_line(lines, position);
    let current = active.unwrap_or(0);
    let area = ui.available_rect_before_wrap();
    let height = area.height() * 0.5;
    for (index, line) in lines.iter().enumerate().skip(current).take(2) {
        let top = area.top() + (index % 2) as f32 * height;
        let row = egui::Rect::from_min_size(
            egui::pos2(area.left(), top),
            egui::vec2(area.width(), height),
        );
        let size = FONT_SIZE;
        let layout = |size| {
            ui.painter().layout_no_wrap(
                line.text.to_string(),
                FontId::proportional(size),
                Color32::WHITE,
            )
        };
        let mut galley = layout(size);
        let scale =
            (row.width() / galley.size().x.max(1.0)).min(row.height() / galley.size().y.max(1.0));
        if scale < 1.0 {
            galley = layout((size * scale).max(1.0));
        }
        let progress = if active == Some(index) {
            let end = lines.get(index + 1).map(|next| next.at).or(duration);
            line_progress(position, line.at, end)
        } else {
            0.0
        };
        paint_lyric(
            &ui.painter().with_clip_rect(row.intersect(ui.clip_rect())),
            row,
            galley,
            progress,
        );
    }
}

/// LRC 提供行时间戳，扫色进度取当前行与下一行之间的播放比例。
fn line_progress(position: Duration, start: Duration, end: Option<Duration>) -> f32 {
    let Some(span) = end
        .and_then(|end| end.checked_sub(start))
        .filter(|span| !span.is_zero())
    else {
        return 1.0;
    };
    (position.saturating_sub(start).as_secs_f32() / span.as_secs_f32()).min(1.0)
}

fn draw_text(ui: &mut egui::Ui, text: &str) {
    let width = ui.available_width().max(1.0);
    let font = FontId::proportional(FONT_SIZE);
    let painter = ui.painter().clone();
    let galley = painter.layout(text.to_owned(), font, Color32::WHITE, width);
    let height = galley.size().y.max(
        ui.available_height()
            .min(ui.ctx().content_rect().height() - 4.0),
    );
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    paint_lyric(&painter, rect, galley, 0.0);
}

fn paint_lyric(
    painter: &egui::Painter,
    rect: egui::Rect,
    galley: std::sync::Arc<egui::Galley>,
    progress: f32,
) {
    let origin = rect.center() - galley.size() * 0.5;
    for offset in [
        Vec2::new(-1.5, 0.0),
        Vec2::new(1.5, 0.0),
        Vec2::new(0.0, -1.5),
        Vec2::new(0.0, 2.0),
    ] {
        painter.galley_with_override_text_color(origin + offset, galley.clone(), Color32::BLACK);
    }
    painter.galley(origin, galley.clone(), Color32::WHITE);
    if progress > 0.0 {
        let clip = egui::Rect::from_min_size(
            origin,
            egui::vec2(galley.size().x * progress, galley.size().y),
        );
        painter
            .with_clip_rect(clip.intersect(painter.clip_rect()))
            .galley_with_override_text_color(origin, galley, Color32::from_rgb(255, 202, 76));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_fits_the_visible_pair_and_caps_long_lines() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            let short = compact_size(ctx, ["A", "B"].into_iter(), 2);
            let preview = compact_size(ctx, ["A", "The upcoming lyric"].into_iter(), 2);
            let long = "long lyric ".repeat(32);
            let capped = compact_size(ctx, [long.as_str(), "B"].into_iter(), 2);
            assert!(short.x < preview.x && preview.x < capped.x);
            assert_eq!(capped.x, 520.0);
            assert_eq!(short.y, preview.y);
            assert!(compact_size(ctx, ["Loading"].into_iter(), 1).y < short.y);
        });
    }

    #[test]
    fn only_primary_press_starts_a_native_window_drag() {
        let ctx = egui::Context::default();
        let app = MusicApp::from_state(&ctx, Default::default(), None, None);
        let requests_drag = |button| {
            let output = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::PointerButton {
                        pos: egui::pos2(40.0, 20.0),
                        button,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    show(&app, ctx);
                },
            );
            output.viewport_output.values().any(|viewport| {
                viewport
                    .commands
                    .iter()
                    .any(|cmd| matches!(cmd, egui::ViewportCommand::StartDrag))
            })
        };
        assert!(!requests_drag(egui::PointerButton::Secondary));
        assert!(requests_drag(egui::PointerButton::Primary));
    }

    #[test]
    fn preview_becomes_active_in_place_and_seeking_restores_both_rows() {
        let Some(Lyrics::Timed(lines)) = Lyrics::parse("[00:01]A\n[00:03]B\n[00:05]C") else {
            panic!("expected timed lyrics");
        };
        let ctx = egui::Context::default();
        let frame = |seconds| {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(760.0, 160.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw_timed(ui, &lines, Duration::from_secs(seconds), None);
                    });
                },
            );
            let mut rows = [String::new(), String::new()];
            let mut highlighted = String::new();
            for shape in output.shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    let row = usize::from(text.pos.y >= 80.0);
                    rows[row].clone_from(&text.galley.job.text);
                    if text.override_text_color == Some(Color32::from_rgb(255, 202, 76)) {
                        highlighted.clone_from(&text.galley.job.text);
                    }
                }
            }
            (rows, highlighted)
        };
        for (seconds, upper, lower, active) in [
            (0, "A", "B", ""),
            (2, "A", "B", "A"),
            (4, "C", "B", "B"),
            (6, "C", "", "C"),
            (4, "C", "B", "B"),
            (4, "C", "B", "B"),
        ] {
            let (rows, highlighted) = frame(seconds);
            assert_eq!(rows, [upper, lower]);
            assert_eq!(highlighted, active);
        }
    }

    #[test]
    fn sweep_follows_playback_position_across_seeks_and_missing_end_times() {
        let start = Duration::from_secs(4);
        let end = Some(Duration::from_secs(8));
        assert_eq!(line_progress(Duration::from_secs(9), start, end), 1.0);
        assert_eq!(line_progress(Duration::from_secs(5), start, end), 0.25);
        assert_eq!(line_progress(Duration::from_secs(3), start, end), 0.0);
        assert_eq!(line_progress(start, start, None), 1.0);
        assert_eq!(line_progress(start, start, Some(start)), 1.0);
    }
}

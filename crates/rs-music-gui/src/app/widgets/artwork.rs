use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, vec2};

pub(in crate::app) fn show(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    edge: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(edge, edge), egui::Sense::hover());
    if let Some(texture) = texture {
        let size = texture.size_vec2();
        let shorter = size.x.min(size.y);
        let uv = Rect::from_center_size(
            egui::pos2(0.5, 0.5),
            vec2(shorter / size.x, shorter / size.y),
        );
        egui::Image::new((texture.id(), rect.size()))
            .uv(uv)
            .corner_radius(6)
            .paint_at(ui, rect);
    } else if ui.is_rect_visible(rect) {
        placeholder(ui.painter(), rect);
    }
    response
}

fn placeholder(painter: &egui::Painter, rect: Rect) {
    let accent = Color32::from_rgb(137, 221, 184);
    let edge = rect.width();
    let at = |x, y| Pos2::new(rect.left() + edge * x, rect.top() + edge * y);
    painter.rect_filled(rect, 6, Color32::from_rgb(32, 58, 57));
    painter.circle_filled(rect.center(), edge * 0.38, Color32::from_rgb(23, 43, 45));
    painter.circle_stroke(
        rect.center(),
        edge * 0.31,
        Stroke::new(edge * 0.018, Color32::from_rgb(53, 83, 77)),
    );
    painter.add(Shape::ellipse_filled(
        at(0.45, 0.65),
        vec2(edge * 0.10, edge * 0.075),
        accent,
    ));
    let stem = Stroke::new(edge * 0.052, accent);
    painter.line_segment([at(0.52, 0.63), at(0.52, 0.33)], stem);
    painter.line_segment([at(0.52, 0.33), at(0.65, 0.40)], stem);
}

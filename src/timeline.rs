//! Helpers for displaying and selecting densely spaced, sorted pose tracks.
use crate::model::Pose;
use crate::ui_style;
#[cfg(test)]
use crate::ui_style::Theme;
use eframe::egui::{
    self, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2,
};

#[derive(Clone, Copy)]
pub(crate) struct Colors {
    pub surface: Color32,
    pub border: Color32,
    pub grid: Color32,
    pub tick: Color32,
    pub track: Color32,
    pub text: Color32,
    pub marker: Color32,
    pub marker_outline: Color32,
    pub accent: Color32,
    pub selected_row: Color32,
}

impl Colors {
    pub fn new(visuals: &egui::Visuals) -> Self {
        let border = visuals.widgets.noninteractive.bg_stroke.color;
        let muted = visuals.weak_text_color();
        Self {
            surface: visuals.panel_fill,
            border,
            grid: visuals.panel_fill.lerp_to_gamma(border, 0.7),
            tick: muted,
            track: border.lerp_to_gamma(muted, 0.7),
            text: visuals.text_color(),
            marker: muted,
            marker_outline: visuals.panel_fill,
            accent: visuals.selection.stroke.color,
            selected_row: visuals.selection.bg_fill,
        }
    }
}

pub(crate) enum Transport {
    Start,
    Play,
    Pause,
}

pub(crate) fn transport_button(ui: &mut Ui, action: Transport, colors: Colors) -> egui::Response {
    let (label, width) = match action {
        Transport::Start => ("В начало", 30.0),
        Transport::Play => ("Смотреть", 40.0),
        Transport::Pause => ("Пауза", 40.0),
    };
    let primary = !matches!(action, Transport::Start);
    let rect = ui.allocate_space(Vec2::new(width, 30.0));
    let response = ui
        .interact(rect.1, rect.0, Sense::click())
        .on_hover_text(label);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let painter = ui.painter();
    let fill = if primary {
        if response.is_pointer_button_down_on() {
            ui_style::PRIMARY_PRESSED
        } else if response.hovered() {
            ui_style::PRIMARY_HOVER
        } else {
            ui_style::ACCENT
        }
    } else if response.hovered() || response.is_pointer_button_down_on() {
        colors.selected_row
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(response.rect, 6, fill);
    let icon_color = if primary { Color32::WHITE } else { colors.text };
    if response.has_focus() {
        painter.rect_stroke(
            response.rect,
            6,
            Stroke::new(1.5, if primary { icon_color } else { colors.accent }),
            egui::StrokeKind::Inside,
        );
    }

    let center = response.rect.center();
    let point = |x, y| center + Vec2::new(x, y);
    match action {
        Transport::Start => {
            painter.line_segment(
                [point(-6.0, -6.0), point(-6.0, 6.0)],
                Stroke::new(1.8, icon_color),
            );
            painter.add(Shape::convex_polygon(
                vec![point(6.0, -6.0), point(6.0, 6.0), point(-3.0, 0.0)],
                icon_color,
                Stroke::NONE,
            ));
        }
        Transport::Play => {
            painter.add(Shape::convex_polygon(
                vec![point(-4.0, -7.0), point(7.0, 0.0), point(-4.0, 7.0)],
                icon_color,
                Stroke::NONE,
            ));
        }
        Transport::Pause => {
            for x in [-3.0, 3.0] {
                painter.line_segment(
                    [point(x, -6.0), point(x, 6.0)],
                    Stroke::new(2.5, icon_color),
                );
            }
        }
    }
    response
}

pub(crate) fn diamond(center: Pos2, radius: f32, fill: Color32, stroke: Stroke) -> Shape {
    let horizontal = Vec2::new(radius, 0.0);
    let vertical = Vec2::new(0.0, radius);
    Shape::convex_polygon(
        vec![
            center - vertical,
            center + horizontal,
            center + vertical,
            center - horizontal,
        ],
        fill,
        stroke,
    )
}

pub(crate) fn duration_label(duration: f32) -> String {
    let number = format!("{duration:.1}");
    format!("{} с", number.trim_end_matches(".0"))
}

#[cfg(test)]
pub(crate) fn marker_bounds(shape: &Shape) -> Option<Rect> {
    let Shape::Path(path) = shape else {
        return None;
    };
    if path.points.len() == 4
        && path.closed
        && path.stroke.is_empty()
        && [Theme::Light, Theme::Dark]
            .map(|theme| Colors::new(&ui_style::theme_visuals(theme)).marker)
            .contains(&path.fill)
    {
        Some(Rect::from_points(&path.points))
    } else {
        None
    }
}

pub(crate) fn ruler_shapes(
    painter: &Painter,
    ruler: Rect,
    tracks_bottom: f32,
    duration: f32,
    time: f32,
    x_at: impl Fn(f32) -> f32,
    colors: Colors,
) -> (Shape, Shape) {
    let line = |x, top, bottom, stroke| {
        Shape::line_segment([Pos2::new(x, top), Pos2::new(x, bottom)], stroke)
    };
    let separator = Stroke::new(1.0, colors.border);
    let mut grid = vec![
        Shape::line_segment([ruler.left_top(), ruler.right_top()], separator),
        Shape::line_segment([ruler.left_bottom(), ruler.right_bottom()], separator),
    ];
    let axis_width = x_at(duration) - x_at(0.0);
    let step = [1, 2, 5, 10, 20, 30, 60]
        .into_iter()
        .find(|&step| axis_width * (step as f32) / duration >= 44.0)
        .unwrap_or(60);
    let major_width = axis_width * step as f32 / duration;
    let subdivisions = if major_width >= 42.0 { 5 } else { 2 };
    let minor_step = step as f32 / subdivisions as f32;
    let end_x = x_at(duration);
    let tick = Stroke::new(1.0, colors.tick);
    let grid_stroke = Stroke::new(1.0, colors.grid);
    let tick_top = ruler.top() + 23.0;
    let tick_bottom = ruler.bottom() - 7.0;
    for i in 0..(duration / minor_step).ceil() as usize {
        let time = i as f32 * minor_step;
        let x = x_at(time);
        if end_x - x < 5.0 {
            continue;
        }
        if i % subdivisions == 0 {
            grid.push(line(x, tick_top, tick_bottom, tick));
            grid.push(line(x, tick_bottom, tracks_bottom, grid_stroke));
            if end_x - x >= 35.0 || i == 0 {
                let label = (i / subdivisions * step).to_string();
                let galley = painter.layout_no_wrap(label, FontId::monospace(12.0), colors.text);
                let pos = Align2::CENTER_TOP
                    .anchor_size(Pos2::new(x, ruler.top() + 6.0), galley.size())
                    .min;
                grid.push(Shape::galley(pos, galley, colors.text));
            }
        } else {
            grid.push(line(x, tick_top + 6.0, tick_bottom - 4.0, tick));
        }
    }
    grid.push(line(end_x, tick_top, tick_bottom, tick));
    grid.push(line(end_x, tick_bottom, tracks_bottom, grid_stroke));
    let end_label = painter.layout_no_wrap(
        duration_label(duration),
        FontId::monospace(12.0),
        colors.text,
    );
    let pos = Align2::RIGHT_TOP
        .anchor_size(Pos2::new(end_x + 4.0, ruler.top() + 6.0), end_label.size())
        .min;
    grid.push(Shape::galley(pos, end_label, colors.text));

    let x = x_at(time);
    let y = ruler.bottom() - 13.0;
    let cursor = vec![
        line(x, y + 5.0, tracks_bottom, Stroke::new(1.5, colors.accent)),
        Shape::convex_polygon(
            vec![
                Pos2::new(x - 6.0, y - 4.0),
                Pos2::new(x + 6.0, y - 4.0),
                Pos2::new(x + 6.0, y + 2.0),
                Pos2::new(x, y + 8.0),
                Pos2::new(x - 6.0, y + 2.0),
            ],
            colors.accent,
            Stroke::NONE,
        ),
    ];
    (Shape::Vec(grid), Shape::Vec(cursor))
}

/// Keep the largest pose marker fully inside the allocated track.
const MARKER_INSET: f32 = 8.0;

pub(crate) fn time_to_x(time: f32, duration: f32, left: f32, width: f32) -> f32 {
    let inset = MARKER_INSET.min(width.max(0.0) / 2.0);
    left + inset + (width - 2.0 * inset).max(0.0) * (time / duration).clamp(0.0, 1.0)
}

pub(crate) fn x_to_time(x: f32, duration: f32, left: f32, width: f32) -> f32 {
    let inset = MARKER_INSET.min(width.max(0.0) / 2.0);
    let usable = width - 2.0 * inset;
    if usable <= 0.0 {
        return 0.0;
    }
    ((x - left - inset) / usable).clamp(0.0, 1.0) * duration
}

pub(crate) fn nearest(poses: &[Pose], time: f32) -> Option<f32> {
    let right = poses.partition_point(|pose| pose.time < time);
    match (
        right.checked_sub(1).and_then(|i| poses.get(i)),
        poses.get(right),
    ) {
        (None, None) => None,
        (Some(left), None) => Some(left.time),
        (None, Some(right)) => Some(right.time),
        (Some(left), Some(right)) => {
            if (time - left.time).abs() <= (right.time - time).abs() {
                Some(left.time)
            } else {
                Some(right.time)
            }
        }
    }
}

/// Show at most one representative per six logical pixels; selection remains exact.
pub(crate) fn markers(poses: &[Pose], duration: f32, width: f32) -> Vec<f32> {
    if poses.is_empty()
        || !duration.is_finite()
        || duration <= 0.0
        || !width.is_finite()
        || width <= 0.0
    {
        return Vec::new();
    }
    let bins = ((width / 6.0).ceil() as usize).clamp(1, 2048);
    let mut result = Vec::with_capacity(bins.min(poses.len()));
    let mut index = 0;
    while let Some(pose) = poses.get(index) {
        result.push(pose.time);
        let bucket =
            (((pose.time as f64 / duration as f64) * bins as f64).floor() as usize).min(bins - 1);
        let upper = ((bucket + 1) as f64 * duration as f64) / bins as f64;
        let next = poses.partition_point(|p| (p.time as f64) < upper);
        index = next.max(index + 1);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Transform;
    fn poses() -> Vec<Pose> {
        (0..100_000)
            .map(|i| Pose {
                time: i as f32 * 0.0005,
                transform: Transform::default(),
                linear: true,
            })
            .collect()
    }
    #[test]
    fn marker_geometry_preserves_endpoints_and_round_trips() {
        let (left, width, duration) = (100.0, 400.0, 60.0);
        assert_eq!(time_to_x(0.0, duration, left, width), 108.0);
        assert_eq!(time_to_x(duration, duration, left, width), 492.0);
        for time in [0.0, 1.0, 30.0, 60.0] {
            assert!(
                (x_to_time(
                    time_to_x(time, duration, left, width),
                    duration,
                    left,
                    width
                ) - time)
                    .abs()
                    < 0.001
            );
        }
        assert_eq!(x_to_time(0.0, duration, left, width), 0.0);
        assert_eq!(x_to_time(999.0, duration, left, width), duration);
    }
    #[test]
    fn dense_markers_are_bounded_but_selection_uses_original_times() {
        let poses = poses();
        let drawn = markers(&poses, 60.0, 600.0);
        assert!(drawn.len() <= 101);
        assert_eq!(
            nearest(&poses, poses[50_001].time),
            Some(poses[50_001].time)
        );
        assert_eq!(nearest(&poses, -1.0), Some(poses[0].time));
        assert_eq!(nearest(&poses, 60.0), Some(poses.last().unwrap().time));
        assert_eq!(nearest(&[], 1.0), None);
    }
}

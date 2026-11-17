//! Shared logical scene rendering for the editor and video frames.
use crate::{Asset, AssetContent, DrawingElement, Project, Shape, Transform, state_at};
use tiny_skia::{
    Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform as Matrix,
};

pub const WIDTH: f32 = 640.0;
pub const HEIGHT: f32 = 360.0;

pub fn rotation_sin_cos(angle: f32) -> (f32, f32) {
    ((angle as f64).rem_euclid(360.0).to_radians() as f32).sin_cos()
}

fn rotate_point(point: [f32; 2], center: [f32; 2], angle: f32) -> [f32; 2] {
    let (sin, cos) = rotation_sin_cos(angle);
    let x = point[0] - center[0];
    let y = point[1] - center[1];
    [center[0] + cos * x - sin * y, center[1] + sin * x + cos * y]
}

struct PreparedElement {
    path: tiny_skia::Path,
    color: [u8; 4],
    fill: bool,
    width: f32,
}
struct PreparedAsset {
    source: Asset,
    elements: Vec<PreparedElement>,
}

/// Owns a reusable pixel buffer and local-space geometry for each referenced asset.
pub struct Renderer {
    scale: u32,
    image: Pixmap,
    assets: std::collections::HashMap<uuid::Uuid, PreparedAsset>,
}
impl Renderer {
    pub fn new(scale: u32) -> Self {
        Self {
            scale,
            image: Pixmap::new(640 * scale, 360 * scale).expect("valid scene dimensions"),
            assets: std::collections::HashMap::new(),
        }
    }
    /// Change only the pixel buffer; prepared logical geometry remains reusable.
    pub fn set_scale(&mut self, scale: u32) {
        if self.scale != scale {
            self.image = Pixmap::new(640 * scale, 360 * scale).expect("valid scene dimensions");
            self.scale = scale;
        }
    }
    pub fn render(&mut self, project: &Project, time: f32) -> &[u8] {
        self.image.fill(Color::from_rgba8(250, 250, 252, 255));
        let index: std::collections::HashMap<_, _> = project
            .assets
            .iter()
            .map(|asset| (asset.id, asset))
            .collect();
        self.assets.retain(|id, _| index.contains_key(id));
        let mut checked = std::collections::HashSet::new();
        for object in &project.objects {
            let Some(asset) = index.get(&object.asset_id).copied() else {
                continue;
            };
            let entry = self
                .assets
                .entry(asset.id)
                .or_insert_with(|| prepare(asset));
            if checked.insert(asset.id) && entry.source != *asset {
                *entry = prepare(asset);
            }
            let t = state_at(object, time);
            let (sin, cos) = rotation_sin_cos(t.angle);
            let s = t.scale * self.scale as f32;
            let matrix = Matrix::from_row(
                cos * s,
                sin * s,
                -sin * s,
                cos * s,
                (WIDTH / 2.0 + t.x) * self.scale as f32,
                (HEIGHT / 2.0 + t.y) * self.scale as f32,
            );
            for element in &entry.elements {
                let mut paint = Paint::default();
                paint.set_color_rgba8(
                    element.color[0],
                    element.color[1],
                    element.color[2],
                    element.color[3],
                );
                if element.fill {
                    self.image
                        .fill_path(&element.path, &paint, FillRule::Winding, matrix, None);
                } else {
                    let stroke = Stroke {
                        width: element.width,
                        line_cap: LineCap::Round,
                        ..Stroke::default()
                    };
                    self.image
                        .stroke_path(&element.path, &paint, &stroke, matrix, None);
                }
            }
        }
        self.image.data()
    }
}
#[cfg(test)]
pub fn render(project: &Project, time: f32, scale: u32) -> Vec<u8> {
    Renderer::new(scale).render(project, time).to_vec()
}

fn prepare(asset: &Asset) -> PreparedAsset {
    let mut elements = Vec::new();
    let mut add = |builder: PathBuilder, color: [u8; 4], fill: bool, width: f32| {
        if let Some(path) = builder.finish() {
            elements.push(PreparedElement {
                path,
                color,
                fill,
                width,
            });
        }
    };
    if asset.content == AssetContent::Builtin {
        let mut b = PathBuilder::new();
        match asset.kind {
            Shape::Ball | Shape::Oval => b.push_circle(0.0, 0.0, 35.0),
            Shape::Rectangle => b.push_rect(Rect::from_xywh(-35.0, -35.0, 70.0, 70.0).unwrap()),
        }
        add(b, asset.color, true, 1.0);
    } else {
        for element in &asset.elements {
            let mut b = PathBuilder::new();
            let (color, fill, width) = match element {
                DrawingElement::Line {
                    from,
                    to,
                    color,
                    width,
                } => {
                    b.move_to(from[0], from[1]);
                    b.line_to(to[0], to[1]);
                    (*color, false, *width)
                }
                DrawingElement::Stroke {
                    points,
                    color,
                    width,
                } => {
                    if let Some(first) = points.first() {
                        if points.len() == 1 {
                            b.push_circle(first[0], first[1], *width / 2.0);
                        } else {
                            b.move_to(first[0], first[1]);
                            for p in &points[1..] {
                                b.line_to(p[0], p[1]);
                            }
                        }
                    }
                    (*color, points.len() == 1, *width)
                }
                DrawingElement::Rect {
                    from,
                    to,
                    angle,
                    color,
                    fill,
                    width,
                }
                | DrawingElement::Oval {
                    from,
                    to,
                    angle,
                    color,
                    fill,
                    width,
                } => {
                    let center = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
                    let x = from[0].min(to[0]);
                    let y = from[1].min(to[1]);
                    let rect =
                        Rect::from_xywh(x, y, (to[0] - from[0]).abs(), (to[1] - from[1]).abs());
                    if *angle == 0.0 && matches!(element, DrawingElement::Oval { .. }) {
                        if let Some(rect) = rect {
                            b.push_oval(rect);
                        }
                    } else if *angle == 0.0 {
                        if let Some(rect) = rect {
                            b.push_rect(rect);
                        }
                    } else if matches!(element, DrawingElement::Oval { .. }) {
                        let rx = (to[0] - from[0]).abs() / 2.0;
                        let ry = (to[1] - from[1]).abs() / 2.0;
                        for i in 0..=64 {
                            let a = std::f32::consts::TAU * i as f32 / 64.0;
                            let p = rotate_point(
                                [center[0] + rx * a.cos(), center[1] + ry * a.sin()],
                                center,
                                *angle,
                            );
                            if i == 0 {
                                b.move_to(p[0], p[1]);
                            } else {
                                b.line_to(p[0], p[1]);
                            }
                        }
                    } else {
                        for (i, point) in [
                            [from[0], from[1]],
                            [to[0], from[1]],
                            [to[0], to[1]],
                            [from[0], to[1]],
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            let p = rotate_point(point, center, *angle);
                            if i == 0 {
                                b.move_to(p[0], p[1]);
                            } else {
                                b.line_to(p[0], p[1]);
                            }
                        }
                        b.close();
                    }
                    (*color, *fill, *width)
                }
            };
            add(b, color, fill, width);
        }
    }
    PreparedAsset {
        source: asset.clone(),
        elements,
    }
}

/// Render an asset into a small square without modifying project data.
pub(crate) fn thumbnail(asset: &Asset) -> Vec<u8> {
    const SIZE: u32 = 96;
    let prepared = prepare(asset);
    let mut image = Pixmap::new(SIZE, SIZE).expect("valid thumbnail dimensions");
    image.fill(Color::from_rgba8(250, 250, 252, 255));
    let mut bounds: Option<[f64; 4]> = None;
    for element in &prepared.elements {
        let rect = element.path.bounds();
        let pad = if element.fill {
            0.0
        } else {
            element.width as f64 / 2.0
        };
        let b = [
            rect.left() as f64 - pad,
            rect.top() as f64 - pad,
            rect.right() as f64 + pad,
            rect.bottom() as f64 + pad,
        ];
        bounds = Some(match bounds {
            None => b,
            Some(old) => [
                old[0].min(b[0]),
                old[1].min(b[1]),
                old[2].max(b[2]),
                old[3].max(b[3]),
            ],
        });
    }
    if let Some(b) = bounds {
        let extent = (b[2] - b[0]).max(b[3] - b[1]).max(1.0);
        let scale = (80.0 / extent) as f32;
        let tx = (48.0 - ((b[0] + b[2]) / 2.0) * scale as f64) as f32;
        let ty = (48.0 - ((b[1] + b[3]) / 2.0) * scale as f64) as f32;
        let matrix = Matrix::from_row(scale, 0.0, 0.0, scale, tx, ty);
        for element in &prepared.elements {
            let mut paint = Paint::default();
            paint.set_color_rgba8(
                element.color[0],
                element.color[1],
                element.color[2],
                element.color[3],
            );
            if element.fill {
                image.fill_path(&element.path, &paint, FillRule::Winding, matrix, None);
            } else {
                let stroke = Stroke {
                    width: element.width,
                    line_cap: LineCap::Round,
                    ..Stroke::default()
                };
                image.stroke_path(&element.path, &paint, &stroke, matrix, None);
            }
        }
    }
    image.data().to_vec()
}

/// Local-space bounds of one element, expanded by half the stroke width.
pub fn element_bounds(element: &DrawingElement) -> Option<[f32; 4]> {
    let mut result: Option<[f32; 4]> = None;
    let mut include = |p: [f32; 2], width: f32| {
        let r = width / 2.0;
        let b = result.get_or_insert([p[0] - r, p[1] - r, p[0] + r, p[1] + r]);
        b[0] = b[0].min(p[0] - r);
        b[1] = b[1].min(p[1] - r);
        b[2] = b[2].max(p[0] + r);
        b[3] = b[3].max(p[1] + r);
    };
    match element {
        DrawingElement::Line {
            from, to, width, ..
        } => {
            include(*from, *width);
            include(*to, *width);
        }
        DrawingElement::Rect {
            from,
            to,
            angle,
            width,
            ..
        } => {
            let center = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
            for point in [
                [from[0], from[1]],
                [to[0], from[1]],
                [to[0], to[1]],
                [from[0], to[1]],
            ] {
                include(rotate_point(point, center, *angle), *width);
            }
        }
        DrawingElement::Oval {
            from,
            to,
            angle,
            width,
            ..
        } => {
            let center = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
            let rx = (to[0] - from[0]).abs() / 2.0;
            let ry = (to[1] - from[1]).abs() / 2.0;
            for i in 0..64 {
                let a = std::f32::consts::TAU * i as f32 / 64.0;
                include(
                    rotate_point(
                        [center[0] + rx * a.cos(), center[1] + ry * a.sin()],
                        center,
                        *angle,
                    ),
                    *width,
                );
            }
        }
        DrawingElement::Stroke { points, width, .. } => {
            for point in points {
                include(*point, *width);
            }
        }
    }
    result
}

/// Local-space bounds, expanded by stroke width.
pub fn bounds(asset: &Asset) -> Option<[f32; 4]> {
    if asset.content == AssetContent::Builtin {
        return Some([-35.0, -35.0, 35.0, 35.0]);
    }
    asset
        .elements
        .iter()
        .filter_map(element_bounds)
        .reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        })
}

fn segment_distance_squared(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let x = p[0] - (a[0] + dx * t);
    let y = p[1] - (a[1] + dy * t);
    x * x + y * y
}
fn ellipse_hit(p: [f32; 2], from: [f32; 2], to: [f32; 2], fill: bool, radius: f32) -> bool {
    let center = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
    let rx = (from[0] - to[0]).abs() / 2.0;
    let ry = (from[1] - to[1]).abs() / 2.0;
    if rx <= 0.0 || ry <= 0.0 {
        return false;
    }
    if (p[0] - center[0]).abs() > rx + radius || (p[1] - center[1]).abs() > ry + radius {
        return false;
    }
    if fill {
        return ((p[0] - center[0]) / (rx + radius)).powi(2)
            + ((p[1] - center[1]) / (ry + radius)).powi(2)
            <= 1.0;
    }
    // Measure distance to a polygonal approximation of the ellipse outline.
    // The step count also grows for larger ellipses to keep the approximation accurate.
    let steps = ((rx.max(ry) * 0.5).ceil() as usize).clamp(64, 1024);
    let at = |i: usize| {
        let a = std::f32::consts::TAU * i as f32 / steps as f32;
        [center[0] + rx * a.cos(), center[1] + ry * a.sin()]
    };
    let mut a = at(0);
    for i in 1..=steps {
        let b = at(i);
        if segment_distance_squared(p, a, b) <= radius * radius {
            return true;
        }
        a = b;
    }
    false
}
/// Hit-test actual drawing geometry in object-local coordinates, with a screen-derived tolerance.
pub fn contains(asset: &Asset, p: [f32; 2], tolerance: f32) -> bool {
    if asset.content == AssetContent::Builtin {
        if asset.color[3] == 0 {
            return false;
        }
        return match asset.kind {
            Shape::Ball | Shape::Oval => p[0] * p[0] + p[1] * p[1] <= (35.0 + tolerance).powi(2),
            Shape::Rectangle => p[0].abs() <= 35.0 + tolerance && p[1].abs() <= 35.0 + tolerance,
        };
    }
    asset
        .elements
        .iter()
        .rev()
        .any(|element| contains_element(element, p, tolerance))
}

pub fn contains_element(element: &DrawingElement, p: [f32; 2], tolerance: f32) -> bool {
    match element {
        DrawingElement::Line {
            from,
            to,
            color,
            width,
        } => {
            color[3] != 0
                && segment_distance_squared(p, *from, *to) <= (width / 2.0 + tolerance).powi(2)
        }
        DrawingElement::Stroke {
            points,
            color,
            width,
        } => {
            let r = (width / 2.0 + tolerance).powi(2);
            color[3] != 0
                && (points
                    .first()
                    .is_some_and(|a| segment_distance_squared(p, *a, *a) <= r)
                    || points
                        .windows(2)
                        .any(|pair| segment_distance_squared(p, pair[0], pair[1]) <= r))
        }
        DrawingElement::Rect {
            from,
            to,
            angle,
            color,
            fill,
            width,
        } => {
            if color[3] == 0 {
                return false;
            }
            let center = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
            let p = rotate_point(p, center, -*angle);
            let (x1, x2) = (from[0].min(to[0]), from[0].max(to[0]));
            let (y1, y2) = (from[1].min(to[1]), from[1].max(to[1]));
            if x1 == x2 || y1 == y2 {
                return false;
            }
            if *fill {
                p[0] >= x1 - tolerance
                    && p[0] <= x2 + tolerance
                    && p[1] >= y1 - tolerance
                    && p[1] <= y2 + tolerance
            } else {
                let r = (width / 2.0 + tolerance).powi(2);
                let points = [[x1, y1], [x2, y1], [x2, y2], [x1, y2], [x1, y1]];
                points
                    .windows(2)
                    .any(|pair| segment_distance_squared(p, pair[0], pair[1]) <= r)
            }
        }
        DrawingElement::Oval {
            from,
            to,
            angle,
            color,
            fill,
            width,
        } => {
            color[3] != 0
                && ellipse_hit(
                    rotate_point(
                        p,
                        [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0],
                        -*angle,
                    ),
                    *from,
                    *to,
                    *fill,
                    if *fill {
                        tolerance
                    } else {
                        width / 2.0 + tolerance
                    },
                )
        }
    }
}

pub fn local_point(t: Transform, x: f32, y: f32) -> [f32; 2] {
    let (sin, cos) = rotation_sin_cos(t.angle);
    let x = x - t.x;
    let y = y - t.y;
    [
        (cos * x + sin * y) / t.scale,
        (-sin * x + cos * y) / t.scale,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AssetContent, Pose, SceneObject};
    use uuid::Uuid;
    fn project_with(element: DrawingElement, transform: Transform) -> Project {
        let mut p = Project::new();
        p.assets[0].content = AssetContent::Drawing;
        p.assets[0].elements = vec![element];
        p.objects.push(SceneObject {
            id: Uuid::new_v4(),
            asset_id: p.assets[0].id,
            name: "test".into(),
            poses: vec![Pose {
                time: 0.0,
                transform,
                linear: false,
            }],
        });
        p
    }
    #[test]
    fn thumbnail_reflects_content_and_empty_drawing() {
        let mut asset = Project::new().assets.remove(0);
        let builtin = thumbnail(&asset);
        asset.color = [3, 90, 200, 255];
        assert_ne!(builtin, thumbnail(&asset));
        asset.content = AssetContent::Drawing;
        let empty = thumbnail(&asset);
        assert!(
            empty
                .chunks_exact(4)
                .all(|pixel| pixel == [250, 250, 252, 255])
        );
        asset.elements.push(DrawingElement::Line {
            from: [-10.0, 0.0],
            to: [10.0, 0.0],
            color: [0, 0, 0, 255],
            width: 2.0,
        });
        assert_ne!(empty, thumbnail(&asset));
    }
    #[test]
    fn changing_preview_scale_keeps_geometry_and_updates_buffer() {
        let project = project_with(
            DrawingElement::Rect {
                from: [-20.0, -20.0],
                to: [20.0, 20.0],
                angle: 0.0,
                color: [200, 20, 30, 255],
                fill: true,
                width: 1.0,
            },
            Transform::default(),
        );
        let mut renderer = Renderer::new(1);
        let one = renderer.render(&project, 0.0).to_vec();
        assert_eq!(renderer.assets.len(), 1);
        renderer.set_scale(2);
        assert_eq!(renderer.assets.len(), 1);
        let two = renderer.render(&project, 0.0);
        assert_eq!(two.len(), 1280 * 720 * 4);
        assert_eq!(pixel(&one, 640, 320, 180), pixel(two, 1280, 640, 360));
    }
    fn pixel(buf: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        buf[(y * width + x) * 4..(y * width + x) * 4 + 4]
            .try_into()
            .unwrap()
    }
    #[test]
    #[ignore = "manual shared-asset comparison measurement"]
    fn shared_asset_comparison_measurement() {
        let project = project_with(
            DrawingElement::Stroke {
                points: (0..20_000).map(|i| [i as f32, (i % 17) as f32]).collect(),
                color: [20, 40, 60, 255],
                width: 2.0,
            },
            Transform::default(),
        );
        let asset = project.assets[0].clone();
        let rounds = 1000;
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            for _ in 0..50 {
                std::hint::black_box(
                    std::hint::black_box(&asset) == std::hint::black_box(&project.assets[0]),
                );
            }
        }
        let per_frame = start.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            std::hint::black_box(
                std::hint::black_box(&asset) == std::hint::black_box(&project.assets[0]),
            );
        }
        let once = start.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
        eprintln!(
            "shared 20,000-point drawing: 50 comparisons {per_frame:.3} ms/frame, one comparison {once:.3} ms/frame"
        );
    }
    #[test]
    fn shared_asset_updates_every_instance_after_an_edit() {
        let mut project = project_with(
            DrawingElement::Rect {
                from: [-10.0, -10.0],
                to: [10.0, 10.0],
                angle: 0.0,
                color: [255, 0, 0, 255],
                fill: true,
                width: 1.0,
            },
            Transform {
                x: -100.0,
                ..Transform::default()
            },
        );
        let mut second = project.objects[0].clone();
        second.id = Uuid::new_v4();
        second.poses[0].transform.x = 100.0;
        project.objects.push(second);
        let mut renderer = Renderer::new(1);
        let before = renderer.render(&project, 0.0).to_vec();
        for x in [220, 420] {
            assert_eq!(pixel(&before, 640, x, 180), [255, 0, 0, 255]);
        }
        let DrawingElement::Rect { color, .. } = &mut project.assets[0].elements[0] else {
            unreachable!()
        };
        *color = [0, 0, 255, 255];
        let after = renderer.render(&project, 0.0);
        for x in [220, 420] {
            assert_eq!(pixel(after, 640, x, 180), [0, 0, 255, 255]);
        }
    }
    #[test]
    fn extreme_but_finite_geometry_does_not_crash_renderer() {
        let project = project_with(
            DrawingElement::Rect {
                from: [-f32::MAX, -1.0],
                to: [f32::MAX, 1.0],
                angle: 0.0,
                color: [255, 0, 0, 255],
                fill: true,
                width: 1.0,
            },
            Transform {
                x: f32::MAX,
                y: -f32::MAX,
                scale: f32::MAX,
                angle: f32::MAX,
            },
        );
        assert!(crate::storage::validate(&project).is_ok());
        assert_eq!(render(&project, 0.0, 1).len(), 640 * 360 * 4);
        assert_eq!(render(&project, 0.0, 2).len(), 1280 * 720 * 4);
    }
    #[test]
    fn extreme_angle_matches_its_equivalent_rotation() {
        let mut project = project_with(
            DrawingElement::Rect {
                from: [-40.0, -10.0],
                to: [40.0, 10.0],
                angle: 0.0,
                color: [255, 0, 0, 255],
                fill: true,
                width: 1.0,
            },
            Transform {
                angle: f32::MAX,
                ..Transform::default()
            },
        );
        let extreme = render(&project, 0.0, 1);
        let extreme_export = render(&project, 0.0, 2);
        let extreme_local = local_point(project.objects[0].poses[0].transform, 15.0, 20.0);
        project.objects[0].poses[0].transform.angle = (f32::MAX as f64).rem_euclid(360.0) as f32;
        assert!(extreme == render(&project, 0.0, 1));
        assert!(extreme_export == render(&project, 0.0, 2));
        assert_eq!(
            extreme_local,
            local_point(project.objects[0].poses[0].transform, 15.0, 20.0)
        );
    }
    #[test]
    fn rotated_drawing_and_position_are_rendered_at_both_sizes() {
        let p = project_with(
            DrawingElement::Rect {
                from: [-40.0, -10.0],
                to: [40.0, 10.0],
                angle: 0.0,
                color: [255, 0, 0, 255],
                fill: true,
                width: 1.0,
            },
            Transform {
                x: 30.0,
                y: 20.0,
                angle: 90.0,
                ..Transform::default()
            },
        );
        let preview = render(&p, 0.0, 1);
        let export = render(&p, 0.0, 2);
        for (x, y, red) in [(350, 225, true), (380, 200, false), (350, 200, true)] {
            assert_eq!(pixel(&preview, 640, x, y)[0..3] == [255, 0, 0], red);
            assert_eq!(pixel(&export, 1280, x * 2, y * 2)[0..3] == [255, 0, 0], red);
        }
    }
    #[test]
    fn ellipse_is_not_a_circle_and_empty_drawing_is_empty() {
        let mut p = project_with(
            DrawingElement::Oval {
                from: [-60.0, -10.0],
                to: [60.0, 10.0],
                angle: 0.0,
                color: [0, 0, 255, 255],
                fill: true,
                width: 1.0,
            },
            Transform::default(),
        );
        let frame = render(&p, 0.0, 1);
        assert_eq!(pixel(&frame, 640, 365, 180), [0, 0, 255, 255]);
        assert_eq!(pixel(&frame, 640, 320, 195), [250, 250, 252, 255]);
        p.assets[0].elements.clear();
        assert_eq!(
            pixel(&render(&p, 0.0, 1), 640, 320, 180),
            [250, 250, 252, 255]
        );
    }
    #[test]
    fn alpha_and_object_order_are_preserved_in_frame() {
        let mut p = project_with(
            DrawingElement::Rect {
                from: [-20.0, -20.0],
                to: [20.0, 20.0],
                angle: 0.0,
                color: [255, 0, 0, 128],
                fill: true,
                width: 1.0,
            },
            Transform::default(),
        );
        let mut back = p.assets[0].clone();
        back.id = Uuid::new_v4();
        back.elements = vec![DrawingElement::Rect {
            from: [-20.0, -20.0],
            to: [20.0, 20.0],
            angle: 0.0,
            color: [0, 0, 255, 255],
            fill: true,
            width: 1.0,
        }];
        p.assets.push(back.clone());
        p.objects.insert(
            0,
            SceneObject {
                id: Uuid::new_v4(),
                asset_id: back.id,
                name: "back".into(),
                poses: vec![Pose {
                    time: 0.0,
                    transform: Transform::default(),
                    linear: false,
                }],
            },
        );
        let color = pixel(&render(&p, 0.0, 1), 640, 320, 180);
        assert!(
            (120..=135).contains(&color[0]) && color[1] < 5 && (120..=135).contains(&color[2]),
            "{color:?}"
        );
        p.objects.swap(0, 1);
        assert_eq!(pixel(&render(&p, 0.0, 1), 640, 320, 180), [0, 0, 255, 255]);
    }
    #[test]
    fn cached_geometry_updates_for_edits_undo_and_reused_ids() {
        let mut p = project_with(
            DrawingElement::Rect {
                from: [-10.0, -10.0],
                to: [10.0, 10.0],
                angle: 0.0,
                color: [255, 0, 0, 255],
                fill: true,
                width: 1.0,
            },
            Transform::default(),
        );
        let original = p.clone();
        let mut renderer = Renderer::new(1);
        let first = renderer.render(&p, 0.0).to_vec();
        p.assets[0].elements = vec![DrawingElement::Oval {
            from: [-40.0, -10.0],
            to: [40.0, 10.0],
            angle: 0.0,
            color: [0, 0, 255, 255],
            fill: true,
            width: 1.0,
        }];
        let changed = renderer.render(&p, 0.0).to_vec();
        assert_ne!(first, changed);
        assert_eq!(changed, render(&p, 0.0, 1));
        assert_eq!(renderer.render(&original, 0.0), first);
        p.assets.clear();
        p.objects.clear();
        renderer.render(&p, 0.0);
        assert_eq!(renderer.render(&original, 0.0), first);
    }
    #[test]
    fn hit_testing_uses_geometry_and_inverse_transform() {
        let mut p = project_with(
            DrawingElement::Oval {
                from: [-60.0, -20.0],
                to: [60.0, 20.0],
                angle: 0.0,
                color: [0, 0, 255, 255],
                fill: false,
                width: 4.0,
            },
            Transform::default(),
        );
        let a = &p.assets[0];
        assert!(!contains(a, [0.0, 0.0], 2.0));
        assert!(contains(a, [60.0, 0.0], 2.0));
        assert!(!contains(a, [55.0, 18.0], 2.0));
        p.assets[0].elements = vec![DrawingElement::Line {
            from: [-30.0, 0.0],
            to: [30.0, 0.0],
            color: [0, 0, 0, 255],
            width: 2.0,
        }];
        let transform = Transform {
            x: 10.0,
            y: 20.0,
            scale: 2.0,
            angle: 90.0,
        };
        assert!(contains(
            &p.assets[0],
            local_point(transform, 10.0, 60.0),
            1.0
        ));
        assert!(!contains(
            &p.assets[0],
            local_point(transform, 25.0, 60.0),
            1.0
        ));
        p.assets[0].elements.clear();
        assert!(!contains(&p.assets[0], [0.0, 0.0], 2.0));
    }
    #[test]
    fn inverse_transform_recovers_local_coordinates() {
        let p = local_point(
            Transform {
                x: 10.0,
                y: 20.0,
                scale: 2.0,
                angle: 90.0,
            },
            10.0,
            40.0,
        );
        assert!((p[0] - 10.0).abs() < 0.001 && p[1].abs() < 0.001);
    }
}

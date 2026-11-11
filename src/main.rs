use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use serde::{Deserialize, Serialize};
use std::{fs, io::{Read, Write}, path::{Path, PathBuf}, process::{Command, Stdio}, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}}, thread, time::{Duration, Instant}};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect as SkRect, Transform as SkTransform};
use uuid::Uuid;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const STEP: f32 = 0.1;
const DEFAULT_DURATION: f32 = 10.0;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq)]
struct Transform { x: f32, y: f32, scale: f32, angle: f32 }
impl Default for Transform { fn default() -> Self { Self { x: 0.0, y: 0.0, scale: 1.0, angle: 0.0 } } }

#[derive(Clone, Serialize, Deserialize)]
struct Pose { time: f32, transform: Transform, linear: bool }
#[derive(Clone, Serialize, Deserialize)]
struct Asset { id: Uuid, name: String, color: [u8; 4], kind: Shape, #[serde(default)] elements: Vec<DrawingElement> }
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq)]
enum Shape { Ball, Rectangle, Oval }
#[derive(Clone, Serialize, Deserialize)]
enum DrawingElement {
    Line { from: [f32; 2], to: [f32; 2], color: [u8; 4], width: f32 },
    Rect { from: [f32; 2], to: [f32; 2], color: [u8; 4], fill: bool, width: f32 },
    Oval { from: [f32; 2], to: [f32; 2], color: [u8; 4], fill: bool, width: f32 },
    Stroke { points: Vec<[f32; 2]>, color: [u8; 4], width: f32 },
}
#[derive(Clone, Copy, PartialEq)]
enum EditorTool { Pencil, Line, Rectangle, Oval, Select }
#[derive(Clone, Serialize, Deserialize)]
struct SceneObject { id: Uuid, asset_id: Uuid, name: String, poses: Vec<Pose> }
#[derive(Clone, Serialize, Deserialize)]
struct Project { version: u32, name: String, duration: f32, assets: Vec<Asset>, objects: Vec<SceneObject> }

impl Project {
    fn new() -> Self {
        let ball = Asset { id: Uuid::new_v4(), name: "Мяч".into(), color: [239, 113, 69, 255], kind: Shape::Ball, elements: vec![] };
        Self { version: 1, name: "Мой мультфильм".into(), duration: DEFAULT_DURATION, assets: vec![ball], objects: vec![] }
    }
    fn asset(&self, id: Uuid) -> Option<&Asset> { self.assets.iter().find(|a| a.id == id) }
}

fn snap(t: f32) -> f32 { (t / STEP).round() * STEP }
fn angle_delta(from: f32, to: f32) -> f32 { (to - from + 180.0).rem_euclid(360.0) - 180.0 }
fn eased(t: f32) -> f32 { t * t * (3.0 - 2.0 * t) }
fn state_at(object: &SceneObject, time: f32) -> Transform {
    let poses = &object.poses;
    if poses.is_empty() { return Transform::default(); }
    if time <= poses[0].time { return poses[0].transform; }
    for pair in poses.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if time <= b.time {
            let mut p = (time - a.time) / (b.time - a.time);
            if !a.linear { p = eased(p); }
            return Transform {
                x: a.transform.x + (b.transform.x - a.transform.x) * p,
                y: a.transform.y + (b.transform.y - a.transform.y) * p,
                scale: a.transform.scale + (b.transform.scale - a.transform.scale) * p,
                angle: a.transform.angle + angle_delta(a.transform.angle, b.transform.angle) * p,
            };
        }
    }
    poses.last().unwrap().transform
}

fn c(color: [u8; 4]) -> Color32 { Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]) }
fn pt(origin: Pos2, p: [f32; 2], scale: f32) -> Pos2 { origin + Vec2::new(p[0], p[1]) * scale }
fn draw_elements(painter: &egui::Painter, elements: &[DrawingElement], origin: Pos2, scale: f32) {
    for element in elements {
        match element {
            DrawingElement::Line { from, to, color, width } => { painter.line_segment([pt(origin, *from, scale), pt(origin, *to, scale)], Stroke::new(width * scale, c(*color))); }
            DrawingElement::Stroke { points, color, width } => {
                for pair in points.windows(2) { painter.line_segment([pt(origin, pair[0], scale), pt(origin, pair[1], scale)], Stroke::new(width * scale, c(*color))); }
            }
            DrawingElement::Rect { from, to, color, fill, width } => { let r = Rect::from_two_pos(pt(origin, *from, scale), pt(origin, *to, scale)); if *fill { painter.rect_filled(r, 3.0, c(*color)); } else { painter.rect_stroke(r, 3.0, Stroke::new(width * scale, c(*color)), egui::StrokeKind::Middle); } }
            DrawingElement::Oval { from, to, color, fill, width } => { let r = Rect::from_two_pos(pt(origin, *from, scale), pt(origin, *to, scale)); if *fill { painter.circle_filled(r.center(), r.width().min(r.height()) / 2.0, c(*color)); } else { painter.circle_stroke(r.center(), r.width().min(r.height()) / 2.0, Stroke::new(width * scale, c(*color))); } }
        }
    }
}

fn write_project(path: &Path, project: &Project) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|e| e.to_string())?;
    let mut archive = ZipWriter::new(file);
    archive.start_file("project.json", SimpleFileOptions::default()).map_err(|e| e.to_string())?;
    archive.write_all(&serde_json::to_vec_pretty(project).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    archive.finish().map_err(|e| e.to_string())?;
    Ok(())
}
fn read_project(path: &Path) -> Result<Project, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    match ZipArchive::new(file) {
        Ok(mut archive) => { let mut json = String::new(); archive.by_name("project.json").map_err(|_| "В архиве нет project.json".to_string())?.read_to_string(&mut json).map_err(|e| e.to_string())?; serde_json::from_str(&json).map_err(|e| e.to_string()) }
        Err(_) => serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string()),
    }
}
fn safely_write_project(path: &Path, project: &Project) -> Result<(), String> {
    let temporary = path.with_extension("animapro.tmp");
    write_project(&temporary, project)?;
    fs::rename(&temporary, path).map_err(|e| { let _ = fs::remove_file(&temporary); e.to_string() })
}
fn recovery_path() -> PathBuf { std::env::temp_dir().join("animapro-autorecovery.animapro") }

struct ExportJob { progress: Arc<AtomicUsize>, total: usize, cancel: Arc<AtomicBool>, result: Arc<Mutex<Option<Result<PathBuf, String>>>> }
fn export_temp_path(output: &Path) -> PathBuf { output.with_file_name(format!("{}.part.mp4", output.file_stem().unwrap_or_default().to_string_lossy())) }
fn export_frame(project: &Project, time: f32) -> Vec<u8> {
    let mut pixmap = Pixmap::new(1280, 720).expect("valid export dimensions"); pixmap.fill(Color::from_rgba8(250, 250, 252, 255));
    for object in &project.objects { let Some(asset) = project.asset(object.asset_id) else { continue }; let tr = state_at(object, time); let x = 640.0 + tr.x; let y = 360.0 + tr.y; let r = 70.0 * tr.scale; let mut paint = Paint::default(); paint.set_color_rgba8(asset.color[0], asset.color[1], asset.color[2], asset.color[3]); let mut path = PathBuilder::new(); match asset.kind { Shape::Ball | Shape::Oval => path.push_circle(x, y, r), Shape::Rectangle => if let Some(rect) = SkRect::from_xywh(x-r, y-r, r*2.0, r*2.0) { path.push_rect(rect) } }; if let Some(path) = path.finish() { pixmap.fill_path(&path, &paint, FillRule::Winding, SkTransform::identity(), None); } }
    pixmap.take()
}
fn bundled_ffmpeg() -> PathBuf {
    let executable = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("animapro"));
    let sibling = executable.parent().unwrap_or_else(|| Path::new(".")).join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
    if sibling.is_file() { return sibling; }
    let app_resource = executable.parent().and_then(|p| p.parent()).map(|p| p.join("Resources").join("ffmpeg"));
    if let Some(path) = app_resource.filter(|p| p.is_file()) { return path; }
    PathBuf::from(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" })
}
fn start_export(project: Project, output: PathBuf) -> ExportJob {
    let total = (project.duration * 30.0).round().max(1.0) as usize; let progress = Arc::new(AtomicUsize::new(0)); let cancel = Arc::new(AtomicBool::new(false)); let result = Arc::new(Mutex::new(None));
    let job = ExportJob { progress: progress.clone(), total, cancel: cancel.clone(), result: result.clone() };
    thread::spawn(move || { let temporary = export_temp_path(&output); let task = (|| -> Result<PathBuf, String> { let mut child = Command::new(bundled_ffmpeg()).args(["-y", "-f", "rawvideo", "-pixel_format", "rgba", "-video_size", "1280x720", "-framerate", "30", "-i", "-", "-an", "-c:v", "libx264", "-pix_fmt", "yuv420p", temporary.to_string_lossy().as_ref()]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("Не удалось запустить FFmpeg: {e}"))?; let mut input = child.stdin.take().ok_or("Не удалось открыть поток FFmpeg")?; for frame in 0..total { if cancel.load(Ordering::Relaxed) { drop(input); let _ = child.kill(); let _ = child.wait(); let _ = fs::remove_file(&temporary); return Err("Экспорт отменён".into()); } input.write_all(&export_frame(&project, frame as f32 / 30.0)).map_err(|e| format!("Ошибка записи кадра: {e}"))?; progress.store(frame + 1, Ordering::Relaxed); } drop(input); let output_status = child.wait_with_output().map_err(|e| e.to_string())?; if !output_status.status.success() { let _ = fs::remove_file(&temporary); return Err(format!("FFmpeg завершился с ошибкой: {}", String::from_utf8_lossy(&output_status.stderr))); } fs::rename(&temporary, &output).map_err(|e| format!("Не удалось поместить готовое видео: {e}"))?; Ok(output) })(); *result.lock().expect("export state") = Some(task); }); job
}

struct AnimaApp {
    project: Project, selected: Option<Uuid>, time: f32, playing: bool, last_frame: Instant,
    undo: Vec<Project>, redo: Vec<Project>, file: Option<PathBuf>, dirty: bool, message: String,
    edit_asset: Option<Uuid>, editor_tool: EditorTool, editor_color: [u8; 4], editor_width: f32,
    editor_fill: bool, editor_start: Option<Pos2>, editor_stroke: Vec<Pos2>, editor_selected: Option<usize>, last_autosave: Instant, export: Option<ExportJob>,
}
impl Default for AnimaApp { fn default() -> Self { let recovered = read_project(&recovery_path()).ok(); let has_recovery = recovered.is_some(); Self { project: recovered.unwrap_or_else(Project::new), selected: None, time: 0.0, playing: false, last_frame: Instant::now(), undo: vec![], redo: vec![], file: None, dirty: has_recovery, message: if has_recovery { "Восстановлена автокопия. Сохраните проект в выбранную папку.".into() } else { "Добро пожаловать! 1. Добавьте мяч  2. Передвиньте его  3. Нажмите ▶".into() }, edit_asset: None, editor_tool: EditorTool::Pencil, editor_color: [42, 83, 170, 255], editor_width: 5.0, editor_fill: true, editor_start: None, editor_stroke: vec![], editor_selected: None, last_autosave: Instant::now(), export: None } } }
impl AnimaApp {
    fn change(&mut self) { self.undo.push(self.project.clone()); if self.undo.len() > 100 { self.undo.remove(0); } self.redo.clear(); self.dirty = true; }
    fn undo(&mut self) { if let Some(previous) = self.undo.pop() { self.redo.push(self.project.clone()); self.project = previous; self.dirty = true; } }
    fn redo(&mut self) { if let Some(next) = self.redo.pop() { self.undo.push(self.project.clone()); self.project = next; self.dirty = true; } }
    fn selected_object(&self) -> Option<&SceneObject> { self.selected.and_then(|id| self.project.objects.iter().find(|o| o.id == id)) }
    fn add_object(&mut self, asset_id: Uuid) { self.change(); let asset = self.project.asset(asset_id).unwrap(); let n = self.project.objects.iter().filter(|o| o.asset_id == asset_id).count() + 1; let id = Uuid::new_v4(); self.project.objects.push(SceneObject { id, asset_id, name: format!("{} {}", asset.name, n), poses: vec![Pose { time: 0.0, transform: Transform::default(), linear: false }] }); self.selected = Some(id); self.message = "Объект добавлен. Перетащите его на сцене!".into(); }
    fn upsert_pose(&mut self, id: Uuid, tr: Transform) { self.change(); let t = snap(self.time); if let Some(o) = self.project.objects.iter_mut().find(|o| o.id == id) { if let Some(p) = o.poses.iter_mut().find(|p| (p.time - t).abs() < 0.01) { p.transform = tr; } else { o.poses.push(Pose { time: t, transform: tr, linear: false }); o.poses.sort_by(|a,b| a.time.total_cmp(&b.time)); } } }
    fn save(&mut self, choose: bool) {
        if choose || self.file.is_none() { self.file = rfd::FileDialog::new().add_filter("Проект AnimaPro", &["animapro"]).set_file_name("мой-мультфильм.animapro").save_file(); }
        if let Some(path) = &self.file { match safely_write_project(path, &self.project) { Ok(()) => { self.dirty = false; let _ = fs::remove_file(recovery_path()); self.message = format!("Сохранено: {}", path.file_name().unwrap_or_default().to_string_lossy()); }, Err(e) => self.message = format!("Не удалось сохранить: {e}"), } }
    }
    fn open(&mut self) { if let Some(path) = rfd::FileDialog::new().add_filter("Проект AnimaPro", &["animapro"]).pick_file() { match read_project(&path) { Ok(p) => { self.project = p; self.file = Some(path); self.selected = None; self.time = 0.0; self.dirty = false; self.message = "Проект открыт".into(); }, Err(_) => self.message = "Не удалось открыть проект: файл повреждён или имеет неверный формат".into() } } }
    fn new_project(&mut self) { self.change(); self.project = Project::new(); self.selected = None; self.time = 0.0; self.file = None; self.dirty = false; }
    fn editor_ui(&mut self, ctx: &egui::Context, asset_id: Uuid, open: &mut bool) {
        let Some(index) = self.project.assets.iter().position(|a| a.id == asset_id) else { *open = false; return; };
        let mut asset = self.project.assets[index].clone();
        let mut modified = false;
        egui::TopBottomPanel::top("draw_tools").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading(&asset.name); ui.separator();
                for (tool, label) in [(EditorTool::Pencil,"✎ Карандаш"), (EditorTool::Line,"╱ Линия"), (EditorTool::Rectangle,"□ Прямоугольник"), (EditorTool::Oval,"○ Овал"), (EditorTool::Select,"↖ Выбрать")] { if ui.selectable_label(self.editor_tool == tool, label).clicked() { self.editor_tool = tool; self.editor_selected = None; } }
                ui.separator(); let mut color = c(self.editor_color); if egui::color_picker::color_edit_button_srgba(ui, &mut color, egui::color_picker::Alpha::Opaque).changed() { self.editor_color = [color.r(), color.g(), color.b(), color.a()]; }
                ui.add(egui::Slider::new(&mut self.editor_width, 1.0..=20.0).text("Толщина"));
                ui.checkbox(&mut self.editor_fill, "Заливка");
                if ui.button("Создать копию").clicked() { let mut copy = asset.clone(); copy.id = Uuid::new_v4(); copy.name = format!("{} — копия", copy.name); self.change(); self.project.assets.push(copy); self.message = "Создан независимый рисунок".into(); }
                if ui.button("Закрыть").clicked() { *open = false; }
            });
            ui.label("Изменения рисунка сразу видны у всех его объектов на сцене. Для отдельного рисунка используйте «Создать копию».");
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
            let painter = ui.painter_at(rect); painter.rect_filled(rect, 0.0, Color32::WHITE); painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::LIGHT_GRAY), egui::StrokeKind::Inside);
            let origin = rect.center(); let scale = 1.0;
            draw_elements(&painter, &asset.elements, origin, scale);
            if self.editor_tool == EditorTool::Select { if let Some(i) = self.editor_selected { if let Some(el) = asset.elements.get(i) { let bounds = element_bounds(el, origin); painter.rect_stroke(bounds.expand(6.0), 2.0, Stroke::new(2.0, Color32::from_rgb(40, 110, 220)), egui::StrokeKind::Outside); } } }
            if response.drag_started() { self.editor_start = response.interact_pointer_pos(); self.editor_stroke.clear(); if let Some(pos) = self.editor_start { self.editor_stroke.push(pos); if self.editor_tool == EditorTool::Select { self.editor_selected = asset.elements.iter().rposition(|e| element_bounds(e, origin).expand(8.0).contains(pos)); } } }
            if response.dragged() { if self.editor_tool == EditorTool::Pencil { if let Some(pos) = response.interact_pointer_pos() { self.editor_stroke.push(pos); } } else if self.editor_tool == EditorTool::Select { if let Some(i) = self.editor_selected { let d = ctx.input(|i| i.pointer.delta()); translate_element(&mut asset.elements[i], d.x, d.y); modified = true; } } }
            if response.drag_stopped() { if let (Some(start), Some(end)) = (self.editor_start, response.interact_pointer_pos()) { let local = |p: Pos2| [p.x-origin.x, p.y-origin.y]; match self.editor_tool { EditorTool::Pencil if self.editor_stroke.len() > 1 => { asset.elements.push(DrawingElement::Stroke { points: self.editor_stroke.iter().map(|p| local(*p)).collect(), color: self.editor_color, width: self.editor_width }); modified = true; }, EditorTool::Line => { asset.elements.push(DrawingElement::Line { from: local(start), to: local(end), color: self.editor_color, width: self.editor_width }); modified = true; }, EditorTool::Rectangle => { asset.elements.push(DrawingElement::Rect { from: local(start), to: local(end), color: self.editor_color, fill: self.editor_fill, width: self.editor_width }); modified = true; }, EditorTool::Oval => { asset.elements.push(DrawingElement::Oval { from: local(start), to: local(end), color: self.editor_color, fill: self.editor_fill, width: self.editor_width }); modified = true; }, _ => {} } } self.editor_start = None; self.editor_stroke.clear(); }
            if ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) { if let Some(i) = self.editor_selected.take() { if i < asset.elements.len() { asset.elements.remove(i); modified = true; } } }
        });
        if modified { self.change(); self.project.assets[index] = asset; }
    }
}

fn element_bounds(element: &DrawingElement, origin: Pos2) -> Rect {
    let mut points: Vec<[f32; 2]> = match element { DrawingElement::Line { from, to, .. } | DrawingElement::Rect { from, to, .. } | DrawingElement::Oval { from, to, .. } => vec![*from, *to], DrawingElement::Stroke { points, .. } => points.clone() };
    if points.is_empty() { points.push([0.0, 0.0]); }
    let mut r = Rect::from_min_max(pt(origin, points[0], 1.0), pt(origin, points[0], 1.0)); for p in points { r.extend_with(pt(origin, p, 1.0)); } r
}
fn translate_element(element: &mut DrawingElement, x: f32, y: f32) { let move_point = |p: &mut [f32;2]| { p[0] += x; p[1] += y; }; match element { DrawingElement::Line { from, to, .. } | DrawingElement::Rect { from, to, .. } | DrawingElement::Oval { from, to, .. } => { move_point(from); move_point(to); }, DrawingElement::Stroke { points, .. } => for point in points { move_point(point); } } }

impl eframe::App for AnimaApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let now = Instant::now(); let dt = now.duration_since(self.last_frame).as_secs_f32(); self.last_frame = now;
        if self.dirty && now.duration_since(self.last_autosave).as_secs() >= 30 { match safely_write_project(&recovery_path(), &self.project) { Ok(()) => self.message = "Автокопия восстановления обновлена".into(), Err(e) => self.message = format!("Не удалось создать автокопию: {e}") } self.last_autosave = now; }
        let export_result = self.export.as_ref().and_then(|job| job.result.lock().expect("export state").take());
        if let Some(done) = export_result { self.message = match done { Ok(path) => format!("Видео готово: {}", path.file_name().unwrap_or_default().to_string_lossy()), Err(error) => error }; self.export = None; } else if self.export.is_some() { ctx.request_repaint_after(Duration::from_millis(100)); }
        if self.playing { self.time += dt; if self.time > self.project.duration { self.time = 0.0; } ctx.request_repaint(); }
        egui::TopBottomPanel::top("top").show(ctx, |ui| { ui.horizontal_wrapped(|ui| {
            if ui.button("📄 Новый").clicked() { self.new_project(); } if ui.button("📂 Открыть").clicked() { self.open(); }
            if ui.button("💾 Сохранить").clicked() { self.save(false); } if self.export.is_none() && ui.button("🎬 Сделать видео").clicked() { if let Some(path) = rfd::FileDialog::new().add_filter("Видео MP4", &["mp4"]).set_file_name("мой-мультфильм.mp4").save_file() { self.export = Some(start_export(self.project.clone(), path)); self.message = "Подготавливаю видео…".into(); } } if let Some(job) = &self.export { let percent = job.progress.load(Ordering::Relaxed) as f32 / job.total as f32 * 100.0; ui.label(format!("🎬 {:.0}%", percent)); if ui.button("Отменить экспорт").clicked() { job.cancel.store(true, Ordering::Relaxed); } }
            ui.separator(); if ui.button("↶").on_hover_text("Отменить").clicked() { self.undo(); } if ui.button("↷").on_hover_text("Повторить").clicked() { self.redo(); }
            ui.separator(); ui.label(if self.dirty { "● есть изменения" } else { "✓ сохранено" });
        }); });
        egui::SidePanel::left("assets").resizable(false).default_width(180.0).show(ctx, |ui| { ui.heading("Объекты"); ui.label("Нажмите +, чтобы добавить на сцену"); ui.separator();
            let assets = self.project.assets.clone(); for a in assets { ui.horizontal(|ui| { let c = Color32::from_rgba_unmultiplied(a.color[0],a.color[1],a.color[2],a.color[3]); ui.colored_label(c, "●"); ui.label(&a.name); if ui.button("+").clicked() { self.add_object(a.id); } if ui.small_button("✎").on_hover_text("Открыть редактор рисунка").clicked() { self.edit_asset = Some(a.id); } }); }
            ui.separator(); if ui.button("＋ Новый рисунок").clicked() { self.change(); let id = Uuid::new_v4(); self.project.assets.push(Asset { id, name: format!("Рисунок {}", self.project.assets.len()), color: [70, 140, 235, 255], kind: Shape::Rectangle, elements: vec![] }); self.edit_asset = Some(id); self.message = "Новый рисунок открыт в редакторе.".into(); }
        });
        egui::SidePanel::right("properties").resizable(false).default_width(210.0).show(ctx, |ui| { ui.heading("Свойства"); if let Some(id) = self.selected { let tr = self.selected_object().map(|o| state_at(o, self.time)).unwrap_or_default(); ui.label(format!("Поза: {:.1} сек.", self.time)); let mut x=tr.x; let mut y=tr.y; let mut s=tr.scale; let mut a=tr.angle; let mut changed=false; changed |= ui.add(egui::DragValue::new(&mut x).prefix("X: ").speed(1.0)).changed(); changed |= ui.add(egui::DragValue::new(&mut y).prefix("Y: ").speed(1.0)).changed(); changed |= ui.add(egui::Slider::new(&mut s, 0.2..=3.0).text("Размер")).changed(); changed |= ui.add(egui::Slider::new(&mut a, -180.0..=180.0).text("Поворот")).changed(); if changed { self.playing=false; self.upsert_pose(id, Transform{x,y,scale:s,angle:a}); } if ui.button("🗑 Удалить объект").clicked() { self.change(); self.project.objects.retain(|o| o.id != id); self.selected=None; } } else { ui.label("Выберите объект на сцене."); } ui.separator(); ui.label("Длительность мультфильма"); let mut duration=self.project.duration; if ui.add(egui::Slider::new(&mut duration,1.0..=60.0).suffix(" сек.")).changed() { let last=self.project.objects.iter().flat_map(|o|o.poses.iter()).map(|p|p.time).fold(0.0_f32,f32::max); if duration >= last { self.change(); self.project.duration=duration; } else { self.message="Нельзя сократить: за границей есть позы".into(); } } });
        egui::CentralPanel::default().show(ctx, |ui| { ui.heading("Сцена"); ui.label("Перетаскивайте объект — в это время автоматически создаётся поза"); let available=ui.available_size(); let (rect,response)=ui.allocate_exact_size(Vec2::new(available.x, (available.y-3.0).max(80.0)), Sense::click_and_drag()); let painter=ui.painter_at(rect); painter.rect_filled(rect, 8.0, Color32::from_rgb(250,250,252)); painter.rect_stroke(rect, 8.0, Stroke::new(1.0,Color32::LIGHT_GRAY), egui::StrokeKind::Outside); let center=rect.center();
            let draw: Vec<_> = self.project.objects.iter().map(|o|(o.id,o.asset_id,state_at(o,self.time))).collect(); for (id,asset_id,tr) in draw.clone() { if let Some(asset)=self.project.asset(asset_id) { let p=center+Vec2::new(tr.x,tr.y); let radius=35.0*tr.scale; if asset.elements.is_empty() { let color=c(asset.color); match asset.kind { Shape::Ball|Shape::Oval=>painter.circle_filled(p,radius,color), Shape::Rectangle=>painter.rect_filled(Rect::from_center_size(p,Vec2::splat(radius*2.0)),4.0,color) }; } else { draw_elements(&painter, &asset.elements, p, tr.scale); } if self.selected==Some(id) { painter.circle_stroke(p,radius+7.0,Stroke::new(2.0,Color32::from_rgb(50,110,220))); } } }
            if response.clicked() { if let Some(pos)=response.interact_pointer_pos() { self.selected=draw.iter().rev().find(|(_,aid,tr)| self.project.asset(*aid).map(|_| (pos-(center+Vec2::new(tr.x,tr.y))).length()<45.0*tr.scale).unwrap_or(false)).map(|x|x.0); } }
            if response.dragged() { if let (Some(id),Some(pos))=(self.selected,response.interact_pointer_pos()) { let old=self.selected_object().map(|o|state_at(o,self.time)).unwrap_or_default(); self.playing=false; self.upsert_pose(id,Transform{x:pos.x-center.x,y:pos.y-center.y,..old}); } }
        });
        egui::TopBottomPanel::bottom("timeline").resizable(true).default_height(190.0).show(ctx, |ui| { ui.horizontal(|ui| { if ui.button(if self.playing{"⏸ Пауза"}else{"▶ Смотреть"}).clicked(){self.playing=!self.playing;} if ui.button("↺ В начало").clicked(){self.time=0.0;self.playing=false;} ui.label(format!("Время: {:.1} сек.",self.time)); }); ui.label(&self.message); ui.separator(); let width=ui.available_width()-160.0; let (r,resp)=ui.allocate_exact_size(Vec2::new(width,28.0),Sense::click_and_drag()); let p=ui.painter_at(r); p.line_segment([r.left_center(),r.right_center()],Stroke::new(2.0,Color32::GRAY)); for sec in 0..=self.project.duration.ceil() as i32 { let x=r.left()+width*sec as f32/self.project.duration; p.line_segment([Pos2::new(x,r.top()),Pos2::new(x,r.bottom())],Stroke::new(1.0,Color32::LIGHT_GRAY)); p.text(Pos2::new(x+2.0,r.top()),egui::Align2::LEFT_TOP,format!("{sec}"),egui::FontId::proportional(10.0),Color32::DARK_GRAY); } let cx=r.left()+width*self.time/self.project.duration; p.line_segment([Pos2::new(cx,r.top()-4.0),Pos2::new(cx,r.bottom()+4.0)],Stroke::new(2.0,Color32::RED)); if (resp.clicked()||resp.dragged()) && resp.interact_pointer_pos().is_some() { let x=resp.interact_pointer_pos().unwrap().x; self.time=snap(((x-r.left())/width*self.project.duration).clamp(0.0,self.project.duration)); }
            ui.separator(); for o in &self.project.objects { ui.horizontal(|ui| { let selected=self.selected==Some(o.id); if ui.selectable_label(selected,&o.name).clicked(){self.selected=Some(o.id)}; let start=ui.cursor().min; let rr=Rect::from_min_size(start,Vec2::new(width,22.0)); let pp=ui.painter_at(rr); pp.line_segment([rr.left_center(),rr.right_center()],Stroke::new(1.0,Color32::LIGHT_GRAY)); for pose in &o.poses { let x=rr.left()+width*pose.time/self.project.duration; pp.circle_filled(Pos2::new(x,rr.center().y),7.0,Color32::from_rgb(246,174,46)); } ui.allocate_space(Vec2::new(width,22.0)); }); }
        });
        if let Some(asset_id) = self.edit_asset {
            let mut open = true;
            ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(asset_id), egui::ViewportBuilder::default().with_title("Редактор рисунка — AnimaPro").with_inner_size([760.0, 560.0]), |editor_ctx, _| {
                if editor_ctx.input(|i| i.viewport().close_requested()) { open = false; }
                self.editor_ui(editor_ctx, asset_id, &mut open);
            });
            if !open { self.edit_asset = None; }
        }
    }
}

fn main() -> eframe::Result<()> { let options=eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 760.0]).with_min_inner_size([1024.0,640.0]), ..Default::default() }; eframe::run_native("AnimaPro", options, Box::new(|_| Ok(Box::new(AnimaApp::default())))) }

#[cfg(test)]
mod tests { use super::*; #[test] fn interpolation_is_smooth() { let o=SceneObject{id:Uuid::new_v4(),asset_id:Uuid::new_v4(),name:"x".into(),poses:vec![Pose{time:0.0,transform:Transform::default(),linear:true},Pose{time:1.0,transform:Transform{x:100.0,..Transform::default()},linear:true}]}; assert_eq!(state_at(&o,0.5).x,50.0); } #[test] fn shortest_rotation() { assert_eq!(angle_delta(170.0,-170.0),20.0); } #[test] fn project_zip_round_trip() { let path=std::env::temp_dir().join(format!("animapro-test-{}.animapro",Uuid::new_v4())); let project=Project::new(); write_project(&path,&project).unwrap(); let reopened=read_project(&path).unwrap(); let _=fs::remove_file(path); assert_eq!(reopened.version,1); assert_eq!(reopened.assets[0].name,"Мяч"); } #[test] fn export_frame_has_hd_rgba_size() { assert_eq!(export_frame(&Project::new(), 0.0).len(), 1280 * 720 * 4); } }

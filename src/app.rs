//! User interface and document interaction state.
#[cfg(test)]
use crate::animation::angle_delta;
use crate::animation::{STEP, same_time, snap, snap_within, state_at};
use crate::model::{
    Asset, AssetContent, DrawingElement, Pose, Project, SceneObject, Shape, Transform,
};
use crate::{diagnostics, recovery, scene, storage, timeline, ui_style};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
#[cfg(test)]
use std::fs;
use std::{
    path::PathBuf,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq)]
enum EditorTool {
    Pencil,
    Line,
    Rectangle,
    Oval,
    Select,
}

/// Paint simple local geometry instead of relying on symbol glyphs in system fonts.
fn editor_tool_button(
    ui: &mut egui::Ui,
    tool: EditorTool,
    label: &str,
    selected: bool,
) -> egui::Response {
    let response = ui.add(egui::Button::selectable(selected, format!("    {label}")));
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    let center = Pos2::new(response.rect.left() + 11.0, response.rect.center().y);
    let painter = ui.painter();
    let stroke = Stroke::new(
        1.5,
        if selected {
            ui.visuals().selection.stroke.color
        } else {
            ui.visuals().text_color()
        },
    );
    let point = |x, y| center + Vec2::new(x, y);
    match tool {
        EditorTool::Pencil => {
            painter.line_segment(
                [point(-5.0, 5.0), point(4.0, -4.0)],
                Stroke::new(3.0, stroke.color),
            );
            painter.line_segment([point(-5.0, 5.0), point(-6.0, 7.0)], stroke);
        }
        EditorTool::Line => {
            painter.line_segment([point(-6.0, 5.0), point(6.0, -5.0)], stroke);
        }
        EditorTool::Rectangle => {
            painter.rect_stroke(
                Rect::from_center_size(center, Vec2::new(12.0, 10.0)),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        EditorTool::Oval => {
            painter.circle_stroke(center, 5.5, stroke);
        }
        EditorTool::Select => {
            painter.add(egui::Shape::closed_line(
                vec![
                    point(-5.0, -6.0),
                    point(-4.0, 6.0),
                    point(-1.0, 3.0),
                    point(3.0, 7.0),
                    point(5.0, 5.0),
                    point(1.0, 1.0),
                    point(5.0, 0.0),
                ],
                stroke,
            ));
        }
    }
    response.on_hover_text(label)
}

fn palette_swatch(ui: &mut egui::Ui, color: [u8; 4], name: &str, selected: bool) -> egui::Response {
    let response = ui.add_sized(
        Vec2::splat(24.0),
        egui::Button::new("").fill(c(color)).selected(selected),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            name,
        )
    });
    if selected || response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            4.0,
            Stroke::new(2.0, ui_style::ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    response.on_hover_text(name)
}

#[derive(Clone, Copy)]
enum EditorTransformMode {
    Resize,
    Rotate,
}

fn preview_scale_for(physical_width: f32) -> u32 {
    if physical_width > scene::WIDTH { 2 } else { 1 }
}

#[derive(Clone)]
struct EditorTransformGesture {
    index: usize,
    original: DrawingElement,
    center: Pos2,
    start_distance: f32,
    start_angle: f32,
    mode: EditorTransformMode,
}

fn c(color: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3])
}
fn pt(origin: Pos2, p: [f32; 2], scale: f32) -> Pos2 {
    origin + Vec2::new(p[0], p[1]) * scale
}
fn draw_elements(painter: &egui::Painter, elements: &[DrawingElement], origin: Pos2, scale: f32) {
    for element in elements {
        match element {
            DrawingElement::Line {
                from,
                to,
                color,
                width,
            } => {
                painter.line_segment(
                    [pt(origin, *from, scale), pt(origin, *to, scale)],
                    Stroke::new(width * scale, c(*color)),
                );
            }
            DrawingElement::Stroke {
                points,
                color,
                width,
            } => {
                if points.len() == 1 {
                    painter.circle_filled(
                        pt(origin, points[0], scale),
                        width * scale / 2.0,
                        c(*color),
                    );
                }
                for pair in points.windows(2) {
                    painter.line_segment(
                        [pt(origin, pair[0], scale), pt(origin, pair[1], scale)],
                        Stroke::new(width * scale, c(*color)),
                    );
                }
            }
            DrawingElement::Rect {
                from,
                to,
                angle,
                color,
                fill,
                width,
            } => {
                let points = rotated_box_points(*from, *to, *angle)
                    .into_iter()
                    .map(|p| pt(origin, p, scale))
                    .collect::<Vec<_>>();
                if *fill {
                    painter.add(egui::Shape::convex_polygon(points, c(*color), Stroke::NONE));
                } else {
                    painter.add(egui::Shape::closed_line(
                        points,
                        Stroke::new(width * scale, c(*color)),
                    ));
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
                let points = rotated_oval_points(*from, *to, *angle, 64)
                    .into_iter()
                    .map(|p| pt(origin, p, scale))
                    .collect::<Vec<_>>();
                if *fill {
                    painter.add(egui::Shape::convex_polygon(points, c(*color), Stroke::NONE));
                } else {
                    painter.add(egui::Shape::closed_line(
                        points,
                        Stroke::new(width * scale, c(*color)),
                    ));
                }
            }
        }
    }
}

fn rotate_about(point: [f32; 2], center: [f32; 2], angle: f32) -> [f32; 2] {
    let (sin, cos) = angle.to_radians().sin_cos();
    let x = point[0] - center[0];
    let y = point[1] - center[1];
    [center[0] + cos * x - sin * y, center[1] + sin * x + cos * y]
}

fn rotated_box_points(from: [f32; 2], to: [f32; 2], angle: f32) -> [[f32; 2]; 4] {
    let center = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
    [
        [from[0], from[1]],
        [to[0], from[1]],
        [to[0], to[1]],
        [from[0], to[1]],
    ]
    .map(|point| rotate_about(point, center, angle))
}

fn rotated_oval_points(from: [f32; 2], to: [f32; 2], angle: f32, steps: usize) -> Vec<[f32; 2]> {
    let center = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
    let rx = (to[0] - from[0]).abs() / 2.0;
    let ry = (to[1] - from[1]).abs() / 2.0;
    (0..steps)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / steps as f32;
            rotate_about(
                [center[0] + rx * a.cos(), center[1] + ry * a.sin()],
                center,
                angle,
            )
        })
        .collect()
}

use crate::export::{ExportError, ExportJob, ExportPhase, start_export};

fn storage_error_message(error: &storage::StorageError) -> String {
    use storage::{StorageError as S, ValidationError as V};
    match error {
        S::Validation(error) => match error {
            V::UnsupportedVersion(version) => format!("Неподдерживаемая версия проекта: {version}"),
            V::SceneSize => "Неподдерживаемый размер сцены".into(),
            V::Duration => "Недопустимая длительность проекта".into(),
            V::ObjectAssetLimit => "Слишком много рисунков или объектов".into(),
            V::DuplicateAsset => "Повторяется идентификатор рисунка".into(),
            V::BuiltinElements => "Встроенная фигура не может содержать элементы рисунка".into(),
            V::ElementPointLimit => "Превышен предел элементов или точек".into(),
            V::Geometry => "Недопустимая геометрия рисунка".into(),
            V::ObjectReference => "Повторяется объект или отсутствует его рисунок".into(),
            V::Pose => "Недопустимая поза объекта".into(),
        },
        S::Io(error) => error.to_string(),
        S::Archive(error) => error.to_string(),
        S::Json(error) => format!("Некорректный проект: {error}"),
        S::TooLarge => "Проект превышает предел 32 МиБ".into(),
        S::MissingProjectJson => "В архиве нет project.json".into(),
        S::MissingSceneSize => "В проекте не указан размер сцены".into(),
        S::MissingElements => "В проекте не указан состав рисунка".into(),
        S::MissingContent => "В проекте не указан тип рисунка".into(),
    }
}

fn snapshot_error_message(error: &recovery::SnapshotError) -> String {
    use recovery::SnapshotError as E;
    match error {
        E::Project(error) => storage_error_message(error),
        E::Serialization(error) => error.to_string(),
        E::TooLarge => "Автокопия превышает допустимый размер".into(),
        E::Io(error) => error.to_string(),
        E::WorkerStopped => "Служба автосохранения остановилась".into(),
    }
}

fn recovery_error_message(error: &recovery::RecoveryError) -> String {
    use recovery::RecoveryError as E;
    match error {
        E::DirectoryUnavailable => "Не найдена пользовательская папка для восстановления".into(),
        E::Io(error) => error.to_string(),
        E::LockUnavailable => "Не удалось заблокировать файл автокопии".into(),
        E::WorkerStopped => "Служба автосохранения остановилась".into(),
        #[cfg(test)]
        E::Disabled => "Автовосстановление отключено для проверки".into(),
    }
}

fn export_error_message(error: ExportError) -> String {
    match error {
        ExportError::Cancelled => "Экспорт отменён".into(),
        ExportError::Start(error) => format!("Не удалось запустить FFmpeg: {error}. Установите FFmpeg с кодировщиком libx264 или используйте комплектную версию."),
        ExportError::EncoderUnresponsive => "Не удалось проверить FFmpeg: процесс не ответил".into(),
        ExportError::EncoderCheck(details) => format!("Не удалось проверить кодировщики FFmpeg: {details}"),
        ExportError::EncoderMissing => "В этой версии FFmpeg нет кодировщика libx264. Используйте подходящую комплектную версию.".into(),
        ExportError::FrameWrite(error) => format!("Ошибка записи кадра: {error}"),
        ExportError::ProcessWait(error) => format!("Ошибка ожидания FFmpeg: {error}"),
        ExportError::EncoderFailed(details) => format!("FFmpeg завершился с ошибкой: {details}"),
        ExportError::FrameTransferInterrupted => "Передача кадров прервалась".into(),
        ExportError::Publish(error) => format!("Не удалось поместить готовое видео: {error}"),
    }
}
#[cfg(test)]
use crate::storage::write_project;
use crate::storage::{read_project, safely_write_project};

#[cfg(test)]
fn export_frame(project: &Project, time: f32) -> Vec<u8> {
    scene::render(project, time, 2)
}

enum PendingAction {
    New,
    Open(PathBuf),
    Close,
}
#[derive(Clone, Copy)]
enum LeaveChoice {
    Save,
    Discard,
    Cancel,
}

struct Thumbnail {
    source: Asset,
    texture: egui::TextureHandle,
}

enum ExportNotice {
    Ready(String),
    Cancelled,
    Failed(String),
}

pub(crate) struct AnimaApp {
    theme: ui_style::Theme,
    saved: Project,
    project: Project,
    selected: Option<Uuid>,
    time: f32,
    playing: bool,
    last_frame: Instant,
    undo: Vec<Project>,
    redo: Vec<Project>,
    file: Option<PathBuf>,
    dirty: bool,
    message: String,
    error: Option<String>,
    edit_asset: Option<Uuid>,
    editor_tool: EditorTool,
    editor_color: [u8; 4],
    editor_width: f32,
    editor_fill: bool,
    editor_start: Option<Pos2>,
    editor_press: Option<Pos2>,
    editor_stroke: Vec<Pos2>,
    editor_stroke_budget: usize,
    editor_stroke_overflow: bool,
    editor_selected: Option<usize>,
    editor_transform: Option<EditorTransformGesture>,
    last_autosave: Instant,
    export: Option<ExportJob>,
    export_notice: Option<ExportNotice>,
    thumbnails: std::collections::HashMap<Uuid, Thumbnail>,
    scene_texture: Option<egui::TextureHandle>,
    scene_renderer: scene::Renderer,
    scene_revision: u64,
    preview_stamp: Option<(u64, f32, u32)>,
    preview_scale: u32,
    preview_target: (u32, Instant),
    drag_offset: Option<Vec2>,
    scene_press: Option<Pos2>,
    recovery: Option<recovery::Recovery>,
    recovered: Option<recovery::Candidate>,
    recovery_prompt_pending: bool,
    transaction: Option<Project>,
    selected_pose: Option<(Uuid, f32)>,
    show_lesson: bool,
    pending_action: Option<PendingAction>,
    close_ready: bool,
    close_sent: bool,
    export_close_prompt: bool,
    recovery_candidates: Vec<recovery::Candidate>,
}
impl Default for AnimaApp {
    fn default() -> Self {
        let recovery = recovery::Recovery::new();
        if recovery.is_err() {
            diagnostics::record(diagnostics::Event::RecoveryUnavailable);
        }
        Self::with_recovery(recovery)
    }
}
impl AnimaApp {
    fn with_recovery(recovery: Result<recovery::Recovery, recovery::RecoveryError>) -> Self {
        let project = Project::new();
        let message = "Добавь рисунок на сцену".into();
        let error = recovery.as_ref().err().map(|e| {
            format!(
                "Автовосстановление недоступно: {}",
                recovery_error_message(e)
            )
        });
        Self {
            theme: ui_style::Theme::Light,
            saved: project.clone(),
            project,
            selected: None,
            time: 0.0,
            playing: false,
            last_frame: Instant::now(),
            undo: vec![],
            redo: vec![],
            file: None,
            dirty: false,
            message,
            error,
            edit_asset: None,
            editor_tool: EditorTool::Pencil,
            editor_color: [42, 83, 170, 255],
            editor_width: 5.0,
            editor_fill: true,
            editor_start: None,
            editor_press: None,
            editor_stroke: vec![],
            editor_stroke_budget: 0,
            editor_stroke_overflow: false,
            editor_selected: None,
            editor_transform: None,
            last_autosave: Instant::now(),
            export: None,
            export_notice: None,
            thumbnails: std::collections::HashMap::new(),
            scene_texture: None,
            scene_renderer: scene::Renderer::new(1),
            scene_revision: 0,
            preview_stamp: None,
            preview_scale: 1,
            preview_target: (1, Instant::now()),
            drag_offset: None,
            scene_press: None,
            recovery: recovery.ok(),
            recovered: None,
            recovery_prompt_pending: true,
            transaction: None,
            selected_pose: None,
            show_lesson: false,
            pending_action: None,
            close_ready: false,
            close_sent: false,
            export_close_prompt: false,
            recovery_candidates: Vec::new(),
        }
    }
}

impl AnimaApp {
    fn record_export_result(&mut self, result: Result<PathBuf, ExportError>) {
        self.export_notice = Some(match result {
            Ok(path) => ExportNotice::Ready(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            ),
            Err(ExportError::Cancelled) => ExportNotice::Cancelled,
            Err(error) => {
                diagnostics::record(diagnostics::Event::Export);
                ExportNotice::Failed(export_error_message(error))
            }
        });
        self.export_close_prompt = false;
    }
    fn report_error(&mut self, event: diagnostics::Event, message: String) {
        diagnostics::record(event);
        self.error = Some(message);
    }
    fn clear_recovery(&mut self) {
        if let Some(Err(e)) = self.recovery.as_ref().map(|r| r.clear()) {
            self.report_error(
                diagnostics::Event::RecoveryDelete,
                format!(
                    "Не удалось удалить автокопию: {}",
                    recovery_error_message(&e)
                ),
            );
        }
        if let Some(candidate) = self.recovered.take()
            && let Err(e) = candidate.discard()
        {
            self.report_error(
                diagnostics::Event::RecoveryDelete,
                format!(
                    "Не удалось удалить восстановленную копию: {}",
                    recovery_error_message(&e)
                ),
            );
        }
    }
    fn recover_candidate(&mut self, candidate: recovery::Candidate) {
        let source = candidate.snapshot.source.clone();
        let saved = source
            .as_deref()
            .and_then(|p| read_project(p).ok())
            .unwrap_or_else(Project::new);
        self.project = candidate.snapshot.project.clone();
        self.invalidate_scene();
        self.saved = saved;
        self.file = source;
        self.dirty = true;
        self.recovered = Some(candidate);
        self.message = "Работа восстановлена. Сохраните проект.".into();
    }
    fn request_action(&mut self, action: PendingAction) {
        self.finish_transaction();
        if self.dirty {
            self.pending_action = Some(action);
        } else {
            self.commit_action(action);
        }
    }
    fn commit_action(&mut self, action: PendingAction) {
        match action {
            PendingAction::New => {
                self.clear_recovery();
                self.replace_project(Project::new(), None);
                self.message = "Новый проект создан".into();
            }
            PendingAction::Open(path) => match read_project(&path) {
                Ok(project) => {
                    self.clear_recovery();
                    self.replace_project(project, Some(path));
                    self.message = "Проект открыт".into();
                }
                Err(error) => self.report_error(
                    diagnostics::Event::ProjectOpen,
                    format!(
                        "Не удалось открыть проект: {}",
                        storage_error_message(&error)
                    ),
                ),
            },
            PendingAction::Close => {
                self.clear_recovery();
                self.close_ready = true;
            }
        }
    }
    fn resolve_leave(&mut self, choice: LeaveChoice, save_succeeded: bool) {
        let Some(action) = self.pending_action.take() else {
            return;
        };
        match choice {
            LeaveChoice::Cancel => {}
            LeaveChoice::Save if !save_succeeded => {}
            LeaveChoice::Save | LeaveChoice::Discard => self.commit_action(action),
        }
    }
    fn confirmation_ui(&mut self, ctx: &egui::Context) {
        if self.pending_action.is_some() {
            let mut choice = None;
            egui::Modal::new(egui::Id::new("leave_confirmation")).show(ctx, |ui| {
                ui.heading("Сохранить работу?");
                ui.label("В проекте есть несохранённые изменения.");
                ui.horizontal(|ui| {
                    if ui.button("Сохранить").clicked() {
                        choice = Some(LeaveChoice::Save);
                    }
                    if ui.button("Не сохранять").clicked() {
                        choice = Some(LeaveChoice::Discard);
                    }
                    if ui.button("Отмена").clicked() {
                        choice = Some(LeaveChoice::Cancel);
                    }
                });
            });
            if let Some(choice) = choice {
                let saved = if matches!(choice, LeaveChoice::Save) {
                    self.save(false)
                } else {
                    false
                };
                self.resolve_leave(choice, saved);
            }
        } else if self.export_close_prompt {
            let mut choice = None;
            egui::Modal::new(egui::Id::new("export_close_confirmation")).show(ctx, |ui| {
                ui.heading("Видео ещё создаётся");
                ui.label("Дождитесь завершения или отмените создание видео.");
                ui.horizontal(|ui| {
                    if ui.button("Отменить экспорт").clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Дождаться").clicked() {
                        choice = Some(false);
                    }
                });
            });
            if let Some(cancel) = choice {
                if cancel && let Some(job) = &self.export {
                    job.cancel.store(true, Ordering::Relaxed);
                }
                self.export_close_prompt = false;
            }
        } else if let Some(candidate) = self.recovery_candidates.first() {
            let description = candidate.snapshot.description();
            let mut choice = None;
            egui::Modal::new(egui::Id::new("recovery_confirmation")).show(ctx, |ui| {
                ui.heading("Восстановление работы");
                ui.label(description);
                ui.label("Восстановить эту копию?");
                ui.horizontal(|ui| {
                    if ui.button("Восстановить").clicked() {
                        choice = Some(0);
                    }
                    if ui.button("Удалить копию").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("Решить позже").clicked() {
                        choice = Some(2);
                    }
                });
            });
            match choice {
                Some(0) => {
                    let candidate = self.recovery_candidates.remove(0);
                    self.recovery_candidates.clear();
                    self.recover_candidate(candidate);
                }
                Some(1) => {
                    let candidate = self.recovery_candidates.remove(0);
                    if let Err(e) = candidate.discard() {
                        self.report_error(
                            diagnostics::Event::RecoveryDelete,
                            format!(
                                "Не удалось удалить автокопию: {}",
                                recovery_error_message(&e)
                            ),
                        );
                    }
                }
                Some(2) => self.recovery_candidates.clear(),
                _ => {}
            }
        }
    }
    fn reset_editor_gesture(&mut self) {
        self.editor_selected = None;
        self.editor_transform = None;
        self.editor_start = None;
        self.editor_press = None;
        self.editor_stroke.clear();
        self.editor_stroke_budget = 0;
        self.editor_stroke_overflow = false;
    }
    fn open_editor(&mut self, id: Uuid) {
        if self.edit_asset != Some(id) {
            self.cancel_transaction();
            self.reset_editor_gesture();
            if self.project.asset(id).is_some() {
                self.edit_asset = Some(id);
            } else {
                self.edit_asset = None;
                self.message = "Рисунок больше недоступен.".into();
            }
        }
    }
    fn sanitize_selection(&mut self) {
        if !self
            .project
            .objects
            .iter()
            .any(|o| Some(o.id) == self.selected)
        {
            self.selected = None;
        }
        if !self
            .project
            .assets
            .iter()
            .any(|a| Some(a.id) == self.edit_asset)
        {
            self.edit_asset = None;
        }
        self.reset_editor_gesture();
        if let Some((id, t)) = self.selected_pose
            && (self.selected != Some(id)
                || !self
                    .project
                    .objects
                    .iter()
                    .any(|o| o.id == id && o.poses.iter().any(|p| p.time == t)))
        {
            self.selected_pose = None;
        }
        self.time = self.time.min(self.project.duration);
    }
    fn push_history(&mut self, previous: Project) {
        self.undo.push(previous);
        while self.undo.len() > 100
            || (!self.undo.is_empty()
                && self.undo.iter().map(project_size).sum::<usize>() > 64 * 1024 * 1024)
        {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    fn invalidate_scene(&mut self) {
        self.scene_revision = self.scene_revision.wrapping_add(1);
    }
    fn change(&mut self) {
        self.invalidate_scene();
        if self.transaction.is_none() {
            self.push_history(self.project.clone());
        }
        self.dirty = true;
    }
    /// Apply only changes that can still be written and reopened within the format limits.
    fn try_edit(&mut self, edit: impl FnOnce(&mut Project)) -> bool {
        let mut candidate = self.project.clone();
        edit(&mut candidate);
        if candidate == self.project {
            return false;
        }
        if let Err(error) = storage::ensure_saveable(&candidate) {
            self.message = format!(
                "Изменение не применено: {}. Сохраните работу и уменьшите проект.",
                storage_error_message(&error)
            );
            return false;
        }
        self.change();
        self.project = candidate;
        true
    }
    fn begin_transaction(&mut self) {
        if self.transaction.is_none() {
            self.transaction = Some(self.project.clone());
        }
    }
    fn finish_transaction(&mut self) {
        if let Some(before) = self.transaction.take() {
            if before != self.project {
                self.push_history(before);
            }
            self.dirty = self.project != self.saved || self.recovered.is_some();
        }
    }
    fn cancel_transaction(&mut self) {
        if let Some(before) = self.transaction.take() {
            self.project = before;
            self.invalidate_scene();
            self.dirty = self.project != self.saved || self.recovered.is_some();
            self.sanitize_selection();
        }
    }
    fn undo(&mut self) {
        self.finish_transaction();
        if let Some(previous) = self.undo.pop() {
            self.redo.push(self.project.clone());
            self.project = previous;
            self.invalidate_scene();
            self.dirty = self.project != self.saved || self.recovered.is_some();
            self.sanitize_selection();
        }
    }
    fn redo(&mut self) {
        self.finish_transaction();
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.project.clone());
            self.project = next;
            self.invalidate_scene();
            self.dirty = self.project != self.saved || self.recovered.is_some();
            self.sanitize_selection();
        }
    }
    fn selected_object(&self) -> Option<&SceneObject> {
        self.selected
            .and_then(|id| self.project.objects.iter().find(|o| o.id == id))
    }
    fn add_object(&mut self, asset_id: Uuid) {
        let Some(asset) = self.project.asset(asset_id) else {
            return;
        };
        let n = self
            .project
            .objects
            .iter()
            .filter(|o| o.asset_id == asset_id)
            .count()
            + 1;
        let id = Uuid::new_v4();
        let object = SceneObject {
            id,
            asset_id,
            name: format!("{} {}", asset.name, n),
            poses: vec![Pose {
                time: 0.0,
                transform: Transform::default(),
                linear: false,
            }],
        };
        if self.try_edit(|p| p.objects.push(object)) {
            self.selected = Some(id);
            self.selected_pose = None;
            self.message = "Объект добавлен. Перетащите его на сцене!".into();
        }
    }
    fn upsert_pose(&mut self, id: Uuid, tr: Transform) {
        let snapped = snap_within(self.time, self.project.duration);
        let Some(object) = self.project.objects.iter().find(|o| o.id == id) else {
            return;
        };
        // A selected legacy pose can be off the 0.1-second grid. Keep its exact
        // time and never merge it with another nearby pose.
        let selected_index = self.selected_pose.and_then(|(selected_id, time)| {
            (selected_id == id && self.time == time)
                .then(|| object.poses.iter().position(|p| p.time == time))
                .flatten()
        });
        let existing = if let Some(index) = selected_index {
            Some(index)
        } else {
            let start = object.poses.partition_point(|p| p.time < snapped - 0.00001);
            let mut matches = object
                .poses
                .iter()
                .enumerate()
                .skip(start)
                .take_while(|(_, p)| p.time <= snapped + 0.00001)
                .filter(|(_, p)| same_time(p.time, snapped))
                .map(|(i, _)| i);
            let first = matches.next();
            if matches.next().is_some() {
                self.message =
                    "Рядом несколько поз. Выберите нужную позу в списке перед изменением.".into();
                return;
            }
            if first.is_some_and(|i| object.poses[i].time != snapped) {
                self.message =
                    "Рядом уже есть поза с другим временем. Выберите её в списке перед изменением."
                        .into();
                return;
            }
            first
        };
        if existing.is_some_and(|i| object.poses[i].transform == tr) {
            return;
        }
        let selected_time = existing.map(|i| object.poses[i].time).unwrap_or(snapped);
        if self.try_edit(|project| {
            let object = project.objects.iter_mut().find(|o| o.id == id).unwrap();
            if let Some(i) = existing {
                object.poses[i].transform = tr;
            } else {
                object.poses.push(Pose {
                    time: snapped,
                    transform: tr,
                    linear: false,
                });
                object.poses.sort_by(|a, b| a.time.total_cmp(&b.time));
            }
        }) {
            self.selected_pose = Some((id, selected_time));
        }
    }
    fn select_pose(&mut self, id: Uuid, time: f32) {
        if self
            .project
            .objects
            .iter()
            .any(|o| o.id == id && o.poses.iter().any(|p| p.time == time))
        {
            self.selected = Some(id);
            self.selected_pose = Some((id, time));
            self.time = time;
            self.playing = false;
        }
    }
    fn create_pose(&mut self) {
        let Some(id) = self.selected else { return };
        let t = snap_within(self.time, self.project.duration);
        let Some(object) = self.selected_object() else {
            return;
        };
        if object.poses.iter().any(|p| same_time(p.time, t)) {
            self.message = "На этой отметке уже есть поза".into();
            return;
        }
        let transform = state_at(object, t);
        if self.try_edit(|project| {
            let object = project.objects.iter_mut().find(|o| o.id == id).unwrap();
            object.poses.push(Pose {
                time: t,
                transform,
                linear: false,
            });
            object.poses.sort_by(|a, b| a.time.total_cmp(&b.time));
        }) {
            self.select_pose(id, t);
            self.message = "Поза добавлена".into();
        }
    }
    fn delete_pose(&mut self) {
        let Some((id, time)) = self.selected_pose else {
            return;
        };
        if !self
            .project
            .objects
            .iter()
            .any(|o| o.id == id && o.poses.iter().any(|p| p.time == time))
        {
            self.selected_pose = None;
            return;
        }
        if self.try_edit(|project| {
            project
                .objects
                .iter_mut()
                .find(|o| o.id == id)
                .unwrap()
                .poses
                .retain(|p| p.time != time)
        }) {
            self.selected_pose = None;
            self.message = "Поза удалена".into();
        }
    }
    fn move_pose(&mut self, delta: f32) {
        let Some((id, time)) = self.selected_pose else {
            return;
        };
        let target = snap(time + delta);
        if target < 0.0 || target > self.project.duration + 0.00001 {
            self.message = "Поза должна оставаться в пределах мультфильма".into();
            return;
        }
        let Some(object) = self.project.objects.iter().find(|o| o.id == id) else {
            return;
        };
        let Some(index) = object.poses.iter().position(|p| p.time == time) else {
            self.selected_pose = None;
            return;
        };
        if time == target {
            return;
        }
        if object
            .poses
            .iter()
            .enumerate()
            .any(|(i, p)| i != index && same_time(p.time, target))
        {
            self.message =
                "На этой отметке уже есть поза. Сначала переместите или удалите её.".into();
            return;
        }
        let target = target.min(self.project.duration);
        if self.try_edit(|project| {
            let object = project.objects.iter_mut().find(|o| o.id == id).unwrap();
            object.poses[index].time = target;
            object.poses.sort_by(|a, b| a.time.total_cmp(&b.time));
        }) {
            self.select_pose(id, target);
            self.message = "Поза перемещена".into();
        }
    }
    fn set_pose_linear(&mut self, linear: bool) {
        let Some((id, time)) = self.selected_pose else {
            return;
        };
        let Some(index) = self
            .project
            .objects
            .iter()
            .find(|o| o.id == id)
            .and_then(|o| {
                o.poses
                    .iter()
                    .position(|p| p.time == time && p.linear != linear)
            })
        else {
            return;
        };
        self.try_edit(|project| {
            project
                .objects
                .iter_mut()
                .find(|o| o.id == id)
                .unwrap()
                .poses[index]
                .linear = linear
        });
    }
    fn reorder_object(&mut self, id: Uuid, forward: bool) {
        let Some(index) = self.project.objects.iter().position(|o| o.id == id) else {
            return;
        };
        let next = if forward {
            index
                .checked_add(1)
                .filter(|n| *n < self.project.objects.len())
        } else {
            index.checked_sub(1)
        };
        if let Some(next) = next {
            self.try_edit(|project| project.objects.swap(index, next));
        }
    }
    fn save(&mut self, choose: bool) -> bool {
        let path = if choose || self.file.is_none() {
            rfd::FileDialog::new()
                .add_filter("Проект AnimaPro", &["animapro"])
                .set_file_name("мой-мультфильм.animapro")
                .save_file()
        } else {
            self.file.clone()
        };
        let Some(path) = path else {
            return false;
        };
        self.save_to_path(path)
    }
    fn save_to_path(&mut self, path: PathBuf) -> bool {
        match safely_write_project(&path, &self.project) {
            Ok(()) => {
                self.file = Some(path);
                self.saved = self.project.clone();
                self.dirty = false;
                self.message = "Проект сохранён".into();
                self.clear_recovery();
                true
            }
            Err(e) => {
                self.report_error(
                    diagnostics::Event::ProjectSave,
                    format!("Не удалось сохранить: {}", storage_error_message(&e)),
                );
                false
            }
        }
    }
    fn replace_project(&mut self, project: Project, file: Option<PathBuf>) {
        self.saved = project.clone();
        self.project = project;
        self.invalidate_scene();
        self.file = file;
        self.undo.clear();
        self.redo.clear();
        self.selected = None;
        self.edit_asset = None;
        self.reset_editor_gesture();
        self.playing = false;
        self.time = 0.0;
        self.dirty = false;
        self.drag_offset = None;
        self.scene_press = None;
        self.transaction = None;
        self.selected_pose = None;
    }
    fn open(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Проект AnimaPro", &["animapro"])
            .pick_file()
        {
            self.request_action(PendingAction::Open(path));
        }
    }
    fn new_project(&mut self) {
        self.request_action(PendingAction::New);
    }
    fn editor_ui(&mut self, ctx: &egui::Context, asset_id: Uuid, open: &mut bool) {
        ui_style::install_theme(ctx, self.theme);
        if self.pending_action.is_none()
            && !self.export_close_prompt
            && self.recovery_candidates.is_empty()
        {
            let (save, save_as, undo, redo) = ctx.input(|i| {
                (
                    shortcut_pressed(i, egui::Key::S, Some(false)),
                    shortcut_pressed(i, egui::Key::S, Some(true)),
                    shortcut_pressed(i, egui::Key::Z, Some(false)),
                    shortcut_pressed(i, egui::Key::Z, Some(true))
                        || shortcut_pressed(i, egui::Key::Y, None),
                )
            });
            if save_as {
                self.save(true);
            } else if save {
                self.save(false);
            }
            if !ctx.wants_keyboard_input() {
                if undo {
                    self.undo();
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                } else if redo {
                    self.redo();
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.cancel_transaction();
            self.reset_editor_gesture();
        }
        let Some(index) = self.project.assets.iter().position(|a| a.id == asset_id) else {
            *open = false;
            return;
        };
        let mut asset = self.project.assets[index].clone();
        if asset.content == AssetContent::Builtin {
            let from = [-35.0, -35.0];
            let to = [35.0, 35.0];
            let color = asset.color;
            let base = match asset.kind {
                Shape::Ball | Shape::Oval => DrawingElement::Oval {
                    from,
                    to,
                    angle: 0.0,
                    color,
                    fill: true,
                    width: 1.0,
                },
                Shape::Rectangle => DrawingElement::Rect {
                    from,
                    to,
                    angle: 0.0,
                    color,
                    fill: true,
                    width: 1.0,
                },
            };
            asset.elements.push(base);
            asset.content = AssetContent::Drawing;
        }
        if self
            .editor_selected
            .is_some_and(|i| i >= asset.elements.len())
        {
            self.editor_selected = None;
        }
        let mut modified = false;
        let mut end_editor_transaction = false;
        egui::TopBottomPanel::top("draw_tools").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui_style::primary(ui, "Готово", true).clicked() {
                    *open = false;
                }
                ui.add_sized(
                    [180.0, ui.spacing().interact_size.y],
                    egui::Label::new(&asset.name).truncate(),
                )
                .on_hover_text(&asset.name);
            });
            ui.vertical(|ui| {
                if ui.button("Создать копию").clicked() {
                    let mut copy = asset.clone();
                    copy.id = Uuid::new_v4();
                    copy.name = format!("{} — копия", copy.name);
                    if self.try_edit(|project| project.assets.push(copy)) {
                        self.message = "Создан независимый рисунок".into();
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label("Цвет:");
                    let mut color = c(self.editor_color);
                    let color_changed = ui
                        .add_enabled_ui(self.editor_tool != EditorTool::Select, |ui| {
                            egui::color_picker::color_edit_button_srgba(
                                ui,
                                &mut color,
                                egui::color_picker::Alpha::OnlyBlend,
                            )
                            .changed()
                        })
                        .inner;
                    if color_changed {
                        self.editor_color = color.to_srgba_unmultiplied();
                    }
                    ui.label("Толщина:");
                    let thickness_enabled = self.editor_tool != EditorTool::Select;
                    let thickness = ui.add_enabled(
                        thickness_enabled,
                        egui::Slider::new(&mut self.editor_width, 1.0..=20.0),
                    );
                    thickness.widget_info(|| {
                        egui::WidgetInfo::slider(
                            thickness_enabled,
                            self.editor_width as f64,
                            "Толщина",
                        )
                    });
                    ui.add_enabled(
                        matches!(self.editor_tool, EditorTool::Rectangle | EditorTool::Oval),
                        egui::Checkbox::new(&mut self.editor_fill, "Заливка"),
                    );
                    let (sample, _) = ui.allocate_exact_size(Vec2::new(35.0, 24.0), Sense::hover());
                    ui.painter().line_segment(
                        [sample.left_center(), sample.right_center()],
                        Stroke::new(self.editor_width.min(20.0), c(self.editor_color)),
                    );
                });
                ui.add_enabled_ui(self.editor_tool != EditorTool::Select, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Палитра:");
                        for (name, color) in [
                            ("Чёрный", [35, 35, 40, 255]),
                            ("Красный", [235, 75, 70, 255]),
                            ("Оранжевый", [240, 165, 45, 255]),
                            ("Зелёный", [65, 165, 105, 255]),
                            ("Синий", [50, 120, 220, 255]),
                            ("Фиолетовый", [150, 85, 195, 255]),
                        ] {
                            if palette_swatch(ui, color, name, self.editor_color == color).clicked()
                            {
                                self.editor_color = color;
                            }
                        }
                    });
                });

                if self.message.starts_with("Изменение не применено") {
                    ui.colored_label(Color32::from_rgb(220, 70, 60), &self.message);
                }
            });
        });
        egui::SidePanel::left("drawing_tools")
            .resizable(false)
            .default_width(145.0)
            .show(ctx, |ui| {
                ui.strong("Инструменты");
                for (tool, label) in [
                    (EditorTool::Select, "Выбрать"),
                    (EditorTool::Pencil, "Карандаш"),
                    (EditorTool::Line, "Линия"),
                    (EditorTool::Rectangle, "Прямоугольник"),
                    (EditorTool::Oval, "Овал"),
                ] {
                    if editor_tool_button(ui, tool, label, self.editor_tool == tool).clicked() {
                        if self.editor_tool != tool {
                            self.cancel_transaction();
                            self.reset_editor_gesture();
                        }
                        self.editor_tool = tool;
                        self.editor_selected = None;
                    }
                }
            });
        egui::SidePanel::right("drawing_elements")
            .default_width(160.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("drawing_elements_panel")
                    .show(ui, |ui| {
                        ui.heading("Элементы");
                        ui.label(
                            "Выберите элемент. Стрелки перемещают его; Shift ускоряет движение.",
                        );
                        egui::ScrollArea::vertical().max_height(300.0).show_rows(
                            ui,
                            ui.spacing().interact_size.y,
                            asset.elements.len(),
                            |ui, rows| {
                                for i in rows {
                                    let (kind, color) = match &asset.elements[i] {
                                        DrawingElement::Line { color, .. } => ("Линия", color),
                                        DrawingElement::Rect { color, .. } => {
                                            ("Прямоугольник", color)
                                        }
                                        DrawingElement::Oval { color, .. } => ("Овал", color),
                                        DrawingElement::Stroke { color, .. } => ("Штрих", color),
                                    };
                                    let transparent = if color[3] == 0 {
                                        " (прозрачный)"
                                    } else {
                                        ""
                                    };
                                    if ui
                                        .selectable_label(
                                            self.editor_selected == Some(i),
                                            format!("{}: {kind}{transparent}", i + 1),
                                        )
                                        .clicked()
                                    {
                                        self.editor_tool = EditorTool::Select;
                                        self.editor_selected = Some(i);
                                    }
                                }
                            },
                        );
                        if ui
                            .add_enabled(
                                self.editor_selected.is_some(),
                                egui::Button::new("Удалить элемент"),
                            )
                            .clicked()
                            && let Some(i) = self.editor_selected.take()
                        {
                            asset.elements.remove(i);
                            modified = true;
                        }
                        if let Some(i) = self.editor_selected.filter(|i| *i < asset.elements.len())
                        {
                            ui.separator();
                            ui.label("Преобразование");
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("− Размер").clicked() {
                                    self.begin_transaction();
                                    transform_element(&mut asset.elements[i], 0.9, 0.0);
                                    modified = true;
                                    end_editor_transaction = true;
                                }
                                if ui.button("+ Размер").clicked() {
                                    self.begin_transaction();
                                    transform_element(&mut asset.elements[i], 1.1, 0.0);
                                    modified = true;
                                    end_editor_transaction = true;
                                }
                            });
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Повернуть −15°").clicked() {
                                    self.begin_transaction();
                                    transform_element(&mut asset.elements[i], 1.0, -15.0);
                                    modified = true;
                                    end_editor_transaction = true;
                                }
                                if ui.button("Повернуть +15°").clicked() {
                                    self.begin_transaction();
                                    transform_element(&mut asset.elements[i], 1.0, 15.0);
                                    modified = true;
                                    end_editor_transaction = true;
                                }
                            });
                        }
                    });
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
            let painter = ui.painter_at(rect); painter.rect_filled(rect, 0.0, Color32::WHITE); painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::LIGHT_GRAY), egui::StrokeKind::Inside);
            let origin = rect.center(); let scale = 1.0;
            if response.is_pointer_button_down_on() && ctx.input(|i| i.pointer.primary_pressed()) { self.editor_press = ctx.input(|i| i.pointer.press_origin()); }
            let released = ctx.input(|i| i.pointer.primary_released());
            let release_pos = ctx.input(|i| i.pointer.latest_pos());
            let quick_drag = released && self.editor_start.is_none() && self.editor_press.zip(release_pos).is_some_and(|(start, end)| start.distance(end) > 6.0);
            draw_elements(&painter, &asset.elements, origin, scale);
            if let (Some(start), Some(end)) = (self.editor_start, response.interact_pointer_pos().or_else(|| ctx.pointer_hover_pos())) {
                let local = |p: Pos2| [p.x-origin.x, p.y-origin.y];
                let preview = match self.editor_tool {
                    EditorTool::Pencil => Some(DrawingElement::Stroke { points: self.editor_stroke.iter().map(|p| local(*p)).collect(), color: self.editor_color, width: self.editor_width }),
                    EditorTool::Line => Some(DrawingElement::Line { from: local(start), to: local(end), color: self.editor_color, width: self.editor_width }),
                    EditorTool::Rectangle => Some(DrawingElement::Rect { from: local(start), to: local(end), angle: 0.0, color: self.editor_color, fill: self.editor_fill, width: self.editor_width }),
                    EditorTool::Oval => Some(DrawingElement::Oval { from: local(start), to: local(end), angle: 0.0, color: self.editor_color, fill: self.editor_fill, width: self.editor_width }),
                    EditorTool::Select => None,
                };
                if let Some(element) = preview { draw_elements(&painter, &[element], origin, scale); }
            }
            if response.clicked() && self.editor_tool == EditorTool::Pencil && let Some(pos) = response.interact_pointer_pos() { asset.elements.push(DrawingElement::Stroke { points: vec![[pos.x-origin.x,pos.y-origin.y]], color: self.editor_color, width: self.editor_width }); modified = true; }
            if response.clicked() && self.editor_tool == EditorTool::Select && let Some(pos) = response.interact_pointer_pos() { self.editor_selected = pick_element(&asset.elements, pos, origin); }
            if self.editor_tool == EditorTool::Select
                && let Some(i) = self.editor_selected
                && let Some(el) = asset.elements.get(i)
            {
                draw_transform_box(&painter, el, origin);
            }
            if response.drag_started()
                && self.editor_tool == EditorTool::Select
                && let (Some(i), Some(pos)) = (self.editor_selected, response.interact_pointer_pos())
                && let Some(el) = asset.elements.get(i)
                && let Some(mode) = transform_hit(el, origin, pos)
            {
                let bounds = transform_box(el, origin);
                let offset = pos - bounds.center();
                self.editor_transform = Some(EditorTransformGesture {
                    index: i,
                    original: el.clone(),
                    center: bounds.center(),
                    start_distance: offset.length().max(1.0),
                    start_angle: offset.angle(),
                    mode,
                });
                self.begin_transaction();
            }
            if (response.drag_started() || quick_drag) && self.editor_transform.is_none() {
                self.editor_start = self.editor_press.or_else(|| ctx.input(|i| i.pointer.press_origin())).or_else(|| response.interact_pointer_pos()); self.editor_stroke.clear(); self.editor_stroke_overflow = false;
                let used_points: usize = self.project.assets.iter().flat_map(|a| &a.elements).map(|e| match e { DrawingElement::Stroke { points, .. } => points.len(), _ => 2 }).sum();
                self.editor_stroke_budget = storage::MAX_POINTS.saturating_sub(used_points);
                if let Some(pos) = self.editor_start {
                    if self.editor_tool != EditorTool::Pencil || self.editor_stroke_budget > 0 { self.editor_stroke.push(pos); } else { self.editor_stroke_overflow = true; }
                    if self.editor_tool == EditorTool::Select { self.editor_selected = pick_element(&asset.elements, pos, origin); if self.editor_selected.is_some() { self.begin_transaction(); } }
                }
            }
            if response.dragged() || response.drag_stopped() || quick_drag { if self.editor_tool == EditorTool::Pencil { if let Some(pos) = response.interact_pointer_pos().or(release_pos) && self.editor_stroke.last() != Some(&pos) { if self.editor_stroke.len() < self.editor_stroke_budget { self.editor_stroke.push(pos); } else { self.editor_stroke_overflow = true; } } } else if let Some(gesture) = &self.editor_transform && let Some(pos) = response.interact_pointer_pos().or(release_pos) { let offset = pos - gesture.center; let mut transformed = gesture.original.clone(); match gesture.mode { EditorTransformMode::Resize => transform_element(&mut transformed, (offset.length() / gesture.start_distance).clamp(0.05, 100.0), 0.0), EditorTransformMode::Rotate => transform_element(&mut transformed, 1.0, (offset.angle() - gesture.start_angle).to_degrees()), } if asset.elements.get(gesture.index) != Some(&transformed) { asset.elements[gesture.index] = transformed; modified = true; } } else if self.editor_tool == EditorTool::Select && let Some(i) = self.editor_selected { let d = if quick_drag { release_pos.unwrap() - self.editor_start.unwrap() } else { ctx.input(|i| i.pointer.delta()) }; if d != Vec2::ZERO { translate_element(&mut asset.elements[i], d.x, d.y); modified = true; } } }
            if response.drag_stopped() || quick_drag { if self.editor_tool == EditorTool::Select { self.editor_transform = None; end_editor_transaction = true; }
            if let (Some(start), Some(end)) = (self.editor_start, response.interact_pointer_pos().or(release_pos)) { let local = |p: Pos2| [p.x-origin.x, p.y-origin.y]; match self.editor_tool { EditorTool::Pencil if self.editor_stroke_overflow => { self.message = "Изменение не применено: превышен предел точек. Сохраните работу и уменьшите проект.".into(); }, EditorTool::Pencil if self.editor_stroke.len() > 1 => { asset.elements.push(DrawingElement::Stroke { points: self.editor_stroke.iter().map(|p| local(*p)).collect(), color: self.editor_color, width: self.editor_width }); modified = true; }, EditorTool::Line => { asset.elements.push(DrawingElement::Line { from: local(start), to: local(end), color: self.editor_color, width: self.editor_width }); modified = true; }, EditorTool::Rectangle | EditorTool::Oval if start.x == end.x || start.y == end.y => { self.message = "Изменение не применено: у фигуры должны быть ширина и высота. Для линии выберите инструмент «Линия».".into(); }, EditorTool::Rectangle => { asset.elements.push(DrawingElement::Rect { from: local(start), to: local(end), angle: 0.0, color: self.editor_color, fill: self.editor_fill, width: self.editor_width }); modified = true; }, EditorTool::Oval => { asset.elements.push(DrawingElement::Oval { from: local(start), to: local(end), angle: 0.0, color: self.editor_color, fill: self.editor_fill, width: self.editor_width }); modified = true; }, _ => {} } } self.editor_start = None; self.editor_stroke.clear(); }
            if released { self.editor_press = None; }
            if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) && let Some(i) = self.editor_selected.take() && i < asset.elements.len() { asset.elements.remove(i); modified = true; }
            if self.editor_tool == EditorTool::Select && !ctx.wants_keyboard_input() && let Some(i) = self.editor_selected && i < asset.elements.len() {
                let (dx, dy) = ctx.input(|input| {
                    input.events.iter().fold((0.0, 0.0), |(x, y), event| {
                        let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { return (x, y); };
                        let step = if modifiers.shift { 10.0 } else { 1.0 };
                        match key { egui::Key::ArrowRight => (x + step, y), egui::Key::ArrowLeft => (x - step, y), egui::Key::ArrowDown => (x, y + step), egui::Key::ArrowUp => (x, y - step), _ => (x, y) }
                    })
                });
                if dx != 0.0 || dy != 0.0 { translate_element(&mut asset.elements[i], dx, dy); modified = true; }
            }
        });
        if modified {
            asset.content = AssetContent::Drawing;
            if self.try_edit(|project| project.assets[index] = asset) {
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        }
        if end_editor_transaction {
            self.finish_transaction();
        }
    }
}

fn shortcut_pressed(input: &egui::InputState, desired: egui::Key, shift: Option<bool>) -> bool {
    input.events.iter().any(|event| {
        matches!(event, egui::Event::Key { key, pressed: true, modifiers, .. }
            if *key == desired && modifiers.command && shift.is_none_or(|required| modifiers.shift == required))
    })
}

fn pose_time_label(time: f32) -> String {
    if time == snap(time) {
        format!("{time:.1}")
    } else {
        time.to_string()
    }
}

fn project_size(project: &Project) -> usize {
    std::mem::size_of::<Project>()
        + project.name.len()
        + project
            .assets
            .iter()
            .map(|a| {
                std::mem::size_of::<Asset>()
                    + a.name.len()
                    + a.elements
                        .iter()
                        .map(|e| {
                            std::mem::size_of::<DrawingElement>()
                                + match e {
                                    DrawingElement::Stroke { points, .. } => {
                                        points.len() * std::mem::size_of::<[f32; 2]>()
                                    }
                                    _ => 0,
                                }
                        })
                        .sum::<usize>()
            })
            .sum::<usize>()
        + project
            .objects
            .iter()
            .map(|o| {
                std::mem::size_of::<SceneObject>()
                    + o.name.len()
                    + o.poses.len() * std::mem::size_of::<Pose>()
            })
            .sum::<usize>()
}

fn element_bounds(element: &DrawingElement, origin: Pos2) -> Rect {
    let bounds = scene::element_bounds(element).unwrap_or([0.0; 4]);
    Rect::from_min_max(
        pt(origin, [bounds[0], bounds[1]], 1.0),
        pt(origin, [bounds[2], bounds[3]], 1.0),
    )
}

fn transform_box(element: &DrawingElement, origin: Pos2) -> Rect {
    element_bounds(element, origin).expand(6.0)
}

fn transform_hit(
    element: &DrawingElement,
    origin: Pos2,
    position: Pos2,
) -> Option<EditorTransformMode> {
    let bounds = transform_box(element, origin);
    let rotate_handle = Pos2::new(bounds.center().x, bounds.top() - 28.0);
    if position.distance(rotate_handle) <= 10.0 {
        return Some(EditorTransformMode::Rotate);
    }
    [
        bounds.left_top(),
        bounds.right_top(),
        bounds.right_bottom(),
        bounds.left_bottom(),
    ]
    .into_iter()
    .any(|handle| position.distance(handle) <= 10.0)
    .then_some(EditorTransformMode::Resize)
}

fn draw_transform_box(painter: &egui::Painter, element: &DrawingElement, origin: Pos2) {
    let bounds = transform_box(element, origin);
    let color = ui_style::ACCENT;
    painter.rect_stroke(
        bounds,
        0.0,
        Stroke::new(2.0, color),
        egui::StrokeKind::Outside,
    );
    let rotate_handle = Pos2::new(bounds.center().x, bounds.top() - 28.0);
    painter.line_segment(
        [bounds.center_top(), rotate_handle],
        Stroke::new(1.5, color),
    );
    painter.circle_filled(rotate_handle, 6.0, Color32::WHITE);
    painter.circle_stroke(rotate_handle, 6.0, Stroke::new(2.0, color));
    for handle in [
        bounds.left_top(),
        bounds.right_top(),
        bounds.right_bottom(),
        bounds.left_bottom(),
    ] {
        painter.rect_filled(
            Rect::from_center_size(handle, Vec2::splat(9.0)),
            1.0,
            Color32::WHITE,
        );
        painter.rect_stroke(
            Rect::from_center_size(handle, Vec2::splat(9.0)),
            1.0,
            Stroke::new(2.0, color),
            egui::StrokeKind::Outside,
        );
    }
}
fn pick_element(elements: &[DrawingElement], position: Pos2, origin: Pos2) -> Option<usize> {
    let local = [position.x - origin.x, position.y - origin.y];
    elements
        .iter()
        .rposition(|element| scene::contains_element(element, local, 6.0))
}

fn translate_element(element: &mut DrawingElement, x: f32, y: f32) {
    let move_point = |p: &mut [f32; 2]| {
        p[0] += x;
        p[1] += y;
    };
    match element {
        DrawingElement::Line { from, to, .. }
        | DrawingElement::Rect { from, to, .. }
        | DrawingElement::Oval { from, to, .. } => {
            move_point(from);
            move_point(to);
        }
        DrawingElement::Stroke { points, .. } => {
            for point in points {
                move_point(point);
            }
        }
    }
}

fn transform_element(element: &mut DrawingElement, scale: f32, angle: f32) {
    let Some(bounds) = scene::element_bounds(element) else {
        return;
    };
    let center = [(bounds[0] + bounds[2]) / 2.0, (bounds[1] + bounds[3]) / 2.0];
    let transform_point = |p: &mut [f32; 2]| {
        p[0] = center[0] + (p[0] - center[0]) * scale;
        p[1] = center[1] + (p[1] - center[1]) * scale;
        *p = rotate_about(*p, center, angle);
    };
    match element {
        DrawingElement::Line { from, to, .. } => {
            transform_point(from);
            transform_point(to);
        }
        DrawingElement::Rect {
            from,
            to,
            angle: element_angle,
            ..
        }
        | DrawingElement::Oval {
            from,
            to,
            angle: element_angle,
            ..
        } => {
            // The shape keeps its local axes; only its own rotation changes.
            from[0] = center[0] + (from[0] - center[0]) * scale;
            from[1] = center[1] + (from[1] - center[1]) * scale;
            to[0] = center[0] + (to[0] - center[0]) * scale;
            to[1] = center[1] + (to[1] - center[1]) * scale;
            *element_angle = (*element_angle + angle).rem_euclid(360.0);
        }
        DrawingElement::Stroke { points, .. } => {
            for point in points {
                transform_point(point);
            }
        }
    }
}

impl AnimaApp {
    fn update_ui(&mut self, ctx: &egui::Context) {
        ui_style::install_theme(ctx, self.theme);
        if self.recovery_prompt_pending {
            self.recovery_prompt_pending = false;
            self.recovery_candidates = self
                .recovery
                .as_ref()
                .map(|r| r.candidates())
                .unwrap_or_default();
        }
        if self.pending_action.is_none()
            && !self.export_close_prompt
            && self.recovery_candidates.is_empty()
        {
            let (save, save_as, undo, redo, play) = ctx.input(|i| {
                (
                    shortcut_pressed(i, egui::Key::S, Some(false)),
                    shortcut_pressed(i, egui::Key::S, Some(true)),
                    shortcut_pressed(i, egui::Key::Z, Some(false)),
                    shortcut_pressed(i, egui::Key::Z, Some(true))
                        || shortcut_pressed(i, egui::Key::Y, None),
                    i.events.iter().any(|event| matches!(event, egui::Event::Key { key: egui::Key::Space, pressed: true, modifiers, .. } if !modifiers.command)),
                )
            });
            if save_as {
                self.save(true);
            } else if save {
                self.save(false);
            }
            if !ctx.wants_keyboard_input() {
                if undo {
                    self.undo();
                } else if redo {
                    self.redo();
                }
                if play {
                    self.playing = !self.playing;
                }
            }
        }
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && !self.close_ready {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.export.is_some() {
                self.export_close_prompt = true;
            } else if self.pending_action.is_none() && self.recovery_candidates.is_empty() {
                self.request_action(PendingAction::Close);
            }
            ctx.request_repaint();
        }
        if self.close_ready && !self.close_sent && !close_requested {
            self.close_sent = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.transaction.is_some() {
                self.cancel_transaction();
                self.drag_offset = None;
            }
            self.scene_press = None;
        }
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        if self.dirty {
            let elapsed = now.duration_since(self.last_autosave);
            if elapsed >= Duration::from_secs(30) {
                if let Some(Err(error)) = self
                    .recovery
                    .as_ref()
                    .map(|recovery| recovery.save(self.project.clone(), self.file.clone()))
                {
                    self.report_error(
                        diagnostics::Event::RecoveryWrite,
                        format!(
                            "Не удалось создать автокопию: {}",
                            snapshot_error_message(&error)
                        ),
                    );
                }
                // Poll the worker at least once even if it finishes immediately.
                ctx.request_repaint_after(Duration::from_millis(100));
                self.last_autosave = now;
                ctx.request_repaint_after(Duration::from_secs(30));
            } else {
                ctx.request_repaint_after(Duration::from_secs(30) - elapsed);
            }
        }
        if self.recovery.as_ref().is_some_and(|r| r.is_pending()) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        while let Some(error) = self.recovery.as_ref().and_then(|r| r.error()) {
            self.report_error(
                diagnostics::Event::RecoveryWrite,
                format!(
                    "Не удалось создать автокопию: {}",
                    snapshot_error_message(&error)
                ),
            );
        }
        let export_result = self
            .export
            .as_ref()
            .and_then(|job| job.result.lock().expect("export state").take());
        if let Some(done) = export_result {
            self.record_export_result(done);
            self.export = None;
        } else if self.export.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.playing {
            self.time += dt;
            if self.time > self.project.duration {
                self.time = 0.0;
            }
            ctx.request_repaint();
        }
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.menu_button("Файл", |ui| {
                    if ui.button("Новый").clicked() {
                        self.new_project();
                        ui.close();
                    }
                    if ui.button("Открыть").clicked() {
                        self.open();
                        ui.close();
                    }
                    if ui.button("Сохранить").clicked() {
                        self.save(false);
                        ui.close();
                    }
                    if ui.button("Сохранить как…").clicked() {
                        self.save(true);
                        ui.close();
                    }
                });
                ui.separator();
                if ui
                    .add_enabled(!self.undo.is_empty(), egui::Button::new("Отменить"))
                    .clicked()
                {
                    self.undo();
                }
                if ui
                    .add_enabled(!self.redo.is_empty(), egui::Button::new("Повторить"))
                    .clicked()
                {
                    self.redo();
                }
                ui.separator();
                ui.add_sized(
                    [100.0, ui.spacing().interact_size.y],
                    egui::Label::new(&self.project.name).truncate(),
                )
                .on_hover_text(&self.project.name);
                ui.label(if self.dirty {
                    "Есть изменения"
                } else {
                    "Сохранено"
                });
                ui.menu_button("Помощь и настройки", |ui| {
                    ui.label("Тема оформления");
                    for (theme, label) in [
                        (ui_style::Theme::Light, "Светлая"),
                        (ui_style::Theme::Dark, "Тёмная"),
                    ] {
                        if ui.selectable_label(self.theme == theme, label).clicked() {
                            self.theme = theme;
                            ui_style::install_theme(ctx, theme);
                            ctx.request_repaint();
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Как сделать мультфильм").clicked() {
                        self.show_lesson = true;
                    }
                    if ui
                        .button("Копировать сведения")
                        .on_hover_text(
                            "Версия и сведения о системе без содержимого проекта и путей",
                        )
                        .clicked()
                    {
                        ctx.copy_text(diagnostics::summary());
                        self.message = "Сведения скопированы".into();
                    }
                    ui.menu_button("Масштаб интерфейса", |ui| {
                        for (label, factor) in [
                            ("Мелкий", 0.85),
                            ("Обычный", 1.0),
                            ("Крупный", 1.25),
                            ("Очень крупный", 1.5),
                        ] {
                            if ui
                                .selectable_label((ctx.zoom_factor() - factor).abs() < 0.01, label)
                                .clicked()
                            {
                                ctx.set_zoom_factor(factor);
                                ui.close();
                            }
                        }
                    });
                });
                let remaining = ui.clip_rect().right() - ui.cursor().left();
                ui.add_space((remaining - 120.0).max(0.0));
                if ui_style::primary(ui, "Сделать видео", self.export.is_none()).clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Видео MP4", &["mp4"])
                        .set_file_name("мой-мультфильм.mp4")
                        .save_file()
                {
                    self.export_notice = None;
                    self.export = Some(start_export(self.project.clone(), path));
                }
            });
        });
        egui::SidePanel::left("assets")
            .resizable(false)
            .default_width(180.0)
            .show(ctx, |ui| {
                ui.heading("Рисунки");
                ui.label("Выберите рисунок для сцены");
                ui.separator();
                let asset_ids: std::collections::HashSet<_> =
                    self.project.assets.iter().map(|asset| asset.id).collect();
                self.thumbnails.retain(|id, _| asset_ids.contains(id));
                let row_height = 48.0
                    + 2.0 * ui.spacing().interact_size.y
                    + 4.0 * ui.spacing().item_spacing.y
                    + 20.0;
                let count = self.project.assets.len();
                egui::ScrollArea::vertical()
                    .id_salt("asset_list")
                    .max_height((ui.available_height() - 42.0).max(50.0))
                    .show_rows(ui, row_height, count, |ui, rows| {
                        for index in rows {
                            let asset = &self.project.assets[index];
                            let id = asset.id;
                            let thumbnail = self.thumbnails.entry(id).or_insert_with(|| {
                                let pixels = scene::thumbnail(asset);
                                let image =
                                    egui::ColorImage::from_rgba_unmultiplied([96, 96], &pixels);
                                Thumbnail {
                                    source: asset.clone(),
                                    texture: ctx.load_texture(
                                        format!("asset-{id}"),
                                        image,
                                        egui::TextureOptions::LINEAR,
                                    ),
                                }
                            });
                            if thumbnail.source != *asset {
                                let pixels = scene::thumbnail(asset);
                                thumbnail.texture.set(
                                    egui::ColorImage::from_rgba_unmultiplied([96, 96], &pixels),
                                    egui::TextureOptions::LINEAR,
                                );
                                thumbnail.source = asset.clone();
                            }
                            let texture_id = thumbnail.texture.id();
                            let name = asset.name.clone();
                            ui.push_id(id, |ui| {
                                egui::Frame::group(ui.style())
                                    .inner_margin(egui::Margin::same(8))
                                    .show(ui, |ui| {
                                        ui.set_min_width((ui.available_width() - 16.0).max(1.0));
                                        ui.horizontal(|ui| {
                                            ui.image((texture_id, Vec2::splat(48.0)));
                                            let width = ui.available_width().max(1.0);
                                            ui.add_sized(
                                                [width, 48.0],
                                                egui::Label::new(&name).truncate(),
                                            )
                                            .on_hover_text(&name);
                                        });
                                        ui.horizontal_wrapped(|ui| {
                                            if ui_style::primary(ui, "На сцену", true).clicked()
                                            {
                                                self.add_object(id);
                                            }
                                            if ui.button("Изменить").clicked() {
                                                self.open_editor(id);
                                            }
                                        });
                                    });
                            });
                        }
                    });
                if ui
                    .add_sized(
                        [ui.available_width(), 32.0],
                        egui::Button::new("Новый рисунок"),
                    )
                    .clicked()
                {
                    self.cancel_transaction();
                    self.reset_editor_gesture();
                    let id = Uuid::new_v4();
                    let asset = Asset {
                        id,
                        name: format!("Рисунок {}", self.project.assets.len()),
                        color: [70, 140, 235, 255],
                        kind: Shape::Rectangle,
                        content: AssetContent::Drawing,
                        elements: vec![],
                    };
                    if self.try_edit(|project| project.assets.push(asset)) {
                        self.open_editor(id);
                        self.message = "Новый рисунок открыт в редакторе.".into();
                    }
                }
            });
        egui::SidePanel::right("properties")
            .resizable(false)
            .default_width(220.0)
            .show(ctx, |ui| {
                if let Some(job) = &self.export {
                    ui.group(|ui| {
                        ui.strong("Создание видео");
                        if job.cancel.load(Ordering::Relaxed) {
                            ui.label("Отменяю экспорт…");
                        } else {
                            match job.phase() {
                                ExportPhase::Preparing => {
                                    ui.label("Проверяю кодировщик…");
                                }
                                ExportPhase::Writing => {
                                    let fraction = (job.progress.load(Ordering::Relaxed) as f32
                                        / job.total.max(1) as f32)
                                        .clamp(0.0, 1.0);
                                    ui.add(egui::ProgressBar::new(fraction).show_percentage());
                                }
                                ExportPhase::Finalizing => {
                                    ui.label("Завершаю видео…");
                                }
                                ExportPhase::Succeeded => {
                                    ui.label("Видео готово");
                                }
                                ExportPhase::Cancelled => {
                                    ui.label("Экспорт отменён");
                                }
                                ExportPhase::Failed => {
                                    ui.label("Ошибка экспорта");
                                }
                            }
                            if ui.button("Отменить экспорт").clicked() {
                                job.cancel.store(true, Ordering::Relaxed);
                            }
                        }
                    });
                }
                if let Some(notice) = &self.export_notice {
                    let mut dismiss = false;
                    ui.group(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong("Создание видео");
                            dismiss = ui.button("Закрыть").clicked();
                        });
                        egui::ScrollArea::vertical()
                            .id_salt("export_notice")
                            .max_height((ui.available_height() * 0.25).clamp(40.0, 100.0))
                            .show(ui, |ui| match notice {
                                ExportNotice::Ready(name) => {
                                    ui.label(format!("Видео готово: {name}"));
                                }
                                ExportNotice::Cancelled => {
                                    ui.label("Экспорт отменён. Можно создать видео снова.");
                                }
                                ExportNotice::Failed(message) => {
                                    ui.colored_label(ui.visuals().error_fg_color, message);
                                    ui.label(
                                        "Исправьте причину и нажмите «Сделать видео» ещё раз.",
                                    );
                                }
                            });
                    });
                    if dismiss {
                        self.export_notice = None;
                    }
                }
                egui::ScrollArea::vertical()
                    .id_salt("properties_scroll")
                    .show(ui, |ui| {
                        let step = if self.project.objects.is_empty() {
                            "Добавь рисунок на сцену"
                        } else if self
                            .project
                            .objects
                            .iter()
                            .all(|object| object.poses.len() < 2)
                        {
                            "Передвинь время и объект"
                        } else {
                            "Нажми «Смотреть»"
                        };
                        ui.label(step);
                        if let Some(error) = &self.error {
                            let mut dismiss = false;
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(Color32::from_rgb(220, 70, 60), error);
                                dismiss = ui.button("Закрыть сообщение").clicked();
                            });
                            if dismiss {
                                self.error = None;
                            }
                        }
                        ui.heading("Свойства");
                        if let Some(id) = self.selected {
                            ui.strong("Объект и порядок слоёв");
                            if let Some(object) = self.selected_object() {
                                let mut name = object.name.clone();
                                ui.label("Название объекта");
                                if ui.text_edit_singleline(&mut name).changed() {
                                    self.try_edit(|project| {
                                        if let Some(o) =
                                            project.objects.iter_mut().find(|o| o.id == id)
                                        {
                                            o.name = name;
                                        }
                                    });
                                }
                            }
                            ui.horizontal(|ui| {
                                let index = self
                                    .project
                                    .objects
                                    .iter()
                                    .position(|o| o.id == id)
                                    .unwrap_or(0);
                                if ui
                                    .add_enabled(index > 0, egui::Button::new("Назад"))
                                    .on_hover_text("Переместить объект за другие")
                                    .clicked()
                                {
                                    self.reorder_object(id, false);
                                }
                                if ui
                                    .add_enabled(
                                        index + 1 < self.project.objects.len(),
                                        egui::Button::new("Вперёд"),
                                    )
                                    .on_hover_text("Переместить объект перед другими")
                                    .clicked()
                                {
                                    self.reorder_object(id, true);
                                }
                            });
                            let tr = self
                                .selected_object()
                                .map(|o| state_at(o, self.time))
                                .unwrap_or_default();
                            ui.label(format!("Время: {:.1} сек.", self.time));
                            let (mut x, mut y, mut scale, mut angle) =
                                (tr.x, tr.y, tr.scale, tr.angle);
                            ui.separator();
                            ui.strong("Положение и внешний вид");
                            let position = ui
                                .horizontal_wrapped(|ui| {
                                    [
                                        ui.add(
                                            egui::DragValue::new(&mut x).prefix("X: ").speed(1.0),
                                        ),
                                        ui.add(
                                            egui::DragValue::new(&mut y).prefix("Y: ").speed(1.0),
                                        ),
                                    ]
                                })
                                .inner;
                            let responses = [
                                position[0].clone(),
                                position[1].clone(),
                                ui.add(egui::Slider::new(&mut scale, 0.2..=3.0).text("Размер")),
                                ui.add(
                                    egui::Slider::new(&mut angle, -180.0..=180.0).text("Поворот"),
                                ),
                            ];
                            if responses.iter().any(|r| r.drag_started()) {
                                self.begin_transaction();
                            }
                            if responses.iter().any(|r| r.changed()) {
                                self.playing = false;
                                self.upsert_pose(id, Transform { x, y, scale, angle });
                            }
                            if responses.iter().any(|r| r.drag_stopped()) {
                                self.finish_transaction();
                            }
                            ui.separator();
                            ui.heading("Позы");
                            ui.label("Нажмите отметку на дорожке или выберите позу ниже.");
                            let pose_count =
                                self.selected_object().map(|o| o.poses.len()).unwrap_or(0);
                            let row_height = ui.spacing().interact_size.y;
                            egui::ScrollArea::vertical()
                                .id_salt("pose_list")
                                .max_height(90.0)
                                .show_rows(ui, row_height, pose_count, |ui, range| {
                                    for index in range {
                                        let Some(time) = self
                                            .selected_object()
                                            .and_then(|o| o.poses.get(index))
                                            .map(|p| p.time)
                                        else {
                                            continue;
                                        };
                                        let selected = self
                                            .selected_pose
                                            .is_some_and(|(object, t)| object == id && t == time);
                                        if ui
                                            .add_sized(
                                                [ui.available_width(), row_height],
                                                egui::Button::selectable(
                                                    selected,
                                                    format!("{} сек.", pose_time_label(time)),
                                                ),
                                            )
                                            .clicked()
                                        {
                                            self.select_pose(id, time);
                                        }
                                    }
                                });
                            if ui.button("Добавить позу в это время").clicked()
                            {
                                self.create_pose();
                            }
                            let selected = self.selected_pose.filter(|(object, _)| *object == id);
                            if let Some((_, time)) = selected {
                                ui.separator();
                                ui.strong("Выбранная поза и движение");
                                ui.label(format!("Выбрана: {} сек.", pose_time_label(time)));
                                ui.horizontal_wrapped(|ui| {
                                    if ui.button("−0,1 с").clicked() {
                                        self.move_pose(-STEP);
                                    }
                                    if ui.button("+0,1 с").clicked() {
                                        self.move_pose(STEP);
                                    }
                                });
                                let linear = self
                                    .selected_object()
                                    .and_then(|object| {
                                        let i = object.poses.partition_point(|p| p.time < time);
                                        object
                                            .poses
                                            .get(i)
                                            .filter(|p| p.time == time)
                                            .map(|p| p.linear)
                                    })
                                    .unwrap_or(false);
                                ui.label("Движение к следующей позе:");
                                ui.horizontal_wrapped(|ui| {
                                    if ui.selectable_label(!linear, "Плавно").clicked() {
                                        self.set_pose_linear(false);
                                    }
                                    if ui.selectable_label(linear, "Равномерно").clicked()
                                    {
                                        self.set_pose_linear(true);
                                    }
                                });
                                if ui
                                    .add(egui::Button::new(
                                        egui::RichText::new("Удалить позу")
                                            .color(ui.visuals().error_fg_color),
                                    ))
                                    .clicked()
                                {
                                    self.delete_pose();
                                }
                            } else {
                                ui.label("Выберите позу для изменения.");
                            }
                            ui.separator();
                            if ui
                                .add(egui::Button::new(
                                    egui::RichText::new("Удалить объект")
                                        .color(ui.visuals().error_fg_color),
                                ))
                                .clicked()
                                && self.try_edit(|project| project.objects.retain(|o| o.id != id))
                            {
                                self.selected = None;
                                self.selected_pose = None;
                            }
                        } else {
                            ui.label("Выбери объект на сцене");
                        }
                    });
            });
        egui::TopBottomPanel::bottom("timeline")
            .resizable(true)
            .default_height(190.0)
            .min_height(150.0)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui_style::primary(
                        ui,
                        if self.playing {
                            "Пауза"
                        } else {
                            "Смотреть"
                        },
                        true,
                    )
                    .clicked()
                    {
                        self.playing = !self.playing;
                    }
                    if ui.button("В начало").clicked() {
                        self.time = 0.0;
                        self.playing = false;
                    }
                    ui.label(format!(
                        "{:.1} / {:.1} сек.",
                        self.time, self.project.duration
                    ));
                });
                let mut duration = self.project.duration;
                let response = ui
                    .horizontal_wrapped(|ui| {
                        ui.label("Длительность:");
                        ui.spacing_mut().slider_width =
                            (ui.clip_rect().width() - 165.0).clamp(20.0, 100.0);
                        ui.add(egui::Slider::new(&mut duration, 1.0..=60.0).suffix(" сек."))
                    })
                    .inner;
                response.widget_info(|| {
                    egui::WidgetInfo::slider(true, duration as f64, "Длительность фильма")
                });
                if response.drag_started() {
                    self.begin_transaction();
                }
                if response.changed() {
                    let last = self
                        .project
                        .objects
                        .iter()
                        .flat_map(|o| o.poses.iter())
                        .map(|p| p.time)
                        .fold(0.0_f32, f32::max);
                    if duration >= last {
                        if self.try_edit(|project| project.duration = duration) {
                            self.time = self.time.min(duration);
                            self.message = "Длительность изменена".into();
                        }
                    } else {
                        self.message = "Нельзя сократить: за границей есть позы".into();
                    }
                }
                if response.drag_stopped() {
                    self.finish_transaction();
                }
                ui.separator();
                let label_width = 140.0;
                let width = (ui.clip_rect().right()
                    - ui.cursor().left()
                    - label_width
                    - ui.spacing().item_spacing.x)
                    .max(10.0);
                ui.horizontal(|ui| {
                    ui.allocate_space(Vec2::new(label_width, 1.0));
                    let (r, response) =
                        ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click_and_drag());
                    let p = ui.painter_at(r);
                    p.line_segment(
                        [r.left_center(), r.right_center()],
                        Stroke::new(2.0, Color32::GRAY),
                    );
                    let step =
                        ((self.project.duration / (width / 45.0).max(1.0)).ceil() as usize).max(1);
                    for sec in (0..=self.project.duration.floor() as usize).step_by(step) {
                        let x =
                            timeline::time_to_x(sec as f32, self.project.duration, r.left(), width);
                        p.line_segment(
                            [Pos2::new(x, r.top()), Pos2::new(x, r.bottom())],
                            Stroke::new(1.0, Color32::LIGHT_GRAY),
                        );
                        p.text(
                            Pos2::new(x, r.top()),
                            if x > r.right() - 35.0 {
                                egui::Align2::RIGHT_TOP
                            } else {
                                egui::Align2::LEFT_TOP
                            },
                            format!("{sec}"),
                            egui::FontId::proportional(12.0),
                            ui.visuals().text_color(),
                        );
                    }
                    let x = timeline::time_to_x(self.time, self.project.duration, r.left(), width);
                    p.line_segment(
                        [Pos2::new(x, r.top()), Pos2::new(x, r.bottom())],
                        Stroke::new(2.0, ui_style::ACCENT),
                    );
                    if (response.clicked() || response.dragged())
                        && let Some(pos) = response.interact_pointer_pos()
                    {
                        self.time = snap_within(
                            timeline::x_to_time(pos.x, self.project.duration, r.left(), width),
                            self.project.duration,
                        );
                        self.playing = false;
                    }
                });
                ui.separator();
                let row_count = self.project.objects.len();
                egui::ScrollArea::vertical()
                    .id_salt("timeline_tracks")
                    .show_rows(ui, 24.0, row_count, |ui, range| {
                        for index in range {
                            let Some(object) = self.project.objects.get(index) else {
                                continue;
                            };
                            let id = object.id;
                            let name = object.name.clone();
                            let marks =
                                timeline::markers(&object.poses, self.project.duration, width);
                            ui.push_id(id, |ui| {
                                ui.horizontal(|ui| {
                                    let (label_rect, label_response) = ui.allocate_exact_size(
                                        Vec2::new(label_width, 24.0),
                                        Sense::click(),
                                    );
                                    label_response.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::WidgetType::SelectableLabel,
                                            ui.is_enabled(),
                                            self.selected == Some(id),
                                            format!("Дорожка: {name}"),
                                        )
                                    });
                                    let text = egui::WidgetText::from(name.clone()).into_galley(
                                        ui,
                                        Some(egui::TextWrapMode::Truncate),
                                        label_width - 8.0,
                                        egui::TextStyle::Body,
                                    );
                                    if self.selected == Some(id) {
                                        ui.painter().rect_filled(
                                            label_rect,
                                            2.0,
                                            ui.visuals().selection.bg_fill,
                                        );
                                    }
                                    ui.painter().galley(
                                        label_rect.left_center()
                                            - Vec2::new(-3.0, text.size().y / 2.0),
                                        text,
                                        ui.visuals().text_color(),
                                    );
                                    let label_response = label_response.on_hover_text(&name);
                                    if self.selected == Some(id) || label_response.has_focus() {
                                        ui.painter().rect_stroke(
                                            label_rect,
                                            2.0,
                                            Stroke::new(1.5, ui_style::ACCENT),
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                    if label_response.clicked() {
                                        self.selected = Some(id);
                                        self.selected_pose = None;
                                    }
                                    let (rect, response) = ui.allocate_exact_size(
                                        Vec2::new(width, 24.0),
                                        Sense::click(),
                                    );
                                    let painter = ui.painter_at(rect);
                                    painter.line_segment(
                                        [rect.left_center(), rect.right_center()],
                                        Stroke::new(1.0, Color32::LIGHT_GRAY),
                                    );
                                    for time in marks {
                                        let x = timeline::time_to_x(
                                            time,
                                            self.project.duration,
                                            rect.left(),
                                            width,
                                        );
                                        painter.circle_filled(
                                            Pos2::new(x, rect.center().y),
                                            6.0,
                                            ui_style::MUTED,
                                        );
                                    }
                                    if let Some((selected_id, time)) = self.selected_pose
                                        && selected_id == id
                                    {
                                        let x = timeline::time_to_x(
                                            time,
                                            self.project.duration,
                                            rect.left(),
                                            width,
                                        );
                                        painter.circle_filled(
                                            Pos2::new(x, rect.center().y),
                                            7.0,
                                            ui_style::ACCENT,
                                        );
                                        painter.circle_stroke(
                                            Pos2::new(x, rect.center().y),
                                            7.0,
                                            Stroke::new(2.0, Color32::WHITE),
                                        );
                                    }
                                    if response.clicked()
                                        && let Some(pos) = response.interact_pointer_pos()
                                    {
                                        let target = timeline::x_to_time(
                                            pos.x,
                                            self.project.duration,
                                            rect.left(),
                                            width,
                                        );
                                        let nearest = self
                                            .project
                                            .objects
                                            .iter()
                                            .find(|o| o.id == id)
                                            .and_then(|o| timeline::nearest(&o.poses, target));
                                        if let Some(time) = nearest.filter(|time| {
                                            (timeline::time_to_x(
                                                *time,
                                                self.project.duration,
                                                rect.left(),
                                                width,
                                            ) - pos.x)
                                                .abs()
                                                <= 10.0
                                        }) {
                                            self.select_pose(id, time);
                                        } else {
                                            self.selected = Some(id);
                                            self.selected_pose = None;
                                            self.time = snap_within(target, self.project.duration);
                                            self.playing = false;
                                        }
                                    }
                                });
                            });
                        }
                    });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).fill(ui_style::workspace(self.theme)))
            .show(ctx, |ui| {
                ui.heading("Сцена");
                if self.project.objects.is_empty() {
                    ui.label("Добавь рисунок из панели слева");
                } else {
                    ui.label("Перетаскивайте объект — в это время автоматически создаётся поза");
                }
                let available = ui.available_size();
                let factor = (available.x / scene::WIDTH)
                    .min(available.y / scene::HEIGHT)
                    .max(0.01);
                let size = Vec2::new(scene::WIDTH, scene::HEIGHT) * factor;
                let (space, _) = ui.allocate_exact_size(available.max(size), Sense::hover());
                let rect = Rect::from_center_size(space.center(), size);
                let response =
                    ui.interact(rect, ui.id().with("scene_canvas"), Sense::click_and_drag());
                let desired_scale = if self.playing {
                    1
                } else {
                    preview_scale_for(size.x * ctx.pixels_per_point())
                };
                if self.preview_target.0 != desired_scale {
                    self.preview_target = (desired_scale, Instant::now());
                }
                if self.preview_scale != desired_scale {
                    let delay = std::time::Duration::from_millis(200);
                    let elapsed = self.preview_target.1.elapsed();
                    if elapsed >= delay {
                        self.preview_scale = desired_scale;
                        self.scene_renderer.set_scale(desired_scale);
                    } else {
                        ctx.request_repaint_after(delay - elapsed);
                    }
                }
                let stamp = (self.scene_revision, self.time, self.preview_scale);
                if self.preview_stamp != Some(stamp) || self.scene_texture.is_none() {
                    let pixels = self.scene_renderer.render(&self.project, self.time);
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [
                            640 * self.preview_scale as usize,
                            360 * self.preview_scale as usize,
                        ],
                        pixels,
                    );
                    if let Some(texture) = &mut self.scene_texture {
                        texture.set(image, egui::TextureOptions::LINEAR);
                    } else {
                        self.scene_texture =
                            Some(ctx.load_texture("scene", image, egui::TextureOptions::LINEAR));
                    }
                    self.preview_stamp = Some(stamp);
                }
                let texture_id = self.scene_texture.as_ref().unwrap().id();
                let painter = ui.painter_at(rect);
                painter.image(
                    texture_id,
                    rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.0, Color32::LIGHT_GRAY),
                    egui::StrokeKind::Inside,
                );
                if self.project.objects.is_empty() {
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Добавь рисунок из панели слева",
                        egui::FontId::proportional(18.0),
                        ui_style::MUTED,
                    );
                }
                let to_scene = |pos: Pos2| (pos - rect.center()) / factor;
                let hit = |pos: Pos2| {
                    let p = to_scene(pos);
                    self.project.objects.iter().rev().find_map(|object| {
                        let tr = state_at(object, self.time);
                        let asset = self.project.asset(object.asset_id)?;
                        let q = scene::local_point(tr, p.x, p.y);
                        let margin = 6.0 / (factor * tr.scale);
                        scene::contains(asset, q, margin).then_some((
                            object.id,
                            object.asset_id,
                            tr,
                        ))
                    })
                };
                let hit_result = response.interact_pointer_pos().and_then(hit);
                if response.is_pointer_button_down_on()
                    && ctx.input(|i| i.pointer.primary_pressed())
                {
                    self.scene_press = ctx.input(|i| i.pointer.press_origin());
                }
                let released = ctx.input(|i| i.pointer.primary_released());
                let release_pos = ctx.input(|i| i.pointer.latest_pos());
                let quick_drag = released
                    && self.drag_offset.is_none()
                    && self
                        .scene_press
                        .zip(release_pos)
                        .is_some_and(|(start, end)| start.distance(end) > 6.0);
                let quick_hit = self.scene_press.and_then(hit);
                let drag_origin = ctx
                    .input(|input| input.pointer.press_origin())
                    .or_else(|| response.interact_pointer_pos());
                let drag_hit = drag_origin.and_then(hit);
                if response.drag_started()
                    && let Some(pos) = drag_origin
                {
                    if let Some((id, _, tr)) = drag_hit {
                        if self.selected != Some(id) {
                            self.selected_pose = None;
                        }
                        self.selected = Some(id);
                        self.drag_offset = Some(to_scene(pos) - Vec2::new(tr.x, tr.y));
                        self.begin_transaction();
                    } else {
                        self.selected = None;
                        self.drag_offset = None;
                    }
                }
                if quick_drag
                    && let Some(pos) = self.scene_press
                    && let Some((id, _, tr)) = quick_hit
                {
                    if self.selected != Some(id) {
                        self.selected_pose = None;
                    }
                    self.selected = Some(id);
                    self.drag_offset = Some(to_scene(pos) - Vec2::new(tr.x, tr.y));
                    self.begin_transaction();
                }
                if response.clicked() {
                    let selection = hit_result.map(|x| x.0);
                    if selection != self.selected {
                        self.selected_pose = None;
                    }
                    self.selected = selection;
                }
                if (response.dragged() || response.drag_stopped() || quick_drag)
                    && let (Some(id), Some(pos), Some(offset)) = (
                        self.selected,
                        response.interact_pointer_pos().or(release_pos),
                        self.drag_offset,
                    )
                {
                    let old = self
                        .selected_object()
                        .map(|o| state_at(o, self.time))
                        .unwrap_or_default();
                    let p = to_scene(pos) - offset;
                    let next = Transform {
                        x: p.x,
                        y: p.y,
                        ..old
                    };
                    if next != old {
                        self.playing = false;
                        self.upsert_pose(id, next);
                    }
                }
                if response.drag_stopped() || quick_drag {
                    self.drag_offset = None;
                    self.finish_transaction();
                }
                if released {
                    self.scene_press = None;
                }
                if let Some(object) = self.selected_object()
                    && let Some(b) = self.project.asset(object.asset_id).and_then(scene::bounds)
                {
                    let tr = state_at(object, self.time);
                    let (sin, cos) = scene::rotation_sin_cos(tr.angle);
                    let corners =
                        [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]].map(|p| {
                            rect.center()
                                + Vec2::new(
                                    tr.x + tr.scale * (cos * p[0] - sin * p[1]),
                                    tr.y + tr.scale * (sin * p[0] + cos * p[1]),
                                ) * factor
                        });
                    painter.add(egui::Shape::closed_line(
                        corners.to_vec(),
                        Stroke::new(2.0, ui_style::ACCENT),
                    ));
                }
            });
        self.confirmation_ui(ctx);
        if self.close_ready && !self.close_sent {
            ctx.request_repaint();
        }
        if self.show_lesson {
            egui::Window::new("Первый мультфильм").open(&mut self.show_lesson).resizable(true).show(ctx, |ui| {
                ui.label("1. В разделе «Рисунки» нажмите «Добавить» у мяча или создайте свой рисунок.");
                ui.label("2. Переместите объект на сцене — это первая поза.");
                ui.label("3. Нажмите на шкалу времени, например на 2 секунды, и снова переместите объект. Появится вторая поза.");
                ui.label("4. Нажмите «В начало», затем «Смотреть».");
                ui.label("5. Сохраните проект, чтобы продолжить работу позже, и нажмите «Сделать видео».");
                ui.label("Позы можно выбирать на дорожке или в свойствах. Там же можно изменить время и способ движения.");
            });
        }
        if let Some(asset_id) = self.edit_asset {
            let mut open = true;
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of(asset_id),
                egui::ViewportBuilder::default()
                    .with_title("Редактор рисунка — AnimaPro")
                    .with_inner_size([760.0, 560.0]),
                |editor_ctx, _| {
                    if editor_ctx.input(|i| i.viewport().close_requested()) {
                        open = false;
                    }
                    self.editor_ui(editor_ctx, asset_id, &mut open);
                },
            );
            if !open {
                self.cancel_transaction();
                self.reset_editor_gesture();
                self.edit_asset = None;
            }
        }
    }
}

impl eframe::App for AnimaApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.update_ui(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn primary_controls_fit_in_both_themes() {
        for theme in [ui_style::Theme::Light, ui_style::Theme::Dark] {
            for (width, height) in [(1024.0, 640.0), (1280.0, 760.0)] {
                for zoom in [1.0, 1.5] {
                    let mut app = AnimaApp::with_recovery(Err("test".into()));
                    app.theme = theme;
                    let ctx = egui::Context::default();
                    ctx.enable_accesskit();
                    ctx.set_zoom_factor(zoom);
                    let input = || egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            Vec2::new(width / zoom, height / zoom),
                        )),
                        ..Default::default()
                    };
                    let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
                    let output = ctx.run(input(), |ctx| app.update_ui(ctx));
                    let nodes = output.platform_output.accesskit_update.unwrap().nodes;
                    for label in ["Файл", "Сделать видео", "Новый рисунок", "Смотреть"]
                    {
                        let bounds = nodes
                            .iter()
                            .find_map(|(_, node)| {
                                (node.label() == Some(label))
                                    .then(|| node.bounds())
                                    .flatten()
                            })
                            .unwrap_or_else(|| panic!("missing {label} in {theme:?}"));
                        assert!(
                            bounds.x0 >= 0.0
                                && bounds.y0 >= 0.0
                                && bounds.x1 <= (width / zoom) as f64
                                && bounds.y1 <= (height / zoom) as f64,
                            "{label}: {bounds:?}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn theme_switch_preserves_project_and_reaches_both_windows() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let before = app.project.clone();
        let id = app.project.assets[0].id;
        let ctx = egui::Context::default();
        for theme in [ui_style::Theme::Light, ui_style::Theme::Dark] {
            app.theme = theme;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
                ..Default::default()
            };
            let _ = ctx.run(input.clone(), |ctx| app.update_ui(ctx));
            assert_eq!(
                ctx.style().visuals.dark_mode,
                theme == ui_style::Theme::Dark
            );
            let mut open = true;
            let _ = ctx.run(input, |ctx| app.editor_ui(ctx, id, &mut open));
            assert_eq!(
                ctx.style().visuals.dark_mode,
                theme == ui_style::Theme::Dark
            );
            assert!(app.project == before);
            assert!(!app.dirty);
        }
    }
    #[test]
    fn timeline_track_has_accessible_name_and_keyboard_selection() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let asset = app.project.assets[0].id;
        app.add_object(asset);
        let first = app.selected.unwrap();
        app.add_object(asset);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let input = |events| egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        let output = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        let (node_id, node) = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .into_iter()
            .find(|(_, node)| node.label() == Some("Дорожка: Мяч 1"))
            .expect("named track");
        assert_eq!(node.toggled(), Some(egui::accesskit::Toggled::False));
        // The node identifier was generated from an egui widget Id in this context.
        let widget_id = unsafe { egui::Id::from_high_entropy_bits(node_id.0) };
        ctx.memory_mut(|memory| memory.request_focus(widget_id));
        for pressed in [true, false] {
            let event = egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            let _ = ctx.run(input(vec![event]), |ctx| app.update_ui(ctx));
        }
        assert_eq!(app.selected, Some(first));
        assert!(!app.playing, "activating a track must not start playback");
    }
    #[test]
    fn timeline_markers_remain_visible_with_an_object_at_target_scales() {
        for (w, h) in [(1024.0_f32, 640.0_f32), (1280.0, 760.0)] {
            for zoom in [0.85_f32, 1.0, 1.25, 1.5] {
                for density in [1.0, 2.0] {
                    let mut app = AnimaApp::with_recovery(Err("test".into()));
                    app.add_object(app.project.assets[0].id);
                    app.project.objects[0].poses.push(Pose {
                        time: app.project.duration,
                        transform: Transform::default(),
                        linear: false,
                    });
                    let ctx = egui::Context::default();
                    ui_style::install(&ctx);
                    ctx.set_zoom_factor(zoom);
                    let input = || {
                        let mut input = egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                Vec2::new(w / zoom, h / zoom),
                            )),
                            ..Default::default()
                        };
                        input
                            .viewports
                            .get_mut(&egui::ViewportId::ROOT)
                            .unwrap()
                            .native_pixels_per_point = Some(density);
                        input
                    };
                    let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
                    let output = ctx.run(input(), |ctx| app.update_ui(ctx));
                    let visible = output
                        .shapes
                        .iter()
                        .filter(|shape| match &shape.shape {
                            egui::Shape::Circle(circle)
                                if circle.fill == ui_style::MUTED && circle.radius == 6.0 =>
                            {
                                let marker =
                                    Rect::from_center_size(circle.center, Vec2::splat(12.0));
                                shape.clip_rect.contains_rect(marker) && marker.bottom() <= h / zoom
                            }
                            _ => false,
                        })
                        .count();
                    assert_eq!(
                        visible, 2,
                        "endpoint markers clipped at {w}x{h}, zoom {zoom}, density {density}"
                    );
                }
            }
        }
    }
    #[test]
    fn duration_control_fits_timeline_at_target_sizes_and_scales() {
        for (w, h) in [(1024.0_f32, 640.0_f32), (1280.0, 760.0)] {
            for zoom in [0.85_f32, 1.0, 1.25, 1.5] {
                for density in [1.0, 2.0] {
                    let mut app = AnimaApp::with_recovery(Err("test".into()));
                    app.add_object(app.project.assets[0].id);
                    app.project.duration = 60.0;
                    let ctx = egui::Context::default();
                    ui_style::install(&ctx);
                    ctx.enable_accesskit();
                    ctx.set_zoom_factor(zoom);
                    let input = || {
                        let mut input = egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                Vec2::new(w / zoom, h / zoom),
                            )),
                            ..Default::default()
                        };
                        input
                            .viewports
                            .get_mut(&egui::ViewportId::ROOT)
                            .unwrap()
                            .native_pixels_per_point = Some(density);
                        input
                    };
                    let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
                    let output = ctx.run(input(), |ctx| app.update_ui(ctx));
                    let right = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Circle(circle)
                                if circle.fill == ui_style::MUTED && circle.radius == 6.0 =>
                            {
                                Some(shape.clip_rect.right())
                            }
                            _ => None,
                        })
                        .expect("timeline marker");
                    let bounds = output
                        .platform_output
                        .accesskit_update
                        .unwrap()
                        .nodes
                        .iter()
                        .find_map(|(_, node)| {
                            (node.label() == Some("Длительность фильма"))
                                .then(|| node.bounds())
                                .flatten()
                        })
                        .expect("duration control");
                    assert!(
                        bounds.x0 >= 0.0
                            && bounds.x1 <= right as f64 + 1.0
                            && bounds.y0 >= 0.0
                            && bounds.y1 <= (h / zoom) as f64 + 1.0,
                        "duration control clipped: {bounds:?} at {w}x{h}, zoom {zoom}, density {density}; right {right}"
                    );
                }
            }
        }
    }
    #[test]
    fn palette_swatch_has_accessible_name_and_responds_to_keyboard() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let input = |events| egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
            events,
            ..Default::default()
        };
        let mut swatch_id = None;
        let output = ctx.run(input(vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                swatch_id = Some(palette_swatch(ui, [235, 75, 70, 255], "Красный", false).id);
            });
        });
        assert!(
            output
                .platform_output
                .accesskit_update
                .unwrap()
                .nodes
                .iter()
                .any(|(_, node)| node.label() == Some("Красный"))
        );
        ctx.memory_mut(|memory| memory.request_focus(swatch_id.unwrap()));
        let mut clicked = false;
        let event = |pressed| egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        for pressed in [true, false] {
            let _ = ctx.run(input(vec![event(pressed)]), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    clicked |= palette_swatch(ui, [235, 75, 70, 255], "Красный", false).clicked();
                });
            });
        }
        assert!(clicked, "focused palette swatch must activate with Space");
    }
    #[test]
    fn export_results_remain_in_a_dismissible_block() {
        for result in [
            Ok(PathBuf::from("очень-длинное-название-видео.mp4")),
            Err(ExportError::Cancelled),
            Err(ExportError::EncoderFailed("длинная ошибка ".repeat(1000))),
            Err(ExportError::EncoderFailed("д".repeat(5000))),
        ] {
            let is_failure = matches!(&result, Err(ExportError::EncoderFailed(_)));
            let mut app = AnimaApp::with_recovery(Err("test".into()));
            app.error = None;
            app.record_export_result(result);
            app.message = "Другое действие".into();
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            ui_style::install(&ctx);
            let input = |events| egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(1024.0 / 1.5, 640.0 / 1.5),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
            let output = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
            let mut error_text_found = false;
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape
                    && text
                        .galley
                        .job
                        .text
                        .starts_with("FFmpeg завершился с ошибкой:")
                {
                    error_text_found = true;
                    assert!(
                        text.galley.rect.width() <= 220.0,
                        "error text overflows its panel: {}",
                        text.galley.rect.width()
                    );
                }
            }
            assert_eq!(error_text_found, is_failure);
            let texture = app.scene_texture.as_ref().unwrap().id();
            assert!(
                output.shapes.iter().any(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) if mesh.texture_id == texture => {
                        let bounds = mesh.calc_bounds();
                        bounds.width() > 50.0
                            && bounds.height() > 25.0
                            && shape.clip_rect.intersects(bounds)
                    }
                    _ => false,
                }),
                "export message must not hide the scene"
            );
            let rect = output
                .platform_output
                .accesskit_update
                .unwrap()
                .nodes
                .iter()
                .find_map(|(_, node)| {
                    (node.label() == Some("Закрыть"))
                        .then(|| node.bounds())
                        .flatten()
                })
                .expect("dismiss export result");
            assert!(
                rect.x0 >= 0.0
                    && rect.y0 >= 0.0
                    && rect.x1 <= 1024.0 / 1.5
                    && rect.y1 <= 640.0 / 1.5
            );
            let pos = Pos2::new(
                ((rect.x0 + rect.x1) / 2.0) as f32,
                ((rect.y0 + rect.y1) / 2.0) as f32,
            );
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let _ = ctx.run(
                input(vec![egui::Event::PointerMoved(pos), button(true)]),
                |ctx| app.update_ui(ctx),
            );
            let _ = ctx.run(input(vec![button(false)]), |ctx| app.update_ui(ctx));
            assert!(
                app.export_notice.is_none(),
                "export result should be dismissible"
            );
            assert!(
                app.error.is_none(),
                "export failures stay in their own block"
            );
        }
    }
    #[test]
    fn long_names_and_error_do_not_hide_scene_or_primary_controls() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let long_name = "Очень длинное название рисунка и мультфильма ".repeat(20);
        app.project.name = long_name.clone();
        app.project.assets[0].name = long_name;
        app.add_object(app.project.assets[0].id);
        app.error =
            Some("Не удалось создать видео: проверьте кодировщик и доступ к папке. ".repeat(20));
        let ctx = egui::Context::default();
        ui_style::install(&ctx);
        ctx.enable_accesskit();
        ctx.set_zoom_factor(1.5);
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(1024.0 / 1.5, 640.0 / 1.5),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
        let output = ctx.run(input(), |ctx| app.update_ui(ctx));
        let nodes = &output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes;
        for label in ["Сделать видео", "Новый рисунок", "Смотреть", "В начало"]
        {
            let rect = nodes
                .iter()
                .find_map(|(_, node)| {
                    (node.label() == Some(label))
                        .then(|| node.bounds())
                        .flatten()
                })
                .unwrap_or_else(|| panic!("missing {label}"));
            assert!(
                rect.x0 >= 0.0
                    && rect.y0 >= 0.0
                    && rect.x1 <= 1024.0 / 1.5
                    && rect.y1 <= 640.0 / 1.5,
                "{label} outside window: {rect:?}"
            );
        }
        let texture = app.scene_texture.as_ref().unwrap().id();
        let visible = output.shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) if mesh.texture_id == texture => {
                let bounds = mesh.calc_bounds();
                bounds.width() > 50.0
                    && bounds.height() > 25.0
                    && shape.clip_rect.intersects(bounds)
            }
            _ => false,
        });
        assert!(visible, "long names or error hid the scene");
    }
    #[test]
    fn primary_controls_stay_inside_layout_at_target_sizes_and_scales() {
        for (w, h) in [(1024.0_f32, 640.0_f32), (1280.0, 760.0)] {
            for zoom in [0.85_f32, 1.0, 1.25, 1.5] {
                for density in [1.0, 2.0] {
                    let mut app = AnimaApp::with_recovery(Err("test".into()));
                    let ctx = egui::Context::default();
                    ui_style::install(&ctx);
                    ctx.enable_accesskit();
                    ctx.set_zoom_factor(zoom);
                    let input = || {
                        let mut input = egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                Vec2::new(w / zoom, h / zoom),
                            )),
                            ..Default::default()
                        };
                        input
                            .viewports
                            .get_mut(&egui::ViewportId::ROOT)
                            .unwrap()
                            .native_pixels_per_point = Some(density);
                        input
                    };
                    let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
                    let output = ctx.run(input(), |ctx| app.update_ui(ctx));
                    let nodes = output.platform_output.accesskit_update.unwrap().nodes;
                    for label in [
                        "Файл",
                        "Сделать видео",
                        "Новый рисунок",
                        "Смотреть",
                        "В начало",
                    ] {
                        let bounds = nodes
                            .iter()
                            .find_map(|(_, node)| {
                                (node.label() == Some(label))
                                    .then(|| node.bounds())
                                    .flatten()
                            })
                            .unwrap_or_else(|| {
                                panic!("missing {label} at {w}x{h}, zoom {zoom}, density {density}")
                            });
                        assert!(
                            bounds.x0 >= 0.0
                                && bounds.y0 >= 0.0
                                && bounds.x1 <= (w / zoom) as f64 + 1.0
                                && bounds.y1 <= (h / zoom) as f64 + 1.0,
                            "{label} outside window: {bounds:?}, {w}x{h}, zoom {zoom}, density {density}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn large_library_prepares_only_visible_thumbnails() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let template = app.project.assets[0].clone();
        app.project.assets = (0..1000)
            .map(|i| {
                let mut asset = template.clone();
                asset.id = Uuid::new_v4();
                asset.name = format!("Рисунок {i}");
                asset
            })
            .collect();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.update_ui(ctx));
        assert!(!app.thumbnails.is_empty());
        assert!(
            app.thumbnails.len() < 20,
            "all assets were prepared: {}",
            app.thumbnails.len()
        );
    }
    #[test]
    fn library_scroll_loads_changed_offscreen_asset() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let template = app.project.assets[0].clone();
        app.project.assets = (0..40)
            .map(|_| {
                let mut asset = template.clone();
                asset.id = Uuid::new_v4();
                asset
            })
            .collect();
        let last = app.project.assets.last().unwrap().id;
        let ctx = egui::Context::default();
        let input = |events| egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        assert!(!app.thumbnails.contains_key(&last));
        app.project.assets.last_mut().unwrap().color = [1, 2, 3, 255];
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(Pos2::new(50.0, 200.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, -100_000.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| app.update_ui(ctx),
        );
        for _ in 0..10 {
            let _ = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        }
        assert_eq!(
            app.thumbnails.get(&last).map(|item| item.source.color),
            Some([1, 2, 3, 255])
        );
    }
    #[test]
    fn thumbnail_cache_tracks_asset_changes_and_reuses_texture() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
        let id = app.project.assets[0].id;
        let texture = app.thumbnails[&id].texture.id();
        app.project.assets[0].color = [1, 2, 3, 255];
        let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
        assert_eq!(app.thumbnails[&id].texture.id(), texture);
        assert_eq!(app.thumbnails[&id].source.color, [1, 2, 3, 255]);
        app.project.assets[0].content = AssetContent::Drawing;
        app.project.assets[0].elements.push(DrawingElement::Line {
            from: [-20.0, 0.0],
            to: [20.0, 0.0],
            color: [0, 0, 0, 255],
            width: 3.0,
        });
        let first = ctx.run(input(), |ctx| app.update_ui(ctx));
        let image = |output: &egui::FullOutput| {
            let delta = output
                .textures_delta
                .set
                .iter()
                .find(|(id, _)| *id == texture)
                .expect("thumbnail texture updated");
            let egui::ImageData::Color(image) = &delta.1.image;
            image.pixels.clone()
        };
        let initial_pixels = image(&first);
        let DrawingElement::Line { color, .. } = &mut app.project.assets[0].elements[0] else {
            unreachable!()
        };
        *color = [230, 50, 30, 255];
        let second = ctx.run(input(), |ctx| app.update_ui(ctx));
        assert_ne!(initial_pixels, image(&second));
        assert_eq!(app.thumbnails[&id].texture.id(), texture);
        let unchanged = ctx.run(input(), |ctx| app.update_ui(ctx));
        assert!(
            !unchanged
                .textures_delta
                .set
                .iter()
                .any(|(id, _)| *id == texture)
        );
        app.project.assets.clear();
        let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
        assert!(app.thumbnails.is_empty());
    }
    #[test]
    fn preview_texture_is_invalidated_when_pixel_density_changes() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let ctx = egui::Context::default();
        let input = |pixels_per_point| {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 760.0))),
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(pixels_per_point);
            input
        };
        let _ = ctx.run(input(1.0), |ctx| app.update_ui(ctx));
        assert_eq!(app.preview_stamp.unwrap().2, 1);
        let texture_id = app.scene_texture.as_ref().unwrap().id();
        let interim = ctx.run(input(2.0), |ctx| app.update_ui(ctx));
        assert_eq!(app.preview_target.0, 2);
        assert_eq!(app.preview_scale, 1);
        assert!(
            !interim
                .textures_delta
                .set
                .iter()
                .any(|(id, _)| *id == texture_id)
        );
        let interim = ctx.run(input(2.0), |ctx| app.update_ui(ctx));
        assert!(
            !interim
                .textures_delta
                .set
                .iter()
                .any(|(id, _)| *id == texture_id)
        );
        app.preview_target.1 = Instant::now() - Duration::from_secs(1);
        let output = ctx.run(input(2.0), |ctx| app.update_ui(ctx));
        assert_eq!(app.preview_stamp.unwrap().2, 2);
        assert_eq!(app.scene_texture.as_ref().unwrap().id(), texture_id);
        assert!(
            output
                .textures_delta
                .set
                .iter()
                .any(|(id, delta)| *id == texture_id && delta.image.size() == [1280, 720])
        );
        let unchanged = ctx.run(input(2.0), |ctx| app.update_ui(ctx));
        assert!(
            !unchanged
                .textures_delta
                .set
                .iter()
                .any(|(id, _)| *id == texture_id)
        );
    }
    #[test]
    fn visual_layout_and_zoom_do_not_change_project_data() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        app.add_object(app.project.assets[0].id);
        app.time = 2.0;
        app.upsert_pose(
            app.selected.unwrap(),
            Transform {
                x: 25.0,
                y: -10.0,
                ..Transform::default()
            },
        );
        let before = serde_json::to_vec(&app.project).unwrap();
        let undo_count = app.undo.len();
        let ctx = egui::Context::default();
        ui_style::install(&ctx);
        for (width, height, zoom, density) in
            [(1024.0, 640.0, 0.85, 1.0), (1280.0, 760.0, 1.5, 2.0)]
        {
            ctx.set_zoom_factor(zoom);
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(width / zoom, height / zoom),
                )),
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(density);
            let _ = ctx.run(input, |ctx| app.update_ui(ctx));
        }
        assert_eq!(serde_json::to_vec(&app.project).unwrap(), before);
        assert_eq!(app.undo.len(), undo_count);
        assert_eq!(app.project.version, 2);
    }
    #[test]
    fn high_resolution_preview_matches_export_frame() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let asset = app.project.assets[0].id;
        app.project.assets[0].content = AssetContent::Drawing;
        app.project.assets[0].elements = vec![DrawingElement::Rect {
            from: [-35.0, -18.0],
            to: [35.0, 18.0],
            angle: 17.0,
            color: [35, 130, 220, 170],
            fill: true,
            width: 2.0,
        }];
        app.add_object(asset);
        let first = app.selected.unwrap();
        app.time = 2.0;
        app.upsert_pose(
            first,
            Transform {
                x: 45.0,
                y: -20.0,
                angle: 35.0,
                ..Transform::default()
            },
        );
        app.add_object(asset);
        let second = app.selected.unwrap();
        app.upsert_pose(
            second,
            Transform {
                x: -25.0,
                y: 18.0,
                scale: 0.8,
                ..Transform::default()
            },
        );
        app.time = 1.0;
        let ctx = egui::Context::default();
        let input = || {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 760.0))),
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(2.0);
            input
        };
        let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
        assert_eq!(app.preview_target.0, 2);
        app.preview_target.1 = Instant::now() - Duration::from_secs(1);
        let output = ctx.run(input(), |ctx| app.update_ui(ctx));
        let texture = app.scene_texture.as_ref().unwrap().id();
        let delta = output
            .textures_delta
            .set
            .iter()
            .find(|(id, _)| *id == texture)
            .expect("preview updated");
        let egui::ImageData::Color(actual) = &delta.1.image;
        let expected = egui::ColorImage::from_rgba_unmultiplied(
            [1280, 720],
            &export_frame(&app.project, app.time),
        );
        assert_eq!(actual.pixels, expected.pixels);
    }
    #[test]
    fn preview_resolution_tracks_physical_width() {
        assert_eq!(preview_scale_for(640.0), 1);
        assert_eq!(preview_scale_for(641.0), 2);
        assert_eq!(preview_scale_for(4000.0), 2);
    }
    #[test]
    fn dense_track_ui_builds_a_bounded_number_of_visible_shapes() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let asset = app.project.assets[0].id;
        app.add_object(asset);
        app.project.duration = 60.0;
        app.project.objects[0].poses = (0..100_000)
            .map(|i| Pose {
                time: i as f32 * 0.0005,
                transform: Transform::default(),
                linear: true,
            })
            .collect();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| app.update_ui(ctx));
        assert!(
            output.shapes.len() < 5_000,
            "too many shapes: {}",
            output.shapes.len()
        );
        assert_eq!(app.project.objects[0].poses.len(), 100_000);
    }
    #[test]
    fn interpolation_is_smooth() {
        let o = SceneObject {
            id: Uuid::new_v4(),
            asset_id: Uuid::new_v4(),
            name: "x".into(),
            poses: vec![
                Pose {
                    time: 0.0,
                    transform: Transform::default(),
                    linear: true,
                },
                Pose {
                    time: 1.0,
                    transform: Transform {
                        x: 100.0,
                        ..Transform::default()
                    },
                    linear: true,
                },
            ],
        };
        assert_eq!(state_at(&o, 0.5).x, 50.0);
    }
    #[test]
    fn shortest_rotation() {
        assert_eq!(angle_delta(170.0, -170.0), 20.0);
    }
    #[test]
    fn save_shortcut_works_while_object_name_has_focus() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        app.add_object(app.project.assets[0].id);
        let expected = app.project.clone();
        let path =
            std::env::temp_dir().join(format!("animapro-focused-save-{}.animapro", Uuid::new_v4()));
        app.file = Some(path.clone());
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let input = |events| egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        let output = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        let (node_id, _) = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .into_iter()
            .find(|(_, node)| node.role() == egui::accesskit::Role::TextInput)
            .expect("object name field");
        // AccessKit node IDs are derived from egui widget IDs in this context.
        let field_id = unsafe { egui::Id::from_high_entropy_bits(node_id.0) };
        ctx.memory_mut(|memory| memory.request_focus(field_id));
        assert!(ctx.wants_keyboard_input());
        let key = egui::Event::Key {
            key: egui::Key::S,
            physical_key: Some(egui::Key::S),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                command: true,
                mac_cmd: true,
                ..Default::default()
            },
        };
        let _ = ctx.run(input(vec![key]), |ctx| app.update_ui(ctx));
        assert!(!app.dirty, "saving must work with the name field focused");
        assert!(read_project(&path).unwrap() == expected);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn save_shortcut_still_saves_after_file_actions_moved_to_menu() {
        for mac in [false, true] {
            let mut app = AnimaApp::with_recovery(Err("test".into()));
            app.add_object(app.project.assets[0].id);
            let expected = app.project.clone();
            let path =
                std::env::temp_dir().join(format!("animapro-shortcut-{}.animapro", Uuid::new_v4()));
            app.file = Some(path.clone());
            let ctx = egui::Context::default();
            let input = |events| egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
            let key = egui::Event::Key {
                key: egui::Key::S,
                physical_key: Some(egui::Key::S),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    command: true,
                    mac_cmd: mac,
                    ctrl: !mac,
                    ..Default::default()
                },
            };
            let _ = ctx.run(input(vec![key]), |ctx| app.update_ui(ctx));
            assert!(!app.dirty);
            assert!(read_project(&path).unwrap() == expected);
            std::fs::remove_file(path).unwrap();
        }
    }
    #[test]
    fn project_zip_round_trip() {
        let path = std::env::temp_dir().join(format!("animapro-test-{}.animapro", Uuid::new_v4()));
        let project = Project::new();
        write_project(&path, &project).unwrap();
        let reopened = read_project(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(reopened.version, 2);
        assert_eq!(reopened.assets[0].name, "Мяч");
    }
    #[test]
    fn export_frame_has_hd_rgba_size() {
        assert_eq!(export_frame(&Project::new(), 0.0).len(), 1280 * 720 * 4);
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;

    fn rect(fill: bool, color: [u8; 4]) -> DrawingElement {
        DrawingElement::Rect {
            from: [-1000.0, -1000.0],
            to: [1000.0, 1000.0],
            angle: 0.0,
            color,
            fill,
            width: 4.0,
        }
    }
    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            events,
            ..Default::default()
        }
    }
    fn pointer(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }
    #[test]
    fn editor_tools_expose_selection_and_accept_keyboard_activation() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let asset = app.project.assets[0].id;
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut open = true;
        let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, asset, &mut open));
        let output = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, asset, &mut open));
        let nodes = output.platform_output.accesskit_update.unwrap().nodes;
        let pencil = nodes
            .iter()
            .find_map(|(_, node)| (node.label() == Some("Карандаш")).then_some(node))
            .expect("pencil tool");
        assert_eq!(pencil.toggled(), Some(egui::accesskit::Toggled::True));
        let (line_id, line) = nodes
            .into_iter()
            .find(|(_, node)| node.label() == Some("Линия"))
            .expect("line tool");
        assert_eq!(line.toggled(), Some(egui::accesskit::Toggled::False));
        // AccessKit node IDs originate from egui widget IDs in this context.
        let id = unsafe { egui::Id::from_high_entropy_bits(line_id.0) };
        ctx.memory_mut(|memory| memory.request_focus(id));
        for pressed in [true, false] {
            let event = egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            let _ = ctx.run(input(vec![event]), |ctx| {
                app.editor_ui(ctx, asset, &mut open)
            });
        }
        assert!(app.editor_tool == EditorTool::Line);
    }
    #[test]
    fn drawing_settings_are_disabled_for_selection_tool() {
        for (tool, disabled) in [(EditorTool::Select, true), (EditorTool::Pencil, false)] {
            let mut app = AnimaApp::with_recovery(Err("test".into()));
            let id = app.project.assets[0].id;
            app.editor_tool = tool;
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let mut open = true;
            let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
            let output = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
            let nodes = output.platform_output.accesskit_update.unwrap().nodes;
            for label in ["Толщина", "Красный"] {
                let node = nodes
                    .iter()
                    .find_map(|(_, node)| (node.label() == Some(label)).then_some(node))
                    .unwrap_or_else(|| panic!("missing {label}"));
                assert_eq!(node.is_disabled(), disabled, "incorrect state for {label}");
            }
        }
    }
    #[test]
    fn editor_palette_is_visible_without_scrolling() {
        for theme in [ui_style::Theme::Light, ui_style::Theme::Dark] {
            for (width, height) in [(760.0, 560.0), (507.0, 373.0)] {
                let mut app = AnimaApp::with_recovery(Err("test".into()));
                app.theme = theme;
                let id = app.project.assets[0].id;
                let ctx = egui::Context::default();
                ctx.enable_accesskit();
                let mut open = true;
                let input = || egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height))),
                    ..Default::default()
                };
                let _ = ctx.run(input(), |ctx| app.editor_ui(ctx, id, &mut open));
                let output = ctx.run(input(), |ctx| app.editor_ui(ctx, id, &mut open));
                let nodes = output.platform_output.accesskit_update.unwrap().nodes;
                for name in [
                    "Чёрный",
                    "Красный",
                    "Оранжевый",
                    "Зелёный",
                    "Синий",
                    "Фиолетовый",
                ] {
                    let bounds = nodes
                        .iter()
                        .find_map(|(_, node)| {
                            (node.label() == Some(name))
                                .then(|| node.bounds())
                                .flatten()
                        })
                        .unwrap_or_else(|| panic!("missing {name}"));
                    assert!(
                        bounds.x0 >= 0.0
                            && bounds.x1 <= width as f64
                            && bounds.y0 >= 0.0
                            && bounds.y1 < height as f64 * 0.6,
                        "{name} at {bounds:?} in {theme:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn editor_thickness_control_can_be_reached_by_scrolling() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let id = app.project.assets[0].id;
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ui_style::install(&ctx);
        let mut open = true;
        let input = |events| egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(507.0, 373.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
        let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(Pos2::new(100.0, 70.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, -80.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| app.editor_ui(ctx, id, &mut open),
        );
        let output = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
        let bounds = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .find_map(|(_, node)| {
                (node.label() == Some("Толщина"))
                    .then(|| node.bounds())
                    .flatten()
            })
            .expect("named thickness slider");
        assert!(
            bounds.x0 >= 0.0 && bounds.x1 <= 507.0 && bounds.y0 >= 20.0 && bounds.y1 <= 150.0,
            "thickness slider not reachable: {bounds:?}"
        );
        let position = Pos2::new(
            bounds.x0 as f32 + 2.0,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        );
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(position),
                pointer(position, true),
            ]),
            |ctx| app.editor_ui(ctx, id, &mut open),
        );
        let _ = ctx.run(input(vec![pointer(position, false)]), |ctx| {
            app.editor_ui(ctx, id, &mut open)
        });
        assert!(
            app.editor_width < 5.0,
            "thickness slider did not respond after scrolling"
        );
        assert!(open);
    }
    #[test]
    fn duration_slider_rejects_cutting_off_existing_poses() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        app.add_object(app.project.assets[0].id);
        let id = app.selected.unwrap();
        app.time = 8.0;
        app.upsert_pose(
            id,
            Transform {
                x: 50.0,
                ..Transform::default()
            },
        );
        let expected = app.project.clone();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ui_style::install(&ctx);
        let _ = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        let output = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        let slider = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .find_map(|(_, node)| {
                (node.role() == egui::accesskit::Role::Slider
                    && node.label() == Some("Длительность фильма"))
                .then(|| node.bounds())
                .flatten()
            })
            .expect("duration slider");
        let pos = Pos2::new(slider.x0 as f32 + 1.0, (slider.y0 + slider.y1) as f32 / 2.0);
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(pos), pointer(pos, true)]),
            |ctx| app.update_ui(ctx),
        );
        let _ = ctx.run(input(vec![pointer(pos, false)]), |ctx| app.update_ui(ctx));
        assert!(app.project == expected);
        assert!(app.message.contains("Нельзя сократить"));
        assert!(app.transaction.is_none());
        let output = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        let slider = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .find_map(|(_, node)| {
                (node.role() == egui::accesskit::Role::Slider
                    && node.label() == Some("Длительность фильма"))
                .then(|| node.bounds())
                .flatten()
            })
            .expect("duration slider after rejection");
        let allowed = Pos2::new(
            slider.x0 as f32 + 17.0,
            (slider.y0 + slider.y1) as f32 / 2.0,
        );
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(allowed),
                pointer(allowed, true),
            ]),
            |ctx| app.update_ui(ctx),
        );
        let _ = ctx.run(input(vec![pointer(allowed, false)]), |ctx| {
            app.update_ui(ctx)
        });
        assert!(
            (8.0..10.0).contains(&app.project.duration),
            "duration {} message {} slider {:?}",
            app.project.duration,
            app.message,
            slider
        );
        assert_eq!(app.project.objects[0].poses.last().unwrap().time, 8.0);
        assert_eq!(app.message, "Длительность изменена");
        app.undo();
        assert!(app.project == expected);
    }
    #[test]
    fn clicking_both_end_markers_selects_original_pose_times() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        app.add_object(app.project.assets[0].id);
        let id = app.selected.unwrap();
        app.project.objects[0].poses = (0..=1000)
            .map(|index| Pose {
                time: index as f32 / 100.0,
                transform: Transform::default(),
                linear: true,
            })
            .collect();
        let ctx = egui::Context::default();
        ui_style::install(&ctx);
        let _ = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        let output = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        let mut markers: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Circle(circle)
                    if circle.fill == ui_style::MUTED && circle.radius == 6.0 =>
                {
                    Some(circle.center)
                }
                _ => None,
            })
            .collect();
        markers.sort_by(|a, b| a.x.total_cmp(&b.x));
        assert!(markers.len() > 2);
        for (position, expected) in [(*markers.last().unwrap(), 10.0), (markers[0], 0.0)] {
            let _ = ctx.run(
                input(vec![
                    egui::Event::PointerMoved(position),
                    pointer(position, true),
                ]),
                |ctx| app.update_ui(ctx),
            );
            let _ = ctx.run(input(vec![pointer(position, false)]), |ctx| {
                app.update_ui(ctx)
            });
            assert_eq!(app.selected_pose, Some((id, expected)));
        }
    }
    #[test]
    fn editor_gesture_and_undo_refresh_scene_and_thumbnail_textures() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let id = app.project.assets[0].id;
        app.add_object(id);
        let root = egui::Context::default();
        let root_input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
            ..Default::default()
        };
        let initial = root.run(root_input(), |ctx| app.update_ui(ctx));
        let scene_texture = app.scene_texture.as_ref().unwrap().id();
        let thumbnail = app.thumbnails[&id].texture.id();
        let pixels = |output: &egui::FullOutput, texture| {
            let delta = output
                .textures_delta
                .set
                .iter()
                .find(|(id, _)| *id == texture)
                .expect("texture update");
            let egui::ImageData::Color(image) = &delta.1.image;
            image.pixels.clone()
        };
        let original_scene = pixels(&initial, scene_texture);
        let original_thumbnail = pixels(&initial, thumbnail);
        let editor = egui::Context::default();
        let mut open = true;
        let _ = editor.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
        let start = Pos2::new(250.0, 350.0);
        let end = Pos2::new(310.0, 390.0);
        let _ = editor.run(
            input(vec![egui::Event::PointerMoved(start), pointer(start, true)]),
            |ctx| app.editor_ui(ctx, id, &mut open),
        );
        let _ = editor.run(
            input(vec![egui::Event::PointerMoved(Pos2::new(280.0, 365.0))]),
            |ctx| app.editor_ui(ctx, id, &mut open),
        );
        let _ = editor.run(
            input(vec![egui::Event::PointerMoved(end), pointer(end, false)]),
            |ctx| app.editor_ui(ctx, id, &mut open),
        );
        let changed = root.run(root_input(), |ctx| app.update_ui(ctx));
        assert_ne!(pixels(&changed, scene_texture), original_scene);
        assert_ne!(pixels(&changed, thumbnail), original_thumbnail);
        app.undo();
        let undone = root.run(root_input(), |ctx| app.update_ui(ctx));
        assert_eq!(pixels(&undone, scene_texture), original_scene);
        assert_eq!(pixels(&undone, thumbnail), original_thumbnail);
    }
    #[test]
    fn editor_copy_is_independent_while_original_instances_remain_shared() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let original = app.project.assets[0].id;
        app.project.assets[0].content = AssetContent::Drawing;
        app.project.assets[0].elements = vec![DrawingElement::Rect {
            from: [-10.0, -10.0],
            to: [10.0, 10.0],
            angle: 0.0,
            color: [240, 20, 20, 255],
            fill: true,
            width: 1.0,
        }];
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut open = true;
        let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, original, &mut open));
        let output = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, original, &mut open));
        let bounds = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .find_map(|(_, node)| {
                (node.label() == Some("Создать копию"))
                    .then(|| node.bounds())
                    .flatten()
            })
            .expect("copy action");
        let pos = Pos2::new(
            ((bounds.x0 + bounds.x1) / 2.0) as f32,
            ((bounds.y0 + bounds.y1) / 2.0) as f32,
        );
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(pos), pointer(pos, true)]),
            |ctx| app.editor_ui(ctx, original, &mut open),
        );
        let _ = ctx.run(input(vec![pointer(pos, false)]), |ctx| {
            app.editor_ui(ctx, original, &mut open)
        });
        assert_eq!(app.project.assets.len(), 2);
        let copy = app.project.assets[1].id;
        assert_ne!(copy, original);
        for (asset, x) in [(original, -120.0), (original, 0.0), (copy, 120.0)] {
            app.add_object(asset);
            app.upsert_pose(
                app.selected.unwrap(),
                Transform {
                    x,
                    ..Transform::default()
                },
            );
        }
        assert!(app.try_edit(|project| {
            let DrawingElement::Rect { color, .. } = &mut project.assets[0].elements[0] else {
                unreachable!()
            };
            *color = [20, 40, 240, 255];
        }));
        let frame = scene::render(&app.project, 0.0, 1);
        let pixel = |x: usize| -> &[u8] {
            let index = (180 * 640 + x) * 4;
            &frame[index..index + 4]
        };
        assert_eq!(pixel(200), [20, 40, 240, 255]);
        assert_eq!(pixel(320), [20, 40, 240, 255]);
        assert_eq!(pixel(440), [240, 20, 20, 255]);
    }
    #[test]
    fn shared_drawing_and_multiple_pose_tracks_survive_save_and_reopen() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let asset_id = app.project.assets[0].id;
        let ctx = egui::Context::default();
        let mut open = true;
        let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, asset_id, &mut open));
        let start = Pos2::new(250.0, 350.0);
        let end = Pos2::new(310.0, 390.0);
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(start), pointer(start, true)]),
            |ctx| app.editor_ui(ctx, asset_id, &mut open),
        );
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(Pos2::new(280.0, 365.0))]),
            |ctx| app.editor_ui(ctx, asset_id, &mut open),
        );
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(end), pointer(end, false)]),
            |ctx| app.editor_ui(ctx, asset_id, &mut open),
        );
        assert!(app.project.assets[0].elements.len() >= 2);
        app.add_object(asset_id);
        let first = app.selected.unwrap();
        app.time = 2.0;
        app.upsert_pose(
            first,
            Transform {
                x: -80.0,
                ..Transform::default()
            },
        );
        app.add_object(asset_id);
        let second = app.selected.unwrap();
        app.time = 3.0;
        app.upsert_pose(
            second,
            Transform {
                x: 90.0,
                y: 25.0,
                ..Transform::default()
            },
        );
        assert_ne!(first, second);
        assert_eq!(
            app.project
                .objects
                .iter()
                .map(|o| o.poses.len())
                .collect::<Vec<_>>(),
            [2, 2]
        );
        let before = app.project.clone();
        let frame = scene::render(&before, 2.5, 2);
        app.undo();
        app.redo();
        assert!(app.project == before);
        let path = std::env::temp_dir().join(format!(
            "animapro-shared-scenario-{}.animapro",
            Uuid::new_v4()
        ));
        storage::safely_write_project(&path, &app.project).unwrap();
        let reopened = storage::read_project(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(reopened == before);
        assert_eq!(scene::render(&reopened, 2.5, 2), frame);
        assert!(
            reopened
                .objects
                .iter()
                .all(|object| object.asset_id == asset_id)
        );
    }
    #[test]
    fn scene_fast_drag_uses_press_position_and_undoes_in_one_step() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        app.add_object(app.project.assets[0].id);
        app.time = 2.0;
        let before = app.project.clone();
        let ctx = egui::Context::default();
        let output = ctx.run(input(vec![]), |ctx| app.update_ui(ctx));
        let texture = app.scene_texture.as_ref().unwrap().id();
        let scene_rect = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some(mesh.calc_bounds()),
                _ => None,
            })
            .expect("scene image");
        let start = scene_rect.center() + Vec2::new(3.0, 0.0);
        let end = start + Vec2::new(100.0, 40.0);
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(start), pointer(start, true)]),
            |ctx| app.update_ui(ctx),
        );
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(end), pointer(end, false)]),
            |ctx| app.update_ui(ctx),
        );
        let moved = state_at(&app.project.objects[0], 2.0);
        assert!(
            moved.x > 0.0 && moved.y > 0.0,
            "object should follow a fast drag"
        );
        app.undo();
        assert!(
            app.project == before,
            "one undo should restore the entire drag"
        );
    }
    #[test]
    fn fast_editor_gestures_create_each_shape_and_undo() {
        for tool in [
            EditorTool::Pencil,
            EditorTool::Line,
            EditorTool::Rectangle,
            EditorTool::Oval,
        ] {
            let mut app = AnimaApp::with_recovery(Err("test".into()));
            let id = app.project.assets[0].id;
            app.project.assets[0].content = AssetContent::Drawing;
            app.project.assets[0].elements.clear();
            app.editor_tool = tool;
            let before = app.project.clone();
            let ctx = egui::Context::default();
            let mut open = true;
            let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
            let start = Pos2::new(300.0, 350.0);
            let end = Pos2::new(400.0, 390.0);
            let _ = ctx.run(
                input(vec![egui::Event::PointerMoved(start), pointer(start, true)]),
                |ctx| app.editor_ui(ctx, id, &mut open),
            );
            let _ = ctx.run(
                input(vec![egui::Event::PointerMoved(end), pointer(end, false)]),
                |ctx| app.editor_ui(ctx, id, &mut open),
            );
            assert_eq!(
                app.project.assets[0].elements.len(),
                1,
                "a fast editor gesture must create its element"
            );
            app.undo();
            assert!(app.project == before);
        }
    }
    #[test]
    fn fast_editor_selection_drag_moves_element_and_undoes() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let id = app.project.assets[0].id;
        app.project.assets[0].content = AssetContent::Drawing;
        app.project.assets[0].elements = vec![rect(true, [0, 0, 255, 255])];
        app.editor_tool = EditorTool::Select;
        let before = app.project.clone();
        let ctx = egui::Context::default();
        let mut open = true;
        let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
        let start = Pos2::new(300.0, 350.0);
        let end = Pos2::new(400.0, 390.0);
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(start), pointer(start, true)]),
            |ctx| app.editor_ui(ctx, id, &mut open),
        );
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(end), pointer(end, false)]),
            |ctx| app.editor_ui(ctx, id, &mut open),
        );
        let DrawingElement::Rect { from, .. } = app.project.assets[0].elements[0] else {
            panic!("expected rectangle")
        };
        assert_eq!(from, [-900.0, -960.0]);
        app.undo();
        assert!(app.project == before);
    }
    #[test]
    fn editor_transform_rotates_and_resizes_selected_rectangle() {
        let mut element = DrawingElement::Rect {
            from: [-20.0, -10.0],
            to: [20.0, 10.0],
            angle: 0.0,
            color: [0, 0, 0, 255],
            fill: true,
            width: 1.0,
        };
        transform_element(&mut element, 1.5, 90.0);
        let DrawingElement::Rect {
            from, to, angle, ..
        } = element
        else {
            unreachable!()
        };
        assert_eq!(from, [-30.0, -15.0]);
        assert_eq!(to, [30.0, 15.0]);
        assert_eq!(angle, 90.0);
    }
    #[test]
    fn transform_handles_identify_resize_and_rotation() {
        let element = rect(true, [0, 0, 0, 255]);
        let origin = Pos2::ZERO;
        let bounds = transform_box(&element, origin);
        assert!(matches!(
            transform_hit(&element, origin, bounds.left_top()),
            Some(EditorTransformMode::Resize)
        ));
        assert!(matches!(
            transform_hit(
                &element,
                origin,
                Pos2::new(bounds.center().x, bounds.top() - 28.0)
            ),
            Some(EditorTransformMode::Rotate)
        ));
    }
    #[test]
    fn editor_save_shortcut_works_with_focused_control() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let asset = app.project.assets[0].id;
        app.add_object(asset);
        let expected = app.project.clone();
        let path = std::env::temp_dir().join(format!(
            "animapro-editor-focused-save-{}.animapro",
            Uuid::new_v4()
        ));
        app.file = Some(path.clone());
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut open = true;
        let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, asset, &mut open));
        let output = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, asset, &mut open));
        let (node_id, _) = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .into_iter()
            .find(|(_, node)| node.role() == egui::accesskit::Role::SpinButton)
            .expect("thickness value field");
        // This identifier originates from the editor's egui widget tree.
        let field_id = unsafe { egui::Id::from_high_entropy_bits(node_id.0) };
        ctx.memory_mut(|memory| memory.request_focus(field_id));
        assert!(ctx.wants_keyboard_input());
        let event = egui::Event::Key {
            key: egui::Key::S,
            physical_key: Some(egui::Key::S),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                command: true,
                mac_cmd: true,
                ..Default::default()
            },
        };
        let _ = ctx.run(input(vec![event]), |ctx| {
            app.editor_ui(ctx, asset, &mut open)
        });
        assert!(!app.dirty);
        assert!(read_project(&path).unwrap() == expected);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn editor_save_shortcut_does_not_bypass_project_confirmation() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let asset = app.project.assets[0].id;
        app.add_object(asset);
        let path =
            std::env::temp_dir().join(format!("animapro-modal-save-{}.animapro", Uuid::new_v4()));
        app.file = Some(path.clone());
        app.request_action(PendingAction::New);
        assert!(app.pending_action.is_some());
        let ctx = egui::Context::default();
        let mut open = true;
        let event = egui::Event::Key {
            key: egui::Key::S,
            physical_key: Some(egui::Key::S),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                command: true,
                mac_cmd: true,
                ..Default::default()
            },
        };
        let _ = ctx.run(input(vec![event]), |ctx| {
            app.editor_ui(ctx, asset, &mut open)
        });
        assert!(!path.exists());
        assert!(app.dirty && app.pending_action.is_some());
    }
    #[test]
    fn editor_shortcut_uses_modifiers_from_key_event() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let id = app.project.assets[0].id;
        let before = app.project.clone();
        assert!(app.try_edit(|p| p.name = "изменённый".into()));
        let ctx = egui::Context::default();
        let mut open = true;
        let key = egui::Event::Key {
            key: egui::Key::Z,
            physical_key: Some(egui::Key::Z),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                command: true,
                mac_cmd: true,
                ..Default::default()
            },
        };
        let _ = ctx.run(input(vec![key]), |ctx| app.editor_ui(ctx, id, &mut open));
        assert!(app.project == before);
    }
    #[test]
    fn single_click_selects_element_and_delete_can_be_undone() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let id = app.project.assets[0].id;
        app.project.assets[0].content = AssetContent::Drawing;
        app.project.assets[0].elements = vec![rect(true, [0, 0, 255, 255])];
        app.saved = app.project.clone();
        app.edit_asset = Some(id);
        app.editor_tool = EditorTool::Select;
        let ctx = egui::Context::default();
        let mut open = true;
        let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
        let pos = Pos2::new(400.0, 400.0);
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(pos), pointer(pos, true)]),
            |ctx| app.editor_ui(ctx, id, &mut open),
        );
        let _ = ctx.run(input(vec![pointer(pos, false)]), |ctx| {
            app.editor_ui(ctx, id, &mut open)
        });
        assert_eq!(app.editor_selected, Some(0));
        let key = egui::Event::Key {
            key: egui::Key::Backspace,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = ctx.run(input(vec![key]), |ctx| app.editor_ui(ctx, id, &mut open));
        assert!(app.project.assets[0].elements.is_empty());
        let modifiers = egui::Modifiers {
            command: true,
            ..Default::default()
        };
        let key = egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let mut undo_input = input(vec![key]);
        undo_input.modifiers = modifiers;
        let _ = ctx.run(undo_input, |ctx| app.editor_ui(ctx, id, &mut open));
        assert_eq!(app.project.assets[0].elements.len(), 1);
    }
    #[test]
    fn picking_uses_topmost_geometry_and_stroke_bounds() {
        let elements = vec![rect(true, [0, 0, 255, 255]), rect(false, [255, 0, 0, 255])];
        assert_eq!(pick_element(&elements, Pos2::ZERO, Pos2::ZERO), Some(0));
        assert_eq!(
            pick_element(&elements, Pos2::new(1000.0, 0.0), Pos2::ZERO),
            Some(1)
        );
        let b = element_bounds(&elements[1], Pos2::ZERO);
        assert_eq!(b.min.x, -1002.0);
        assert_eq!(b.max.x, 1002.0);
    }
    #[test]
    fn pencil_stroke_includes_press_and_release_positions() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let id = app.project.assets[0].id;
        app.add_object(id);
        let original_frame = scene::render(&app.project, 0.0, 1);
        app.editor_tool = EditorTool::Pencil;
        let ctx = egui::Context::default();
        let mut open = true;
        let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
        let start = Pos2::new(250.0, 350.0);
        let middle = Pos2::new(280.0, 365.0);
        let end = Pos2::new(310.0, 390.0);
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(start), pointer(start, true)]),
            |ctx| app.editor_ui(ctx, id, &mut open),
        );
        let _ = ctx.run(input(vec![egui::Event::PointerMoved(middle)]), |ctx| {
            app.editor_ui(ctx, id, &mut open)
        });
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(end), pointer(end, false)]),
            |ctx| app.editor_ui(ctx, id, &mut open),
        );
        assert!(matches!(
            app.project.assets[0].elements[0],
            DrawingElement::Oval { fill: true, .. }
        ));
        let DrawingElement::Stroke { points, .. } = &app.project.assets[0].elements[1] else {
            panic!("expected a pencil stroke")
        };
        assert_eq!(points.last().unwrap()[0] - points[0][0], end.x - start.x);
        assert_eq!(points.last().unwrap()[1] - points[0][1], end.y - start.y);
        let mut base_only = app.project.clone();
        base_only.assets[0].elements.pop();
        assert_eq!(scene::render(&base_only, 0.0, 1), original_frame);
        app.undo();
        assert!(app.project.assets[0].elements.is_empty());
        assert!(app.project.assets[0].content == AssetContent::Builtin);
    }
    #[test]
    fn deleting_builtin_leaves_a_persistently_empty_drawing_and_is_undoable() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let id = app.project.assets[0].id;
        app.add_object(id);
        let before = scene::render(&app.project, 0.0, 1);
        app.editor_tool = EditorTool::Select;
        app.editor_selected = Some(0);
        let ctx = egui::Context::default();
        let mut open = true;
        let key = egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = ctx.run(input(vec![key]), |ctx| app.editor_ui(ctx, id, &mut open));
        assert!(app.project.assets[0].content == AssetContent::Drawing);
        assert!(app.project.assets[0].elements.is_empty());
        let empty_frame = scene::render(&app.project, 0.0, 1);
        assert_ne!(empty_frame, before);
        let path = std::env::temp_dir().join(format!("animapro-empty-{}.animapro", Uuid::new_v4()));
        storage::safely_write_project(&path, &app.project).unwrap();
        let reopened = storage::read_project(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(reopened.assets[0].content == AssetContent::Drawing);
        assert!(reopened.assets[0].elements.is_empty());
        assert_eq!(scene::render(&reopened, 0.0, 1), empty_frame);
        app.undo();
        assert!(app.project.assets[0].content == AssetContent::Builtin);
        assert_eq!(scene::render(&app.project, 0.0, 1), before);
    }
    #[test]
    fn selected_transparent_element_moves_with_keyboard_and_undoes() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let id = app.project.assets[0].id;
        app.project.assets[0].content = AssetContent::Drawing;
        app.project.assets[0].elements = vec![rect(true, [0, 0, 0, 0])];
        app.editor_tool = EditorTool::Select;
        app.editor_selected = Some(0);
        let before = app.project.assets[0].elements[0].clone();
        let ctx = egui::Context::default();
        let mut open = true;
        let modifiers = egui::Modifiers {
            shift: true,
            ..Default::default()
        };
        let key = egui::Event::Key {
            key: egui::Key::ArrowRight,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let _ = ctx.run(input(vec![key]), |ctx| app.editor_ui(ctx, id, &mut open));
        let DrawingElement::Rect { from, .. } = &app.project.assets[0].elements[0] else {
            panic!("expected rectangle")
        };
        assert_eq!(from[0], -990.0);
        app.undo();
        assert!(app.project.assets[0].elements[0] == before);
    }
    #[test]
    fn escape_during_element_drag_restores_position_until_release() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let id = app.project.assets[0].id;
        app.project.assets[0].content = AssetContent::Drawing;
        app.project.assets[0].elements = vec![rect(true, [0, 0, 255, 255])];
        app.saved = app.project.clone();
        app.edit_asset = Some(id);
        app.editor_tool = EditorTool::Select;
        let before = app.project.clone();
        let ctx = egui::Context::default();
        let mut open = true;
        let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
        let start = Pos2::new(300.0, 350.0);
        let moved = Pos2::new(330.0, 370.0);
        let _ = ctx.run(
            input(vec![egui::Event::PointerMoved(start), pointer(start, true)]),
            |ctx| app.editor_ui(ctx, id, &mut open),
        );
        let _ = ctx.run(input(vec![egui::Event::PointerMoved(moved)]), |ctx| {
            app.editor_ui(ctx, id, &mut open)
        });
        assert!(app.project != before);
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = ctx.run(input(vec![escape]), |ctx| app.editor_ui(ctx, id, &mut open));
        assert!(app.project == before);
        let end = Pos2::new(360.0, 390.0);
        let _ = ctx.run(input(vec![egui::Event::PointerMoved(end)]), |ctx| {
            app.editor_ui(ctx, id, &mut open)
        });
        let _ = ctx.run(input(vec![pointer(end, false)]), |ctx| {
            app.editor_ui(ctx, id, &mut open)
        });
        assert!(app.project == before);
        assert!(app.undo.is_empty());
    }
    #[test]
    fn zero_area_shapes_are_rejected_without_changing_project() {
        for tool in [EditorTool::Rectangle, EditorTool::Oval] {
            for end in [Pos2::new(340.0, 350.0), Pos2::new(300.0, 390.0)] {
                let mut app = AnimaApp::with_recovery(Err("test".into()));
                let id = app.project.assets[0].id;
                app.editor_tool = tool;
                let before = app.project.clone();
                let ctx = egui::Context::default();
                let mut open = true;
                let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
                let start = Pos2::new(300.0, 350.0);
                let _ = ctx.run(
                    input(vec![egui::Event::PointerMoved(start), pointer(start, true)]),
                    |ctx| app.editor_ui(ctx, id, &mut open),
                );
                let _ = ctx.run(input(vec![egui::Event::PointerMoved(end)]), |ctx| {
                    app.editor_ui(ctx, id, &mut open)
                });
                let _ = ctx.run(input(vec![pointer(end, false)]), |ctx| {
                    app.editor_ui(ctx, id, &mut open)
                });
                assert!(app.project == before);
                assert!(app.message.contains("ширина и высота"));
                assert!(app.undo.is_empty());
            }
        }
    }
    #[test]
    fn editor_clears_selection_when_undo_removes_selected_element() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let id = app.project.assets[0].id;
        assert!(app.try_edit(|p| {
            p.assets[0].content = AssetContent::Drawing;
            p.assets[0].elements.push(rect(true, [0, 0, 0, 0]));
        }));
        app.editor_selected = Some(0);
        app.undo();
        let ctx = egui::Context::default();
        let mut open = true;
        let _ = ctx.run(input(vec![]), |ctx| app.editor_ui(ctx, id, &mut open));
        assert_eq!(app.editor_selected, None);
    }
    #[test]
    fn switching_to_an_asset_removed_by_cancel_does_not_leave_a_stale_editor() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let first = app.project.assets[0].id;
        app.edit_asset = Some(first);
        app.begin_transaction();
        let mut temporary = app.project.assets[0].clone();
        temporary.id = Uuid::new_v4();
        let temporary_id = temporary.id;
        assert!(app.try_edit(|project| project.assets.push(temporary)));
        app.open_editor(temporary_id);
        assert!(app.edit_asset.is_none());
        assert!(app.project.asset(temporary_id).is_none());
        assert!(app.message.contains("недоступен"));
    }
    #[test]
    fn changing_drawing_cancels_gesture_and_clears_element_selection() {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let first = app.project.assets[0].id;
        let mut second = app.project.assets[0].clone();
        second.id = Uuid::new_v4();
        let second_id = second.id;
        app.project.assets.push(second);
        app.saved = app.project.clone();
        app.edit_asset = Some(first);
        app.begin_transaction();
        assert!(app.try_edit(|p| p.assets[0].name = "temporary".into()));
        app.editor_selected = Some(4);
        app.editor_start = Some(Pos2::new(1.0, 2.0));
        app.editor_stroke.push(Pos2::ZERO);
        app.editor_stroke_overflow = true;
        app.open_editor(second_id);
        assert_eq!(app.edit_asset, Some(second_id));
        assert_eq!(app.project.assets[0].name, "Мяч");
        assert!(
            app.transaction.is_none()
                && app.editor_selected.is_none()
                && app.editor_start.is_none()
                && app.editor_stroke.is_empty()
                && !app.editor_stroke_overflow
        );
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    fn app() -> AnimaApp {
        AnimaApp::with_recovery(Err("disabled for test".into()))
    }
    #[test]
    fn one_drag_is_one_undo_and_redo_restores_saved_state() {
        let mut app = app();
        let asset = app.project.assets[0].id;
        app.add_object(asset);
        let id = app.selected.unwrap();
        app.saved = app.project.clone();
        app.dirty = false;
        app.undo.clear();
        app.begin_transaction();
        for x in [1.0, 5.0, 20.0] {
            app.upsert_pose(
                id,
                Transform {
                    x,
                    ..Transform::default()
                },
            );
        }
        app.finish_transaction();
        assert_eq!(app.undo.len(), 1);
        assert!(app.dirty);
        app.undo();
        assert_eq!(state_at(app.selected_object().unwrap(), 0.0).x, 0.0);
        assert!(!app.dirty);
        app.redo();
        assert_eq!(state_at(app.selected_object().unwrap(), 0.0).x, 20.0);
        assert!(app.dirty);
    }
    #[test]
    fn rejected_over_limit_edit_preserves_project_and_redo_history() {
        let mut app = app();
        let asset = app.project.assets[0].id;
        app.add_object(asset);
        app.undo();
        assert_eq!(app.redo.len(), 1);
        let before = app.project.clone();
        let accepted = app.try_edit(|project| {
            project.assets[0].content = AssetContent::Drawing;
            project.assets[0].elements.push(DrawingElement::Stroke {
                points: vec![[0.0, 0.0]; storage::MAX_POINTS + 1],
                color: [0, 0, 0, 255],
                width: 1.0,
            });
        });
        assert!(!accepted && app.project == before);
        assert_eq!(app.redo.len(), 1);
        assert!(app.message.contains("предел"));
        app.redo();
        assert_eq!(app.project.objects.len(), 1);
    }
    #[test]
    fn object_limit_is_checked_before_adding_or_changing_selection() {
        let mut app = app();
        let asset = app.project.assets[0].id;
        app.project.objects = (0..storage::MAX_OBJECTS)
            .map(|_| SceneObject {
                id: Uuid::new_v4(),
                asset_id: asset,
                name: "Объект".into(),
                poses: vec![],
            })
            .collect();
        app.saved = app.project.clone();
        app.selected = Some(app.project.objects[0].id);
        let selected = app.selected;
        app.add_object(asset);
        assert_eq!(app.project.objects.len(), storage::MAX_OBJECTS);
        assert_eq!(app.selected, selected);
        assert!(app.undo.is_empty() && !app.dirty);
        assert!(app.message.contains("Слишком много"));
    }
    #[test]
    fn asset_and_element_limits_are_checked_before_apply() {
        let mut app = app();
        let template = app.project.assets[0].clone();
        app.project.assets = (0..storage::MAX_ASSETS)
            .map(|_| {
                let mut a = template.clone();
                a.id = Uuid::new_v4();
                a
            })
            .collect();
        app.saved = app.project.clone();
        let accepted = app.try_edit(|project| {
            let mut a = template.clone();
            a.id = Uuid::new_v4();
            project.assets.push(a);
        });
        assert!(
            !accepted && app.project.assets.len() == storage::MAX_ASSETS && app.undo.is_empty()
        );
        app.project.assets.truncate(1);
        app.saved = app.project.clone();
        let accepted = app.try_edit(|project| {
            project.assets[0].content = AssetContent::Drawing;
            project.assets[0].elements = vec![
                DrawingElement::Line {
                    from: [0.0, 0.0],
                    to: [1.0, 1.0],
                    color: [0, 0, 0, 255],
                    width: 1.0
                };
                storage::MAX_ELEMENTS + 1
            ];
        });
        assert!(!accepted && app.project.assets[0].elements.is_empty() && app.undo.is_empty());
    }
    #[test]
    fn invalid_transform_is_not_applied_or_added_to_history() {
        let mut app = app();
        let asset = app.project.assets[0].id;
        app.add_object(asset);
        let id = app.selected.unwrap();
        let before = app.project.clone();
        let count = app.undo.len();
        app.upsert_pose(
            id,
            Transform {
                x: f32::INFINITY,
                ..Transform::default()
            },
        );
        assert!(app.project == before);
        assert_eq!(app.undo.len(), count);
        assert!(app.message.contains("не применено"));
    }
    #[test]
    fn json_limit_is_checked_before_applying_oversized_name() {
        let mut app = app();
        let before = app.project.clone();
        let accepted =
            app.try_edit(|project| project.name = "x".repeat(storage::MAX_JSON as usize + 1));
        assert!(!accepted && app.project == before && app.undo.is_empty() && !app.dirty);
        assert!(app.message.contains("32 МиБ"));
    }
    #[test]
    fn scene_revision_changes_on_edits_undo_redo_and_project_switch() {
        let mut app = app();
        let start = app.scene_revision;
        let asset = app.project.assets[0].id;
        app.add_object(asset);
        assert_ne!(app.scene_revision, start);
        let edited = app.scene_revision;
        app.undo();
        assert_ne!(app.scene_revision, edited);
        let undone = app.scene_revision;
        app.redo();
        assert_ne!(app.scene_revision, undone);
        let redone = app.scene_revision;
        app.replace_project(Project::new(), None);
        assert_ne!(app.scene_revision, redone);
    }
    #[test]
    fn empty_or_cancelled_gesture_does_not_enter_history() {
        let mut app = app();
        let original = app.project.clone();
        app.begin_transaction();
        app.finish_transaction();
        assert!(app.undo.is_empty());
        assert!(!app.dirty);
        app.begin_transaction();
        app.change();
        app.project.name = "changed".into();
        app.cancel_transaction();
        assert!(app.project == original);
        assert!(app.undo.is_empty());
        assert!(!app.dirty);
    }
    #[test]
    fn changing_project_clears_history_and_selections() {
        let mut app = app();
        let asset = app.project.assets[0].id;
        app.add_object(asset);
        app.edit_asset = Some(asset);
        app.begin_transaction();
        let next = Project::new();
        app.replace_project(next.clone(), None);
        app.undo();
        assert!(app.project == next);
        assert!(app.selected.is_none());
        assert!(app.edit_asset.is_none());
        assert!(app.transaction.is_none());
    }
}

#[cfg(test)]
mod pose_tests {
    use super::*;
    fn app() -> (AnimaApp, Uuid) {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let asset = app.project.assets[0].id;
        app.add_object(asset);
        let id = app.selected.unwrap();
        app.undo.clear();
        app.saved = app.project.clone();
        app.dirty = false;
        (app, id)
    }
    #[test]
    fn poses_create_select_move_reject_collision_delete_and_undo() {
        let (mut app, id) = app();
        app.time = 0.2;
        app.create_pose();
        assert!(
            app.selected_pose
                .is_some_and(|(object, t)| object == id && same_time(t, 0.2))
        );
        app.move_pose(-STEP);
        assert!(same_time(app.selected_pose.unwrap().1, 0.1));
        let before = app.project.clone();
        let history = app.undo.len();
        app.move_pose(-STEP);
        assert!(app.project == before);
        assert_eq!(app.undo.len(), history);
        assert!(app.message.contains("уже есть"));
        app.delete_pose();
        assert_eq!(app.selected_object().unwrap().poses.len(), 1);
        app.undo();
        assert_eq!(app.selected_object().unwrap().poses.len(), 2);
        app.select_pose(id, 0.1);
        app.move_pose(-STEP);
        assert_eq!(app.selected_object().unwrap().poses.len(), 2);
    }
    #[test]
    fn closely_spaced_legacy_poses_are_edited_and_deleted_independently() {
        let (mut app, id) = app();
        let t1 = 0.5_f32;
        let t2 = 0.500005_f32;
        assert!(t1 != t2 && same_time(t1, t2));
        app.project.objects[0].poses.extend([
            Pose {
                time: t1,
                transform: Transform {
                    x: 10.0,
                    ..Transform::default()
                },
                linear: false,
            },
            Pose {
                time: t2,
                transform: Transform {
                    x: 20.0,
                    ..Transform::default()
                },
                linear: false,
            },
        ]);
        storage::ensure_saveable(&app.project).unwrap();
        app.select_pose(id, t2);
        app.upsert_pose(
            id,
            Transform {
                x: 40.0,
                ..Transform::default()
            },
        );
        let poses = &app.selected_object().unwrap().poses;
        assert_eq!(poses.len(), 3);
        assert_eq!(poses[1].transform.x, 10.0);
        assert_eq!(poses[2].transform.x, 40.0);
        app.set_pose_linear(true);
        assert!(
            !app.selected_object().unwrap().poses[1].linear
                && app.selected_object().unwrap().poses[2].linear
        );
        app.delete_pose();
        assert_eq!(app.selected_object().unwrap().poses.len(), 2);
        assert_eq!(app.selected_object().unwrap().poses[1].time, t1);
        app.undo();
        assert_eq!(app.selected_object().unwrap().poses.len(), 3);
        app.select_pose(id, t2);
        app.move_pose(STEP);
        assert_eq!(app.selected_object().unwrap().poses[1].time, t1);
        assert!(
            app.selected_object()
                .unwrap()
                .poses
                .iter()
                .any(|p| same_time(p.time, 0.6))
        );
    }
    #[test]
    fn a_single_nearby_legacy_pose_is_not_modified_implicitly() {
        let (mut app, id) = app();
        let legacy = 0.500005_f32;
        app.project.objects[0].poses.push(Pose {
            time: legacy,
            transform: Transform::default(),
            linear: false,
        });
        app.time = 0.5;
        app.selected_pose = None;
        let before = app.project.clone();
        app.upsert_pose(
            id,
            Transform {
                x: 50.0,
                ..Transform::default()
            },
        );
        assert!(app.project == before);
        assert!(app.message.contains("другим временем"));
        app.select_pose(id, legacy);
        app.upsert_pose(
            id,
            Transform {
                x: 50.0,
                ..Transform::default()
            },
        );
        let poses = &app.selected_object().unwrap().poses;
        assert_eq!(poses.len(), 2);
        assert_eq!(poses[1].time, legacy);
        assert_eq!(poses[1].transform.x, 50.0);
    }
    #[test]
    fn ambiguous_nearby_poses_are_not_changed_without_explicit_selection() {
        let (mut app, id) = app();
        app.project.objects[0].poses.extend([
            Pose {
                time: 0.5,
                transform: Transform::default(),
                linear: false,
            },
            Pose {
                time: 0.500005,
                transform: Transform::default(),
                linear: false,
            },
        ]);
        app.time = 0.5;
        app.selected_pose = None;
        let before = app.project.clone();
        let history = app.undo.len();
        app.upsert_pose(
            id,
            Transform {
                x: 50.0,
                ..Transform::default()
            },
        );
        assert!(app.project == before);
        assert_eq!(app.undo.len(), history);
        assert!(app.message.contains("несколько поз"));
        assert_ne!(pose_time_label(0.5), pose_time_label(0.500005));
    }
    #[test]
    fn interpolation_choice_changes_motion_and_is_undoable() {
        let (mut app, id) = app();
        app.time = 1.0;
        app.upsert_pose(
            id,
            Transform {
                x: 100.0,
                ..Transform::default()
            },
        );
        app.select_pose(id, 0.0);
        assert!((state_at(app.selected_object().unwrap(), 0.25).x - 15.625).abs() < 0.001);
        app.set_pose_linear(true);
        assert!((state_at(app.selected_object().unwrap(), 0.25).x - 25.0).abs() < 0.001);
        app.undo();
        assert!((state_at(app.selected_object().unwrap(), 0.25).x - 15.625).abs() < 0.001);
    }
    #[test]
    fn duration_boundary_and_non_grid_legacy_poses_are_preserved() {
        let (mut app, id) = app();
        app.project.duration = 1.05;
        app.time = 1.05;
        app.upsert_pose(
            id,
            Transform {
                x: 12.0,
                ..Transform::default()
            },
        );
        assert!(
            app.selected_object()
                .unwrap()
                .poses
                .iter()
                .all(|p| p.time <= 1.05)
        );
        storage::validate(&app.project).unwrap();
        app.project.objects[0].poses.push(Pose {
            time: 1.005,
            transform: Transform::default(),
            linear: false,
        });
        app.project.objects[0]
            .poses
            .sort_by(|a, b| a.time.total_cmp(&b.time));
        app.time = 1.0;
        app.upsert_pose(
            id,
            Transform {
                x: 30.0,
                ..Transform::default()
            },
        );
        assert!(
            app.selected_object()
                .unwrap()
                .poses
                .iter()
                .any(|p| same_time(p.time, 1.005))
        );
        storage::validate(&app.project).unwrap();
    }
    #[test]
    fn reordering_changes_layer_order_and_undo_restores_it() {
        let (mut app, first) = app();
        let asset = app.project.assets[0].id;
        app.add_object(asset);
        let second = app.selected.unwrap();
        app.reorder_object(first, true);
        assert!(app.project.objects.iter().map(|o| o.id).eq([second, first]));
        app.undo();
        assert!(app.project.objects.iter().map(|o| o.id).eq([first, second]));
    }
}

#[cfg(test)]
mod transition_tests {
    use super::*;
    fn dirty_app() -> AnimaApp {
        let mut app = AnimaApp::with_recovery(Err("test".into()));
        let id = app.project.assets[0].id;
        app.add_object(id);
        app
    }
    #[test]
    fn cancelled_or_failed_save_never_replaces_dirty_project() {
        for choice in [LeaveChoice::Cancel, LeaveChoice::Save] {
            let mut app = dirty_app();
            let before = app.project.clone();
            let original_path = PathBuf::from("original.animapro");
            app.file = Some(original_path.clone());
            let next_path = PathBuf::from("next.animapro");
            app.request_action(PendingAction::Open(next_path));
            app.resolve_leave(choice, false);
            assert!(app.project == before);
            assert_eq!(app.file, Some(original_path));
            assert!(app.dirty);
            assert!(!app.close_ready);
            assert!(app.pending_action.is_none());
        }
    }
    #[test]
    fn discard_switches_projects_and_clears_history() {
        let mut app = dirty_app();
        let next = Project::new();
        let path = std::env::temp_dir().join(format!("animapro-transition-{}", Uuid::new_v4()));
        safely_write_project(&path, &next).unwrap();
        app.request_action(PendingAction::Open(path.clone()));
        app.resolve_leave(LeaveChoice::Discard, false);
        assert!(app.project == next);
        assert_eq!(app.file, Some(path.clone()));
        assert!(!app.dirty);
        assert!(app.undo.is_empty() && app.redo.is_empty() && app.selected.is_none());
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn reopening_current_file_after_save_reads_the_newly_saved_state() {
        let mut app = dirty_app();
        let path = std::env::temp_dir().join(format!("animapro-self-open-{}", Uuid::new_v4()));
        safely_write_project(&path, &Project::new()).unwrap();
        app.file = Some(path.clone());
        let expected = app.project.clone();
        app.request_action(PendingAction::Open(path.clone()));
        let saved = app.save_to_path(path.clone());
        app.resolve_leave(LeaveChoice::Save, saved);
        assert!(app.project == expected);
        assert!(read_project(&path).unwrap() == expected);
        assert!(!app.dirty);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn invalid_target_keeps_dirty_project_and_history_after_discard_choice() {
        let mut app = dirty_app();
        let expected = app.project.clone();
        let undo_count = app.undo.len();
        let path = std::env::temp_dir().join(format!("animapro-invalid-{}", Uuid::new_v4()));
        fs::write(&path, b"broken").unwrap();
        app.request_action(PendingAction::Open(path.clone()));
        app.resolve_leave(LeaveChoice::Discard, false);
        assert!(app.project == expected);
        assert!(app.dirty);
        assert_eq!(app.undo.len(), undo_count);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn close_requires_a_successful_save_or_explicit_discard() {
        let mut app = dirty_app();
        app.request_action(PendingAction::Close);
        app.resolve_leave(LeaveChoice::Save, false);
        assert!(!app.close_ready);
        app.request_action(PendingAction::Close);
        app.resolve_leave(LeaveChoice::Cancel, false);
        assert!(!app.close_ready);
        app.request_action(PendingAction::Close);
        app.resolve_leave(LeaveChoice::Discard, false);
        assert!(app.close_ready);
    }
}

#[cfg(test)]
mod performance_measurement {
    use super::*;
    use std::hint::black_box;
    #[test]
    #[ignore = "manual dense-track measurement; reports timings without a machine-dependent assertion"]
    fn dense_track_workload() {
        let mut app = AnimaApp::with_recovery(Err("benchmark".into()));
        let id = app.project.assets[0].id;
        app.add_object(id);
        app.project.duration = 60.0;
        app.project.objects[0].poses = (0..100_000)
            .map(|i| Pose {
                time: i as f32 * 0.0005,
                transform: Transform {
                    x: i as f32,
                    ..Transform::default()
                },
                linear: true,
            })
            .collect();
        let object = &app.project.objects[0];
        let start = Instant::now();
        for _ in 0..1_000 {
            black_box(state_at(black_box(object), black_box(49.0)));
        }
        let lookup = start.elapsed().as_secs_f64() * 1_000_000.0 / 1_000.0;
        let start = Instant::now();
        for _ in 0..100 {
            black_box(
                object
                    .poses
                    .windows(2)
                    .find(|pair| black_box(49.0) <= pair[1].time),
            );
        }
        let linear = start.elapsed().as_secs_f64() * 1_000_000.0 / 100.0;
        let start = Instant::now();
        for _ in 0..100 {
            black_box(timeline::markers(black_box(&object.poses), 60.0, 600.0));
        }
        let markers = start.elapsed().as_secs_f64() * 1_000_000.0 / 100.0;
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
        let start = Instant::now();
        for _ in 0..20 {
            black_box(ctx.run(input(), |ctx| app.update_ui(ctx)));
        }
        let ui = start.elapsed().as_secs_f64() * 1_000.0 / 20.0;
        eprintln!(
            "dense track 100000 poses: indexed lookup {lookup:.2} µs, linear lookup {linear:.2} µs, marker selection {markers:.2} µs, headless UI {ui:.2} ms/frame"
        );
    }
    fn control_project() -> Project {
        let mut project = Project::new();
        project.duration = 60.0;
        project.assets.clear();
        for i in 0..50 {
            let elements = (0..20)
                .map(|j| DrawingElement::Stroke {
                    points: (0..20)
                        .map(|k| {
                            [
                                k as f32 * 2.0 - 20.0,
                                (j as f32 * 2.0 - 20.0) + (k % 3) as f32,
                            ]
                        })
                        .collect(),
                    color: [30 + (i % 10) as u8 * 10, 90, 170, 255],
                    width: 2.0,
                })
                .collect();
            let id = Uuid::new_v4();
            project.assets.push(Asset {
                id,
                name: format!("Рисунок {i}"),
                color: [0, 0, 0, 255],
                kind: Shape::Rectangle,
                content: AssetContent::Drawing,
                elements,
            });
            project.objects.push(SceneObject {
                id: Uuid::new_v4(),
                asset_id: id,
                name: format!("Объект {i}"),
                poses: vec![Pose {
                    time: 0.0,
                    transform: Transform {
                        x: (i % 10) as f32 * 55.0 - 250.0,
                        y: (i / 10) as f32 * 55.0 - 110.0,
                        ..Transform::default()
                    },
                    linear: true,
                }],
            });
        }
        project
    }
    #[test]
    #[ignore = "manual CPU texture preparation measurement; no GPU upload"]
    fn preview_texture_preparation_cost() {
        let project = control_project();
        for scale in [1_u32, 2] {
            let mut renderer = scene::Renderer::new(scale);
            let pixels = renderer.render(&project, 0.0).to_vec();
            let size = [640 * scale as usize, 360 * scale as usize];
            let ctx = egui::Context::default();
            let mut texture = ctx.load_texture(
                "texture_measurement",
                egui::ColorImage::from_rgba_unmultiplied(size, &pixels),
                egui::TextureOptions::LINEAR,
            );
            let iterations = 50;
            let start = Instant::now();
            for _ in 0..iterations {
                black_box(ctx.run(egui::RawInput::default(), |_| {
                    let image = egui::ColorImage::from_rgba_unmultiplied(size, black_box(&pixels));
                    texture.set(image, egui::TextureOptions::LINEAR);
                }));
            }
            eprintln!(
                "texture {scale}x: CPU conversion and egui staging {:.3} ms/frame; excludes GPU upload",
                start.elapsed().as_secs_f64() * 1000.0 / iterations as f64
            );
        }
    }
    #[test]
    #[ignore = "manual control workload; reports timings without a machine-dependent assertion"]
    fn control_workload() {
        let project = control_project();
        let iterations = 20;
        let mut preview_renderer = scene::Renderer::new(1);
        black_box(preview_renderer.render(&project, 0.0));
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(preview_renderer.render(black_box(&project), 0.0));
        }
        let preview = start.elapsed().as_secs_f64() * 1000.0 / iterations as f64;
        let mut export_renderer = scene::Renderer::new(2);
        black_box(export_renderer.render(&project, 0.0));
        let start = Instant::now();
        for _ in 0..5 {
            black_box(export_renderer.render(black_box(&project), 0.0));
        }
        let export = start.elapsed().as_secs_f64() * 1000.0 / 5.0;
        let start = Instant::now();
        for _ in 0..100 {
            black_box(project.clone());
        }
        let history = start.elapsed().as_secs_f64() * 10.0;
        let start = Instant::now();
        for _ in 0..20 {
            storage::ensure_saveable(black_box(&project)).unwrap();
        }
        let validation = start.elapsed().as_secs_f64() * 50.0;
        let mut app = AnimaApp::with_recovery(Err("benchmark".into()));
        app.project = project.clone();
        app.saved = project.clone();
        app.selected = Some(project.objects[0].id);
        app.error = None;
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
        let start = Instant::now();
        for i in 1..=20 {
            app.time = i as f32 / 10.0;
            black_box(ctx.run(input(), |ctx| app.update_ui(ctx)));
        }
        let ui = start.elapsed().as_secs_f64() * 50.0;
        app.begin_transaction();
        let id = project.objects[0].id;
        let start = Instant::now();
        for i in 0..20 {
            app.upsert_pose(
                id,
                Transform {
                    x: i as f32,
                    ..Transform::default()
                },
            );
        }
        let edit = start.elapsed().as_secs_f64() * 50.0;
        app.finish_transaction();
        let path = std::env::temp_dir().join(format!("animapro-perf-{}.animapro", Uuid::new_v4()));
        let start = Instant::now();
        safely_write_project(&path, &project).unwrap();
        let save = start.elapsed().as_secs_f64() * 1000.0;
        fs::remove_file(path).unwrap();
        eprintln!(
            "control workload: 60 s, 50 objects, 1000 strokes, 20000 points; preview {preview:.2} ms/frame, headless UI with scene {ui:.2} ms/frame, export {export:.2} ms/frame, snapshot {history:.2} ms, validation {validation:.2} ms, guarded edit {edit:.2} ms, save {save:.2} ms"
        );
    }
    #[test]
    #[ignore = "manual concurrent export and recovery measurement; requires FFmpeg with libx264"]
    fn concurrent_background_workload() {
        let dir = std::env::temp_dir().join(format!("animapro-concurrent-perf-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let project = control_project();
        let recovery = recovery::Recovery::in_directory(dir.join("recovery")).unwrap();
        let mut app = AnimaApp::with_recovery(Ok(recovery));
        app.project = project.clone();
        app.saved = project.clone();
        app.selected = Some(project.objects[0].id);
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 640.0))),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| app.update_ui(ctx));
        let job = start_export(project.clone(), dir.join("film.mp4"));
        let deadline = Instant::now() + Duration::from_secs(15);
        while job.phase() == ExportPhase::Preparing {
            assert!(Instant::now() < deadline, "encoder preparation timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            job.phase(),
            ExportPhase::Writing,
            "FFmpeg with libx264 is required"
        );
        app.recovery.as_ref().unwrap().save(project, None).unwrap();
        app.export = Some(job);
        let mut slowest: f64 = 0.0;
        let mut recovery_frames = 0;
        let start = Instant::now();
        for i in 1..=20 {
            app.time = i as f32 / 10.0;
            if app.recovery.as_ref().unwrap().is_pending() {
                recovery_frames += 1;
            }
            let frame = Instant::now();
            black_box(ctx.run(input(), |ctx| app.update_ui(ctx)));
            slowest = slowest.max(frame.elapsed().as_secs_f64() * 1000.0);
        }
        let mean = start.elapsed().as_secs_f64() * 50.0;
        let job = app.export.take().expect("export finished unexpectedly");
        job.cancel.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(15);
        while job.result.lock().unwrap().is_none() {
            assert!(Instant::now() < deadline, "export cancellation timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(job.phase(), ExportPhase::Cancelled);
        let recovery = app.recovery.as_ref().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while recovery.is_pending() {
            assert!(Instant::now() < deadline, "recovery timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(recovery.error().is_none(), "recovery failed");
        assert!(app.error.is_none(), "background work reported an error");
        assert!(fs::read_dir(dir.join("recovery")).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "json")
        }));
        drop(app);
        fs::remove_dir_all(dir).unwrap();
        eprintln!(
            "concurrent export and recovery: headless UI mean {mean:.2} ms/frame, slowest of 20 {slowest:.2} ms, recovery pending at start of {recovery_frames} frames"
        );
    }
}

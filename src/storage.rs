use crate::model::{Asset, AssetContent, DrawingElement, Project, SceneObject, Shape};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};
use uuid::Uuid;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

pub(super) const MAX_JSON: u64 = 32 * 1024 * 1024;
pub(super) const MAX_ASSETS: usize = 1_000;
pub(super) const MAX_OBJECTS: usize = 1_000;
pub(super) const MAX_ELEMENTS: usize = 100_000;
pub(super) const MAX_POINTS: usize = 1_000_000;

#[derive(Debug)]
pub(super) enum StorageError {
    Validation(ValidationError),
    Io(std::io::Error),
    Archive(zip::result::ZipError),
    Json(serde_json::Error),
    TooLarge,
    MissingProjectJson,
    MissingSceneSize,
    MissingElements,
    MissingContent,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ValidationError {
    UnsupportedVersion(u32),
    SceneSize,
    Duration,
    ObjectAssetLimit,
    DuplicateAsset,
    BuiltinElements,
    ElementPointLimit,
    Geometry,
    ObjectReference,
    Pose,
}

pub(super) fn validate(project: &Project) -> Result<(), ValidationError> {
    if project.version != 2 {
        return Err(ValidationError::UnsupportedVersion(project.version));
    }
    if project.scene_size != [640, 360] {
        return Err(ValidationError::SceneSize);
    }
    if !project.duration.is_finite() || !(1.0..=60.0).contains(&project.duration) {
        return Err(ValidationError::Duration);
    }
    if project.assets.len() > MAX_ASSETS || project.objects.len() > MAX_OBJECTS {
        return Err(ValidationError::ObjectAssetLimit);
    }
    let mut assets = HashSet::new();
    let mut objects = HashSet::new();
    let mut elements = 0usize;
    let mut points = 0usize;
    for asset in &project.assets {
        if !assets.insert(asset.id) {
            return Err(ValidationError::DuplicateAsset);
        }
        if asset.content == AssetContent::Builtin && !asset.elements.is_empty() {
            return Err(ValidationError::BuiltinElements);
        }
        elements = elements.saturating_add(asset.elements.len());
        if elements > MAX_ELEMENTS {
            return Err(ValidationError::ElementPointLimit);
        }
        for element in &asset.elements {
            let valid_point = |p: &[f32; 2]| p.iter().all(|v| v.is_finite());
            let (count, width, valid) = match element {
                DrawingElement::Line {
                    from, to, width, ..
                }
                | DrawingElement::Rect {
                    from, to, width, ..
                }
                | DrawingElement::Oval {
                    from, to, width, ..
                } => (2, *width, valid_point(from) && valid_point(to)),
                DrawingElement::Stroke { points, width, .. } => {
                    (points.len(), *width, points.iter().all(valid_point))
                }
            };
            let valid_angle = match element {
                DrawingElement::Rect { angle, .. } | DrawingElement::Oval { angle, .. } => {
                    angle.is_finite()
                }
                _ => true,
            };
            points = points.saturating_add(count);
            if points > MAX_POINTS {
                return Err(ValidationError::ElementPointLimit);
            }
            if !width.is_finite() || width <= 0.0 || !valid || !valid_angle {
                return Err(ValidationError::Geometry);
            }
        }
    }
    for object in &project.objects {
        if !objects.insert(object.id) || !assets.contains(&object.asset_id) {
            return Err(ValidationError::ObjectReference);
        }
        let mut previous = -1.0;
        for pose in &object.poses {
            let t = pose.transform;
            if !pose.time.is_finite()
                || pose.time < 0.0
                || pose.time > project.duration
                || pose.time <= previous
                || !t.x.is_finite()
                || !t.y.is_finite()
                || !t.scale.is_finite()
                || t.scale <= 0.0
                || !t.angle.is_finite()
            {
                return Err(ValidationError::Pose);
            }
            previous = pose.time;
        }
    }
    Ok(())
}

/// Check the exact compact JSON size without allocating another project-sized buffer.
pub(super) fn ensure_saveable(project: &Project) -> Result<(), StorageError> {
    validate(project).map_err(StorageError::Validation)?;
    struct Counter {
        size: u64,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.size = self.size.saturating_add(bytes.len() as u64);
            if self.size > MAX_JSON {
                return Err(std::io::Error::other("size limit"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { size: 0 };
    serde_json::to_writer(&mut counter, project).map_err(|e| {
        if counter.size > MAX_JSON {
            StorageError::TooLarge
        } else {
            StorageError::Json(e)
        }
    })
}

#[derive(Deserialize)]
struct StoredAsset {
    id: Uuid,
    name: String,
    color: [u8; 4],
    kind: Shape,
    content: Option<AssetContent>,
    elements: Option<Vec<DrawingElement>>,
}
#[derive(Deserialize)]
struct StoredProject {
    version: u32,
    scene_size: Option<[u32; 2]>,
    name: String,
    duration: f32,
    assets: Vec<StoredAsset>,
    objects: Vec<SceneObject>,
}
fn decode_project(bytes: &[u8]) -> Result<Project, StorageError> {
    let stored: StoredProject = serde_json::from_slice(bytes).map_err(StorageError::Json)?;
    if stored.version != 1 && stored.version != 2 {
        return Err(StorageError::Validation(
            ValidationError::UnsupportedVersion(stored.version),
        ));
    }
    let scene_size = if stored.version == 1 {
        [640, 360]
    } else {
        stored.scene_size.ok_or(StorageError::MissingSceneSize)?
    };
    let mut assets = Vec::with_capacity(stored.assets.len());
    for asset in stored.assets {
        let elements = match asset.elements {
            Some(elements) => elements,
            None if stored.version == 1 => Vec::new(),
            None => return Err(StorageError::MissingElements),
        };
        let content = if stored.version == 1 {
            if elements.is_empty() {
                AssetContent::Builtin
            } else {
                AssetContent::Drawing
            }
        } else {
            asset.content.ok_or(StorageError::MissingContent)?
        };
        assets.push(Asset {
            id: asset.id,
            name: asset.name,
            color: asset.color,
            kind: asset.kind,
            content,
            elements,
        });
    }
    Ok(Project {
        version: 2,
        scene_size,
        name: stored.name,
        duration: stored.duration,
        assets,
        objects: stored.objects,
    })
}

pub(super) fn read_project(path: &Path) -> Result<Project, StorageError> {
    let file = fs::File::open(path).map_err(StorageError::Io)?;
    let mut bytes = Vec::new();
    match ZipArchive::new(file) {
        Ok(mut archive) => {
            archive
                .by_name("project.json")
                .map_err(|_| StorageError::MissingProjectJson)?
                .take(MAX_JSON + 1)
                .read_to_end(&mut bytes)
                .map_err(StorageError::Io)?;
        }
        Err(_) => {
            fs::File::open(path)
                .map_err(StorageError::Io)?
                .take(MAX_JSON + 1)
                .read_to_end(&mut bytes)
                .map_err(StorageError::Io)?;
        }
    }
    if bytes.len() as u64 > MAX_JSON {
        return Err(StorageError::TooLarge);
    }
    let project = decode_project(&bytes)?;
    ensure_saveable(&project)?;
    Ok(project)
}

pub(super) fn write_project(path: &Path, project: &Project) -> Result<(), StorageError> {
    ensure_saveable(project)?;
    let json = serde_json::to_vec(project).map_err(StorageError::Json)?;
    if json.len() as u64 > MAX_JSON {
        return Err(StorageError::TooLarge);
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(StorageError::Io)?;
    let mut archive = ZipWriter::new(file);
    archive
        .start_file("project.json", SimpleFileOptions::default())
        .map_err(StorageError::Archive)?;
    archive.write_all(&json).map_err(StorageError::Io)?;
    archive
        .finish()
        .map_err(StorageError::Archive)?
        .sync_all()
        .map_err(StorageError::Io)
}

pub(super) fn safely_write_project(path: &Path, project: &Project) -> Result<(), StorageError> {
    let temporary = path.with_file_name(format!(".animapro-{}.tmp", Uuid::new_v4()));
    let result = write_project(&temporary, project)
        .and_then(|()| fs::rename(&temporary, path).map_err(StorageError::Io));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pose, SceneObject, Transform};
    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("animapro-storage-{}", Uuid::new_v4()))
    }
    #[test]
    fn migrates_v1_asset_content_without_changing_coordinates() {
        let path = path();
        let mut project = Project::new();
        project.version = 1;
        project.assets[0].elements.push(DrawingElement::Line {
            from: [12.0, -7.0],
            to: [30.0, 4.0],
            color: [0, 0, 0, 255],
            width: 3.0,
        });
        let mut json = serde_json::to_value(&project).unwrap();
        json.as_object_mut().unwrap().remove("scene_size");
        json["assets"][0].as_object_mut().unwrap().remove("content");
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let migrated = read_project(&path).unwrap();
        assert!(migrated.version == 2 && migrated.assets[0].content == AssetContent::Drawing);
        assert!(migrated.assets[0].elements == project.assets[0].elements);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn reads_v1_zip_with_builtin_and_drawn_assets() {
        let path = path();
        let mut old = Project::new();
        old.version = 1;
        let mut drawing = old.assets[0].clone();
        drawing.id = Uuid::new_v4();
        drawing.elements.push(DrawingElement::Oval {
            from: [-50.0, -10.0],
            to: [50.0, 10.0],
            angle: 0.0,
            color: [100, 20, 30, 128],
            fill: false,
            width: 2.0,
        });
        old.assets.push(drawing);
        let mut value = serde_json::to_value(&old).unwrap();
        value.as_object_mut().unwrap().remove("scene_size");
        for asset in value["assets"].as_array_mut().unwrap() {
            asset.as_object_mut().unwrap().remove("content");
        }
        let mut zip = ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file("project.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&serde_json::to_vec(&value).unwrap()).unwrap();
        zip.finish().unwrap();
        let opened = read_project(&path).unwrap();
        assert!(
            opened.version == 2
                && opened.assets[0].content == AssetContent::Builtin
                && opened.assets[1].content == AssetContent::Drawing
        );
        assert!(opened.assets[1].elements == old.assets[1].elements);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn v2_requires_explicit_content_and_scene_fields() {
        let path = path();
        let mut project = Project::new();
        project.assets[0].content = AssetContent::Drawing;
        project.assets[0].elements.push(DrawingElement::Line {
            from: [0.0, 0.0],
            to: [10.0, 10.0],
            color: [0, 0, 0, 255],
            width: 1.0,
        });
        let original = serde_json::to_value(&project).unwrap();
        for field in ["content", "elements"] {
            let mut value = original.clone();
            value["assets"][0].as_object_mut().unwrap().remove(field);
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(matches!(
                read_project(&path),
                Err(StorageError::MissingContent | StorageError::MissingElements)
            ));
        }
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove("scene_size");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            read_project(&path),
            Err(StorageError::MissingSceneSize)
        ));
        let mut value = original;
        value["assets"][0]["content"] = serde_json::json!("Builtin");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            read_project(&path),
            Err(StorageError::Validation(ValidationError::BuiltinElements))
        ));
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn corrupt_archives_missing_json_and_future_versions_fail_safely() {
        let path = path();
        fs::write(&path, b"PK\x03\x04 broken archive").unwrap();
        assert!(read_project(&path).is_err());
        let mut zip = ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file("other.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"{}").unwrap();
        zip.finish().unwrap();
        assert!(matches!(
            read_project(&path),
            Err(StorageError::MissingProjectJson)
        ));
        let mut future = serde_json::to_value(Project::new()).unwrap();
        future["version"] = serde_json::json!(123);
        fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        assert!(matches!(
            read_project(&path),
            Err(StorageError::Validation(
                ValidationError::UnsupportedVersion(123)
            ))
        ));
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn rejects_oversized_decompressed_zip_content() {
        let path = path();
        let mut zip = ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file(
            "project.json",
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        let block = [b' '; 65536];
        for _ in 0..(MAX_JSON / block.len() as u64) {
            zip.write_all(&block).unwrap();
        }
        zip.write_all(b"x").unwrap();
        zip.finish().unwrap();
        assert!(matches!(read_project(&path), Err(StorageError::TooLarge)));
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn failed_replacement_keeps_directory_and_removes_temporary_file() {
        let dir = path();
        fs::create_dir(&dir).unwrap();
        let target = dir.join("existing.animapro");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), b"original").unwrap();
        assert!(safely_write_project(&target, &Project::new()).is_err());
        assert_eq!(fs::read(target.join("keep")).unwrap(), b"original");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn compact_valid_project_does_not_become_unsaveable_due_to_pretty_printing() {
        let mut project = Project::new();
        project.assets[0].content = AssetContent::Drawing;
        project.assets[0].elements.push(DrawingElement::Stroke {
            points: vec![[100.12346, -100.12346]; 700_000],
            color: [0, 0, 0, 255],
            width: 1.0,
        });
        assert!(serde_json::to_vec(&project).unwrap().len() < MAX_JSON as usize);
        assert!(serde_json::to_vec_pretty(&project).unwrap().len() > MAX_JSON as usize);
        let path = path();
        safely_write_project(&path, &project).unwrap();
        assert!(read_project(&path).unwrap() == project);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn validates_limits_and_duplicate_ids() {
        let mut project = Project::new();
        project.assets.push(project.assets[0].clone());
        assert!(validate(&project).is_err());
        project.assets.pop();
        project.assets[0].content = AssetContent::Drawing;
        project.assets[0].elements.push(DrawingElement::Stroke {
            points: vec![[0.0, 0.0]; 1_000_001],
            color: [0, 0, 0, 255],
            width: 1.0,
        });
        assert_eq!(validate(&project), Err(ValidationError::ElementPointLimit));
    }
    #[test]
    fn rejects_unknown_version_and_invalid_references() {
        let mut project = Project::new();
        project.version = 99;
        assert!(validate(&project).is_err());
        project.version = 2;
        project.objects.push(SceneObject {
            id: Uuid::new_v4(),
            asset_id: Uuid::new_v4(),
            name: "x".into(),
            poses: vec![Pose {
                time: 0.0,
                transform: Transform::default(),
                linear: false,
            }],
        });
        assert!(validate(&project).is_err());
    }
    #[test]
    fn rejects_duplicate_and_out_of_range_pose_times() {
        let mut project = Project::new();
        let pose = Pose {
            time: 0.0,
            transform: Transform::default(),
            linear: false,
        };
        project.objects.push(SceneObject {
            id: Uuid::new_v4(),
            asset_id: project.assets[0].id,
            name: "x".into(),
            poses: vec![pose.clone(), pose],
        });
        assert!(validate(&project).is_err());
        project.objects[0].poses[1].time = 61.0;
        assert!(validate(&project).is_err());
    }
    #[test]
    fn failed_save_preserves_existing_file() {
        let path = path();
        fs::write(&path, b"original").unwrap();
        let mut project = Project::new();
        project.duration = 0.0;
        assert!(safely_write_project(&path, &project).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn reads_legacy_json_and_rejects_oversized_input() {
        let path = path();
        fs::write(&path, serde_json::to_vec(&Project::new()).unwrap()).unwrap();
        assert!(read_project(&path).is_ok());
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_JSON + 1).unwrap();
        assert!(matches!(read_project(&path), Err(StorageError::TooLarge)));
        fs::remove_file(path).unwrap();
    }
}

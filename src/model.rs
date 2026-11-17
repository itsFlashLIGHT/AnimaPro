//! Serializable project data, independent of windows and storage.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq)]
pub(crate) struct Transform {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) scale: f32,
    pub(crate) angle: f32,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            angle: 0.0,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Pose {
    pub(crate) time: f32,
    pub(crate) transform: Transform,
    pub(crate) linear: bool,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Asset {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) color: [u8; 4],
    pub(crate) kind: Shape,
    pub(crate) content: AssetContent,
    pub(crate) elements: Vec<DrawingElement>,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub(crate) enum AssetContent {
    #[default]
    Builtin,
    Drawing,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq)]
pub(crate) enum Shape {
    Ball,
    Rectangle,
    Oval,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub(crate) enum DrawingElement {
    Line {
        from: [f32; 2],
        to: [f32; 2],
        color: [u8; 4],
        width: f32,
    },
    Rect {
        from: [f32; 2],
        to: [f32; 2],
        #[serde(default)]
        angle: f32,
        color: [u8; 4],
        fill: bool,
        width: f32,
    },
    Oval {
        from: [f32; 2],
        to: [f32; 2],
        #[serde(default)]
        angle: f32,
        color: [u8; 4],
        fill: bool,
        width: f32,
    },
    Stroke {
        points: Vec<[f32; 2]>,
        color: [u8; 4],
        width: f32,
    },
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct SceneObject {
    pub(crate) id: Uuid,
    pub(crate) asset_id: Uuid,
    pub(crate) name: String,
    pub(crate) poses: Vec<Pose>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Project {
    pub(crate) version: u32,
    pub(crate) scene_size: [u32; 2],
    pub(crate) name: String,
    pub(crate) duration: f32,
    pub(crate) assets: Vec<Asset>,
    pub(crate) objects: Vec<SceneObject>,
}
fn scene_size() -> [u32; 2] {
    [640, 360]
}
impl Project {
    pub(crate) fn new() -> Self {
        let ball = Asset {
            id: Uuid::new_v4(),
            name: "Мяч".into(),
            color: [239, 113, 69, 255],
            kind: Shape::Ball,
            content: AssetContent::Builtin,
            elements: vec![],
        };
        Self {
            version: 2,
            scene_size: scene_size(),
            name: "Мой мультфильм".into(),
            duration: 10.0,
            assets: vec![ball],
            objects: vec![],
        }
    }
    pub(crate) fn asset(&self, id: Uuid) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }
}

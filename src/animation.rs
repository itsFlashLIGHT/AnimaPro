//! Time sampling and interpolation of project poses.
use crate::model::{SceneObject, Transform};
pub(crate) const STEP: f32 = 0.1;

pub(crate) fn snap(t: f32) -> f32 {
    (t / STEP).round() * STEP
}
pub(crate) fn snap_within(t: f32, duration: f32) -> f32 {
    let max_step = ((duration / STEP) + 0.0001).floor() * STEP;
    snap(t).clamp(0.0, max_step.min(duration))
}
pub(crate) fn same_time(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.00001
}
pub(crate) fn angle_delta(from: f32, to: f32) -> f32 {
    let from = (from as f64).rem_euclid(360.0);
    let to = (to as f64).rem_euclid(360.0);
    ((to - from + 180.0).rem_euclid(360.0) - 180.0) as f32
}
fn eased(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
pub(crate) fn state_at(object: &SceneObject, time: f32) -> Transform {
    let poses = &object.poses;
    if poses.is_empty() {
        return Transform::default();
    }
    if time <= poses[0].time {
        return poses[0].transform;
    }
    // Validated projects keep pose times strictly increasing.
    let right = poses.partition_point(|pose| pose.time < time);
    if right == 0 {
        return poses[0].transform;
    }
    if right == poses.len() {
        return poses.last().unwrap().transform;
    }
    let (a, b) = (&poses[right - 1], &poses[right]);
    let mut p = (time - a.time) / (b.time - a.time);
    if !a.linear {
        p = eased(p);
    }
    let lerp = |from: f32, to: f32| (from as f64 + (to as f64 - from as f64) * p as f64) as f32;
    Transform {
        x: lerp(a.transform.x, b.transform.x),
        y: lerp(a.transform.y, b.transform.y),
        scale: lerp(a.transform.scale, b.transform.scale),
        angle: ((if a.transform.angle.abs() > 360.0 {
            (a.transform.angle as f64).rem_euclid(360.0)
        } else {
            a.transform.angle as f64
        }) + angle_delta(a.transform.angle, b.transform.angle) as f64 * p as f64)
            as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Pose;
    use uuid::Uuid;

    #[test]
    fn finite_extreme_coordinates_do_not_overflow_between_poses() {
        let object = SceneObject {
            id: Uuid::new_v4(),
            asset_id: Uuid::new_v4(),
            name: "test".into(),
            poses: vec![
                Pose {
                    time: 0.0,
                    transform: Transform {
                        x: -f32::MAX,
                        y: f32::MAX,
                        angle: -f32::MAX,
                        ..Transform::default()
                    },
                    linear: true,
                },
                Pose {
                    time: 1.0,
                    transform: Transform {
                        x: f32::MAX,
                        y: -f32::MAX,
                        angle: f32::MAX,
                        ..Transform::default()
                    },
                    linear: true,
                },
            ],
        };
        let state = state_at(&object, 0.5);
        assert_eq!(state.x, 0.0);
        assert_eq!(state.y, 0.0);
        assert!(state.angle.is_finite());
        assert!(angle_delta(-f32::MAX, f32::MAX).is_finite());
    }
    #[test]
    fn large_angle_interpolates_as_its_equivalent_rotation() {
        let object = SceneObject {
            id: Uuid::new_v4(),
            asset_id: Uuid::new_v4(),
            name: "test".into(),
            poses: vec![
                Pose {
                    time: 0.0,
                    transform: Transform {
                        angle: f32::MAX,
                        ..Transform::default()
                    },
                    linear: true,
                },
                Pose {
                    time: 1.0,
                    transform: Transform {
                        angle: 90.0,
                        ..Transform::default()
                    },
                    linear: true,
                },
            ],
        };
        assert_eq!(state_at(&object, 0.5).angle, 45.0);
        assert_eq!(state_at(&object, 1.0).angle, 90.0);
    }

    #[test]
    fn dense_pose_track_finds_adjacent_poses_and_endpoints() {
        let object = SceneObject {
            id: Uuid::new_v4(),
            asset_id: Uuid::new_v4(),
            name: "dense".into(),
            poses: (0..100_000)
                .map(|i| Pose {
                    time: i as f32 * 0.0005,
                    transform: Transform {
                        x: i as f32,
                        ..Transform::default()
                    },
                    linear: true,
                })
                .collect(),
        };
        let start = &object.poses[50_000];
        let end = &object.poses[50_001];
        let middle = (start.time + end.time) / 2.0;
        assert!((state_at(&object, middle).x - 50_000.5).abs() < 0.1);
        assert_eq!(state_at(&object, -1.0).x, 0.0);
        assert_eq!(state_at(&object, f32::NAN).x, 0.0);
        assert_eq!(state_at(&object, 60.0).x, 99_999.0);
        assert_eq!(state_at(&object, start.time).x, 50_000.0);
    }
    #[test]
    fn linear_and_smooth_motion_retain_expected_values() {
        let mut object = SceneObject {
            id: Uuid::new_v4(),
            asset_id: Uuid::new_v4(),
            name: "test".into(),
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
                        angle: -170.0,
                        ..Transform::default()
                    },
                    linear: true,
                },
            ],
        };
        object.poses[0].transform.angle = 170.0;
        assert_eq!(state_at(&object, 0.25).x, 25.0);
        assert_eq!(state_at(&object, 0.5).angle, 180.0);
        object.poses[0].linear = false;
        assert_eq!(state_at(&object, 0.25).x, 15.625);
        assert_eq!(state_at(&object, -1.0).x, 0.0);
        assert_eq!(state_at(&object, 2.0).x, 100.0);
    }
}

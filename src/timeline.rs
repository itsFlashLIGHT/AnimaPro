//! Helpers for displaying and selecting densely spaced, sorted pose tracks.
use crate::model::Pose;

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

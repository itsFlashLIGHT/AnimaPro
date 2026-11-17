//! Background FFmpeg lifecycle and video publication.
use crate::{Project, scene};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum ExportPhase {
    Preparing,
    Writing,
    Finalizing,
    Succeeded,
    Cancelled,
    Failed,
}
#[derive(Debug)]
pub(super) enum ExportError {
    Cancelled,
    Start(io::Error),
    EncoderUnresponsive,
    EncoderCheck(String),
    EncoderMissing,
    FrameWrite(io::Error),
    ProcessWait(io::Error),
    EncoderFailed(String),
    FrameTransferInterrupted,
    Publish(io::Error),
}
pub(super) struct ExportJob {
    pub(super) progress: Arc<AtomicUsize>,
    pub(super) total: usize,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) result: Arc<Mutex<Option<Result<PathBuf, ExportError>>>>,
    phase: Arc<AtomicU8>,
}
impl ExportJob {
    pub(super) fn phase(&self) -> ExportPhase {
        match self.phase.load(Ordering::Acquire) {
            0 => ExportPhase::Preparing,
            1 => ExportPhase::Writing,
            2 => ExportPhase::Finalizing,
            3 => ExportPhase::Succeeded,
            4 => ExportPhase::Cancelled,
            _ => ExportPhase::Failed,
        }
    }
}
fn export_temp_path(output: &Path) -> PathBuf {
    output.with_file_name(format!(".animapro-{}.part.mp4", Uuid::new_v4()))
}
fn bundled_ffmpeg() -> PathBuf {
    let executable = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("animapro"));
    let sibling = executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
    if sibling.is_file() {
        return sibling;
    }
    let app_resource = executable
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("Resources").join("ffmpeg"));
    if let Some(path) = app_resource.filter(|p| p.is_file()) {
        return path;
    }
    PathBuf::from(if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    })
}
fn check_encoder(executable: &Path, cancel: &AtomicBool) -> Result<(), ExportError> {
    let mut child = Command::new(executable)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(ExportError::Start)?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let read_limited = |mut stream: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut data = Vec::new();
            let mut buffer = [0u8; 4096];
            while let Ok(n) = stream.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                if data.len() < 1024 * 1024 {
                    let keep = n.min(1024 * 1024 - data.len());
                    data.extend_from_slice(&buffer[..keep]);
                }
            }
            data
        })
    };
    let out_reader = read_limited(Box::new(stdout));
    let err_reader = read_limited(Box::new(stderr));
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if cancel.load(Ordering::Relaxed) || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => thread::sleep(Duration::from_millis(40)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let output = out_reader.join().unwrap_or_default();
    let error = err_reader.join().unwrap_or_default();
    if cancel.load(Ordering::Relaxed) {
        return Err(ExportError::Cancelled);
    }
    let Some(status) = status else {
        return Err(ExportError::EncoderUnresponsive);
    };
    if !status.success() {
        return Err(ExportError::EncoderCheck(
            String::from_utf8_lossy(&error).into_owned(),
        ));
    }
    let has_encoder = String::from_utf8_lossy(&output)
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some("libx264"));
    if !has_encoder {
        return Err(ExportError::EncoderMissing);
    }
    Ok(())
}
pub(super) fn start_export(project: Project, output: PathBuf) -> ExportJob {
    start_export_with(project, output, bundled_ffmpeg())
}
fn start_export_with(project: Project, output: PathBuf, executable: PathBuf) -> ExportJob {
    let total = (project.duration * 30.0).round().max(1.0) as usize;
    let progress = Arc::new(AtomicUsize::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(None));
    let phase = Arc::new(AtomicU8::new(ExportPhase::Preparing as u8));
    let job = ExportJob {
        progress: progress.clone(),
        total,
        cancel: cancel.clone(),
        result: result.clone(),
        phase: phase.clone(),
    };
    thread::spawn(move || {
        let temporary = export_temp_path(&output);
        let task = (|| -> Result<PathBuf, ExportError> {
            check_encoder(&executable, &cancel)?;
            let mut child = Command::new(executable)
                .args([
                    "-y",
                    "-f",
                    "rawvideo",
                    "-pixel_format",
                    "rgba",
                    "-video_size",
                    "1280x720",
                    "-framerate",
                    "30",
                    "-i",
                    "-",
                    "-an",
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                ])
                .arg(&temporary)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(ExportError::Start)?;
            let mut input = child.stdin.take().expect("piped stdin");
            let mut stderr = child.stderr.take().expect("piped stderr");
            let log = thread::spawn(move || {
                let mut tail = Vec::new();
                let mut buf = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    tail.extend_from_slice(&buf[..n]);
                    if tail.len() > 16_384 {
                        tail.drain(..tail.len() - 16_384);
                    }
                }
                String::from_utf8_lossy(&tail).into_owned()
            });
            let (sender, receiver) = std::sync::mpsc::channel();
            phase.store(ExportPhase::Writing as u8, Ordering::Release);
            let writer_phase = phase.clone();
            let writer = thread::spawn(move || {
                let result = (|| -> Result<(), ExportError> {
                    let mut renderer = scene::Renderer::new(2);
                    for frame in 0..total {
                        input
                            .write_all(renderer.render(&project, frame as f32 / 30.0))
                            .map_err(ExportError::FrameWrite)?;
                        progress.store(frame + 1, Ordering::Relaxed);
                    }
                    Ok(())
                })();
                drop(input);
                if result.is_ok() {
                    writer_phase.store(ExportPhase::Finalizing as u8, Ordering::Release);
                }
                let _ = sender.send(result);
            });
            let mut write_result = None;
            let status = loop {
                if write_result.is_none() {
                    write_result = receiver.try_recv().ok();
                }
                if cancel.load(Ordering::Relaxed) || matches!(write_result, Some(Err(_))) {
                    let _ = child.kill();
                    break child.wait().map_err(ExportError::ProcessWait);
                }
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Ok(None) => thread::sleep(Duration::from_millis(40)),
                    Err(e) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(ExportError::ProcessWait(e));
                    }
                }
            };
            let _ = writer.join();
            if write_result.is_none() {
                write_result = receiver.try_recv().ok();
            }
            let log = log.join().unwrap_or_default();
            if cancel.load(Ordering::Relaxed) {
                return Err(ExportError::Cancelled);
            }
            let status = status?;
            if !status.success() {
                return Err(ExportError::EncoderFailed(log));
            }
            match write_result {
                Some(Ok(())) => {}
                Some(Err(e)) => return Err(e),
                None => return Err(ExportError::FrameTransferInterrupted),
            }
            fs::rename(&temporary, &output).map_err(ExportError::Publish)?;
            Ok(output.clone())
        })();
        if task.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        let terminal = match &task {
            Ok(_) => ExportPhase::Succeeded,
            Err(ExportError::Cancelled) => ExportPhase::Cancelled,
            Err(_) => ExportPhase::Failed,
        };
        phase.store(terminal as u8, Ordering::Release);
        *result.lock().expect("export state") = Some(task);
    });
    job
}

#[cfg(test)]
mod export_tests {
    use super::*;
    use crate::storage::{read_project, safely_write_project};
    use crate::{AssetContent, DrawingElement, Pose, SceneObject, Transform};
    fn fake(mode: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("animapro-export-test-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let executable = dir.join(format!("{mode}{}", std::env::consts::EXE_SUFFIX));
        fs::copy(env!("ANIMAPRO_TEST_ENCODER"), &executable).expect("test encoder is unavailable");
        (dir, executable)
    }
    fn wait(job: &ExportJob) -> Result<PathBuf, ExportError> {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(result) = job.result.lock().unwrap().take() {
                return result;
            }
            assert!(Instant::now() < until, "export did not finish");
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn wait_until_started(executable: &Path) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !executable.with_extension("started").exists() {
            assert!(Instant::now() < deadline, "test encoder did not start");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn real_encoder_available() -> bool {
        let tools_present = ["ffmpeg", "ffprobe"].iter().all(|tool| {
            Command::new(tool)
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        });
        let available =
            tools_present && check_encoder(&test_encoder_path(), &AtomicBool::new(false)).is_ok();
        if !available {
            assert_ne!(
                std::env::var("ANIMAPRO_REQUIRE_FFMPEG_TESTS").as_deref(),
                Ok("1"),
                "FFmpeg with libx264 and ffprobe are required for this test run"
            );
            eprintln!("Skipping real video test: FFmpeg with libx264 or ffprobe is unavailable");
        }
        available
    }
    fn test_encoder_path() -> PathBuf {
        std::env::var_os("ANIMAPRO_TEST_FFMPEG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("ffmpeg"))
    }
    #[test]
    fn cancellation_stops_encoder_blocking_stdin_and_preserves_output() {
        let (dir, exe) = fake("blocked");
        let output = dir.join("film.mp4");
        fs::write(&output, b"previous").unwrap();
        let job = start_export_with(Project::new(), output.clone(), exe.clone());
        wait_until_started(&exe);
        assert_eq!(job.phase(), ExportPhase::Writing);
        thread::sleep(Duration::from_millis(100));
        job.cancel.store(true, Ordering::Relaxed);
        assert!(wait(&job).is_err());
        assert_eq!(job.phase(), ExportPhase::Cancelled);
        assert_eq!(fs::read(&output).unwrap(), b"previous");
        fs::remove_file(exe.with_extension("started")).unwrap();
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn real_ffmpeg_encodes_short_video_when_available() {
        if !real_encoder_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("animapro-real-export-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let output = dir.join("film.mp4");
        let mut project = Project::new();
        project.duration = 1.0;
        let job = start_export_with(project, output.clone(), test_encoder_path());
        let result = wait(&job);
        assert!(result.is_ok(), "export failed: {:?}", result.err());
        assert_eq!(job.phase(), ExportPhase::Succeeded);
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height,r_frame_rate",
                "-of",
                "csv=p=0",
            ])
            .arg(&output)
            .output()
            .unwrap();
        assert!(probe.status.success());
        assert!(String::from_utf8_lossy(&probe.stdout).contains("1280,720,30/1"));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn drawing_survives_save_reopen_and_encoded_motion() {
        if !real_encoder_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("animapro-scenario-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let project_path = dir.join("учебный проект.animapro");
        let video = dir.join("мультфильм.mp4");
        let mut project = Project::new();
        project.duration = 1.0;
        project.assets[0].content = AssetContent::Drawing;
        project.assets[0].elements = vec![DrawingElement::Rect {
            from: [-20.0, -10.0],
            to: [20.0, 10.0],
            angle: 0.0,
            color: [245, 15, 15, 255],
            fill: true,
            width: 1.0,
        }];
        project.objects.push(SceneObject {
            id: Uuid::new_v4(),
            asset_id: project.assets[0].id,
            name: "Герой".into(),
            poses: vec![
                Pose {
                    time: 0.0,
                    transform: Transform {
                        x: -100.0,
                        angle: 90.0,
                        ..Transform::default()
                    },
                    linear: true,
                },
                Pose {
                    time: 1.0,
                    transform: Transform {
                        x: 100.0,
                        angle: 90.0,
                        ..Transform::default()
                    },
                    linear: true,
                },
            ],
        });
        safely_write_project(&project_path, &project).unwrap();
        let reopened = read_project(&project_path).unwrap();
        assert!(project == reopened);
        let job = start_export_with(reopened.clone(), video.clone(), test_encoder_path());
        assert!(wait(&job).is_ok());
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration:stream=codec_type,width,height,r_frame_rate",
                "-of",
                "json",
            ])
            .arg(&video)
            .output()
            .unwrap();
        assert!(probe.status.success());
        let info: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let streams = info["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0]["codec_type"], "video");
        assert_eq!(streams[0]["width"], 1280);
        assert_eq!(streams[0]["height"], 720);
        assert_eq!(streams[0]["r_frame_rate"], "30/1");
        let duration: f64 = info["format"]["duration"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((duration - 1.0).abs() <= 1.0 / 30.0);
        for (seconds, x) in [("0", 440usize), ("0.5", 640usize)] {
            let decoded = Command::new("ffmpeg")
                .args(["-v", "error", "-ss", seconds, "-i"])
                .arg(&video)
                .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
                .output()
                .unwrap();
            assert!(decoded.status.success());
            assert_eq!(decoded.stdout.len(), 1280 * 720 * 4);
            let sample = |x: usize, y: usize| -> &[u8] {
                let i = (y * 1280 + x) * 4;
                &decoded.stdout[i..i + 4]
            };
            let red = sample(x, 385);
            assert!(
                red[0] > 190 && red[1] < 65 && red[2] < 65,
                "expected red drawing: {red:?}"
            );
            let background = sample(x + 30, 360);
            assert!(
                background[0] > 210 && background[1] > 210 && background[2] > 210,
                "rotation or background mismatch: {background:?}"
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn preflight_rejects_missing_encoder_without_replacing_existing_video() {
        let (dir, exe) = fake("no_encoder");
        let output = dir.join("film.mp4");
        fs::write(&output, b"previous").unwrap();
        let job = start_export_with(Project::new(), output.clone(), exe);
        let error = wait(&job).unwrap_err();
        assert!(matches!(error, ExportError::EncoderMissing));
        assert_eq!(job.phase(), ExportPhase::Failed);
        assert_eq!(fs::read(&output).unwrap(), b"previous");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn preflight_cancellation_stops_unresponsive_encoder() {
        let (dir, exe) = fake("unresponsive");
        let output = dir.join("film.mp4");
        let job = start_export_with(Project::new(), output, exe.clone());
        wait_until_started(&exe);
        assert_eq!(job.phase(), ExportPhase::Preparing);
        job.cancel.store(true, Ordering::Relaxed);
        assert!(matches!(wait(&job).unwrap_err(), ExportError::Cancelled));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn failed_video_publication_preserves_destination_and_cleans_temporary_file() {
        let (dir, exe) = fake("output");
        let output = dir.join("film.mp4");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("keep"), b"original").unwrap();
        let mut project = Project::new();
        project.duration = 1.0;
        let job = start_export_with(project, output.clone(), exe);
        assert!(matches!(wait(&job).unwrap_err(), ExportError::Publish(_)));
        assert_eq!(fs::read(output.join("keep")).unwrap(), b"original");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn drains_large_stderr_and_reports_encoder_failure() {
        let (dir, exe) = fake("noisy_failure");
        let output = dir.join("film.mp4");
        let job = start_export_with(Project::new(), output.clone(), exe);
        assert!(wait(&job).is_err());
        assert!(!output.exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}

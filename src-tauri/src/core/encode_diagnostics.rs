//! Bounded, local diagnostics for real encodes (estimation samples are excluded).
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::sys::job_logs::Sink;

fn record(sink: Option<&Arc<Sink>>, message: std::fmt::Arguments) {
    if let Some(sink) = sink {
        sink.write(&message.to_string());
    } else {
        log::info!("{message}");
    }
}

#[derive(Default)]
pub(super) struct Output {
    sink: Option<Arc<Sink>>,
    progress: BTreeMap<String, String>,
    last_progress: Option<Instant>,
    stderr: std::collections::VecDeque<String>,
    stderr_lines: usize,
}

impl Output {
    pub fn stdout(&mut self, line: &str) {
        if let Some((key, value)) = line.trim().split_once('=') {
            if matches!(
                key,
                "frame"
                    | "fps"
                    | "total_size"
                    | "out_time"
                    | "speed"
                    | "dup_frames"
                    | "drop_frames"
                    | "progress"
            ) {
                self.progress
                    .insert(key.to_string(), value.chars().take(256).collect());
                self.last_progress = Some(Instant::now());
            }
        }
    }

    pub fn stderr(&mut self, id: &str, line: &str) {
        let line: String = line.trim_end().chars().take(2048).collect();
        if self.stderr_lines < 80 {
            record(
                self.sink.as_ref(),
                format_args!("[encode {id}] ffmpeg: {line}"),
            );
        } else if self.stderr_lines == 80 {
            record(
                self.sink.as_ref(),
                format_args!(
                    "[encode {id}] further stderr kept as a bounded tail for the final record"
                ),
            );
        }
        self.stderr_lines += 1;
        if self.stderr.len() == 40 {
            self.stderr.pop_front();
        }
        self.stderr.push_back(line);
    }
}

pub(super) struct Diagnostics {
    pub output: Arc<Mutex<Output>>,
    id: String,
    pid: u32,
    started: Instant,
    heartbeat: tokio::task::JoinHandle<()>,
    finished: bool,
}

impl Diagnostics {
    pub fn start(id: String, pid: u32, command: &str, sink: Option<Arc<Sink>>) -> Self {
        let interval = sink.as_ref().map(|s| s.interval_seconds).unwrap_or(30);
        record(
            sink.as_ref(),
            format_args!(
                "[encode {id}] started pid={pid} logical_cpus={} command={command}",
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1)
            ),
        );
        let output = Arc::new(Mutex::new(Output {
            sink,
            ..Output::default()
        }));
        let started = Instant::now();
        let heartbeat = tokio::spawn({
            let output = output.clone();
            let id = id.clone();
            async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(interval)).await;
                    snapshot(&id, pid, started, &output, "running");
                }
            }
        });
        Self {
            output,
            id,
            pid,
            started,
            heartbeat,
            finished: false,
        }
    }

    pub fn finish(&mut self, status: &str) {
        self.finished = true;
        self.heartbeat.abort();
        snapshot(&self.id, self.pid, self.started, &self.output, status);
        if let Ok(output) = self.output.lock() {
            record(
                output.sink.as_ref(),
                format_args!(
                    "[encode {}] final stderr tail ({} lines total):\n{}",
                    self.id,
                    output.stderr_lines,
                    output.stderr.iter().cloned().collect::<Vec<_>>().join("\n")
                ),
            );
        }
        log::logger().flush();
    }
}

impl Drop for Diagnostics {
    fn drop(&mut self) {
        if !self.finished {
            self.finish("interrupted");
        }
    }
}

fn snapshot(id: &str, pid: u32, started: Instant, output: &Mutex<Output>, status: &str) {
    if let Ok(output) = output.lock() {
        record(output.sink.as_ref(), format_args!("[encode {id}] status={status} pid={pid} elapsed_s={:.1} progress_age_s={:.1} progress={:?} resources={}",
            started.elapsed().as_secs_f64(), output.last_progress.unwrap_or(started).elapsed().as_secs_f64(), output.progress, resources(pid)));
    }
}

#[cfg(not(windows))]
fn resources(_pid: u32) -> String {
    "OS resource counters unavailable on this platform".to_string()
}

#[cfg(windows)]
fn resources(pid: u32) -> String {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::{
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
            Threading::{
                GetProcessIoCounters, GetProcessTimes, OpenProcess, IO_COUNTERS,
                PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
            },
        },
    };
    // Read-only counters; unsupported or exited processes must not affect compression.
    unsafe {
        let mut system: MEMORYSTATUSEX = std::mem::zeroed();
        system.dwLength = std::mem::size_of_val(&system) as u32;
        let mut values = Vec::new();
        if GlobalMemoryStatusEx(&mut system) != 0 {
            values.push(format!(
                "system_ram_load_pct={} available_ram_bytes={}",
                system.dwMemoryLoad, system.ullAvailPhys
            ));
        }
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            values.push("process_counters_unavailable".to_string());
            return values.join(" ");
        }
        let mut created: FILETIME = std::mem::zeroed();
        let mut exited: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        if GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0 {
            let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
            values.push(format!(
                "cpu_total_s={:.3}",
                (ticks(kernel) + ticks(user)) as f64 / 10_000_000.0
            ));
        }
        let mut io: IO_COUNTERS = std::mem::zeroed();
        if GetProcessIoCounters(handle, &mut io) != 0 {
            values.push(format!(
                "read_bytes={} write_bytes={}",
                io.ReadTransferCount, io.WriteTransferCount
            ));
        }
        let mut memory: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        memory.cb = std::mem::size_of_val(&memory) as u32;
        if GetProcessMemoryInfo(handle, &mut memory, std::mem::size_of_val(&memory) as u32) != 0 {
            values.push(format!(
                "working_set_bytes={} peak_working_set_bytes={} page_faults={}",
                memory.WorkingSetSize, memory.PeakWorkingSetSize, memory.PageFaultCount
            ));
        }
        CloseHandle(handle);
        values.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_log_retains_progress_and_errors_after_cancellation() {
        let path = std::env::temp_dir().join(format!("compresso-encode-{}.log", nanoid::nanoid!()));
        let sink = Arc::new(Sink::create(&path, 600).unwrap());
        let mut diagnostic = Diagnostics::start(
            "file-test".to_string(),
            std::process::id(),
            "ffmpeg command",
            Some(sink),
        );
        diagnostic.output.lock().unwrap().stdout("fps=2.5\n");
        diagnostic
            .output
            .lock()
            .unwrap()
            .stderr("file-test", "decoder warning");
        snapshot(
            &diagnostic.id,
            diagnostic.pid,
            diagnostic.started,
            &diagnostic.output,
            "running",
        );
        diagnostic.finish("cancelled");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("ffmpeg command"));
        assert!(text.contains("status=running"));
        assert!(text.contains("\"fps\": \"2.5\""));
        assert!(text.contains("decoder warning"));
        assert!(text.contains("status=cancelled"));
        drop(diagnostic);
        // Allow the aborted heartbeat to drop its last reference to the file on Windows.
        tokio::task::yield_now().await;
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn progress_and_stderr_are_bounded_and_keep_latest_values() {
        let mut output = Output::default();
        output.stdout("frame=20\n");
        output.stdout("frame=30\n");
        output.stdout("speed=0.01x\n");
        output.stdout("unrecognized=ignore\n");
        for i in 0..100 {
            output.stderr("test", &format!("line {i}"));
        }
        assert_eq!(output.progress.len(), 2);
        assert_eq!(output.progress["frame"], "30");
        assert!(output.last_progress.is_some());
        assert_eq!(output.stderr.len(), 40);
        assert_eq!(output.stderr.front().unwrap(), "line 60");
        assert_eq!(output.stderr.back().unwrap(), "line 99");
        output.stderr("test", &"é".repeat(3000));
        assert_eq!(output.stderr.back().unwrap().chars().count(), 2048);
    }

    #[cfg(windows)]
    #[test]
    fn windows_resources_can_read_this_process() {
        let counters = resources(std::process::id());
        assert!(counters.contains("cpu_total_s="), "{counters}");
        assert!(counters.contains("working_set_bytes="), "{counters}");
        assert!(counters.contains("read_bytes="), "{counters}");
        assert!(counters.contains("system_ram_load_pct="), "{counters}");
    }

    #[tokio::test]
    async fn cancellation_preserves_progress_and_flushes_a_final_record() {
        struct Capture(Mutex<Vec<String>>);
        impl log::Log for Capture {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, record: &log::Record) {
                self.0.lock().unwrap().push(record.args().to_string());
            }
            fn flush(&self) {
                self.0.lock().unwrap().push("FLUSHED".to_string());
            }
        }
        static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));
        log::set_logger(&CAPTURE).unwrap();
        log::set_max_level(log::LevelFilter::Info);
        let mut diagnostic = Diagnostics::start(
            "cancel-test".to_string(),
            std::process::id(),
            "ffmpeg test",
            None,
        );
        diagnostic.output.lock().unwrap().stdout("frame=74\n");
        diagnostic
            .output
            .lock()
            .unwrap()
            .stderr("cancel-test", "source: 8K HEVC HDR");
        snapshot(
            &diagnostic.id,
            diagnostic.pid,
            diagnostic.started,
            &diagnostic.output,
            "running",
        );
        diagnostic.finish("cancelled");
        tokio::task::yield_now().await;
        assert!(diagnostic.heartbeat.is_finished());
        drop(diagnostic);
        #[cfg(windows)]
        {
            use std::{
                io::{BufRead, BufReader},
                process::{Command, Stdio},
            };
            let ffmpeg = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("bin/compresso_ffmpeg-x86_64-pc-windows-msvc.exe");
            let mut child = Command::new(ffmpeg)
                .args([
                    "-hide_banner",
                    "-re",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=64x64:rate=30",
                    "-t",
                    "60",
                    "-progress",
                    "pipe:1",
                    "-nostats",
                    "-f",
                    "null",
                    "-",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut live = Diagnostics::start(
                "live-cancel-test".to_string(),
                child.id(),
                "ffmpeg synthetic source",
                None,
            );
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();
            let output = live.output.clone();
            let stdout_reader = std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    output.lock().unwrap().stdout(&line);
                }
            });
            let output = live.output.clone();
            let stderr_reader = std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    output.lock().unwrap().stderr("live-cancel-test", &line);
                }
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while live.output.lock().unwrap().last_progress.is_none() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            snapshot(&live.id, live.pid, live.started, &live.output, "running");
            child.kill().unwrap();
            child.wait().unwrap();
            stdout_reader.join().unwrap();
            stderr_reader.join().unwrap();
            live.finish("cancelled");
            let output = live.output.lock().unwrap();
            assert!(output.progress["frame"].trim().parse::<u64>().unwrap() > 0);
            assert!(output.stderr_lines > 0);
        }
        let records = CAPTURE.0.lock().unwrap().join("\n");
        assert!(records.contains("status=running"));
        assert!(records.contains("status=cancelled"));
        assert!(records.contains("\"frame\": \"74\""));
        assert!(records.contains("source: 8K HEVC HDR"));
        assert!(records.contains("final stderr tail"));
        assert!(records.contains("FLUSHED"));
        assert!(!records.contains("status=interrupted"));
    }
}

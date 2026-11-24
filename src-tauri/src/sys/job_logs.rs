//! One diagnostic file per input, shared by its encoder attempts and size retries.
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::Manager;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub interval_seconds: u64,
    pub kept_jobs: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval_seconds: 30,
            kept_jobs: 10,
        }
    }
}

impl Settings {
    fn validate(&self) -> Result<(), String> {
        if ![30, 60, 300, 600].contains(&self.interval_seconds)
            || !(1..=1000).contains(&self.kept_jobs)
        {
            return Err("Invalid job log settings".to_string());
        }
        Ok(())
    }
}

struct Store {
    dir: PathBuf,
    settings: Settings,
    active: HashSet<PathBuf>,
}

impl Store {
    fn update(&mut self, settings: Settings) -> Result<(), String> {
        settings.validate()?;
        let bytes = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
        let pending = self.dir.join("settings.pending");
        std::fs::write(&pending, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(pending, self.dir.join("settings.json")).map_err(|e| e.to_string())?;
        self.settings = settings;
        self.prune();
        Ok(())
    }

    fn open(dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let settings = std::fs::read(dir.join("settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
            .filter(|settings| settings.validate().is_ok())
            .unwrap_or_default();
        let store = Self {
            dir,
            settings,
            active: HashSet::new(),
        };
        store.prune();
        Ok(store)
    }

    fn prune(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut finished: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("Job_"))
                    && p.extension().is_some_and(|ext| ext == "log")
                    && !self.active.contains(p)
            })
            .collect();
        finished.sort();
        let remove = finished.len().saturating_sub(self.settings.kept_jobs);
        for path in finished.into_iter().take(remove) {
            if let Err(error) = std::fs::remove_file(&path) {
                log::warn!("[job logs] Could not remove {}: {error}", path.display());
            }
        }
    }
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn with_store<T>(
    app: &tauri::AppHandle,
    f: impl FnOnce(&mut Store) -> Result<T, String>,
) -> Result<T, String> {
    let mut store = STORE.lock().map_err(|e| e.to_string())?;
    if store.is_none() {
        *store = Some(Store::open(
            app.path()
                .app_log_dir()
                .map_err(|e| e.to_string())?
                .join("jobs"),
        )?);
    }
    f(store.as_mut().unwrap())
}

pub fn settings(app: &tauri::AppHandle) -> Result<Settings, String> {
    with_store(app, |store| Ok(store.settings))
}

pub fn set_settings(app: &tauri::AppHandle, settings: Settings) -> Result<(), String> {
    settings.validate()?;
    with_store(app, |store| store.update(settings))
}

pub struct Sink {
    file: Mutex<Option<File>>,
    pub interval_seconds: u64,
}

impl Sink {
    pub fn create(path: &std::path::Path, interval_seconds: u64) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            file: Mutex::new(Some(file)),
            interval_seconds,
        })
    }

    pub fn write(&self, message: &str) {
        if let Ok(mut file) = self.file.lock() {
            if let Some(file) = file.as_mut() {
                if let Err(error) = writeln!(file, "{} {message}", time::OffsetDateTime::now_utc())
                    .and_then(|_| file.flush())
                {
                    log::warn!("[job logs] Could not write diagnostics: {error}");
                }
            }
        }
    }
}

pub struct JobLog {
    pub sink: Arc<Sink>,
    path: PathBuf,
    finished: bool,
    started: std::time::Instant,
}

impl JobLog {
    pub fn start(
        app: &tauri::AppHandle,
        input: &str,
        video_id: &str,
        batch_id: Option<&str>,
    ) -> Option<Self> {
        let result = with_store(app, |store| {
            let name = format!(
                "Job_{:039}_{}.log",
                time::OffsetDateTime::now_utc().unix_timestamp_nanos(),
                nanoid::nanoid!()
            );
            let path = store.dir.join(name);
            let sink = Arc::new(Sink::create(&path, store.settings.interval_seconds)?);
            store.active.insert(path.clone());
            let job = Self {
                sink,
                path,
                finished: false,
                started: std::time::Instant::now(),
            };
            job.sink.write(&format!(
                "job started input={input:?} media={video_id} batch={batch_id:?} app_version={} interval_seconds={}",
                app.package_info().version, job.sink.interval_seconds,
            ));
            log::info!("[job logs] video={video_id} log={}", job.path.display());
            Ok(job)
        });
        match result {
            Ok(job) => Some(job),
            Err(error) => {
                log::warn!("[job logs] Could not start diagnostics: {error}");
                None
            }
        }
    }

    pub fn finish(&mut self, status: &str) {
        self.sink.write(&format!(
            "job status={status} elapsed_s={:.1}",
            self.started.elapsed().as_secs_f64()
        ));
        self.finished = true;
        if let Ok(mut file) = self.sink.file.lock() {
            file.take();
        }
        if let Ok(mut store) = STORE.lock() {
            if let Some(store) = store.as_mut() {
                store.active.remove(&self.path);
                store.prune();
            }
        }
    }
}

impl Drop for JobLog {
    fn drop(&mut self) {
        if !self.finished {
            self.finish("interrupted");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_intervals_and_retention() {
        for interval_seconds in [30, 60, 300, 600] {
            assert!(Settings {
                interval_seconds,
                kept_jobs: 10
            }
            .validate()
            .is_ok());
        }
        for settings in [
            Settings {
                interval_seconds: 0,
                kept_jobs: 10,
            },
            Settings {
                interval_seconds: 30,
                kept_jobs: 0,
            },
            Settings {
                interval_seconds: 60,
                kept_jobs: 1001,
            },
        ] {
            assert!(settings.validate().is_err());
        }
    }

    #[test]
    fn settings_survive_repeated_updates_and_failed_saves_do_not_change_them() {
        let dir =
            std::env::temp_dir().join(format!("compresso-job-settings-{}", nanoid::nanoid!()));
        let mut store = Store::open(dir.clone()).unwrap();
        for interval_seconds in [30, 60, 300, 600] {
            store
                .update(Settings {
                    interval_seconds,
                    kept_jobs: 10,
                })
                .unwrap();
            assert_eq!(
                Store::open(dir.clone()).unwrap().settings.interval_seconds,
                interval_seconds
            );
        }
        std::fs::create_dir(dir.join("settings.pending")).unwrap();
        assert!(store
            .update(Settings {
                interval_seconds: 30,
                kept_jobs: 5
            })
            .is_err());
        assert_eq!(store.settings.interval_seconds, 600);
        assert_eq!(Store::open(dir.clone()).unwrap().settings.kept_jobs, 10);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn one_job_file_contains_multiple_attempts_and_its_cancelled_result() {
        let dir = std::env::temp_dir().join(format!("compresso-job-sink-{}", nanoid::nanoid!()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Job_001.log");
        let file = File::create(&path).unwrap();
        let mut job = JobLog {
            sink: Arc::new(Sink {
                file: Mutex::new(Some(file)),
                interval_seconds: 60,
            }),
            path: path.clone(),
            finished: false,
            started: std::time::Instant::now(),
        };
        job.sink.write("attempt=1 encoder=GPU status=failed");
        job.sink.write("attempt=2 encoder=CPU frame=74");
        job.finish("cancelled");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("attempt=1"));
        assert!(text.contains("attempt=2"));
        assert!(text.contains("job status=cancelled"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        drop(job);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retention_counts_each_file_and_protects_active_jobs() {
        let dir = std::env::temp_dir().join(format!("compresso-job-logs-{}", nanoid::nanoid!()));
        let mut store = Store::open(dir.clone()).unwrap();
        store.settings.kept_jobs = 10;
        for i in 0..15 {
            std::fs::write(dir.join(format!("Job_{i:03}.log")), "same queue").unwrap();
        }
        let active = dir.join("Job_000.log");
        store.active.insert(active.clone());
        std::fs::write(dir.join("unrelated.log"), "keep").unwrap();
        store.prune();
        assert!(active.exists());
        assert!(!dir.join("Job_004.log").exists());
        assert!(dir.join("Job_005.log").exists());
        assert!(dir.join("unrelated.log").exists());
        store.active.remove(&active);
        store.prune();
        assert!(!active.exists());
        // A restart treats logs from interrupted processes as finished and enforces persisted retention.
        std::fs::write(
            dir.join("settings.json"),
            r#"{"intervalSeconds":600,"keptJobs":5}"#,
        )
        .unwrap();
        let restarted = Store::open(dir.clone()).unwrap();
        assert_eq!(restarted.settings.interval_seconds, 600);
        assert!(!dir.join("Job_009.log").exists());
        assert!(dir.join("Job_010.log").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

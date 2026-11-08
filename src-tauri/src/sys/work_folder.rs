use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::{AppHandle, Manager};
use tauri_plugin_fs::FsExt;

const MARKER: &str = ".compresso-work-folder";
const SIGNATURE: &[u8] = b"CompressO work folder v1\n";

pub struct WorkFolder {
    default: PathBuf,
    config_file: PathBuf,
    active_parent: Option<PathBuf>,
    configured_parent: Mutex<Option<PathBuf>>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkFolderInfo {
    current_folder: PathBuf,
    next_folder: PathBuf,
    is_custom: bool,
    restart_required: bool,
}

impl WorkFolder {
    fn load(app_data: &Path) -> Result<Self, String> {
        let config_file = app_data.join("work-folder.json");
        let parent: Option<PathBuf> = match fs::read(&config_file) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|err| err.to_string())?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.to_string()),
        };
        if parent.as_ref().is_some_and(|path| !path.is_absolute()) {
            return Err("The work folder must be an absolute path".into());
        }
        Ok(Self {
            default: app_data.join("assets"),
            config_file,
            active_parent: parent.clone(),
            configured_parent: Mutex::new(parent),
        })
    }

    fn path_for(&self, parent: Option<&Path>) -> PathBuf {
        parent
            .map(|path| path.join("CompressO-work").join("assets"))
            .unwrap_or_else(|| self.default.clone())
    }

    fn info(&self, parent: Option<&Path>) -> WorkFolderInfo {
        let current_folder = self.path_for(self.active_parent.as_deref());
        let next_folder = self.path_for(parent);
        WorkFolderInfo {
            restart_required: current_folder != next_folder,
            current_folder,
            next_folder,
            is_custom: parent.is_some(),
        }
    }

    fn ensure_current(&self) -> Result<PathBuf, String> {
        if let Some(parent) = &self.active_parent {
            return prepare_custom_directory(parent);
        }
        fs::create_dir_all(&self.default).map_err(|err| err.to_string())?;
        Ok(self.default.clone())
    }

    fn configure(&self, parent: Option<PathBuf>) -> Result<WorkFolderInfo, String> {
        let mut configured = self
            .configured_parent
            .lock()
            .map_err(|err| err.to_string())?;
        let directory = match &parent {
            Some(parent) => prepare_custom_directory(parent)?,
            None => {
                fs::create_dir_all(&self.default).map_err(|err| err.to_string())?;
                self.default.clone()
            }
        };
        // Fail before saving the preference when the folder cannot actually accept files.
        let probe = directory.join(format!(".write-test-{}", nanoid::nanoid!()));
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .map_err(|err| err.to_string())?;
        fs::remove_file(&probe).map_err(|err| err.to_string())?;
        let text = serde_json::to_string(&parent).map_err(|err| err.to_string())?;
        super::cli_settings::write_atomically(&self.config_file, &text)
            .map_err(|err| err.to_string())?;
        *configured = parent;
        Ok(self.info(configured.as_deref()))
    }
}

fn prepare_custom_directory(parent: &Path) -> Result<PathBuf, String> {
    if !parent.is_absolute() || !parent.is_dir() {
        return Err("Choose an existing folder with an absolute path".into());
    }
    let managed = parent.join("CompressO-work");
    fs::create_dir_all(&managed).map_err(|err| err.to_string())?;
    let marker = managed.join(MARKER);
    if marker.exists() {
        if fs::read(&marker).map_err(|err| err.to_string())? != SIGNATURE {
            return Err("This CompressO-work folder is not owned by CompressO".into());
        }
    } else {
        // Cache cleanup must never claim a folder already containing somebody else's files.
        if fs::read_dir(&managed)
            .map_err(|err| err.to_string())?
            .next()
            .is_some()
        {
            return Err("Choose a folder with an empty CompressO-work subfolder".into());
        }
        fs::write(&marker, SIGNATURE).map_err(|err| err.to_string())?;
    }
    let assets = managed.join("assets");
    fs::create_dir_all(&assets).map_err(|err| err.to_string())?;
    Ok(assets)
}

pub fn init(app: &AppHandle) -> Result<(), String> {
    let directory = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let state = WorkFolder::load(&directory)?;
    let active = state.path_for(state.active_parent.as_deref());
    app.fs_scope()
        .allow_directory(&active, true)
        .map_err(|err| err.to_string())?;
    app.asset_protocol_scope()
        .allow_directory(&active, true)
        .map_err(|err| err.to_string())?;
    app.manage(state);
    Ok(())
}

pub fn ensure_current(app: &AppHandle) -> Result<PathBuf, String> {
    app.state::<WorkFolder>().ensure_current()
}

#[tauri::command]
pub fn get_work_folder(app: AppHandle) -> Result<WorkFolderInfo, String> {
    let state = app.state::<WorkFolder>();
    let configured = state
        .configured_parent
        .lock()
        .map_err(|err| err.to_string())?;
    Ok(state.info(configured.as_deref()))
}

#[tauri::command]
pub fn set_work_folder(app: AppHandle, folder: Option<PathBuf>) -> Result<WorkFolderInfo, String> {
    app.state::<WorkFolder>().configure(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("compresso-work-test-{}", nanoid::nanoid!()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn changing_folder_waits_for_restart_and_preserves_old_outputs() {
        let fixture = Fixture::new();
        let app_data = fixture.0.join("profile");
        let state = WorkFolder::load(&app_data).unwrap();
        let old = state.ensure_current().unwrap();
        fs::write(old.join("unsaved.mp4"), b"keep").unwrap();
        let info = state.configure(Some(fixture.0.clone())).unwrap();
        assert!(info.restart_required);
        assert_eq!(state.ensure_current().unwrap(), old);
        assert_eq!(fs::read(old.join("unsaved.mp4")).unwrap(), b"keep");
        let restarted = WorkFolder::load(&app_data).unwrap();
        assert_eq!(restarted.ensure_current().unwrap(), info.next_folder);
        assert!(!restarted.info(Some(&fixture.0)).restart_required);
        let reset = restarted.configure(None).unwrap();
        assert!(reset.restart_required);
        assert_eq!(
            WorkFolder::load(&app_data)
                .unwrap()
                .ensure_current()
                .unwrap(),
            old
        );
    }

    #[test]
    fn never_claims_an_existing_nonempty_folder_or_changes_settings_after_failure() {
        let fixture = Fixture::new();
        let state = WorkFolder::load(&fixture.0.join("profile")).unwrap();
        state.configure(None).unwrap();
        let managed = fixture.0.join("CompressO-work");
        fs::create_dir(&managed).unwrap();
        fs::write(managed.join("important.txt"), b"keep").unwrap();
        assert!(state.configure(Some(fixture.0.clone())).is_err());
        assert_eq!(fs::read(managed.join("important.txt")).unwrap(), b"keep");
        assert_eq!(fs::read(&state.config_file).unwrap(), b"null");
        assert!(state.configure(Some(PathBuf::from("relative"))).is_err());
        assert!(state.configure(Some(fixture.0.join("missing"))).is_err());
    }

    #[test]
    fn missing_custom_drive_does_not_fall_back_to_the_profile() {
        let fixture = Fixture::new();
        let app_data = fixture.0.join("profile");
        let parent = fixture.0.join("drive");
        fs::create_dir(&parent).unwrap();
        WorkFolder::load(&app_data)
            .unwrap()
            .configure(Some(parent.clone()))
            .unwrap();
        let offline = fixture.0.join("offline");
        fs::rename(&parent, &offline).unwrap();
        let state = WorkFolder::load(&app_data).unwrap();
        assert!(state.ensure_current().is_err());
        assert!(!state.default.exists());
        // The settings can still be opened and reset, even though the old drive is absent.
        assert!(state.configure(None).unwrap().restart_required);
    }
}

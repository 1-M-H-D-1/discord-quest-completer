use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;
use tauri::{path::BaseDirectory, Manager};

use crate::steam_metadata::Plan;
const MARKER: &str = ".dqc-steam-runner.json";

#[derive(Default)]
pub struct SteamRunner(pub Mutex<HashMap<String, Child>>);

impl SteamRunner {
    pub fn shutdown(&self) {
        if let Ok(mut child) = self.0.lock() {
            for process in child.values_mut() {
                let _ = process.kill();
                let _ = process.wait();
            }
            child.clear();
        }
    }
}

#[derive(serde::Serialize)]
pub struct Status {
    installed: bool,
    running: bool,
    message: String,
}

struct Files {
    game: Plan,
    dir: PathBuf,
    manifest: PathBuf,
}

fn plain_path(path: &Path) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(format!(
                    "Linked paths are not supported: {}",
                    path.display()
                ));
            }
        }
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Linked paths are not supported: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn files(library: &Path, game: Plan) -> Result<Files, String> {
    game.validate()?;
    let library = library.canonicalize().map_err(|e| e.to_string())?;
    let steamapps = library.join("steamapps");
    let common = steamapps.join("common");
    if !common.is_dir() {
        return Err("Select a Steam library folder containing steamapps/common.".into());
    }
    let result = Files {
        game: game.clone(),
        dir: common.join(&game.folder),
        manifest: steamapps.join(format!("appmanifest_{}.acf", game.steam_id)),
    };
    for path in [
        &steamapps,
        &common,
        &result.dir,
        &result.manifest,
        &result.dir.join(&game.executable),
        &result.dir.join(MARKER),
    ] {
        plain_path(path)?;
    }
    for dir in directories(&result) {
        plain_path(&dir)?;
    }
    Ok(result)
}

fn owned(files: &Files, runner: &[u8]) -> bool {
    fs::read(files.dir.join(MARKER))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Plan>(&bytes).ok())
        .as_ref()
        == Some(&files.game)
        && fs::read(files.dir.join(&files.game.executable))
            .ok()
            .as_deref()
            == Some(runner)
}

fn directories(files: &Files) -> Vec<PathBuf> {
    let mut dirs = vec![];
    let mut current = files
        .dir
        .join(&files.game.executable)
        .parent()
        .unwrap()
        .to_path_buf();
    loop {
        dirs.push(current.clone());
        if current == files.dir {
            break;
        }
        current = current.parent().unwrap().to_path_buf();
    }
    dirs
}

fn verify_contents(files: &Files) -> Result<(), String> {
    let allowed_dirs = directories(files);
    let exe = files.dir.join(&files.game.executable);
    let marker = files.dir.join(MARKER);
    for dir in &allowed_dirs {
        plain_path(dir)?;
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            plain_path(&path)?;
            if path != exe && path != marker && !allowed_dirs.contains(&path) {
                return Err(
                    "The game folder contains additional files. Nothing was removed.".into(),
                );
            }
        }
    }
    Ok(())
}

fn write_new(path: &Path, data: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {}", path.display(), e))?;
    if let Err(error) = file.write_all(data).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error.to_string());
    }
    Ok(())
}

fn setup(files: &Files, runner: &[u8]) -> Result<(), String> {
    if files.manifest.exists() || files.dir.exists() {
        return Err("Game files already exist in this library. Nothing was overwritten. Use Remove for a fallback previously created by this app.".into());
    }
    fs::create_dir(&files.dir).map_err(|e| e.to_string())?;
    let mut wrote_exe = false;
    let mut wrote_marker = false;
    let result = (|| {
        let executable = files.dir.join(&files.game.executable);
        fs::create_dir_all(executable.parent().unwrap()).map_err(|e| e.to_string())?;
        write_new(&executable, runner)?;
        wrote_exe = true;
        write_new(
            &files.dir.join(MARKER),
            &serde_json::to_vec(&files.game).map_err(|e| e.to_string())?,
        )?;
        wrote_marker = true;
        write_new(&files.manifest, files.game.manifest().as_bytes())
    })();
    if result.is_err() {
        if wrote_exe {
            let _ = fs::remove_file(files.dir.join(&files.game.executable));
        }
        if wrote_marker {
            let _ = fs::remove_file(files.dir.join(MARKER));
        }
        for dir in directories(files) {
            let _ = fs::remove_dir(dir);
        }
    }
    result
}

fn remove(files: &Files, runner: &[u8]) -> Result<(), String> {
    if !owned(files, runner) {
        return Err(
            "These files are not an unchanged runner created by this app. Nothing was removed."
                .into(),
        );
    }
    if files.manifest.exists()
        && fs::read(&files.manifest).map_err(|e| e.to_string())? != files.game.manifest().as_bytes()
    {
        return Err("Steam or another program changed the manifest. Nothing was removed; inspect the files manually.".into());
    }
    verify_contents(files)?;
    if files.manifest.exists() {
        fs::remove_file(&files.manifest).map_err(|e| e.to_string())?;
    }
    fs::remove_file(files.dir.join(&files.game.executable)).map_err(|e| e.to_string())?;
    fs::remove_file(files.dir.join(MARKER)).map_err(|e| e.to_string())?;
    for dir in directories(files) {
        fs::remove_dir(dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn require_steam_closed() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = Command::new("tasklist.exe")
            .args(["/FI", "IMAGENAME eq steam.exe", "/FO", "CSV", "/NH"])
            .creation_flags(0x08000000)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err("Could not check whether Steam is closed.".into());
        }
        if String::from_utf8_lossy(&output.stdout)
            .to_lowercase()
            .contains("\"steam.exe\"")
        {
            return Err(
                "Exit Steam from its tray menu before setting up or removing the fallback.".into(),
            );
        }
        Ok(())
    }
    #[cfg(not(windows))]
    Err("The Steam fallback currently supports Windows only.".into())
}

#[tauri::command]
pub fn steam_fallback_action(
    handle: tauri::AppHandle,
    state: tauri::State<'_, SteamRunner>,
    plan: Plan,
    library: String,
    action: String,
) -> Result<Status, String> {
    if !cfg!(target_os = "windows") {
        return Err("The Steam fallback currently supports Windows only.".into());
    }
    plan.validate()?;
    let game_id = plan.steam_id.clone();
    let mut children = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(process) = children.get_mut(&game_id) {
        if process.try_wait().map_err(|e| e.to_string())?.is_some() {
            children.remove(&game_id);
        }
    }
    // Stop only the process this app launched, even if the selected library changed.
    if action == "stop" {
        if let Some(process) = children.get_mut(&game_id) {
            process.kill().map_err(|e| e.to_string())?;
            process.wait().map_err(|e| e.to_string())?;
            children.remove(&game_id);
        }
        return Ok(Status {
            installed: false,
            running: false,
            message: "Runner stopped.".into(),
        });
    }
    let files = files(Path::new(&library), plan)?;
    // The current repository bundles runners under resources/; older portable
    // builds used data/. Accept both without changing upstream resource settings.
    let source = ["resources/src-win.exe", "data/src-win.exe"]
        .iter()
        .filter_map(|name| handle.path().resolve(name, BaseDirectory::Resource).ok())
        .find(|path| path.is_file())
        .ok_or("Cannot find the bundled Windows runner.")?;
    let runner = fs::read(source).map_err(|e| format!("Cannot read bundled runner: {}", e))?;
    let message = match action.as_str() {
        "setup" => {
            require_steam_closed()?;
            setup(&files, &runner)?;
            "Setup complete. Restart Steam and Discord, then click Launch Steam runner."
        }
        "launch" => {
            if children.contains_key(&game_id) {
                return Err("The Steam runner is already running.".into());
            }
            if !owned(&files, &runner) || !files.manifest.is_file() {
                return Err("Set up the fallback in this library first. The app will not launch an existing game installation.".into());
            }
            children.insert(
                game_id.clone(),
                Command::new(files.dir.join(&files.game.executable))
                    .args(["--title", &files.game.name])
                    .current_dir(&files.dir)
                    .spawn()
                    .map_err(|e| e.to_string())?,
            );
            "Runner launched. Check Discord manually; recognition is not verified by this app."
        }
        "remove" => {
            if children.contains_key(&game_id) {
                return Err("Stop the Steam runner before removing it.".into());
            }
            require_steam_closed()?;
            remove(&files, &runner)?;
            "Steam fallback removed."
        }
        "status" => "",
        _ => return Err("Unknown Steam fallback action.".into()),
    };
    Ok(Status {
        installed: owned(&files, &runner) && files.manifest.is_file(),
        running: children.contains_key(&game_id),
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan(id: &str, folder: &str) -> Plan {
        Plan {
            steam_id: id.into(),
            name: "Example".into(),
            folder: folder.into(),
            executable: "Game/Binaries/game.exe".into(),
        }
    }
    fn fixture() -> (PathBuf, Files) {
        let root = std::env::temp_dir().join(format!(
            "dqc-steam-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("steamapps/common")).unwrap();
        let paths = files(&root, plan("123", "Example")).unwrap();
        (root, paths)
    }
    fn cleanup(root: PathBuf) {
        let root = root.canonicalize().unwrap();
        assert_eq!(
            root.parent(),
            Some(std::env::temp_dir().canonicalize().unwrap().as_path())
        );
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dqc-steam-test-"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn nested_setup_and_removal_preserve_other_games() {
        let (root, first) = fixture();
        let second = files(&root, plan("456", "Second")).unwrap();
        setup(&first, b"runner").unwrap();
        setup(&second, b"runner").unwrap();
        assert!(owned(&first, b"runner"));
        assert!(first.dir.join("Game/Binaries/game.exe").is_file());
        remove(&first, b"runner").unwrap();
        assert!(!first.dir.exists());
        assert!(!first.manifest.exists());
        assert!(owned(&second, b"runner"));
        remove(&second, b"runner").unwrap();
        cleanup(root);
    }
    #[test]
    fn existing_game_and_manifest_are_never_overwritten() {
        let (root, paths) = fixture();
        fs::write(&paths.manifest, "real installation").unwrap();
        assert!(setup(&paths, b"runner").is_err());
        assert_eq!(
            fs::read_to_string(&paths.manifest).unwrap(),
            "real installation"
        );
        fs::remove_file(&paths.manifest).unwrap();
        fs::create_dir(&paths.dir).unwrap();
        fs::write(paths.dir.join("save"), "keep").unwrap();
        assert!(setup(&paths, b"runner").is_err());
        assert!(paths.dir.join("save").is_file());
        cleanup(root);
    }
    #[test]
    fn refuses_modified_manifest_runner_and_additional_nested_files() {
        let (root, paths) = fixture();
        setup(&paths, b"runner").unwrap();
        fs::write(&paths.manifest, "changed").unwrap();
        assert!(remove(&paths, b"runner").is_err());
        fs::write(&paths.manifest, paths.game.manifest()).unwrap();
        let extra = paths.dir.join("Game/save.dat");
        fs::write(&extra, "keep").unwrap();
        assert!(remove(&paths, b"runner").is_err());
        fs::remove_file(extra).unwrap();
        fs::write(paths.dir.join(&paths.game.executable), "real game").unwrap();
        assert!(remove(&paths, b"runner").is_err());
        assert!(paths.manifest.exists());
        cleanup(root);
    }
    #[test]
    fn invalid_library_and_traversal_are_rejected() {
        let (root, paths) = fixture();
        assert!(files(&root.join("steamapps"), paths.game.clone()).is_err());
        let mut bad = paths.game.clone();
        bad.executable = "../../outside.exe".into();
        assert!(files(&root, bad).is_err());
        cleanup(root);
    }
}

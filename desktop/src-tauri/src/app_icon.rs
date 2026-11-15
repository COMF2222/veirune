use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::{AppHandle, Manager};

const DEFAULT: u8 = 9;
const PNGS: [&[u8]; 12] = [
    include_bytes!("../resources/app-icons/01.png"),
    include_bytes!("../resources/app-icons/02.png"),
    include_bytes!("../resources/app-icons/03.png"),
    include_bytes!("../resources/app-icons/04.png"),
    include_bytes!("../resources/app-icons/05.png"),
    include_bytes!("../resources/app-icons/06.png"),
    include_bytes!("../resources/app-icons/07.png"),
    include_bytes!("../resources/app-icons/08.png"),
    include_bytes!("../resources/app-icons/09.png"),
    include_bytes!("../resources/app-icons/10.png"),
    include_bytes!("../resources/app-icons/11.png"),
    include_bytes!("../resources/app-icons/12.png"),
];
#[cfg(windows)]
const ICOS: [&[u8]; 12] = [
    include_bytes!("../resources/app-icons/01.ico"),
    include_bytes!("../resources/app-icons/02.ico"),
    include_bytes!("../resources/app-icons/03.ico"),
    include_bytes!("../resources/app-icons/04.ico"),
    include_bytes!("../resources/app-icons/05.ico"),
    include_bytes!("../resources/app-icons/06.ico"),
    include_bytes!("../resources/app-icons/07.ico"),
    include_bytes!("../resources/app-icons/08.ico"),
    include_bytes!("../resources/app-icons/09.ico"),
    include_bytes!("../resources/app-icons/10.ico"),
    include_bytes!("../resources/app-icons/11.ico"),
    include_bytes!("../resources/app-icons/12.ico"),
];

#[derive(Serialize, Deserialize)]
pub struct IconState {
    selected: u8,
    supported: bool,
    note: Option<String>,
}
fn valid(id: u8) -> Result<usize, String> {
    if (1..=12).contains(&id) {
        Ok(usize::from(id - 1))
    } else {
        Err("Choose an icon from 1 to 12".into())
    }
}
fn current(app: &AppHandle) -> u8 {
    app.path()
        .app_data_dir()
        .ok()
        .and_then(|dir| std::fs::read(dir.join("app-icon.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<IconState>(&bytes).ok())
        .filter(|state| valid(state.selected).is_ok())
        .map_or(DEFAULT, |state| state.selected)
}
fn prepare(app: &AppHandle, id: u8) -> Result<std::path::PathBuf, String> {
    let index = valid(id)?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("app-icons");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("icon-{id:02}.png"));
    std::fs::write(&path, PNGS[index]).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(ICOS[index]));
        let icon = dir.join(format!("icon-{id:02}-{}.ico", &digest[..12]));
        std::fs::write(&icon, ICOS[index]).map_err(|e| e.to_string())?;
        return Ok(icon);
    }
    #[cfg(not(windows))]
    {
        Ok(path)
    }
}
fn window_icons(app: &AppHandle, id: u8) -> Result<(), String> {
    let image = tauri::image::Image::from_bytes(PNGS[valid(id)?]).map_err(|e| e.to_string())?;
    for window in app.webview_windows().values() {
        window.set_icon(image.clone()).map_err(|e| e.to_string())?;
    }
    if let Some(tray) = app.tray_by_id("main-tray") {
        tray.set_icon(Some(image)).map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[cfg(windows)]
fn system_icon(path: &Path, create: bool) -> Result<Option<String>, String> {
    use base64::Engine;
    use std::{
        io::Write,
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    let script = include_str!("app_icon_windows.ps1");
    let encoded: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let input = serde_json::json!({"executable": executable, "icon": path, "create": create});
    let system_root =
        std::env::var_os("SystemRoot").ok_or("Windows system directory unavailable")?;
    let shell = std::path::PathBuf::from(system_root)
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut child = Command::new(shell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &base64::engine::general_purpose::STANDARD.encode(encoded),
        ])
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let input_result = child
        .stdin
        .take()
        .ok_or("Icon helper input unavailable")?
        .write_all(input.to_string().as_bytes());
    if input_result.is_err() {
        let _ = child.kill();
    }
    let started = std::time::Instant::now();
    let result = loop {
        if let Some(result) = child.try_wait().map_err(|e| e.to_string())? {
            break result;
        }
        if started.elapsed() >= std::time::Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Shortcut icon change timed out".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    input_result.map_err(|e| e.to_string())?;
    if !result.success() {
        return Err("Could not update the application shortcut".into());
    }
    Ok(None)
}
#[cfg(target_os = "macos")]
fn system_icon(path: &Path, _create: bool) -> Result<Option<String>, String> {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage, NSWorkspace, NSWorkspaceIconCreationOptions};
    use objc2_foundation::{NSBundle, NSString};
    let main = MainThreadMarker::new().ok_or("Icon change needs the main thread")?;
    let filename = NSString::from_str(path.to_str().ok_or("Invalid icon path")?);
    let image = NSImage::initWithContentsOfFile(NSImage::alloc(), &filename)
        .ok_or("Could not load icon")?;
    let application = NSApplication::sharedApplication(main);
    // SAFETY: AppKit runs on the main thread and the retained image lives through the call.
    unsafe {
        application.setApplicationIconImage(Some(&image));
    }
    let writable = NSWorkspace::sharedWorkspace().setIcon_forFile_options(
        Some(&image),
        &NSBundle::mainBundle().bundlePath(),
        NSWorkspaceIconCreationOptions::empty(),
    );
    Ok((!writable).then(|| "finder-readonly".into()))
}
#[cfg(not(any(windows, target_os = "macos")))]
fn system_icon(_path: &Path, _create: bool) -> Result<Option<String>, String> {
    Ok(Some("window-tray-only".into()))
}

pub fn restore(app: &AppHandle) {
    let id = current(app);
    #[cfg(windows)]
    {
        if let Err(error) = window_icons(app, id) {
            log::warn!("Could not restore app icon: {error}");
        }
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = prepare(&app, id).and_then(|path| system_icon(&path, false)) {
                log::warn!("Could not restore shortcut icon: {error}");
            }
        });
    }
    #[cfg(not(windows))]
    {
        let result = prepare(app, id).and_then(|path| {
            window_icons(app, id)?;
            system_icon(&path, false).map(|_| ())
        });
        if let Err(error) = result {
            log::warn!("Could not restore app icon: {error}");
        }
    }
}

#[tauri::command]
pub async fn application_icon(app: AppHandle, id: Option<u8>) -> Result<IconState, String> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _guard = LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let Some(id) = id else {
        return Ok(IconState {
            selected: current(&app),
            supported: true,
            note: None,
        });
    };
    valid(id)?;
    let handle = app.clone();
    let path = tauri::async_runtime::spawn_blocking(move || prepare(&handle, id))
        .await
        .map_err(|e| e.to_string())??;
    #[cfg(target_os = "macos")]
    let note = {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let handle = app.clone();
        app.run_on_main_thread(move || {
            let _ = sender.send(window_icons(&handle, id).and_then(|_| system_icon(&path, true)));
        })
        .map_err(|e| e.to_string())?;
        receiver.await.map_err(|e| e.to_string())??
    };
    #[cfg(not(target_os = "macos"))]
    let note = {
        let handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let note = system_icon(&path, true)?;
            window_icons(&handle, id)?;
            Ok::<_, String>(note)
        })
        .await
        .map_err(|e| e.to_string())??
    };
    let state = IconState {
        selected: id,
        supported: true,
        note,
    };
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let temporary = dir.join("app-icon.json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec(&state).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, dir.join("app-icon.json")).map_err(|e| e.to_string())?;
    Ok(state)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn choices_are_bounded_and_default_exists() {
        assert!(valid(0).is_err());
        assert!(valid(13).is_err());
        for id in 1..=12 {
            assert!(PNGS[valid(id).unwrap()].starts_with(b"\x89PNG"));
        }
        assert_eq!(valid(DEFAULT), Ok(8));
    }
}

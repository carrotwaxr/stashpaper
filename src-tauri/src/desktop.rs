//! Putting images on the desktop. Most desktops go through the `wallpaper` crate
//! (plus gsettings for GNOME's quirks); Windows sets one image per monitor
//! natively; wlroots compositors (sway, Hyprland, ...) get one managed `swaybg`.

use crate::error::AppError;
use crate::settings::FitMode;
use crate::MonitorInfo;
use std::path::{Path, PathBuf};

/// Where a rotation's files go.
#[derive(Debug)]
pub enum Placement<'a> {
    /// One image on every monitor, fit per the Fit Mode setting
    Single(&'a Path),
    /// One composite image spanning all monitors
    Spanned(&'a Path),
    /// A different image on each monitor (only Windows asks for this)
    #[cfg_attr(not(windows), allow(dead_code))]
    PerMonitor(&'a [(MonitorInfo, PathBuf)]),
}

/// How a desktop handles a different image per monitor.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PerMonitor {
    /// It takes one image per monitor directly (Windows)
    #[cfg_attr(not(windows), allow(dead_code))]
    Native,
    /// It can span one composite image across all monitors (GNOME and friends)
    Spanned,
    /// It can't; every monitor gets the same image
    Unsupported,
}

// On Windows only the Windows backend is ever chosen
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
enum Backend {
    /// The `wallpaper` crate, for a desktop it knows
    Crate { spans: bool },
    /// A wlroots compositor: swaybg, one process at a time
    Swaybg,
    /// Another X11 window manager: feh sets the root window's background
    Feh,
    /// IDesktopWallpaper
    #[cfg(windows)]
    Windows,
}

/// Which backend a Linux session gets, from `XDG_CURRENT_DESKTOP` and whether it
/// runs on Wayland. What goes to the crate matches the crate's own rules
/// exactly (its linux/mod.rs), so it only gets desktops it can handle.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn linux_backend(current_desktop: &str, wayland: bool) -> Backend {
    let d = current_desktop;
    let gnome_like = d.contains("GNOME") || d == "Unity" || d == "Pantheon";
    if gnome_like || matches!(d, "X-Cinnamon" | "MATE" | "Deepin") {
        return Backend::Crate { spans: true };
    }
    if matches!(d, "KDE" | "LXDE" | "XFCE") {
        // The crate's "span" is crop per screen on KDE and zoomed on XFCE, so a
        // composite would repeat on each screen
        return Backend::Crate { spans: false };
    }
    // For anything else the crate would start an unmanaged swaybg (even on
    // X11, where it can't work), so pick the right tool here
    if wayland {
        Backend::Swaybg
    } else {
        Backend::Feh
    }
}

pub struct Desktop {
    backend: Backend,
    /// swaybg or feh; tests point this at a fake
    program: PathBuf,
    /// The swaybg we started, and where its pid is kept across restarts
    swaybg: Option<std::process::Child>,
    swaybg_pid_file: Option<PathBuf>,
}

impl Desktop {
    /// The backend for this session. `state_dir` is where a swaybg pid is kept.
    pub fn detect(state_dir: Option<&Path>) -> Self {
        #[cfg(windows)]
        let backend = Backend::Windows;
        #[cfg(target_os = "macos")]
        // macOS sets the same picture on every screen; no spanning
        let backend = Backend::Crate { spans: false };
        #[cfg(all(unix, not(target_os = "macos")))]
        let backend = linux_backend(
            &std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            std::env::var_os("WAYLAND_DISPLAY").is_some()
                || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland"),
        );
        let program = PathBuf::from(if backend == Backend::Feh {
            "feh"
        } else {
            "swaybg"
        });
        Self {
            backend,
            program,
            swaybg: None,
            swaybg_pid_file: state_dir.map(|dir| dir.join("swaybg.pid")),
        }
    }

    pub fn per_monitor(&self) -> PerMonitor {
        match self.backend {
            Backend::Crate { spans: true } => PerMonitor::Spanned,
            Backend::Crate { spans: false } | Backend::Swaybg | Backend::Feh => {
                PerMonitor::Unsupported
            }
            #[cfg(windows)]
            Backend::Windows => PerMonitor::Native,
        }
    }

    pub fn apply(&mut self, placement: Placement, fit: FitMode) -> Result<(), AppError> {
        match (self.backend, placement) {
            (Backend::Crate { .. }, Placement::Single(path)) => set_with_crate(path, fit),
            (Backend::Crate { .. }, Placement::Spanned(path)) => span_with_crate(path),
            (Backend::Swaybg, Placement::Single(path)) => self.set_with_swaybg(path, fit),
            (Backend::Feh, Placement::Single(path)) => self.set_with_feh(path, fit),
            #[cfg(windows)]
            (Backend::Windows, Placement::Single(path)) => windows::set_all(path, fit),
            #[cfg(windows)]
            (Backend::Windows, Placement::Spanned(path)) => windows::set_all(path, FitMode::Span),
            #[cfg(windows)]
            (Backend::Windows, Placement::PerMonitor(images)) => windows::set_each(images),
            // The engine only asks for what per_monitor() says is possible
            (_, _) => Err(AppError::Wallpaper(
                "this desktop can't show a different image per monitor".into(),
            )),
        }
    }

    /// Start a swaybg for `path`, then stop the previous one (ours from this run,
    /// or the one a previous run left behind), so the desktop never goes blank
    /// and no swaybg is left over.
    fn set_with_swaybg(&mut self, path: &Path, fit: FitMode) -> Result<(), AppError> {
        let mode = match fit {
            FitMode::Center => "center",
            FitMode::Crop | FitMode::Span => "fill",
            FitMode::Fit => "fit",
            FitMode::Stretch => "stretch",
            FitMode::Tile => "tile",
        };
        let mut child = std::process::Command::new(&self.program)
            .arg("-i")
            .arg(path)
            .args(["-m", mode])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| {
                AppError::Wallpaper(if e.kind() == std::io::ErrorKind::NotFound {
                    "this desktop needs swaybg to show wallpapers: install the swaybg package"
                        .into()
                } else {
                    format!("couldn't start swaybg: {}", e)
                })
            })?;
        // A swaybg that can't reach the compositor or load the image usually
        // exits straight away. One that fails later, or runs without drawing,
        // isn't caught here.
        std::thread::sleep(std::time::Duration::from_millis(300));
        if let Ok(Some(status)) = child.try_wait() {
            return Err(AppError::Wallpaper(format!(
                "swaybg exited ({}): the image may be unreadable, or this compositor \
                 doesn't support wlr-layer-shell",
                status
            )));
        }

        match self.swaybg.take() {
            Some(mut previous) => {
                let _ = previous.kill();
                let _ = previous.wait();
            }
            None => self.stop_leftover_swaybg(child.id()),
        }
        if let Some(pid_file) = &self.swaybg_pid_file {
            if let Err(e) = std::fs::write(pid_file, child.id().to_string()) {
                log::warn!(
                    "Couldn't save swaybg's pid, so a restart may leave it running: {}",
                    e
                );
            }
        }
        self.swaybg = Some(child);
        Ok(())
    }

    fn set_with_feh(&self, path: &Path, fit: FitMode) -> Result<(), AppError> {
        let mode = match fit {
            FitMode::Center => "--bg-center",
            FitMode::Crop | FitMode::Span => "--bg-fill",
            FitMode::Fit => "--bg-max",
            FitMode::Stretch => "--bg-scale",
            FitMode::Tile => "--bg-tile",
        };
        let status = std::process::Command::new(&self.program)
            .arg(mode)
            .arg(path)
            .status()
            .map_err(|e| {
                AppError::Wallpaper(if e.kind() == std::io::ErrorKind::NotFound {
                    "this desktop needs feh to show wallpapers: install the feh package".into()
                } else {
                    format!("couldn't run feh: {}", e)
                })
            })?;
        if status.success() {
            Ok(())
        } else {
            Err(AppError::Wallpaper(format!("feh failed ({})", status)))
        }
    }

    /// Stop the swaybg a previous run started, if it's still that process.
    fn stop_leftover_swaybg(&self, ours: u32) {
        let Some(pid_file) = &self.swaybg_pid_file else {
            return;
        };
        let Some(pid) = std::fs::read_to_string(pid_file)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|pid| *pid != ours)
        else {
            return;
        };
        // Only if that pid still belongs to a swaybg: pids get reused
        let comm = std::fs::read_to_string(format!("/proc/{}/comm", pid)).unwrap_or_default();
        if comm.trim() == "swaybg" {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
        }
    }
}

fn gsettings(key: &str, value: &str) {
    let _ = std::process::Command::new("gsettings")
        .args(["set", "org.gnome.desktop.background", key, value])
        .output();
}

/// Set the fit mode. The crate can't do this on macOS or on desktops it doesn't
/// recognize, but by then the image itself is set, so that's not a failure.
fn set_mode_or_log(mode: wallpaper::Mode) {
    if let Err(e) = wallpaper::set_mode(mode) {
        log::warn!("Couldn't set the fit mode: {}", e);
    }
}

fn path_str(path: &Path) -> Result<&str, AppError> {
    path.to_str()
        .ok_or_else(|| AppError::Wallpaper("Invalid file path".into()))
}

fn set_with_crate(path: &Path, fit: FitMode) -> Result<(), AppError> {
    let path = path_str(path)?;
    wallpaper::set_from_path(path).map_err(|e| AppError::Wallpaper(e.to_string()))?;

    // GNOME fixes: set picture-uri-dark for dark mode, and picture-options to
    // match the fit mode (important when switching back from spanned). Harmless
    // on other desktops: gsettings just writes GNOME's keys.
    if cfg!(target_os = "linux") {
        gsettings("picture-uri-dark", &format!("file://{}", path));
        gsettings(
            "picture-options",
            match fit {
                FitMode::Center => "centered",
                FitMode::Crop => "zoom",
                FitMode::Fit => "scaled",
                FitMode::Span => "spanned",
                FitMode::Stretch => "stretched",
                FitMode::Tile => "wallpaper",
            },
        );
    }

    set_mode_or_log(match fit {
        FitMode::Center => wallpaper::Mode::Center,
        FitMode::Crop => wallpaper::Mode::Crop,
        FitMode::Fit => wallpaper::Mode::Fit,
        FitMode::Span => wallpaper::Mode::Span,
        FitMode::Stretch => wallpaper::Mode::Stretch,
        FitMode::Tile => wallpaper::Mode::Tile,
    });
    Ok(())
}

fn span_with_crate(path: &Path) -> Result<(), AppError> {
    let path = path_str(path)?;
    wallpaper::set_from_path(path).map_err(|e| AppError::Wallpaper(e.to_string()))?;
    if cfg!(target_os = "linux") {
        gsettings("picture-uri-dark", &format!("file://{}", path));
        gsettings("picture-options", "spanned");
    }
    set_mode_or_log(wallpaper::Mode::Span);
    Ok(())
}

/// A monitor as the system names it: its id and (x, y, width, height).
type SystemMonitor = (String, (i32, i32, u32, u32));

/// Which image goes on which monitor: each image's monitor by its rectangle, and
/// any the system reports differently in list order.
#[cfg_attr(not(windows), allow(dead_code))]
fn match_monitors<'a>(
    images: &'a [(MonitorInfo, PathBuf)],
    system: &[SystemMonitor],
) -> Vec<(String, &'a Path)> {
    let rect = |m: &MonitorInfo| (m.x, m.y, m.width, m.height);
    let mut matched: Vec<(String, &Path)> = Vec::new();
    let mut unmatched_images = Vec::new();
    let mut free: Vec<&SystemMonitor> = system.iter().collect();
    for (monitor, path) in images {
        match free.iter().position(|(_, r)| *r == rect(monitor)) {
            Some(i) => matched.push((free.remove(i).0.clone(), path.as_path())),
            None => unmatched_images.push(path.as_path()),
        }
    }
    matched.extend(
        free.into_iter()
            .map(|(id, _)| id.clone())
            .zip(unmatched_images),
    );
    matched
}

#[cfg(windows)]
mod windows {
    use super::*;
    use ::windows::core::{HSTRING, PCWSTR};
    use ::windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_APARTMENTTHREADED,
    };
    use ::windows::Win32::UI::Shell::{
        DesktopWallpaper, IDesktopWallpaper, DESKTOP_WALLPAPER_POSITION, DWPOS_CENTER, DWPOS_FILL,
        DWPOS_FIT, DWPOS_SPAN, DWPOS_STRETCH, DWPOS_TILE,
    };

    fn err(e: ::windows::core::Error) -> AppError {
        AppError::Wallpaper(e.to_string())
    }

    /// COM for the current thread, for as long as this lives.
    struct Com;

    impl Com {
        fn init() -> Result<Self, AppError> {
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
                .ok()
                .map_err(err)?;
            Ok(Com)
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            unsafe { CoUninitialize() }
        }
    }

    fn desktop_wallpaper() -> Result<IDesktopWallpaper, AppError> {
        unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) }.map_err(err)
    }

    fn position(fit: FitMode) -> DESKTOP_WALLPAPER_POSITION {
        match fit {
            FitMode::Center => DWPOS_CENTER,
            FitMode::Crop => DWPOS_FILL,
            FitMode::Fit => DWPOS_FIT,
            FitMode::Span => DWPOS_SPAN,
            FitMode::Stretch => DWPOS_STRETCH,
            FitMode::Tile => DWPOS_TILE,
        }
    }

    /// The attached monitors: device path and (x, y, width, height).
    pub fn monitors(dw: &IDesktopWallpaper) -> Result<Vec<SystemMonitor>, AppError> {
        let count = unsafe { dw.GetMonitorDevicePathCount() }.map_err(err)?;
        let mut monitors = Vec::new();
        for index in 0..count {
            let raw = unsafe { dw.GetMonitorDevicePathAt(index) }.map_err(err)?;
            let id = unsafe { raw.to_string() };
            unsafe { CoTaskMemFree(Some(raw.0 as *const _)) };
            let Ok(id) = id else { continue };
            // Detached monitors are listed too. Their GetMonitorRECT succeeds
            // with S_FALSE and an empty rectangle
            if let Ok(r) = unsafe { dw.GetMonitorRECT(&HSTRING::from(id.as_str())) } {
                if r.right > r.left && r.bottom > r.top {
                    let size = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
                    monitors.push((id, (r.left, r.top, size.0, size.1)));
                }
            }
        }
        Ok(monitors)
    }

    pub fn set_all(path: &Path, fit: FitMode) -> Result<(), AppError> {
        let _com = Com::init()?;
        let dw = desktop_wallpaper()?;
        unsafe { dw.SetPosition(position(fit)) }.map_err(err)?;
        // A null monitor id means every monitor
        unsafe { dw.SetWallpaper(PCWSTR::null(), &HSTRING::from(path.as_os_str())) }.map_err(err)
    }

    pub fn set_each(images: &[(MonitorInfo, PathBuf)]) -> Result<(), AppError> {
        let _com = Com::init()?;
        let dw = desktop_wallpaper()?;
        // Before the images: a position of Span would join them into one
        unsafe { dw.SetPosition(DWPOS_FILL) }.map_err(err)?;
        let system = monitors(&dw)?;
        let pairs = match_monitors(images, &system);
        // Reporting success here would let cleanup delete files still on screen
        if pairs.is_empty() || pairs.len() < images.len().min(system.len()) {
            return Err(AppError::Wallpaper(format!(
                "Windows reported {} monitor(s) and only {} matched",
                system.len(),
                pairs.len()
            )));
        }
        // A monitor plugged in since the images were picked gets the last one,
        // rather than keep a file cleanup is about to delete
        let last = images.last().map(|(_, path)| path.as_path());
        let leftover: Vec<(String, &Path)> = system
            .iter()
            .filter(|(id, _)| !pairs.iter().any(|(matched, _)| matched == id))
            .filter_map(|(id, _)| last.map(|path| (id.clone(), path)))
            .collect();
        for (id, path) in pairs.into_iter().chain(leftover) {
            unsafe {
                dw.SetWallpaper(
                    &HSTRING::from(id.as_str()),
                    &HSTRING::from(path.as_os_str()),
                )
            }
            .map_err(err)?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Sets real wallpapers and reads them back. It changes the desktop, so
        /// it only runs where STASHPAPER_DESKTOP_TEST is set (the Windows CI
        /// job), and there a missing desktop is a failure, not a skip.
        #[test]
        fn sets_and_reads_back_wallpapers() {
            if std::env::var_os("STASHPAPER_DESKTOP_TEST").is_none() {
                eprintln!("skipped: set STASHPAPER_DESKTOP_TEST to run");
                return;
            }
            let _com = Com::init().expect("COM");
            let system = monitors(&desktop_wallpaper().expect("IDesktopWallpaper")).unwrap();
            assert!(!system.is_empty(), "no attached monitors");

            let dir = tempfile::tempdir().unwrap();
            let make = |name: &str, shade: u8| {
                let path = dir.path().join(name);
                image::RgbImage::from_pixel(8, 8, image::Rgb([shade, 80, 160]))
                    .save(&path)
                    .unwrap();
                path
            };
            // What each monitor shows, read through a fresh object each time
            // (one made before a change may not see it)
            let shown = || -> Vec<Option<std::ffi::OsString>> {
                let dw = desktop_wallpaper().unwrap();
                system
                    .iter()
                    .map(|(id, _)| {
                        let raw = unsafe { dw.GetWallpaper(&HSTRING::from(id.as_str())) }.unwrap();
                        let text = unsafe { raw.to_string() }.unwrap();
                        unsafe { CoTaskMemFree(Some(raw.0 as *const _)) };
                        std::path::Path::new(&text)
                            .file_name()
                            .map(|n| n.to_owned())
                    })
                    .collect()
            };
            // Explorer may apply a change a moment later
            let wait_for = |expected: Vec<Option<std::ffi::OsString>>| {
                let mut got = shown();
                for _ in 0..50 {
                    if got == expected {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    got = shown();
                }
                panic!(
                    "monitors {:?} show {:?}, expected {:?}",
                    system, got, expected
                );
            };

            // One image everywhere, with the fit mode applied
            let everywhere = make("everywhere.png", 10);
            set_all(&everywhere, FitMode::Fit).unwrap();
            wait_for(vec![
                everywhere.file_name().map(|n| n.to_owned());
                system.len()
            ]);
            let dw = desktop_wallpaper().unwrap();
            assert_eq!(unsafe { dw.GetPosition() }.unwrap(), DWPOS_FIT);

            // A different image per monitor
            let images: Vec<(MonitorInfo, PathBuf)> = system
                .iter()
                .enumerate()
                .map(|(i, (_, (x, y, w, h)))| {
                    let info = MonitorInfo {
                        width: *w,
                        height: *h,
                        x: *x,
                        y: *y,
                        scale_factor: 1.0,
                    };
                    (info, make(&format!("monitor_{}.png", i), 60 + i as u8))
                })
                .collect();
            set_each(&images).unwrap();
            wait_for(
                images
                    .iter()
                    .map(|(_, path)| path.file_name().map(|n| n.to_owned()))
                    .collect(),
            );
            let dw = desktop_wallpaper().unwrap();
            assert_eq!(unsafe { dw.GetPosition() }.unwrap(), DWPOS_FILL);
            eprintln!("checked {} monitor(s)", system.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_desktops_get_the_right_backend() {
        for name in [
            "ubuntu:GNOME",
            "GNOME",
            "GNOME-Classic:GNOME",
            "Unity",
            "X-Cinnamon",
            "MATE",
            "Deepin",
        ] {
            assert_eq!(
                linux_backend(name, true),
                Backend::Crate { spans: true },
                "{name}"
            );
        }
        assert_eq!(linux_backend("KDE", true), Backend::Crate { spans: false });
        assert_eq!(
            linux_backend("XFCE", false),
            Backend::Crate { spans: false }
        );
        assert_eq!(
            linux_backend("LXDE", false),
            Backend::Crate { spans: false }
        );
        // the crate matches these exactly, so a compound value is unknown to it
        assert_eq!(linux_backend("Unity:Unity7:ubuntu", false), Backend::Feh);
        for name in ["sway", "Hyprland", "niri", "Unity:Unity7:ubuntu", ""] {
            assert_eq!(linux_backend(name, true), Backend::Swaybg, "{name}");
        }
        for name in ["i3", "bspwm", ""] {
            assert_eq!(linux_backend(name, false), Backend::Feh, "{name}");
        }
    }

    #[test]
    fn each_backend_says_what_it_can_do_per_monitor() {
        let with = |backend| Desktop {
            backend,
            program: PathBuf::new(),
            swaybg: None,
            swaybg_pid_file: None,
        };
        assert_eq!(
            with(Backend::Crate { spans: true }).per_monitor(),
            PerMonitor::Spanned
        );
        for backend in [
            Backend::Crate { spans: false },
            Backend::Swaybg,
            Backend::Feh,
        ] {
            assert_eq!(
                with(backend).per_monitor(),
                PerMonitor::Unsupported,
                "{backend:?}"
            );
        }
    }

    fn monitor(x: i32, y: i32, width: u32, height: u32) -> MonitorInfo {
        MonitorInfo {
            width,
            height,
            x,
            y,
            scale_factor: 1.0,
        }
    }

    fn sorted(mut pairs: Vec<(String, &Path)>) -> Vec<(String, &Path)> {
        pairs.sort();
        pairs
    }

    #[test]
    fn images_go_to_the_monitor_with_the_same_rectangle() {
        let images = vec![
            (monitor(0, 0, 1920, 1080), PathBuf::from("left.jpg")),
            (monitor(1920, 0, 2560, 1440), PathBuf::from("right.jpg")),
        ];
        // the system lists them in the other order
        let system = vec![
            ("DISPLAY2".to_string(), (1920, 0, 2560, 1440)),
            ("DISPLAY1".to_string(), (0, 0, 1920, 1080)),
        ];
        assert_eq!(
            sorted(match_monitors(&images, &system)),
            vec![
                ("DISPLAY1".to_string(), Path::new("left.jpg")),
                ("DISPLAY2".to_string(), Path::new("right.jpg")),
            ]
        );
    }

    #[test]
    fn monitors_reported_differently_are_paired_in_order() {
        // e.g. scaled coordinates that don't line up with Tauri's
        let images = vec![
            (monitor(0, 0, 3840, 2160), PathBuf::from("a.jpg")),
            (monitor(3840, 0, 1920, 1080), PathBuf::from("b.jpg")),
        ];
        let system = vec![
            ("one".to_string(), (0, 0, 1920, 1080)),
            ("two".to_string(), (1920, 0, 1920, 1080)),
        ];
        assert_eq!(
            match_monitors(&images, &system),
            vec![
                ("one".to_string(), Path::new("a.jpg")),
                ("two".to_string(), Path::new("b.jpg"))
            ]
        );
    }

    #[test]
    fn a_partial_match_pairs_only_the_rest_in_order() {
        let images = vec![
            (monitor(0, 0, 1920, 1080), PathBuf::from("primary.jpg")),
            (monitor(1920, 0, 3840, 2160), PathBuf::from("scaled.jpg")),
        ];
        // the primary matches exactly; the scaled one is reported differently,
        // and there's a third monitor with no image
        let system = vec![
            ("scaled".to_string(), (1920, 0, 1920, 1080)),
            ("primary".to_string(), (0, 0, 1920, 1080)),
            ("third".to_string(), (3840, 0, 1920, 1080)),
        ];
        assert_eq!(
            sorted(match_monitors(&images, &system)),
            vec![
                ("primary".to_string(), Path::new("primary.jpg")),
                ("scaled".to_string(), Path::new("scaled.jpg")),
            ]
        );
    }

    #[cfg(target_os = "linux")]
    mod processes {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// An executable script standing in for swaybg or feh.
        fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{}\n", body)).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn swaybg(dir: &Path, program: PathBuf) -> Desktop {
            Desktop {
                backend: Backend::Swaybg,
                program,
                swaybg: None,
                swaybg_pid_file: Some(dir.join("swaybg.pid")),
            }
        }

        fn running(pid: u32) -> bool {
            // a killed child of this test process lingers as a zombie until reaped
            std::fs::read_to_string(format!("/proc/{}/stat", pid))
                .is_ok_and(|stat| !stat.contains(") Z "))
        }

        fn wait_until_gone(child: &mut std::process::Child) -> bool {
            for _ in 0..30 {
                if let Ok(Some(_)) = child.try_wait() {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        }

        #[test]
        fn swaybg_keeps_one_process_and_records_it() {
            let dir = tempfile::tempdir().unwrap();
            let mut desktop = swaybg(dir.path(), fake(dir.path(), "swaybg", "sleep 30"));
            let image = dir.path().join("wallpaper.jpg");

            desktop
                .apply(Placement::Single(&image), FitMode::Crop)
                .unwrap();
            let first = desktop.swaybg.as_ref().unwrap().id();
            desktop
                .apply(Placement::Single(&image), FitMode::Crop)
                .unwrap();
            let second = desktop.swaybg.as_ref().unwrap().id();

            assert_ne!(first, second);
            assert!(!running(first), "the previous swaybg is stopped");
            assert!(running(second));
            let saved = std::fs::read_to_string(dir.path().join("swaybg.pid")).unwrap();
            assert_eq!(saved, second.to_string());
            let _ = desktop.swaybg.take().unwrap().kill();
        }

        #[test]
        fn a_failing_swaybg_leaves_the_old_one_up() {
            let dir = tempfile::tempdir().unwrap();
            let mut desktop = swaybg(dir.path(), fake(dir.path(), "swaybg", "sleep 30"));
            let image = dir.path().join("wallpaper.jpg");
            desktop
                .apply(Placement::Single(&image), FitMode::Crop)
                .unwrap();
            let first = desktop.swaybg.as_ref().unwrap().id();

            desktop.program = fake(dir.path(), "broken", "exit 1");
            let err = desktop
                .apply(Placement::Single(&image), FitMode::Crop)
                .unwrap_err();
            assert!(err.to_string().contains("swaybg exited"), "{err}");
            assert!(running(first), "the old wallpaper stays");
            let saved = std::fs::read_to_string(dir.path().join("swaybg.pid")).unwrap();
            assert_eq!(saved, first.to_string());
            let _ = desktop.swaybg.take().unwrap().kill();
        }

        #[test]
        fn a_missing_tool_says_what_to_install() {
            let dir = tempfile::tempdir().unwrap();
            let image = dir.path().join("wallpaper.jpg");
            let mut desktop = swaybg(dir.path(), dir.path().join("no-such-swaybg"));
            let err = desktop
                .apply(Placement::Single(&image), FitMode::Crop)
                .unwrap_err();
            assert!(
                err.to_string().contains("install the swaybg package"),
                "{err}"
            );

            let mut feh = Desktop {
                backend: Backend::Feh,
                program: dir.path().join("no-such-feh"),
                swaybg: None,
                swaybg_pid_file: None,
            };
            let err = feh
                .apply(Placement::Single(&image), FitMode::Crop)
                .unwrap_err();
            assert!(err.to_string().contains("install the feh package"), "{err}");
        }

        #[test]
        fn feh_gets_the_fit_mode() {
            let dir = tempfile::tempdir().unwrap();
            let args = dir.path().join("args");
            let program = fake(
                dir.path(),
                "feh",
                &format!("echo \"$@\" > {}", args.display()),
            );
            let mut feh = Desktop {
                backend: Backend::Feh,
                program,
                swaybg: None,
                swaybg_pid_file: None,
            };
            let image = dir.path().join("wallpaper.jpg");
            feh.apply(Placement::Single(&image), FitMode::Fit).unwrap();
            let got = std::fs::read_to_string(&args).unwrap();
            assert_eq!(got.trim(), format!("--bg-max {}", image.display()));
        }

        #[test]
        fn a_leftover_swaybg_from_a_previous_run_is_stopped_but_nothing_else() {
            let dir = tempfile::tempdir().unwrap();
            let program = fake(dir.path(), "swaybg", "sleep 30");
            let image = dir.path().join("wallpaper.jpg");

            // a swaybg a previous run left behind
            let mut leftover = std::process::Command::new(&program).spawn().unwrap();
            std::fs::write(dir.path().join("swaybg.pid"), leftover.id().to_string()).unwrap();
            let mut desktop = swaybg(dir.path(), program.clone());
            desktop
                .apply(Placement::Single(&image), FitMode::Crop)
                .unwrap();
            assert!(
                wait_until_gone(&mut leftover),
                "the leftover swaybg is stopped"
            );
            let _ = desktop.swaybg.take().unwrap().kill();

            // a pid that now belongs to something else is left alone
            let mut other = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap();
            std::fs::write(dir.path().join("swaybg.pid"), other.id().to_string()).unwrap();
            let mut desktop = swaybg(dir.path(), program);
            desktop
                .apply(Placement::Single(&image), FitMode::Crop)
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert!(
                matches!(other.try_wait(), Ok(None)),
                "not a swaybg: still running"
            );
            let _ = other.kill();
            let _ = desktop.swaybg.take().unwrap().kill();
        }
    }
}

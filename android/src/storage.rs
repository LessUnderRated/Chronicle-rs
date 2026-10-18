//! Where the game keeps its files on Android, and the port's own files the APK carries.
//!
//! The app's internal storage holds `data/` (extracted from the player's disc on first run) and `save/`.
//! The game looks for `lang/` and `glyphs/` beside its executable and then in the save folder
//! (port/src/localize.cpp, tools/kazarov/port/platform/glyphs.cpp); an app has no executable directory
//! of its own, so they are installed into `save/` from the APK's assets, listed in
//! `chronicle-assets.txt` by app/build.gradle.kts.

use std::ffi::{CStr, CString};
use std::fs;
use std::path::{Path, PathBuf};

use crate::sdl;

const MANIFEST: &str = "chronicle-assets.txt";
const STAMP: &str = ".chronicle-assets";

pub fn internal_root() -> PathBuf {
    let path = unsafe { sdl::SDL_GetAndroidInternalStoragePath() };
    if path.is_null() {
        crate::logcat::write(&format!("no internal storage path: {}", sdl::error()));
        return PathBuf::from(".");
    }
    PathBuf::from(
        unsafe { CStr::from_ptr(path) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// An APK asset's bytes. SDL resolves a relative path against the APK's assets on Android.
fn asset(name: &str) -> Option<Vec<u8>> {
    let name = CString::new(name).ok()?;
    let mut size = 0usize;
    let data = unsafe { sdl::SDL_LoadFile(name.as_ptr(), &mut size) };
    if data.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, size) }.to_vec();
    unsafe { sdl::SDL_free(data) };
    Some(bytes)
}

/// Copies the listed assets into `save/` when the APK's list differs from the one installed last, so an
/// update brings new strings and symbols while a player's edits to them last until then.
pub fn install_assets(save: &Path) {
    let Some(manifest) = asset(MANIFEST) else {
        crate::logcat::write(&format!("{MANIFEST} is missing from the APK"));
        return;
    };
    let stamp = save.join(STAMP);
    if fs::read(&stamp).ok().as_deref() == Some(manifest.as_slice()) {
        return;
    }
    let text = String::from_utf8_lossy(&manifest);
    let mut copied = 0;
    for relative in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        if relative
            .split('/')
            .any(|part| part == ".." || part.is_empty())
        {
            continue;
        }
        let Some(bytes) = asset(relative) else {
            crate::logcat::write(&format!("asset {relative} is listed but missing"));
            continue;
        };
        let target = save.join(relative);
        if let Some(parent) = target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match fs::write(&target, bytes) {
            Ok(()) => copied += 1,
            Err(error) => {
                crate::logcat::write(&format!("cannot write {}: {error}", target.display()))
            }
        }
    }
    let _ = fs::write(&stamp, &manifest);
    crate::logcat::write(&format!(
        "installed {copied} port files into {}",
        save.display()
    ));
}

/// The first-run screen hands the game whatever SDL's file dialog returned, which on Android is a
/// `content://` URI that std::ifstream cannot open. SDL can open it; the descriptor behind its stream is
/// then named through /proc/self/fd, which the disc reader opens like any path, as PPSSPP and Dolphin
/// do. Where the provider refuses that second open (Android's storage daemon checks who reopens a
/// file), the disc is copied into the app's own storage from the descriptor, which stays readable.
///
/// # Safety
///
/// `uri` is a NUL-terminated string and `out` has room for `size` bytes.
#[no_mangle]
pub unsafe extern "C" fn chronicle_android_resolve_uri(
    uri: *const std::ffi::c_char,
    out: *mut std::ffi::c_char,
    size: usize,
) -> bool {
    if uri.is_null() || out.is_null() || size == 0 {
        return false;
    }
    let Some(path) = resolve_uri(uri) else {
        return false;
    };
    let text = path.to_string_lossy();
    if text.len() + 1 > size {
        return false;
    }
    std::ptr::copy_nonoverlapping(text.as_ptr(), out.cast::<u8>(), text.len());
    *out.add(text.len()) = 0;
    true
}

fn resolve_uri(uri: *const std::ffi::c_char) -> Option<PathBuf> {
    let io = unsafe { sdl::SDL_IOFromFile(uri, c"rb".as_ptr()) };
    if io.is_null() {
        crate::logcat::write(&format!("cannot open the chosen file: {}", sdl::error()));
        return None;
    }
    let props = unsafe { sdl::SDL_GetIOProperties(io) };
    let file = unsafe {
        sdl::SDL_GetPointerProperty(
            props,
            sdl::SDL_PROP_IOSTREAM_STDIO_FILE_POINTER.as_ptr().cast(),
            std::ptr::null_mut(),
        )
    } as *mut libc::FILE;
    let fd = if file.is_null() {
        -1
    } else {
        unsafe { libc::dup(libc::fileno(file)) }
    };
    unsafe { sdl::SDL_CloseIO(io) };
    if fd < 0 {
        crate::logcat::write("the chosen file has no descriptor");
        return None;
    }

    readable_path(fd, &internal_root().join("disc"), &mut |_, _| true)
}

/// A path the disc reader can open for `fd`, a descriptor of the chosen disc image: /proc/self/fd/N
/// when the file can be opened again through it (the descriptor then stays open for the rest of the
/// run, since the path names it), or else a copy in `copy_dir` made from the descriptor, `progress`
/// hearing of the bytes copied and the total and returning false to stop. It owns `fd` from here: kept
/// open behind a /proc path, closed otherwise.
pub fn readable_path(
    fd: i32,
    copy_dir: &Path,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Option<PathBuf> {
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::FromRawFd;

    let proc_path = PathBuf::from(format!("/proc/self/fd/{fd}"));
    let mut probe = [0u8; 2048];
    if fs::File::open(&proc_path)
        .and_then(|mut f| f.read_exact(&mut probe))
        .is_ok()
    {
        crate::logcat::write(&format!("reading the disc through {}", proc_path.display()));
        return Some(proc_path);
    }

    let mut source = unsafe { fs::File::from_raw_fd(fd) };
    let total = source.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = source.seek(SeekFrom::Start(0));
    let _ = fs::create_dir_all(copy_dir);
    let target = copy_dir.join(DISC_COPY);
    crate::logcat::write(&format!("copying the disc into {}", target.display()));
    let result = (|| -> std::io::Result<u64> {
        let mut out = fs::File::create(&target)?;
        let mut buffer = vec![0u8; 4 << 20];
        let mut copied = 0u64;
        loop {
            let n = source.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            out.write_all(&buffer[..n])?;
            copied += n as u64;
            if !progress(copied, total) {
                return Err(std::io::Error::other("cancelled"));
            }
        }
        out.sync_all()?;
        Ok(copied)
    })();
    match result {
        Ok(bytes) => {
            crate::logcat::write(&format!("copied {bytes} bytes"));
            Some(target)
        }
        Err(error) => {
            crate::logcat::write(&format!("copying the disc failed: {error}"));
            let _ = fs::remove_file(&target);
            None
        }
    }
}

const DISC_COPY: &str = "disc.iso";

/// The copy made by resolve_uri is only needed until the data is extracted.
pub fn remove_disc_copy_when_extracted(data: &Path) {
    let copy = internal_root().join("disc").join(DISC_COPY);
    if copy.exists() && data.join("languages.json").exists() {
        let _ = fs::remove_file(&copy);
    }
}

//! Chronicle's Android host, built the way doukutsu-rs builds its Android app (reference/doukutsu-rs,
//! drsandroid/): SDL's own activity loads libmain.so and calls SDL_main, defined here. This crate sets
//! up what Android does differently, then runs the game's main (port/src/main.cpp, `dc_main`).
//!
//! - logcat: the game's stdout and stderr, which Android otherwise discards
//! - storage: data/ and save/ in the app's internal storage, the port's own files from the APK, and
//!   the disc image the first-run screen's file selector picks
//! - touch: the touch screen as a gamepad, and the overlay that draws it

mod logcat;
mod sdl;
mod storage;
#[cfg(test)]
mod test_stubs;
mod touch;

pub use storage::chronicle_android_resolve_uri;
pub use touch::{chronicle_touch_generation, chronicle_touch_geometry};

use std::ffi::{c_char, CString};

extern "C" {
    fn dc_main(argc: i32, argv: *const *const c_char) -> i32;
}

#[no_mangle]
pub extern "C" fn SDL_main(_argc: i32, _argv: *const *const c_char) -> i32 {
    logcat::redirect_stdio();
    std::panic::set_hook(Box::new(|info| logcat::write(&format!("panic: {info}"))));

    let root = storage::internal_root();
    let data = root.join("data");
    let save = root.join("save");
    let _ = std::fs::create_dir_all(&save);
    if let Err(error) = std::env::set_current_dir(&root) {
        logcat::write(&format!("cannot enter {}: {error}", root.display()));
    }
    // The port falls back to $HOME for its per-user directory (tools/kazarov/port/platform/paths.cpp).
    std::env::set_var("HOME", &root);
    storage::install_assets(&save);
    storage::remove_disc_copy_when_extracted(&data);
    touch::init();

    // argv[0] only names the program; the game finds lang/ and glyphs/ in save/, where they were installed.
    let args = [
        root.join("darkcloud").to_string_lossy().into_owned(),
        "--data".into(),
        data.to_string_lossy().into_owned(),
        "--save".into(),
        save.to_string_lossy().into_owned(),
    ];
    let args: Vec<CString> = args
        .iter()
        .map(|a| CString::new(a.as_str()).unwrap())
        .collect();
    let mut argv: Vec<*const c_char> = args.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    logcat::write(&format!(
        "starting with data in {} and saves in {}",
        data.display(),
        save.display()
    ));
    let status = unsafe { dc_main(args.len() as i32, argv.as_ptr()) };
    logcat::write(&format!("the game exited with {status}"));
    status
}

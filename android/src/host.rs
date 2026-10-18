//! What the app's own menu (app/.../ChronicleActivity.java) asks of the running game, which the game
//! takes between ticks on its own thread (port/src/main.cpp, `PumpAndroid`): a pause while the menu is
//! open, the speed, and settings changed there.

use std::collections::VecDeque;
use std::ffi::c_char;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

static PAUSED: AtomicBool = AtomicBool::new(false);
static SPEED: AtomicU32 = AtomicU32::new(1);
static SETTINGS: Mutex<VecDeque<(String, String)>> = Mutex::new(VecDeque::new());

pub fn set_paused(paused: bool) {
    PAUSED.store(paused, Ordering::Relaxed);
}

/// 1 or 2: how many of the game's ticks a tick of real time runs.
pub fn set_speed(speed: u32) {
    SPEED.store(speed.clamp(1, 2), Ordering::Relaxed);
}

pub fn speed() -> u32 {
    SPEED.load(Ordering::Relaxed)
}

/// A setting by its config.json name ("video.aspect") and its value as JSON text ("\"4:3\"").
pub fn queue_setting(name: String, value: String) {
    SETTINGS.lock().unwrap().push_back((name, value));
}

#[no_mangle]
pub extern "C" fn chronicle_android_pump() {
    crate::touch::pump();
}

#[no_mangle]
pub extern "C" fn chronicle_android_paused() -> bool {
    PAUSED.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn chronicle_android_speed() -> u32 {
    speed()
}

/// The oldest setting queued, written as two NUL-terminated strings; false when there is none. One
/// that does not fit is dropped.
///
/// # Safety
///
/// `name` has room for `name_size` bytes and `value` for `value_size`.
#[no_mangle]
pub unsafe extern "C" fn chronicle_android_take_setting(
    name: *mut c_char,
    name_size: usize,
    value: *mut c_char,
    value_size: usize,
) -> bool {
    loop {
        let Some((n, v)) = SETTINGS.lock().unwrap().pop_front() else {
            return false;
        };
        if n.len() < name_size && v.len() < value_size && !n.contains('\0') && !v.contains('\0') {
            std::ptr::copy_nonoverlapping(n.as_ptr(), name.cast::<u8>(), n.len());
            *name.add(n.len()) = 0;
            std::ptr::copy_nonoverlapping(v.as_ptr(), value.cast::<u8>(), v.len());
            *value.add(v.len()) = 0;
            return true;
        }
        crate::logcat::write(&format!("setting {n} is too long"));
    }
}

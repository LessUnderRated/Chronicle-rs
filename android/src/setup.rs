//! The setup screen's native half (app/src/main/java/.../SetupActivity.java): before the game starts,
//! the player picks the disc image of their own copy in Android's file picker, and the screen shows
//! what it is and extracts its data, through the same code the game's own first-run screen uses
//! (port/src/platform/android_setup.cpp).

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;

use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jint, jstring};
use jni::JNIEnv;

extern "C" {
    fn chronicle_identify_disc(source: *const c_char, json: *mut c_char, size: usize) -> bool;
    fn chronicle_extract_disc(
        source: *const c_char,
        root: *const c_char,
        tick: Option<unsafe extern "C" fn(*mut c_void, u64, u64, u64, u64) -> bool>,
        user: *mut c_void,
        error: *mut c_char,
        size: usize,
    ) -> bool;
}

fn string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    env.get_string(value).ok().map(Into::into)
}

fn java(env: &mut JNIEnv, text: &str) -> jstring {
    env.new_string(text)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// SetupActivity.Progress.update(done, total, bytes, totalBytes): false stops the work. A Java
/// exception stops it too.
fn report(
    env: &mut JNIEnv,
    progress: &JObject,
    done: u64,
    total: u64,
    bytes: u64,
    total_bytes: u64,
) -> bool {
    if progress.is_null() {
        return true;
    }
    let args = [
        JValue::Long(done as i64),
        JValue::Long(total as i64),
        JValue::Long(bytes as i64),
        JValue::Long(total_bytes as i64),
    ];
    match env
        .call_method(progress, "update", "(JJJJ)Z", &args)
        .and_then(|v| v.z())
    {
        Ok(go_on) => go_on,
        Err(_) => {
            let _ = env.exception_clear();
            false
        }
    }
}

/// A path the disc reader can open for `fd`, which the activity got from the file picker and hands
/// over; null when there is none. May copy the whole disc into `copy_dir`, reporting through
/// `progress` (done and total are 0 for a copy).
#[no_mangle]
pub extern "system" fn Java_io_github_lessunderrated_chronicle_SetupActivity_nativeOpenDisc<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    fd: jint,
    copy_dir: JString<'l>,
    progress: JObject<'l>,
) -> jstring {
    let Some(copy_dir) = string(&mut env, &copy_dir) else {
        return std::ptr::null_mut();
    };
    let path = crate::storage::readable_path(fd, Path::new(&copy_dir), &mut |bytes, total| {
        report(&mut env, &progress, 0, 0, bytes, total)
    });
    match path {
        Some(path) => java(&mut env, &path.to_string_lossy()),
        None => std::ptr::null_mut(),
    }
}

/// What the disc at `path` is, as the JSON chronicle_identify_disc writes.
#[no_mangle]
pub extern "system" fn Java_io_github_lessunderrated_chronicle_SetupActivity_nativeIdentify<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    path: JString<'l>,
) -> jstring {
    let Some(path) = string(&mut env, &path).and_then(|p| CString::new(p).ok()) else {
        return java(&mut env, r#"{"error":"no path"}"#);
    };
    let mut json = vec![0 as c_char; 4096];
    unsafe { chronicle_identify_disc(path.as_ptr(), json.as_mut_ptr(), json.len()) };
    let text = unsafe { CStr::from_ptr(json.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    java(&mut env, &text)
}

struct Tick<'a, 'l> {
    env: &'a mut JNIEnv<'l>,
    progress: &'a JObject<'l>,
}

unsafe extern "C" fn tick(
    user: *mut c_void,
    files: u64,
    total_files: u64,
    bytes: u64,
    total_bytes: u64,
) -> bool {
    let tick = &mut *(user as *mut Tick);
    report(
        tick.env,
        tick.progress,
        files,
        total_files,
        bytes,
        total_bytes,
    )
}

/// Extracts the disc at `path` into `data_dir`, on the calling thread, reporting through `progress`.
/// Null when the data is in place, else why not.
#[no_mangle]
pub extern "system" fn Java_io_github_lessunderrated_chronicle_SetupActivity_nativeExtract<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    path: JString<'l>,
    data_dir: JString<'l>,
    progress: JObject<'l>,
) -> jstring {
    let (Some(path), Some(data_dir)) = (
        string(&mut env, &path).and_then(|p| CString::new(p).ok()),
        string(&mut env, &data_dir).and_then(|p| CString::new(p).ok()),
    ) else {
        return java(&mut env, "no path");
    };
    let mut error = vec![0 as c_char; 4096];
    let ok = {
        let mut state = Tick {
            env: &mut env,
            progress: &progress,
        };
        unsafe {
            chronicle_extract_disc(
                path.as_ptr(),
                data_dir.as_ptr(),
                Some(tick),
                &mut state as *mut Tick as *mut c_void,
                error.as_mut_ptr(),
                error.len(),
            )
        }
    };
    if ok {
        return std::ptr::null_mut();
    }
    let text = unsafe { CStr::from_ptr(error.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    crate::logcat::write(&format!("extraction failed: {text}"));
    java(&mut env, &text)
}

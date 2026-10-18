//! The Java side's way in: `io.github.lessunderrated.chronicle.Native`, whose methods are registered
//! when the library loads, so only JNI_OnLoad has to be kept by the linker. The touch overlay and the
//! app's menu call them on the activity's thread.

use std::ffi::c_void;
use std::io::Read;
use std::os::fd::FromRawFd;
use std::path::Path;

use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jfloat, jint, jlong, jstring, JNI_VERSION_1_6};
use jni::{JNIEnv, JavaVM, NativeMethod};

use crate::touch::{self, Insets};

fn java(env: &mut JNIEnv, text: &str) -> jstring {
    env.new_string(text)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

fn rust(env: &mut JNIEnv, text: &JString) -> Option<String> {
    if text.is_null() {
        return None;
    }
    env.get_string(text).ok().map(Into::into)
}

extern "system" fn default_layout(
    mut env: JNIEnv,
    _: JClass,
    width: jint,
    height: jint,
    left: jfloat,
    top: jfloat,
    right: jfloat,
    bottom: jfloat,
) -> jstring {
    let json = touch::default_layout(
        width as f32,
        height as f32,
        Insets {
            left,
            top,
            right,
            bottom,
        },
    );
    java(&mut env, &json)
}

#[allow(clippy::too_many_arguments)]
extern "system" fn set_layout(
    mut env: JNIEnv,
    _: JClass,
    width: jint,
    height: jint,
    left: jfloat,
    top: jfloat,
    right: jfloat,
    bottom: jfloat,
    saved: JString,
) -> jstring {
    let saved = rust(&mut env, &saved);
    let json = touch::set_layout(
        width as f32,
        height as f32,
        Insets {
            left,
            top,
            right,
            bottom,
        },
        saved.as_deref(),
    );
    java(&mut env, &json)
}

extern "system" fn touch_event(
    _: JNIEnv,
    _: JClass,
    action: jint,
    id: jlong,
    x: jfloat,
    y: jfloat,
) {
    touch::touch(action, id as u64, x, y);
}

extern "system" fn pad_state(mut env: JNIEnv, _: JClass) -> jstring {
    let json = touch::pad_state();
    java(&mut env, &json)
}

extern "system" fn set_paused(_: JNIEnv, _: JClass, paused: jboolean) {
    crate::host::set_paused(paused != 0);
}

extern "system" fn set_speed(_: JNIEnv, _: JClass, speed: jint) {
    crate::host::set_speed(speed.max(1) as u32);
}

extern "system" fn speed(_: JNIEnv, _: JClass) -> jint {
    crate::host::speed() as jint
}

extern "system" fn set_setting(mut env: JNIEnv, _: JClass, name: JString, value: JString) {
    if let (Some(name), Some(value)) = (rust(&mut env, &name), rust(&mut env, &value)) {
        crate::host::queue_setting(name, value);
    }
}

/// Reads the card behind `fd` (handed over: closed here) and imports its saves into `saves`; the
/// result as JSON, `{"error"}` or `{"found": [{"source", "folder"?, "converted_from"?, "skipped"?}]}`.
extern "system" fn import_card(mut env: JNIEnv, _: JClass, fd: jint, saves: JString) -> jstring {
    let Some(saves) = rust(&mut env, &saves) else {
        return java(&mut env, r#"{"error":"no save folder"}"#);
    };
    let mut bytes = Vec::new();
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    if let Err(error) = (&mut file).take(64 << 20).read_to_end(&mut bytes) {
        let json = serde_json::json!({ "error": format!("the card could not be read: {error}") });
        return java(&mut env, &json.to_string());
    }
    let json = match crate::memcard::import(&bytes, Path::new(&saves)) {
        Err(error) => serde_json::json!({ "error": error }),
        Ok(found) => {
            let found: Vec<serde_json::Value> = found
                .into_iter()
                .map(|f| match f.outcome {
                    crate::memcard::Outcome::Imported { folder, converted_from } => {
                        serde_json::json!({ "source": f.source, "folder": folder, "converted_from": converted_from })
                    }
                    crate::memcard::Outcome::AlreadyThere => {
                        serde_json::json!({ "source": f.source, "skipped": "already imported" })
                    }
                    crate::memcard::Outcome::Damaged(why) => serde_json::json!({ "source": f.source, "skipped": why }),
                })
                .collect();
            serde_json::json!({ "found": found })
        }
    };
    crate::logcat::write(&format!("card import: {json}"));
    java(&mut env, &json.to_string())
}

fn method(name: &str, sig: &str, fn_ptr: *mut c_void) -> NativeMethod {
    NativeMethod {
        name: name.into(),
        sig: sig.into(),
        fn_ptr,
    }
}

/// # Safety
///
/// Called by the JVM with its own pointer when the library loads.
#[no_mangle]
pub unsafe extern "system" fn JNI_OnLoad(vm: *mut jni::sys::JavaVM, _: *mut c_void) -> jint {
    let Ok(vm) = JavaVM::from_raw(vm) else {
        return JNI_VERSION_1_6;
    };
    let Ok(mut env) = vm.get_env() else {
        return JNI_VERSION_1_6;
    };
    let methods = [
        method(
            "defaultLayout",
            "(IIFFFF)Ljava/lang/String;",
            default_layout as *mut c_void,
        ),
        method(
            "setLayout",
            "(IIFFFFLjava/lang/String;)Ljava/lang/String;",
            set_layout as *mut c_void,
        ),
        method("touch", "(IJFF)V", touch_event as *mut c_void),
        method("padState", "()Ljava/lang/String;", pad_state as *mut c_void),
        method("setPaused", "(Z)V", set_paused as *mut c_void),
        method("setSpeed", "(I)V", set_speed as *mut c_void),
        method("speed", "()I", speed as *mut c_void),
        method(
            "setSetting",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            set_setting as *mut c_void,
        ),
        method(
            "importCard",
            "(ILjava/lang/String;)Ljava/lang/String;",
            import_card as *mut c_void,
        ),
    ];
    match env.find_class("io/github/lessunderrated/chronicle/Native") {
        Ok(class) => {
            if env.register_native_methods(&class, &methods).is_err() {
                let _ = env.exception_clear();
                crate::logcat::write("registering Native's methods failed");
            }
        }
        Err(_) => {
            let _ = env.exception_clear();
            crate::logcat::write("no Native class to register");
        }
    }
    JNI_VERSION_1_6
}

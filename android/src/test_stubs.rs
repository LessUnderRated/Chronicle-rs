//! Host test builds link against these instead of SDL, liblog and the game, which exist only in the
//! APK. Only the pure parts of the crate are tested; none of these is expected to run.

#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_int, c_void};

use crate::sdl::*;

#[no_mangle]
pub extern "C" fn __android_log_write(
    _priority: c_int,
    _tag: *const c_char,
    _text: *const c_char,
) -> c_int {
    0
}
#[no_mangle]
pub extern "C" fn dc_main(_argc: i32, _argv: *const *const c_char) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn SDL_GetError() -> *const c_char {
    c"".as_ptr()
}
#[no_mangle]
pub extern "C" fn SDL_SetHint(_name: *const c_char, _value: *const c_char) -> bool {
    true
}
#[no_mangle]
pub extern "C" fn SDL_InitSubSystem(_flags: u32) -> bool {
    true
}
#[no_mangle]
pub extern "C" fn SDL_free(_memory: *mut c_void) {}
#[no_mangle]
pub extern "C" fn SDL_LoadFile(_file: *const c_char, _size: *mut usize) -> *mut c_void {
    std::ptr::null_mut()
}
#[no_mangle]
pub extern "C" fn SDL_IOFromFile(_file: *const c_char, _mode: *const c_char) -> *mut SDL_IOStream {
    std::ptr::null_mut()
}
#[no_mangle]
pub extern "C" fn SDL_GetIOProperties(_context: *mut SDL_IOStream) -> SDL_PropertiesID {
    0
}
#[no_mangle]
pub extern "C" fn SDL_GetPointerProperty(
    _props: SDL_PropertiesID,
    _name: *const c_char,
    default: *mut c_void,
) -> *mut c_void {
    default
}
#[no_mangle]
pub extern "C" fn SDL_CloseIO(_context: *mut SDL_IOStream) -> bool {
    true
}
#[no_mangle]
pub extern "C" fn SDL_GetAndroidInternalStoragePath() -> *const c_char {
    std::ptr::null()
}
#[no_mangle]
pub extern "C" fn SDL_AddEventWatch(_filter: SDL_EventFilter, _userdata: *mut c_void) -> bool {
    true
}
#[no_mangle]
pub extern "C" fn SDL_AttachVirtualJoystick(
    _desc: *const SDL_VirtualJoystickDesc,
) -> SDL_JoystickID {
    0
}
#[no_mangle]
pub extern "C" fn SDL_DetachVirtualJoystick(_instance: SDL_JoystickID) -> bool {
    true
}
#[no_mangle]
pub extern "C" fn SDL_IsJoystickVirtual(_instance: SDL_JoystickID) -> bool {
    false
}
#[no_mangle]
pub extern "C" fn SDL_OpenJoystick(_instance: SDL_JoystickID) -> *mut SDL_Joystick {
    std::ptr::null_mut()
}
#[no_mangle]
pub extern "C" fn SDL_CloseJoystick(_joystick: *mut SDL_Joystick) {}
#[no_mangle]
pub extern "C" fn SDL_SetJoystickVirtualAxis(
    _joystick: *mut SDL_Joystick,
    _axis: c_int,
    _value: i16,
) -> bool {
    true
}
#[no_mangle]
pub extern "C" fn SDL_SetJoystickVirtualButton(
    _joystick: *mut SDL_Joystick,
    _button: c_int,
    _down: bool,
) -> bool {
    true
}
#[no_mangle]
pub extern "C" fn SDL_GetGamepads(_count: *mut c_int) -> *mut SDL_JoystickID {
    std::ptr::null_mut()
}

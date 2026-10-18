//! The part of SDL3's C API the crate calls, declared by hand against the SDL that android/native.cmake
//! pins (release-3.4.18), so the build needs no bindgen and no second copy of SDL.

#![allow(non_camel_case_types, non_upper_case_globals, dead_code)]

use std::ffi::{c_char, c_int, c_void};

pub type SDL_JoystickID = u32;
pub type SDL_PropertiesID = u32;

#[repr(C)]
pub struct SDL_Joystick {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SDL_IOStream {
    _private: [u8; 0],
}

pub const SDL_INIT_GAMEPAD: u32 = 0x0000_2000;

pub const SDL_EVENT_KEY_DOWN: u32 = 0x300;
pub const SDL_EVENT_KEY_UP: u32 = 0x301;
pub const SDL_EVENT_GAMEPAD_ADDED: u32 = 0x653;
pub const SDL_EVENT_GAMEPAD_REMOVED: u32 = 0x654;
pub const SDL_EVENT_FINGER_DOWN: u32 = 0x700;
pub const SDL_EVENT_FINGER_UP: u32 = 0x701;
pub const SDL_EVENT_FINGER_MOTION: u32 = 0x702;
pub const SDL_EVENT_FINGER_CANCELED: u32 = 0x703;

pub const SDL_SCANCODE_AC_BACK: u32 = 270;

pub const SDL_JOYSTICK_TYPE_GAMEPAD: u16 = 1;
pub const SDL_JOYSTICK_AXIS_MAX: i16 = 32767;

pub const SDL_GAMEPAD_BUTTON_SOUTH: c_int = 0;
pub const SDL_GAMEPAD_BUTTON_EAST: c_int = 1;
pub const SDL_GAMEPAD_BUTTON_WEST: c_int = 2;
pub const SDL_GAMEPAD_BUTTON_NORTH: c_int = 3;
pub const SDL_GAMEPAD_BUTTON_BACK: c_int = 4;
pub const SDL_GAMEPAD_BUTTON_START: c_int = 6;
pub const SDL_GAMEPAD_BUTTON_LEFT_SHOULDER: c_int = 9;
pub const SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER: c_int = 10;
pub const SDL_GAMEPAD_BUTTON_DPAD_UP: c_int = 11;
pub const SDL_GAMEPAD_BUTTON_DPAD_DOWN: c_int = 12;
pub const SDL_GAMEPAD_BUTTON_DPAD_LEFT: c_int = 13;
pub const SDL_GAMEPAD_BUTTON_DPAD_RIGHT: c_int = 14;
/// SDL_GAMEPAD_BUTTON_COUNT: a virtual gamepad with this many buttons maps them one to one.
pub const SDL_GAMEPAD_BUTTON_COUNT: u16 = 26;

pub const SDL_GAMEPAD_AXIS_LEFTX: c_int = 0;
pub const SDL_GAMEPAD_AXIS_LEFTY: c_int = 1;
pub const SDL_GAMEPAD_AXIS_RIGHTX: c_int = 2;
pub const SDL_GAMEPAD_AXIS_RIGHTY: c_int = 3;
pub const SDL_GAMEPAD_AXIS_LEFT_TRIGGER: c_int = 4;
pub const SDL_GAMEPAD_AXIS_RIGHT_TRIGGER: c_int = 5;
pub const SDL_GAMEPAD_AXIS_COUNT: u16 = 6;

pub const SDL_PROP_IOSTREAM_STDIO_FILE_POINTER: &[u8] = b"SDL.iostream.stdio.file\0";

pub const SDL_HINT_TOUCH_MOUSE_EVENTS: &[u8] = b"SDL_TOUCH_MOUSE_EVENTS\0";
pub const SDL_HINT_ANDROID_TRAP_BACK_BUTTON: &[u8] = b"SDL_ANDROID_TRAP_BACK_BUTTON\0";

/// SDL_TouchFingerEvent.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct SDL_TouchFingerEvent {
    pub type_: u32,
    pub reserved: u32,
    pub timestamp: u64,
    pub touch_id: u64,
    pub finger_id: u64,
    pub x: f32,
    pub y: f32,
    pub dx: f32,
    pub dy: f32,
    pub pressure: f32,
    pub window_id: u32,
}

/// SDL_GamepadDeviceEvent.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct SDL_GamepadDeviceEvent {
    pub type_: u32,
    pub reserved: u32,
    pub timestamp: u64,
    pub which: SDL_JoystickID,
}

/// SDL_KeyboardEvent.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct SDL_KeyboardEvent {
    pub type_: u32,
    pub reserved: u32,
    pub timestamp: u64,
    pub window_id: u32,
    pub which: u32,
    pub scancode: u32,
    pub key: u32,
    pub mod_: u16,
    pub raw: u16,
    pub down: bool,
    pub repeat: bool,
}

/// SDL_Event: 128 bytes, read through whichever member its type names.
#[repr(C)]
pub union SDL_Event {
    pub type_: u32,
    pub tfinger: SDL_TouchFingerEvent,
    pub gdevice: SDL_GamepadDeviceEvent,
    pub key: SDL_KeyboardEvent,
    pub padding: [u8; 128],
}

const _: () = assert!(std::mem::size_of::<SDL_Event>() == 128);

/// SDL_VirtualJoystickDesc, `version` being its size as SDL_INIT_INTERFACE sets it.
#[repr(C)]
pub struct SDL_VirtualJoystickDesc {
    pub version: u32,
    pub type_: u16,
    pub padding: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    pub naxes: u16,
    pub nbuttons: u16,
    pub nballs: u16,
    pub nhats: u16,
    pub ntouchpads: u16,
    pub nsensors: u16,
    pub padding2: [u16; 2],
    pub button_mask: u32,
    pub axis_mask: u32,
    pub name: *const c_char,
    pub touchpads: *const c_void,
    pub sensors: *const c_void,
    pub userdata: *mut c_void,
    pub update: Option<unsafe extern "C" fn(*mut c_void)>,
    pub set_player_index: Option<unsafe extern "C" fn(*mut c_void, c_int)>,
    pub rumble: Option<unsafe extern "C" fn(*mut c_void, u16, u16) -> bool>,
    pub rumble_triggers: Option<unsafe extern "C" fn(*mut c_void, u16, u16) -> bool>,
    pub set_led: Option<unsafe extern "C" fn(*mut c_void, u8, u8, u8) -> bool>,
    pub send_effect: Option<unsafe extern "C" fn(*mut c_void, *const c_void, c_int) -> bool>,
    pub set_sensors_enabled: Option<unsafe extern "C" fn(*mut c_void, bool) -> bool>,
    pub cleanup: Option<unsafe extern "C" fn(*mut c_void)>,
}

// SDL_joystick.h asserts the same size for 64-bit targets.
const _: () = assert!(std::mem::size_of::<SDL_VirtualJoystickDesc>() == 136);

pub type SDL_EventFilter =
    unsafe extern "C" fn(userdata: *mut c_void, event: *mut SDL_Event) -> bool;

extern "C" {
    pub fn SDL_GetError() -> *const c_char;
    pub fn SDL_SetHint(name: *const c_char, value: *const c_char) -> bool;
    pub fn SDL_InitSubSystem(flags: u32) -> bool;
    pub fn SDL_free(memory: *mut c_void);
    pub fn SDL_LoadFile(file: *const c_char, datasize: *mut usize) -> *mut c_void;

    pub fn SDL_IOFromFile(file: *const c_char, mode: *const c_char) -> *mut SDL_IOStream;
    pub fn SDL_GetIOProperties(context: *mut SDL_IOStream) -> SDL_PropertiesID;
    pub fn SDL_GetPointerProperty(
        props: SDL_PropertiesID,
        name: *const c_char,
        default_value: *mut c_void,
    ) -> *mut c_void;
    pub fn SDL_CloseIO(context: *mut SDL_IOStream) -> bool;

    pub fn SDL_GetAndroidInternalStoragePath() -> *const c_char;

    pub fn SDL_AddEventWatch(filter: SDL_EventFilter, userdata: *mut c_void) -> bool;

    pub fn SDL_AttachVirtualJoystick(desc: *const SDL_VirtualJoystickDesc) -> SDL_JoystickID;
    pub fn SDL_DetachVirtualJoystick(instance_id: SDL_JoystickID) -> bool;
    pub fn SDL_IsJoystickVirtual(instance_id: SDL_JoystickID) -> bool;
    pub fn SDL_OpenJoystick(instance_id: SDL_JoystickID) -> *mut SDL_Joystick;
    pub fn SDL_CloseJoystick(joystick: *mut SDL_Joystick);
    pub fn SDL_SetJoystickVirtualAxis(joystick: *mut SDL_Joystick, axis: c_int, value: i16)
        -> bool;
    pub fn SDL_SetJoystickVirtualButton(
        joystick: *mut SDL_Joystick,
        button: c_int,
        down: bool,
    ) -> bool;
    pub fn SDL_GetGamepads(count: *mut c_int) -> *mut SDL_JoystickID;
}

pub fn error() -> String {
    unsafe { std::ffi::CStr::from_ptr(SDL_GetError()) }
        .to_string_lossy()
        .into_owned()
}

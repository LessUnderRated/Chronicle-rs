//! Touch controls: an SDL virtual gamepad driven by the screen.
//!
//! doukutsu-rs puts its touch controls inside the game's player logic. Dark Cloud's input already goes
//! through SDL gamepads (tools/kazarov/port/platform/input.cpp), so here the screen is a gamepad
//! instead: everything above it, button prompts and the camera included, sees a controller.
//!
//! This is the model: where the controls are (a default that keeps out of the HUD, or the player's
//! own arrangement), what the fingers on them press, and the virtual gamepad that reports it. The
//! view is Android's (app/.../TouchOverlay.java), drawn above the game: it draws the controls, edits
//! their places, sizes and opacity, and hands every touch here (jni.rs).
//!
//! Threads: touches arrive on the activity's thread and only change the state here. SDL's joystick
//! calls run on the game's thread, in the virtual gamepad's Update callback and in `pump`, which the
//! game calls between ticks. Neither lock is ever held while waiting for the other.

use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::sdl::{self, *};

// ---- Layout ---------------------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
enum Shape {
    Circle { x: f32, y: f32, r: f32 },
    Rect { x: f32, y: f32, w: f32, h: f32 },
}

impl Shape {
    fn contains(&self, px: f32, py: f32, slack: f32) -> bool {
        match *self {
            Shape::Circle { x, y, r } => (px - x).hypot(py - y) <= r * slack,
            Shape::Rect { x, y, w, h } => {
                let (sx, sy) = (w * (slack - 1.0) / 2.0, h * (slack - 1.0) / 2.0);
                px >= x - sx && px <= x + w + sx && py >= y - sy && py <= y + h + sy
            }
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum Control {
    Button(c_int),
    Trigger(c_int),
    Dpad,
    LeftStick,
    RightStick,
}

struct Element {
    /// The control's name in a saved arrangement and to the view.
    id: &'static str,
    control: Control,
    shape: Shape,
    /// 0 to 1, as the player set it.
    opacity: f32,
}

struct Layout {
    width: f32,
    height: f32,
    elements: Vec<Element>,
    stick_radius: f32,
    left_rest: (f32, f32),
}

/// How far the window's safe area (clear of a display cutout and the system's gesture edges) lies
/// inside each edge, in pixels.
#[repr(C)]
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct Insets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Where the game's HUD lies on a window `width` x `height` with `insets`, in pixels: the pieces the
/// port anchors to the window's edges (docs/PC.md, "Window aspect") keep their distance from the
/// frame's edge to the safe area's, at the frame's scale. The controls keep out of it.
struct Hud {
    /// Below this the top band is clear: the status panel, the quick items, the floor plate, the clock.
    top_band: f32,
    /// Right of the status panel (anchored left) and left of the quick items (in the frame's centre).
    left_gap: (f32, f32),
    /// Right of the quick items and left of the floor plate (anchored right).
    right_gap: (f32, f32),
    /// Above this, in the bottom left corner, is the weapon's gauge.
    gauge_top: f32,
    /// Right of this, in the bottom left corner, the gauge ends.
    gauge_right: f32,
}

impl Hud {
    fn new(width: f32, height: f32, insets: Insets) -> Hud {
        let scale = (width / 640.0).min(height / 480.0);
        let frame_left = (width - 640.0 * scale) / 2.0;
        // dun/gameloop.cpp's kStatusAnchor: the status panel left of 0x124, the floor plate from 0x1FC;
        // the panel's rows end by 0x58 and the gauge stands 0x34 above the frame's bottom.
        let status_right = insets.left + 0x124 as f32 * scale;
        let items = (
            frame_left + 0x124 as f32 * scale,
            frame_left + 0x1FC as f32 * scale,
        );
        let plate_left = width - insets.right - (640 - 0x1FC) as f32 * scale;
        Hud {
            top_band: insets.top + 0x58 as f32 * scale,
            left_gap: (status_right, items.0),
            right_gap: (items.1, plate_left),
            gauge_top: height - insets.bottom - 0x38 as f32 * scale,
            gauge_right: insets.left + 0xAC as f32 * scale,
        }
    }
}

fn circle(id: &'static str, control: Control, x: f32, y: f32, r: f32) -> Element {
    Element {
        id,
        control,
        shape: Shape::Circle { x, y, r },
        opacity: 1.0,
    }
}

fn rect(id: &'static str, control: Control, x: f32, y: f32, w: f32, h: f32) -> Element {
    Element {
        id,
        control,
        shape: Shape::Rect { x, y, w, h },
        opacity: 1.0,
    }
}

impl Layout {
    fn new(width: f32, height: f32, insets: Insets) -> Layout {
        let u = width.min(height);
        let hud = Hud::new(width, height, insets);
        let margin = u * 0.04;
        let left = insets.left + margin;
        let right = width - insets.right - margin;
        let bottom = height - insets.bottom - margin;

        // Face buttons in the bottom right corner, as on a pad.
        let face_r = u * 0.075;
        let face_gap = u * 0.125;
        let (fx, fy) = (right - face_gap - face_r, bottom - face_gap - face_r);

        // Shoulders on the side edges, under the HUD's top band; L2 and R2 above L1 and R1, as held.
        let shoulder_w = u * 0.22;
        let shoulder_h = u * 0.105;
        let shoulder_gap = u * 0.025;
        let l2_y = hud.top_band + shoulder_gap;
        let l1_y = l2_y + shoulder_h + shoulder_gap;

        // The d-pad under the left shoulders.
        let dpad_r = u * 0.12;
        let dpad_y = l1_y + shoulder_h + shoulder_gap + dpad_r;

        // SELECT and START in the gaps either side of the quick items where the window is wide enough
        // for them, else at the bottom's centre.
        let pill_w = u * 0.19;
        let pill_h = u * 0.075;
        let fits = |gap: (f32, f32)| gap.1 - gap.0 >= pill_w + margin;
        let (select, start) = if fits(hud.left_gap) && fits(hud.right_gap) {
            let top = insets.top + margin * 0.5;
            (
                ((hud.left_gap.0 + hud.left_gap.1 - pill_w) / 2.0, top),
                ((hud.right_gap.0 + hud.right_gap.1 - pill_w) / 2.0, top),
            )
        } else {
            let y = bottom - pill_h;
            (
                (width / 2.0 - pill_w - margin * 0.5, y),
                (width / 2.0 + margin * 0.5, y),
            )
        };

        let elements = vec![
            circle(
                "cross",
                Control::Button(SDL_GAMEPAD_BUTTON_SOUTH),
                fx,
                fy + face_gap,
                face_r,
            ),
            circle(
                "circle",
                Control::Button(SDL_GAMEPAD_BUTTON_EAST),
                fx + face_gap,
                fy,
                face_r,
            ),
            circle(
                "square",
                Control::Button(SDL_GAMEPAD_BUTTON_WEST),
                fx - face_gap,
                fy,
                face_r,
            ),
            circle(
                "triangle",
                Control::Button(SDL_GAMEPAD_BUTTON_NORTH),
                fx,
                fy - face_gap,
                face_r,
            ),
            rect(
                "l2",
                Control::Trigger(SDL_GAMEPAD_AXIS_LEFT_TRIGGER),
                left,
                l2_y,
                shoulder_w,
                shoulder_h,
            ),
            rect(
                "l1",
                Control::Button(SDL_GAMEPAD_BUTTON_LEFT_SHOULDER),
                left,
                l1_y,
                shoulder_w,
                shoulder_h,
            ),
            rect(
                "r2",
                Control::Trigger(SDL_GAMEPAD_AXIS_RIGHT_TRIGGER),
                right - shoulder_w,
                l2_y,
                shoulder_w,
                shoulder_h,
            ),
            rect(
                "r1",
                Control::Button(SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER),
                right - shoulder_w,
                l1_y,
                shoulder_w,
                shoulder_h,
            ),
            rect(
                "select",
                Control::Button(SDL_GAMEPAD_BUTTON_BACK),
                select.0,
                select.1,
                pill_w,
                pill_h,
            ),
            rect(
                "start",
                Control::Button(SDL_GAMEPAD_BUTTON_START),
                start.0,
                start.1,
                pill_w,
                pill_h,
            ),
            circle("dpad", Control::Dpad, left + dpad_r, dpad_y, dpad_r),
        ];
        let stick_radius = u * 0.14;
        // The left stick's resting place: right of the d-pad, clear of the weapon's gauge below it.
        let rest_x =
            (left + 2.0 * dpad_r + margin + stick_radius).max(hud.gauge_right * 0.5 + stick_radius);
        let rest_y = (hud.gauge_top - margin - stick_radius).min(bottom - stick_radius);
        Layout {
            width,
            height,
            elements,
            stick_radius,
            left_rest: (rest_x, rest_y),
        }
    }

    fn hit(&self, px: f32, py: f32) -> Option<&Element> {
        self.elements
            .iter()
            .find(|e| e.shape.contains(px, py, 1.0))
            .or_else(|| self.elements.iter().find(|e| e.shape.contains(px, py, 1.3)))
    }
}

// ---- State ----------------------------------------------------------------------------------------

struct Finger {
    control: Control,
    origin: (f32, f32),
    position: (f32, f32),
}

struct State {
    layout: Layout,
    fingers: HashMap<u64, Finger>,
    back_down: bool,
    joystick: usize, // *mut SDL_Joystick, kept as an address so State is Send
    instance: SDL_JoystickID,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// Real gamepads connected; the touch pad steps aside while there is one.
static REAL_PADS: Mutex<Vec<SDL_JoystickID>> = Mutex::new(Vec::new());
static WANT_ATTACHED: AtomicBool = AtomicBool::new(true);

/// What the virtual gamepad reports, from the fingers on the screen.
#[derive(Debug, Default, PartialEq)]
struct Outputs {
    buttons: u32,
    axes: [i16; 6],
}

fn deflection(finger: &Finger, radius: f32) -> (f32, f32) {
    let (dx, dy) = (
        finger.position.0 - finger.origin.0,
        finger.position.1 - finger.origin.1,
    );
    let length = dx.hypot(dy);
    if length < radius * 0.12 {
        return (0.0, 0.0);
    }
    let scale = (length / radius).min(1.0) / length;
    (dx * scale, dy * scale)
}

fn axis(value: f32) -> i16 {
    (value.clamp(-1.0, 1.0) * SDL_JOYSTICK_AXIS_MAX as f32) as i16
}

impl State {
    fn outputs(&self) -> Outputs {
        let mut out = Outputs::default();
        let radius = self.layout.stick_radius;
        for finger in self.fingers.values() {
            match finger.control {
                Control::Button(b) => out.buttons |= 1 << b,
                Control::Trigger(a) => out.axes[a as usize] = SDL_JOYSTICK_AXIS_MAX,
                Control::Dpad => {
                    let (dx, dy) = (
                        finger.position.0 - finger.origin.0,
                        finger.position.1 - finger.origin.1,
                    );
                    if dx.hypot(dy) > radius * 0.15 {
                        // Eight directions: a diagonal presses both of its neighbours.
                        let angle = dy.atan2(dx);
                        let sector =
                            ((angle / (std::f32::consts::PI / 4.0)).round() as i32).rem_euclid(8);
                        let (right, down, left, up) = (
                            matches!(sector, 7 | 0 | 1),
                            matches!(sector, 1..=3),
                            matches!(sector, 3..=5),
                            matches!(sector, 5..=7),
                        );
                        if up {
                            out.buttons |= 1 << SDL_GAMEPAD_BUTTON_DPAD_UP;
                        }
                        if down {
                            out.buttons |= 1 << SDL_GAMEPAD_BUTTON_DPAD_DOWN;
                        }
                        if left {
                            out.buttons |= 1 << SDL_GAMEPAD_BUTTON_DPAD_LEFT;
                        }
                        if right {
                            out.buttons |= 1 << SDL_GAMEPAD_BUTTON_DPAD_RIGHT;
                        }
                    }
                }
                Control::LeftStick => {
                    let (x, y) = deflection(finger, radius);
                    out.axes[SDL_GAMEPAD_AXIS_LEFTX as usize] = axis(x);
                    out.axes[SDL_GAMEPAD_AXIS_LEFTY as usize] = axis(y);
                }
                Control::RightStick => {
                    let (x, y) = deflection(finger, radius);
                    out.axes[SDL_GAMEPAD_AXIS_RIGHTX as usize] = axis(x);
                    out.axes[SDL_GAMEPAD_AXIS_RIGHTY as usize] = axis(y);
                }
            }
        }
        if self.back_down {
            out.buttons |= 1 << SDL_GAMEPAD_BUTTON_START;
        }
        out
    }

    fn finger_down(&mut self, id: u64, px: f32, py: f32) {
        let control = match self.layout.hit(px, py) {
            Some(element) => element.control,
            None if px < self.layout.width / 2.0 => Control::LeftStick,
            None => Control::RightStick,
        };
        let origin = match (control, self.layout.hit(px, py).map(|e| e.shape)) {
            (Control::Dpad, Some(Shape::Circle { x, y, .. })) => (x, y),
            _ => (px, py),
        };
        self.fingers.insert(
            id,
            Finger {
                control,
                origin,
                position: (px, py),
            },
        );
    }

    fn finger_motion(&mut self, id: u64, px: f32, py: f32) {
        let radius = self.layout.stick_radius;
        if let Some(finger) = self.fingers.get_mut(&id) {
            finger.position = (px, py);
            // A stick's centre follows a finger that runs past its edge, so reversing is immediate.
            if matches!(finger.control, Control::LeftStick | Control::RightStick) {
                let (dx, dy) = (px - finger.origin.0, py - finger.origin.1);
                let length = dx.hypot(dy);
                if length > radius {
                    let excess = (length - radius) / length;
                    finger.origin.0 += dx * excess;
                    finger.origin.1 += dy * excess;
                }
            }
        }
    }
}

// ---- The virtual gamepad -------------------------------------------------------------------------

/// SDL calls this from SDL_UpdateJoysticks on the game's thread, before it reads the pad's state.
unsafe extern "C" fn update(_userdata: *mut c_void) {
    let guard = STATE.lock().unwrap();
    let Some(state) = guard.as_ref() else { return };
    if state.joystick == 0 {
        return;
    }
    let joystick = state.joystick as *mut SDL_Joystick;
    let out = state.outputs();
    for button in 0..SDL_GAMEPAD_BUTTON_COUNT as c_int {
        SDL_SetJoystickVirtualButton(joystick, button, out.buttons & (1 << button) != 0);
    }
    for (index, value) in out.axes.iter().enumerate() {
        SDL_SetJoystickVirtualAxis(joystick, index as c_int, *value);
    }
}

/// Attaching and detaching push SDL events, which run the event watch; neither is called with STATE
/// held, or a finger arriving at that moment would wait on SDL while SDL waits on it.
fn attach() -> Option<(SDL_JoystickID, usize)> {
    let desc = SDL_VirtualJoystickDesc {
        version: std::mem::size_of::<SDL_VirtualJoystickDesc>() as u32,
        type_: SDL_JOYSTICK_TYPE_GAMEPAD,
        padding: 0,
        vendor_id: 0,
        product_id: 0,
        naxes: SDL_GAMEPAD_AXIS_COUNT,
        nbuttons: SDL_GAMEPAD_BUTTON_COUNT,
        nballs: 0,
        nhats: 0,
        ntouchpads: 0,
        nsensors: 0,
        padding2: [0; 2],
        button_mask: 0,
        axis_mask: 0,
        name: c"Touch screen".as_ptr(),
        touchpads: std::ptr::null(),
        sensors: std::ptr::null(),
        userdata: std::ptr::null_mut(),
        update: Some(update),
        set_player_index: None,
        rumble: None,
        rumble_triggers: None,
        set_led: None,
        send_effect: None,
        set_sensors_enabled: None,
        cleanup: None,
    };
    let instance = unsafe { SDL_AttachVirtualJoystick(&desc) };
    if instance == 0 {
        crate::logcat::write(&format!(
            "cannot attach the touch gamepad: {}",
            sdl::error()
        ));
        return None;
    }
    let joystick = unsafe { SDL_OpenJoystick(instance) };
    Some((instance, joystick as usize))
}

fn detach(instance: SDL_JoystickID, joystick: usize) {
    if joystick != 0 {
        unsafe { SDL_CloseJoystick(joystick as *mut SDL_Joystick) };
    }
    unsafe { SDL_DetachVirtualJoystick(instance) };
}

/// Brings the touch pad in or out as real gamepads come and go. On the game's thread, as `update` is,
/// so `update` never sees a pad half made.
fn sync_attached() {
    let want = WANT_ATTACHED.load(Ordering::Relaxed);
    let (instance, joystick) = match STATE.lock().unwrap().as_ref() {
        Some(state) => (state.instance, state.joystick),
        None => return,
    };
    if want && instance == 0 {
        if let Some((instance, joystick)) = attach() {
            if let Some(state) = STATE.lock().unwrap().as_mut() {
                state.instance = instance;
                state.joystick = joystick;
            }
        }
    } else if !want && instance != 0 {
        if let Some(state) = STATE.lock().unwrap().as_mut() {
            state.instance = 0;
            state.joystick = 0;
            state.fingers.clear();
        }
        detach(instance, joystick);
    }
}

// ---- Events --------------------------------------------------------------------------------------

unsafe extern "C" fn watch(_userdata: *mut c_void, event: *mut SDL_Event) -> bool {
    let kind = (*event).type_;
    match kind {
        SDL_EVENT_GAMEPAD_ADDED | SDL_EVENT_GAMEPAD_REMOVED => {
            // SDL holds its joystick lock here; attaching or detaching waits for the next frame.
            let which = (*event).gdevice.which;
            let mut pads = REAL_PADS.lock().unwrap();
            if kind == SDL_EVENT_GAMEPAD_ADDED {
                if !SDL_IsJoystickVirtual(which) && !pads.contains(&which) {
                    pads.push(which);
                }
            } else {
                pads.retain(|&pad| pad != which);
            }
            WANT_ATTACHED.store(pads.is_empty(), Ordering::Relaxed);
        }
        SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP => {
            let key = (*event).key;
            if key.scancode == SDL_SCANCODE_AC_BACK {
                if let Some(state) = STATE.lock().unwrap().as_mut() {
                    state.back_down = key.down;
                }
            }
        }
        _ => {}
    }
    true
}

pub fn init() {
    unsafe {
        // A finger on the controls must not also move the menus' mouse pointer.
        SDL_SetHint(SDL_HINT_TOUCH_MOUSE_EVENTS.as_ptr().cast(), c"0".as_ptr());
        // Back pauses (START) instead of leaving the app.
        SDL_SetHint(
            SDL_HINT_ANDROID_TRAP_BACK_BUTTON.as_ptr().cast(),
            c"1".as_ptr(),
        );
        if !SDL_InitSubSystem(SDL_INIT_GAMEPAD) {
            crate::logcat::write(&format!("SDL_InitSubSystem(GAMEPAD): {}", sdl::error()));
        }
    }
    ensure_state();
    unsafe { SDL_AddEventWatch(watch, std::ptr::null_mut()) };
    sync_attached();
}

/// The view may hand over its layout before the game starts; either makes the state.
fn ensure_state() {
    let mut guard = STATE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(State {
            layout: Layout::new(1920.0, 1080.0, Insets::default()),
            fingers: HashMap::new(),
            back_down: false,
            joystick: 0,
            instance: 0,
        });
    }
}

// ---- The view's side ------------------------------------------------------------------------------

/// A control as the view draws it and an arrangement saves it, in window pixels.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ElementJson {
    pub id: String,
    pub shape: String,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub r: f32,
    #[serde(default)]
    pub w: f32,
    #[serde(default)]
    pub h: f32,
    #[serde(default = "full")]
    pub opacity: f32,
}

fn full() -> f32 {
    1.0
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LayoutJson {
    pub width: f32,
    pub height: f32,
    pub stick_radius: f32,
    pub left_rest: [f32; 2],
    pub elements: Vec<ElementJson>,
}

impl Layout {
    fn to_json(&self) -> LayoutJson {
        let element = |e: &Element| {
            let (shape, x, y, r, w, h) = match e.shape {
                Shape::Circle { x, y, r } => ("circle", x, y, r, 0.0, 0.0),
                Shape::Rect { x, y, w, h } => ("rect", x, y, 0.0, w, h),
            };
            ElementJson {
                id: e.id.into(),
                shape: shape.into(),
                x,
                y,
                r,
                w,
                h,
                opacity: e.opacity,
            }
        };
        LayoutJson {
            width: self.width,
            height: self.height,
            stick_radius: self.stick_radius,
            left_rest: [self.left_rest.0, self.left_rest.1],
            elements: self.elements.iter().map(element).collect(),
        }
    }

    /// The player's arrangement laid over this layout: each control it names takes its place, size and
    /// opacity, scaled from the window it was made on to this one (sizes by the smaller ratio, so a
    /// round button stays round); a control it does not name keeps the default's.
    fn arranged(mut self, saved: &LayoutJson) -> Layout {
        if !(saved.width > 0.0 && saved.height > 0.0) {
            return self;
        }
        let sx = self.width / saved.width;
        let sy = self.height / saved.height;
        let size = sx.min(sy);
        let place = |v: f32| v.is_finite() && v >= 0.0;
        for element in &mut self.elements {
            let Some(mine) = saved.elements.iter().find(|e| e.id == element.id) else {
                continue;
            };
            if !place(mine.x) || !place(mine.y) {
                continue;
            }
            match element.shape {
                Shape::Circle { .. } if mine.shape == "circle" && mine.r > 0.0 => {
                    element.shape = Shape::Circle {
                        x: mine.x * sx,
                        y: mine.y * sy,
                        r: mine.r * size,
                    };
                }
                Shape::Rect { .. } if mine.shape == "rect" && mine.w > 0.0 && mine.h > 0.0 => {
                    element.shape = Shape::Rect {
                        x: mine.x * sx,
                        y: mine.y * sy,
                        w: mine.w * size,
                        h: mine.h * size,
                    };
                }
                _ => {}
            }
            if mine.opacity.is_finite() {
                element.opacity = mine.opacity.clamp(0.05, 1.0);
            }
        }
        self
    }
}

/// The default layout for a window `width` x `height` with `insets`, as JSON: what the view offers
/// when the player resets theirs.
pub fn default_layout(width: f32, height: f32, insets: Insets) -> String {
    serde_json::to_string(&Layout::new(width, height, insets).to_json()).unwrap_or_default()
}

/// Makes the controls the default for this window with the player's arrangement `saved` (JSON, as
/// this returned it) over it, and returns the result as JSON for the view to draw.
pub fn set_layout(width: f32, height: f32, insets: Insets, saved: Option<&str>) -> String {
    let mut layout = Layout::new(width, height, insets);
    if let Some(saved) = saved.and_then(|text| serde_json::from_str::<LayoutJson>(text).ok()) {
        layout = layout.arranged(&saved);
    }
    let json = serde_json::to_string(&layout.to_json()).unwrap_or_default();
    ensure_state();
    if let Some(state) = STATE.lock().unwrap().as_mut() {
        state.layout = layout;
        state.fingers.clear();
    }
    json
}

/// A touch from the view, in window pixels: action 0 down, 1 move, 2 up or cancelled.
pub fn touch(action: i32, id: u64, x: f32, y: f32) {
    let mut guard = STATE.lock().unwrap();
    let Some(state) = guard.as_mut() else {
        return;
    };
    if state.instance == 0 {
        return;
    }
    match action {
        0 => state.finger_down(id, x, y),
        1 => state.finger_motion(id, x, y),
        _ => {
            state.fingers.remove(&id);
        }
    }
}

/// What the view draws pressed: the buttons down as SDL_GamepadButton bits (L2 and R2 as bits 30
/// and 31), each stick a finger holds (its centre, its knob, whether it is the left stick), and
/// whether the touch pad is in use at all (not while a controller is connected).
pub fn pad_state() -> String {
    let guard = STATE.lock().unwrap();
    let Some(state) = guard.as_ref() else {
        return "{}".into();
    };
    let out = state.outputs();
    let mut buttons = out.buttons;
    if out.axes[SDL_GAMEPAD_AXIS_LEFT_TRIGGER as usize] != 0 {
        buttons |= 1 << 30;
    }
    if out.axes[SDL_GAMEPAD_AXIS_RIGHT_TRIGGER as usize] != 0 {
        buttons |= 1 << 31;
    }
    let radius = state.layout.stick_radius;
    let sticks: Vec<serde_json::Value> = state
        .fingers
        .values()
        .filter(|f| matches!(f.control, Control::LeftStick | Control::RightStick))
        .map(|f| {
            let (dx, dy) = deflection(f, radius);
            serde_json::json!({
                "x": f.origin.0,
                "y": f.origin.1,
                "knob_x": f.origin.0 + dx * radius,
                "knob_y": f.origin.1 + dy * radius,
                "left": f.control == Control::LeftStick,
            })
        })
        .collect();
    serde_json::json!({"buttons": buttons, "sticks": sticks, "active": state.instance != 0})
        .to_string()
}

/// Between the game's ticks, on its thread: the touch pad comes or goes as controllers do.
pub fn pump() {
    sync_attached();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            layout: Layout::new(2340.0, 1080.0, Insets::default()),
            fingers: HashMap::new(),
            back_down: false,
            joystick: 0,
            instance: 1,
        }
    }

    fn centre(state: &State, control: Control) -> (f32, f32) {
        let element = state
            .layout
            .elements
            .iter()
            .find(|e| e.control == control)
            .unwrap();
        match element.shape {
            Shape::Circle { x, y, .. } => (x, y),
            Shape::Rect { x, y, w, h } => (x + w / 2.0, y + h / 2.0),
        }
    }

    #[test]
    fn face_buttons_press_their_gamepad_buttons() {
        let mut state = state();
        for button in [
            SDL_GAMEPAD_BUTTON_SOUTH,
            SDL_GAMEPAD_BUTTON_EAST,
            SDL_GAMEPAD_BUTTON_WEST,
            SDL_GAMEPAD_BUTTON_NORTH,
        ] {
            let (x, y) = centre(&state, Control::Button(button));
            state.finger_down(7, x, y);
            assert_eq!(state.outputs().buttons, 1 << button);
            state.fingers.remove(&7);
        }
        assert_eq!(state.outputs(), Outputs::default());
    }

    #[test]
    fn triggers_are_full_axes() {
        let mut state = state();
        let (x, y) = centre(&state, Control::Trigger(SDL_GAMEPAD_AXIS_RIGHT_TRIGGER));
        state.finger_down(1, x, y);
        assert_eq!(
            state.outputs().axes[SDL_GAMEPAD_AXIS_RIGHT_TRIGGER as usize],
            SDL_JOYSTICK_AXIS_MAX
        );
    }

    #[test]
    fn empty_halves_are_floating_sticks() {
        let mut state = state();
        let radius = state.layout.stick_radius;
        state.finger_down(1, 700.0, 900.0);
        state.finger_down(2, 1600.0, 600.0);
        state.finger_motion(1, 700.0 + radius * 3.0, 900.0);
        state.finger_motion(2, 1600.0, 600.0 - radius * 0.5);
        let out = state.outputs();
        assert_eq!(
            out.axes[SDL_GAMEPAD_AXIS_LEFTX as usize],
            SDL_JOYSTICK_AXIS_MAX
        );
        assert_eq!(out.axes[SDL_GAMEPAD_AXIS_LEFTY as usize], 0);
        assert!(
            (out.axes[SDL_GAMEPAD_AXIS_RIGHTY as usize] as f32
                + SDL_JOYSTICK_AXIS_MAX as f32 * 0.5)
                .abs()
                < 400.0
        );
        // The left stick's centre followed the finger past the edge: reversing a little goes back at once.
        state.finger_motion(1, 700.0 + radius * 1.5, 900.0);
        assert!(state.outputs().axes[SDL_GAMEPAD_AXIS_LEFTX as usize] < 0);
    }

    #[test]
    fn small_movements_are_dead_zone() {
        let mut state = state();
        state.finger_down(1, 700.0, 900.0);
        state.finger_motion(1, 703.0, 899.0);
        assert_eq!(state.outputs(), Outputs::default());
    }

    #[test]
    fn dpad_diagonals_press_both_directions() {
        let mut state = state();
        let (x, y) = centre(&state, Control::Dpad);
        let r = state.layout.stick_radius;
        state.finger_down(1, x + r * 0.5, y - r * 0.5);
        let up_right = (1 << SDL_GAMEPAD_BUTTON_DPAD_UP) | (1 << SDL_GAMEPAD_BUTTON_DPAD_RIGHT);
        assert_eq!(state.outputs().buttons, up_right);
        state.finger_motion(1, x, y + r * 0.6);
        assert_eq!(state.outputs().buttons, 1 << SDL_GAMEPAD_BUTTON_DPAD_DOWN);
    }

    #[test]
    fn back_is_start() {
        let mut state = state();
        state.back_down = true;
        assert_eq!(state.outputs().buttons, 1 << SDL_GAMEPAD_BUTTON_START);
    }

    #[test]
    fn controls_do_not_overlap() {
        let layout = Layout::new(2340.0, 1080.0, Insets::default());
        for (i, a) in layout.elements.iter().enumerate() {
            let (x, y) = match a.shape {
                Shape::Circle { x, y, .. } => (x, y),
                Shape::Rect { x, y, w, h } => (x + w / 2.0, y + h / 2.0),
            };
            for (j, b) in layout.elements.iter().enumerate() {
                assert!(
                    i == j || !b.shape.contains(x, y, 1.0),
                    "{i} and {j} overlap"
                );
            }
        }
    }

    fn bounds(shape: Shape) -> (f32, f32, f32, f32) {
        match shape {
            Shape::Circle { x, y, r } => (x - r, y - r, x + r, y + r),
            Shape::Rect { x, y, w, h } => (x, y, x + w, y + h),
        }
    }

    fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
        a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
    }

    /// The S26 in landscape with its camera hole on the left, a 16:9 phone and a 4:3 tablet: every
    /// control stays in the safe area and out of the HUD the port moves to the window's edges.
    #[test]
    fn controls_keep_out_of_the_hud() {
        for (width, height, insets) in [
            (
                2340.0,
                1080.0,
                Insets {
                    left: 110.0,
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                },
            ),
            (2340.0, 1080.0, Insets::default()),
            (
                1920.0,
                1080.0,
                Insets {
                    left: 0.0,
                    top: 0.0,
                    right: 0.0,
                    bottom: 48.0,
                },
            ),
            (2048.0, 1536.0, Insets::default()),
        ] {
            let layout = Layout::new(width, height, insets);
            let hud = Hud::new(width, height, insets);
            let status = (0.0, 0.0, hud.left_gap.0, hud.top_band);
            let items = (hud.left_gap.1, 0.0, hud.right_gap.0, hud.top_band);
            let plate = (hud.right_gap.1, 0.0, width, hud.top_band);
            let gauge = (0.0, hud.gauge_top, hud.gauge_right, height);
            for element in &layout.elements {
                let b = bounds(element.shape);
                assert!(
                    b.0 >= insets.left
                        && b.1 >= insets.top
                        && b.2 <= width - insets.right
                        && b.3 <= height - insets.bottom,
                    "{width}x{height}: a control leaves the safe area"
                );
                for (name, zone) in [
                    ("status", status),
                    ("quick items", items),
                    ("floor plate", plate),
                    ("gauge", gauge),
                ] {
                    assert!(
                        !overlaps(b, zone),
                        "{width}x{height}: a control covers the {name}"
                    );
                }
            }
            let (x, y) = layout.left_rest;
            let r = layout.stick_radius;
            assert!(
                !overlaps((x - r, y - r, x + r, y + r), gauge),
                "{width}x{height}: the stick covers the gauge"
            );
        }
    }

    #[test]
    fn select_and_start_go_to_the_bottom_without_room_at_the_top() {
        let narrow = Layout::new(2048.0, 1536.0, Insets::default());
        let wide = Layout::new(2340.0, 1080.0, Insets::default());
        let y = |layout: &Layout| match layout
            .elements
            .iter()
            .find(|e| e.control == Control::Button(SDL_GAMEPAD_BUTTON_START))
            .unwrap()
            .shape
        {
            Shape::Rect { y, .. } => y,
            Shape::Circle { y, .. } => y,
        };
        assert!(y(&narrow) > 1536.0 / 2.0);
        assert!(y(&wide) < 1080.0 / 4.0);
    }
}

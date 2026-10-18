//! Touch controls: an SDL virtual gamepad driven by the screen, and the overlay that shows it.
//!
//! doukutsu-rs puts its touch controls inside the game's player logic. Dark Cloud's input already goes
//! through SDL gamepads (tools/kazarov/port/platform/input.cpp), so here the screen is a gamepad
//! instead: everything above it, button prompts and the camera included, sees a controller.
//!
//! Threads: SDL delivers finger events on the activity's thread, through the event watch, which only
//! records them here. SDL's joystick calls run on the game's thread, in the virtual gamepad's Update
//! callback and in `chronicle_touch_geometry`, which the game calls once per presented frame. Neither
//! lock is ever held while waiting for the other.

use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

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

#[derive(Copy, Clone, PartialEq, Eq)]
enum Symbol {
    Cross,
    Circle,
    Square,
    Triangle,
    Text(&'static str),
    Dpad,
}

struct Element {
    control: Control,
    shape: Shape,
    symbol: Symbol,
    color: [u8; 3],
}

struct Layout {
    width: f32,
    height: f32,
    elements: Vec<Element>,
    stick_radius: f32,
    left_rest: (f32, f32),
}

impl Layout {
    fn new(width: f32, height: f32) -> Layout {
        let u = width.min(height);
        let margin = u * 0.05;
        let face_r = u * 0.075;
        let face_gap = u * 0.125;
        let (fx, fy) = (
            width - margin - face_gap - face_r,
            height - margin - face_gap - face_r,
        );
        let shoulder_w = u * 0.24;
        let shoulder_h = u * 0.11;
        let pill_w = u * 0.2;
        let pill_h = u * 0.075;
        let dpad_r = u * 0.13;
        let elements = vec![
            Element {
                control: Control::Button(SDL_GAMEPAD_BUTTON_SOUTH),
                shape: Shape::Circle {
                    x: fx,
                    y: fy + face_gap,
                    r: face_r,
                },
                symbol: Symbol::Cross,
                color: [0x7C, 0xA8, 0xFF],
            },
            Element {
                control: Control::Button(SDL_GAMEPAD_BUTTON_EAST),
                shape: Shape::Circle {
                    x: fx + face_gap,
                    y: fy,
                    r: face_r,
                },
                symbol: Symbol::Circle,
                color: [0xFF, 0x6E, 0x6E],
            },
            Element {
                control: Control::Button(SDL_GAMEPAD_BUTTON_WEST),
                shape: Shape::Circle {
                    x: fx - face_gap,
                    y: fy,
                    r: face_r,
                },
                symbol: Symbol::Square,
                color: [0xF4, 0x8C, 0xD8],
            },
            Element {
                control: Control::Button(SDL_GAMEPAD_BUTTON_NORTH),
                shape: Shape::Circle {
                    x: fx,
                    y: fy - face_gap,
                    r: face_r,
                },
                symbol: Symbol::Triangle,
                color: [0x5C, 0xE6, 0xB8],
            },
            Element {
                control: Control::Trigger(SDL_GAMEPAD_AXIS_LEFT_TRIGGER),
                shape: Shape::Rect {
                    x: margin,
                    y: margin,
                    w: shoulder_w,
                    h: shoulder_h,
                },
                symbol: Symbol::Text("L2"),
                color: [0xFF; 3],
            },
            Element {
                control: Control::Button(SDL_GAMEPAD_BUTTON_LEFT_SHOULDER),
                shape: Shape::Rect {
                    x: margin,
                    y: margin * 1.4 + shoulder_h,
                    w: shoulder_w,
                    h: shoulder_h,
                },
                symbol: Symbol::Text("L1"),
                color: [0xFF; 3],
            },
            Element {
                control: Control::Trigger(SDL_GAMEPAD_AXIS_RIGHT_TRIGGER),
                shape: Shape::Rect {
                    x: width - margin - shoulder_w,
                    y: margin,
                    w: shoulder_w,
                    h: shoulder_h,
                },
                symbol: Symbol::Text("R2"),
                color: [0xFF; 3],
            },
            Element {
                control: Control::Button(SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER),
                shape: Shape::Rect {
                    x: width - margin - shoulder_w,
                    y: margin * 1.4 + shoulder_h,
                    w: shoulder_w,
                    h: shoulder_h,
                },
                symbol: Symbol::Text("R1"),
                color: [0xFF; 3],
            },
            Element {
                control: Control::Button(SDL_GAMEPAD_BUTTON_BACK),
                shape: Shape::Rect {
                    x: width / 2.0 - pill_w - margin * 0.5,
                    y: margin,
                    w: pill_w,
                    h: pill_h,
                },
                symbol: Symbol::Text("SELECT"),
                color: [0xFF; 3],
            },
            Element {
                control: Control::Button(SDL_GAMEPAD_BUTTON_START),
                shape: Shape::Rect {
                    x: width / 2.0 + margin * 0.5,
                    y: margin,
                    w: pill_w,
                    h: pill_h,
                },
                symbol: Symbol::Text("START"),
                color: [0xFF; 3],
            },
            Element {
                control: Control::Dpad,
                shape: Shape::Circle {
                    x: margin + dpad_r,
                    y: height * 0.5,
                    r: dpad_r,
                },
                symbol: Symbol::Dpad,
                color: [0xFF; 3],
            },
        ];
        Layout {
            width,
            height,
            elements,
            stick_radius: u * 0.14,
            left_rest: (margin + u * 0.3, height - margin - u * 0.2),
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
/// Bumped whenever the overlay looks different, so the game records it again only then.
static GENERATION: AtomicU64 = AtomicU64::new(1);
/// Real gamepads connected; the touch pad steps aside while there is one.
static REAL_PADS: Mutex<Vec<SDL_JoystickID>> = Mutex::new(Vec::new());
static WANT_ATTACHED: AtomicBool = AtomicBool::new(true);

fn changed() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

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
            changed();
        }
    } else if !want && instance != 0 {
        if let Some(state) = STATE.lock().unwrap().as_mut() {
            state.instance = 0;
            state.joystick = 0;
            state.fingers.clear();
        }
        detach(instance, joystick);
        changed();
    }
}

// ---- Events --------------------------------------------------------------------------------------

unsafe extern "C" fn watch(_userdata: *mut c_void, event: *mut SDL_Event) -> bool {
    let kind = (*event).type_;
    match kind {
        SDL_EVENT_FINGER_DOWN
        | SDL_EVENT_FINGER_MOTION
        | SDL_EVENT_FINGER_UP
        | SDL_EVENT_FINGER_CANCELED => {
            let finger = (*event).tfinger;
            let mut guard = STATE.lock().unwrap();
            let Some(state) = guard.as_mut() else {
                return true;
            };
            if state.instance == 0 {
                return true;
            }
            let (px, py) = (
                finger.x * state.layout.width,
                finger.y * state.layout.height,
            );
            match kind {
                SDL_EVENT_FINGER_DOWN => state.finger_down(finger.finger_id, px, py),
                SDL_EVENT_FINGER_MOTION => state.finger_motion(finger.finger_id, px, py),
                _ => {
                    state.fingers.remove(&finger.finger_id);
                }
            }
            changed();
        }
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
    *STATE.lock().unwrap() = Some(State {
        layout: Layout::new(1920.0, 1080.0),
        fingers: HashMap::new(),
        back_down: false,
        joystick: 0,
        instance: 0,
    });
    unsafe { SDL_AddEventWatch(watch, std::ptr::null_mut()) };
    sync_attached();
}

// ---- The overlay ---------------------------------------------------------------------------------

/// A vertex of the overlay's triangles, in window pixels. Alpha is GS-style: 0x80 is opaque.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct TouchVertex {
    pub x: f32,
    pub y: f32,
    pub color: [u8; 4],
}

struct Mesh<'a> {
    out: &'a mut [TouchVertex],
    count: usize,
}

impl Mesh<'_> {
    fn triangle(&mut self, points: [(f32, f32); 3], color: [u8; 4]) {
        if self.count + 3 > self.out.len() {
            return;
        }
        for (x, y) in points {
            self.out[self.count] = TouchVertex { x, y, color };
            self.count += 1;
        }
    }

    fn quad(&mut self, a: (f32, f32), b: (f32, f32), c: (f32, f32), d: (f32, f32), color: [u8; 4]) {
        self.triangle([a, b, c], color);
        self.triangle([a, c, d], color);
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        self.quad((x, y), (x + w, y), (x + w, y + h), (x, y + h), color);
    }

    fn disc(&mut self, cx: f32, cy: f32, r: f32, color: [u8; 4]) {
        let steps = 32;
        for i in 0..steps {
            let a0 = i as f32 / steps as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / steps as f32 * std::f32::consts::TAU;
            self.triangle(
                [
                    (cx, cy),
                    (cx + r * a0.cos(), cy + r * a0.sin()),
                    (cx + r * a1.cos(), cy + r * a1.sin()),
                ],
                color,
            );
        }
    }

    fn ring(&mut self, cx: f32, cy: f32, r: f32, width: f32, color: [u8; 4]) {
        let steps = 32;
        let inner = r - width;
        for i in 0..steps {
            let a0 = i as f32 / steps as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / steps as f32 * std::f32::consts::TAU;
            self.quad(
                (cx + inner * a0.cos(), cy + inner * a0.sin()),
                (cx + r * a0.cos(), cy + r * a0.sin()),
                (cx + r * a1.cos(), cy + r * a1.sin()),
                (cx + inner * a1.cos(), cy + inner * a1.sin()),
                color,
            );
        }
    }

    fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, color: [u8; 4]) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length = dx.hypot(dy).max(1e-3);
        let (nx, ny) = (-dy / length * width / 2.0, dx / length * width / 2.0);
        self.quad(
            (a.0 + nx, a.1 + ny),
            (b.0 + nx, b.1 + ny),
            (b.0 - nx, b.1 - ny),
            (a.0 - nx, a.1 - ny),
            color,
        );
    }

    fn text(&mut self, text: &str, cx: f32, cy: f32, pixel: f32, color: [u8; 4]) {
        let width = text.len() as f32 * 6.0 * pixel - pixel;
        let mut pen = cx - width / 2.0;
        let top = cy - 3.5 * pixel;
        for c in text.chars() {
            if let Some(rows) = glyph(c) {
                for (row, bits) in rows.iter().enumerate() {
                    for column in 0..5 {
                        if bits & (0x10 >> column) != 0 {
                            self.rect(
                                pen + column as f32 * pixel,
                                top + row as f32 * pixel,
                                pixel,
                                pixel,
                                color,
                            );
                        }
                    }
                }
            }
            pen += 6.0 * pixel;
        }
    }
}

/// The 5x7 rows of the letters the labels use, from tools/kazarov/port/platform/overlay.cpp's font.
fn glyph(c: char) -> Option<[u8; 7]> {
    Some(match c {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        _ => return None,
    })
}

const FILL: u8 = 0x14;
const FILL_PRESSED: u8 = 0x3C;
const EDGE: u8 = 0x38;
const MARK: u8 = 0x60;

fn rgba(rgb: [u8; 3], alpha: u8) -> [u8; 4] {
    [rgb[0], rgb[1], rgb[2], alpha]
}

fn draw(state: &State, mesh: &mut Mesh) {
    let pressed = state.outputs();
    let layout = &state.layout;
    let u = layout.width.min(layout.height);
    let white = [0xFF; 3];
    for element in &layout.elements {
        let down = match element.control {
            Control::Button(b) => pressed.buttons & (1 << b) != 0,
            Control::Trigger(a) => pressed.axes[a as usize] != 0,
            _ => false,
        };
        let fill = rgba(white, if down { FILL_PRESSED } else { FILL });
        match element.shape {
            Shape::Circle { x, y, r } => {
                mesh.disc(x, y, r, fill);
                mesh.ring(x, y, r, u * 0.006, rgba(white, EDGE));
                let mark = rgba(element.color, MARK);
                let s = r * 0.42;
                let w = u * 0.011;
                match element.symbol {
                    Symbol::Cross => {
                        mesh.line((x - s, y - s), (x + s, y + s), w, mark);
                        mesh.line((x - s, y + s), (x + s, y - s), w, mark);
                    }
                    Symbol::Circle => mesh.ring(x, y, s * 1.15, w, mark),
                    Symbol::Square => {
                        let c = [
                            (x - s, y - s),
                            (x + s, y - s),
                            (x + s, y + s),
                            (x - s, y + s),
                        ];
                        for i in 0..4 {
                            mesh.line(c[i], c[(i + 1) % 4], w, mark);
                        }
                    }
                    Symbol::Triangle => {
                        let c = [
                            (x, y - s * 1.15),
                            (x + s * 1.1, y + s * 0.75),
                            (x - s * 1.1, y + s * 0.75),
                        ];
                        for i in 0..3 {
                            mesh.line(c[i], c[(i + 1) % 3], w, mark);
                        }
                    }
                    Symbol::Dpad => {
                        let arm = r * 0.62;
                        let half = r * 0.2;
                        let bits = pressed.buttons;
                        for (button, dx, dy) in [
                            (SDL_GAMEPAD_BUTTON_DPAD_UP, 0.0, -1.0),
                            (SDL_GAMEPAD_BUTTON_DPAD_DOWN, 0.0, 1.0),
                            (SDL_GAMEPAD_BUTTON_DPAD_LEFT, -1.0, 0.0),
                            (SDL_GAMEPAD_BUTTON_DPAD_RIGHT, 1.0, 0.0),
                        ] {
                            let alpha = if bits & (1 << button) != 0 {
                                FILL_PRESSED * 2
                            } else {
                                EDGE
                            };
                            let (tx, ty) = (x + dx * arm, y + dy * arm);
                            let (bx, by) = (x + dx * half, y + dy * half);
                            let (px, py) = (-dy * half, dx * half);
                            mesh.quad(
                                (bx + px, by + py),
                                (tx + px, ty + py),
                                (tx - px, ty - py),
                                (bx - px, by - py),
                                rgba(white, alpha.min(0x80)),
                            );
                        }
                    }
                    Symbol::Text(_) => {}
                }
            }
            Shape::Rect { x, y, w, h } => {
                mesh.rect(x, y, w, h, fill);
                let edge = u * 0.005;
                let e = rgba(white, EDGE);
                mesh.rect(x, y, w, edge, e);
                mesh.rect(x, y + h - edge, w, edge, e);
                mesh.rect(x, y, edge, h, e);
                mesh.rect(x + w - edge, y, edge, h, e);
                if let Symbol::Text(label) = element.symbol {
                    let pixel = (h * 0.075).max(1.0).round();
                    mesh.text(label, x + w / 2.0, y + h / 2.0, pixel, rgba(white, MARK));
                }
            }
        }
    }
    let radius = layout.stick_radius;
    let mut left_drawn = false;
    for finger in state.fingers.values() {
        if !matches!(finger.control, Control::LeftStick | Control::RightStick) {
            continue;
        }
        left_drawn |= finger.control == Control::LeftStick;
        let (x, y) = deflection(finger, radius);
        mesh.disc(finger.origin.0, finger.origin.1, radius, rgba(white, FILL));
        mesh.ring(
            finger.origin.0,
            finger.origin.1,
            radius,
            u * 0.006,
            rgba(white, EDGE),
        );
        mesh.disc(
            finger.origin.0 + x * radius,
            finger.origin.1 + y * radius,
            radius * 0.45,
            rgba(white, FILL_PRESSED),
        );
    }
    if !left_drawn {
        // Where the left stick is, faintly, until a thumb puts it somewhere else.
        let (x, y) = layout.left_rest;
        mesh.ring(x, y, radius, u * 0.005, rgba(white, FILL * 2));
        mesh.disc(x, y, radius * 0.45, rgba(white, FILL));
    }
}

/// Changes whenever the overlay's picture does; 0 while there is no overlay.
#[no_mangle]
pub extern "C" fn chronicle_touch_generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// The overlay as triangles for a window `width` x `height` pixels: the number of vertices written
/// to `out` (at most `capacity`), 0 when the touch controls are hidden. Called by the game on its
/// own thread once per presented frame, which is also where the touch pad comes and goes.
///
/// # Safety
///
/// `out` has room for `capacity` vertices.
#[no_mangle]
pub unsafe extern "C" fn chronicle_touch_geometry(
    width: u32,
    height: u32,
    out: *mut TouchVertex,
    capacity: usize,
) -> usize {
    sync_attached();
    let mut guard = STATE.lock().unwrap();
    let Some(state) = guard.as_mut() else {
        return 0;
    };
    if width > 0
        && height > 0
        && (state.layout.width != width as f32 || state.layout.height != height as f32)
    {
        state.layout = Layout::new(width as f32, height as f32);
        state.fingers.clear();
        changed();
    }
    if state.instance == 0 || out.is_null() || capacity == 0 {
        return 0;
    }
    let mut mesh = Mesh {
        out: std::slice::from_raw_parts_mut(out, capacity),
        count: 0,
    };
    draw(state, &mut mesh);
    mesh.count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            layout: Layout::new(2340.0, 1080.0),
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
        let layout = Layout::new(2340.0, 1080.0);
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

    #[test]
    fn overlay_fits_its_buffer() {
        let mut state = state();
        state.finger_down(1, 700.0, 900.0);
        state.finger_down(2, 1600.0, 600.0);
        let mut buffer = vec![
            TouchVertex {
                x: 0.0,
                y: 0.0,
                color: [0; 4]
            };
            32768
        ];
        let mut mesh = Mesh {
            out: &mut buffer,
            count: 0,
        };
        draw(&state, &mut mesh);
        assert!(mesh.count > 0 && mesh.count < 32768 && mesh.count.is_multiple_of(3));
    }
}

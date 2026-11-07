//! The game reports through stdout and stderr, which Android discards; doukutsu-rs's drsandroid pipes
//! both into logcat the same way. `adb logcat -s Chronicle` shows them.
//!
//! A phone without a computer has no logcat to read, so every line also goes into `save/logs/` in the
//! app's storage: `chronicle.log` for this run, `chronicle-previous.log` for the one before. A crash
//! (a fatal signal) writes where it happened there before Android's own handler takes over, and the
//! setup screen offers the last run's log for sharing when that run ended badly (CrashReport.java).

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::os::fd::IntoRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Once, OnceLock};
use std::time::Instant;

const ANDROID_LOG_INFO: c_int = 4;
const ANDROID_LOG_FATAL: c_int = 7;

pub const CURRENT: &str = "chronicle.log";
pub const PREVIOUS: &str = "chronicle-previous.log";
/// A run that logs in a loop stops filling the phone here.
const LIMIT: u64 = 16 << 20;

static FILE: AtomicI32 = AtomicI32::new(-1);
static WRITTEN: AtomicU64 = AtomicU64::new(0);
static START: OnceLock<Instant> = OnceLock::new();

extern "C" {
    fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

pub fn write(line: &str) {
    if let Ok(text) = CString::new(line) {
        unsafe { __android_log_write(ANDROID_LOG_INFO, c"Chronicle".as_ptr(), text.as_ptr()) };
    }
    let seconds = START
        .get()
        .map_or(0.0, |start| start.elapsed().as_secs_f64());
    append(format!("{seconds:9.3} {line}\n").as_bytes());
}

/// One write per line, so lines from different threads stay whole in the O_APPEND file.
fn append(bytes: &[u8]) {
    let fd = FILE.load(Ordering::Relaxed);
    if fd < 0 || WRITTEN.fetch_add(bytes.len() as u64, Ordering::Relaxed) > LIMIT {
        return;
    }
    unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
}

/// Starts this run's log in `dir` (the last run's kept beside it), takes over stdout and stderr, and
/// installs the crash handler. The first call does it; the setup screen and the game both call it.
pub fn open(dir: &Path) {
    static OPENED: Once = Once::new();
    OPENED.call_once(|| {
        start_file(dir);
        redirect_stdio();
        unsafe { install_crash_handler() };
        write(&format!("log of process {}", std::process::id()));
    });
}

fn start_file(dir: &Path) {
    START.get_or_init(Instant::now);
    let _ = fs::create_dir_all(dir);
    let current = dir.join(CURRENT);
    if current.exists() {
        let _ = fs::rename(&current, dir.join(PREVIOUS));
    }
    match OpenOptions::new().create(true).append(true).open(&current) {
        Ok(file) => FILE.store(file.into_raw_fd(), Ordering::Relaxed),
        Err(error) => write(&format!("cannot write {}: {error}", current.display())),
    }
}

// ---- stdout and stderr -------------------------------------------------------------------------

static PIPE: AtomicI32 = AtomicI32::new(-1);
/// The reader holds bytes it has not written out yet.
static BUSY: AtomicBool = AtomicBool::new(false);

fn redirect_stdio() {
    let mut pipe = [0 as c_int; 2];
    unsafe {
        if libc::pipe(pipe.as_mut_ptr()) != 0 {
            return;
        }
        libc::dup2(pipe[1], libc::STDOUT_FILENO);
        libc::dup2(pipe[1], libc::STDERR_FILENO);
        libc::close(pipe[1]);
        // C's stdout is fully buffered once it is not a terminal; lines should reach logcat as written.
        libc::setvbuf(stdout_file(), std::ptr::null_mut(), libc::_IOLBF, 0);
        // The game stops with exit() after printing why (a missing Vulkan feature, say); the reader
        // gets to write that line down before the process goes.
        libc::atexit(drain_at_exit);
    }
    let fd = pipe[0];
    PIPE.store(fd, Ordering::Relaxed);
    std::thread::Builder::new()
        .name("logcat".into())
        .spawn(move || {
            let mut buffer = [0u8; 4096];
            let mut pending = Vec::new();
            loop {
                BUSY.store(!pending.is_empty(), Ordering::Relaxed);
                let n = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
                if n <= 0 {
                    if n < 0
                        && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                    {
                        continue;
                    }
                    break;
                }
                BUSY.store(true, Ordering::Relaxed);
                pending.extend_from_slice(&buffer[..n as usize]);
                while let Some(end) = pending.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=end).collect();
                    write(String::from_utf8_lossy(&line[..end]).trim_end_matches('\r'));
                }
            }
        })
        .ok();
}

/// Waits up to `limit_ms` for the reader to take everything written to stdout and stderr so far.
fn wait_for_reader(limit_ms: u32) {
    let fd = PIPE.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    for _ in 0..limit_ms / 10 {
        let mut queued: c_int = 0;
        unsafe { libc::ioctl(fd, libc::FIONREAD, &mut queued) };
        if queued == 0 && !BUSY.load(Ordering::Relaxed) {
            break;
        }
        sleep_ms(10);
    }
    sleep_ms(20);
}

fn sleep_ms(ms: u32) {
    let time = libc::timespec {
        tv_sec: 0,
        tv_nsec: ms as libc::c_long * 1_000_000,
    };
    unsafe { libc::nanosleep(&time, std::ptr::null_mut()) };
}

extern "C" fn drain_at_exit() {
    unsafe { libc::fflush(std::ptr::null_mut()) };
    wait_for_reader(500);
}

extern "C" {
    static stdout: *mut libc::FILE;
}

fn stdout_file() -> *mut libc::FILE {
    unsafe { stdout }
}

// ---- Crashes -----------------------------------------------------------------------------------

const SIGNALS: [c_int; 7] = [
    libc::SIGSEGV,
    libc::SIGBUS,
    libc::SIGFPE,
    libc::SIGILL,
    libc::SIGABRT,
    libc::SIGTRAP,
    libc::SIGSYS,
];
const NO_ACTION: libc::sigaction = unsafe { std::mem::zeroed() };
static mut PREVIOUS_ACTIONS: [libc::sigaction; SIGNALS.len()] = [NO_ACTION; SIGNALS.len()];

unsafe fn install_crash_handler() {
    for (i, &signal) in SIGNALS.iter().enumerate() {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_crash as *const () as usize;
        // bionic gives every thread an alternate stack, so a stack overflow is reported too.
        action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(signal, &action, std::ptr::addr_of_mut!(PREVIOUS_ACTIONS[i]));
    }
}

/// A line built on the stack: the handler runs where the heap may be what broke.
struct Line {
    bytes: [u8; 512],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Line {
            bytes: [0; 512],
            len: 0,
        }
    }

    /// To logcat (as a fatal line) and to the file.
    fn emit(&mut self) {
        let len = self.len.min(self.bytes.len() - 1);
        self.bytes[len] = 0;
        unsafe {
            __android_log_write(
                ANDROID_LOG_FATAL,
                c"Chronicle".as_ptr(),
                self.bytes.as_ptr().cast(),
            )
        };
        self.bytes[len] = b'\n';
        append(&self.bytes[..=len]);
        self.len = 0;
    }
}

impl std::fmt::Write for Line {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let room = self.bytes.len() - 1 - self.len;
        let n = text.len().min(room);
        self.bytes[self.len..self.len + n].copy_from_slice(&text.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

fn signal_name(signal: c_int) -> &'static str {
    match signal {
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGBUS => "SIGBUS",
        libc::SIGFPE => "SIGFPE",
        libc::SIGILL => "SIGILL",
        libc::SIGABRT => "SIGABRT",
        libc::SIGTRAP => "SIGTRAP",
        libc::SIGSYS => "SIGSYS",
        _ => "signal",
    }
}

/// `pc 0x<offset in its library> <library> (<symbol>+0x<offset>)`, as a tombstone writes a frame,
/// so it can be matched against the unstripped libmain.so the release carries.
fn describe(line: &mut Line, address: usize) {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    if address == 0 || unsafe { libc::dladdr(address as *const c_void, &mut info) } == 0 {
        let _ = write!(line, "pc {address:#x} (unknown)");
        return;
    }
    let base = info.dli_fbase as usize;
    let library = if info.dli_fname.is_null() {
        "?"
    } else {
        let path = unsafe { CStr::from_ptr(info.dli_fname) }
            .to_str()
            .unwrap_or("?");
        path.rsplit('/').next().unwrap_or(path)
    };
    let _ = write!(line, "pc {:#x} {library}", address.wrapping_sub(base));
    if !info.dli_sname.is_null() {
        let symbol = unsafe { CStr::from_ptr(info.dli_sname) }
            .to_str()
            .unwrap_or("?");
        let _ = write!(
            line,
            " ({symbol}+{:#x})",
            address.wrapping_sub(info.dli_saddr as usize)
        );
    }
}

#[repr(C)]
struct UnwindContext {
    _private: [u8; 0],
}

extern "C" {
    fn _Unwind_Backtrace(
        trace: extern "C" fn(*mut UnwindContext, *mut c_void) -> c_int,
        argument: *mut c_void,
    ) -> c_int;
    fn _Unwind_GetIP(context: *mut UnwindContext) -> usize;
}

const URC_NO_REASON: c_int = 0;
const URC_END_OF_STACK: c_int = 5;

struct Frames {
    count: usize,
}

extern "C" fn frame(context: *mut UnwindContext, argument: *mut c_void) -> c_int {
    let frames = unsafe { &mut *argument.cast::<Frames>() };
    if frames.count >= 48 {
        return URC_END_OF_STACK;
    }
    let ip = unsafe { _Unwind_GetIP(context) };
    if ip == 0 {
        return URC_END_OF_STACK;
    }
    let mut line = Line::new();
    let _ = write!(line, "  #{:02} ", frames.count);
    // A return address points after the call; the call is the instruction before it.
    describe(&mut line, if frames.count == 0 { ip } else { ip - 1 });
    line.emit();
    frames.count += 1;
    URC_NO_REASON
}

extern "C" fn on_crash(signal: c_int, info: *mut libc::siginfo_t, context: *mut c_void) {
    static CRASHED: AtomicBool = AtomicBool::new(false);
    let index = SIGNALS.iter().position(|&s| s == signal).unwrap_or(0);
    if !CRASHED.swap(true, Ordering::SeqCst) {
        report(signal, info, context);
    }
    unsafe {
        // Android's handler next: it writes the tombstone and ends the process. A fault comes back
        // to it when the instruction runs again; a sent signal (abort's) is sent again, and arrives
        // once this handler returns.
        libc::sigaction(
            signal,
            std::ptr::addr_of!(PREVIOUS_ACTIONS[index]),
            std::ptr::null_mut(),
        );
        if info.is_null() || (*info).si_code <= 0 {
            libc::syscall(libc::SYS_tgkill, libc::getpid(), libc::gettid(), signal);
        }
    }
}

fn report(signal: c_int, info: *mut libc::siginfo_t, context: *mut c_void) {
    // What the game printed just before, first.
    wait_for_reader(200);
    let mut line = Line::new();
    let mut thread = [0u8; 17];
    unsafe { libc::prctl(libc::PR_GET_NAME, thread.as_mut_ptr()) };
    let thread = CStr::from_bytes_until_nul(&thread)
        .ok()
        .and_then(|name| name.to_str().ok())
        .unwrap_or("?");
    let (code, address) = if info.is_null() {
        (0, 0)
    } else {
        unsafe { ((*info).si_code, (*info).si_addr() as usize) }
    };
    let _ = write!(
        line,
        "fatal signal {signal} ({}), code {code}, fault address {address:#x}, in thread {} ({thread})",
        signal_name(signal),
        unsafe { libc::gettid() }
    );
    line.emit();
    #[cfg(target_arch = "aarch64")]
    if !context.is_null() {
        let mcontext = unsafe { &(*context.cast::<libc::ucontext_t>()).uc_mcontext };
        let _ = write!(line, "  at ");
        describe(&mut line, mcontext.pc as usize);
        line.emit();
        let _ = write!(line, "  lr ");
        describe(&mut line, mcontext.regs[30] as usize);
        line.emit();
        let _ = write!(line, "  sp {:#x}", mcontext.sp);
        line.emit();
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = context;
    let _ = write!(line, "backtrace:");
    line.emit();
    let mut frames = Frames { count: 0 };
    unsafe { _Unwind_Backtrace(frame, std::ptr::addr_of_mut!(frames).cast()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_last_run_and_writes_whole_lines() {
        let dir = std::env::temp_dir().join(format!("chronicle-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CURRENT), "the run before\n").unwrap();
        start_file(&dir);
        write("first");
        let mut line = Line::new();
        let _ = write!(line, "fatal signal {}", 11);
        line.emit();
        let previous = fs::read_to_string(dir.join(PREVIOUS)).unwrap();
        assert_eq!(previous, "the run before\n");
        let current = fs::read_to_string(dir.join(CURRENT)).unwrap();
        let lines: Vec<&str> = current.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" first"), "{current}");
        assert_eq!(lines[1], "fatal signal 11");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn describes_an_address_in_a_library() {
        let mut line = Line::new();
        describe(&mut line, libc::malloc as *const () as usize);
        let text = std::str::from_utf8(&line.bytes[..line.len]).unwrap();
        assert!(text.starts_with("pc 0x") && text.contains("libc"), "{text}");
    }

    #[test]
    fn a_crash_writes_where_it_happened() {
        let dir = std::env::temp_dir().join(format!("chronicle-crash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            start_file(&dir);
            unsafe {
                install_crash_handler();
                std::ptr::read_volatile(std::ptr::null::<u32>());
                libc::_exit(0);
            }
        }
        let mut status = 0;
        unsafe { libc::waitpid(pid, &mut status, 0) };
        assert!(libc::WIFSIGNALED(status), "status {status:#x}");
        assert_eq!(libc::WTERMSIG(status), libc::SIGSEGV);
        let log = fs::read_to_string(dir.join(CURRENT)).unwrap();
        assert!(log.contains("fatal signal 11 (SIGSEGV)"), "{log}");
        // The frames come from the host's unwinder, which in a forked test process sometimes finds
        // none from inside the handler; the report around them is the handler's own.
        assert!(log.contains("\nbacktrace:\n"), "{log}");
        let _ = fs::remove_dir_all(&dir);
    }
}

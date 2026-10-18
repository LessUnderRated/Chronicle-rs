//! The game reports through stdout and stderr, which Android discards; doukutsu-rs's drsandroid pipes
//! both into logcat the same way. `adb logcat -s Chronicle` shows them.

use std::ffi::{c_char, c_int, CString};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::os::fd::FromRawFd;

const ANDROID_LOG_INFO: c_int = 4;

extern "C" {
    fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

pub fn write(line: &str) {
    if let Ok(text) = CString::new(line) {
        unsafe { __android_log_write(ANDROID_LOG_INFO, c"Chronicle".as_ptr(), text.as_ptr()) };
    }
}

pub fn redirect_stdio() {
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
    }
    let reader = unsafe { File::from_raw_fd(pipe[0]) };
    std::thread::Builder::new()
        .name("logcat".into())
        .spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        while line.last() == Some(&b'\n') {
                            line.pop();
                        }
                        write(&String::from_utf8_lossy(&line));
                    }
                }
            }
        })
        .ok();
}

extern "C" {
    static stdout: *mut libc::FILE;
}

fn stdout_file() -> *mut libc::FILE {
    unsafe { stdout }
}

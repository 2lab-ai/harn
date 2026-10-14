//! The real `harn` view on a pseudo-terminal whose master side is then closed, as when the tmux
//! session that started it is killed. crossterm 0.29 retries a read that returns 0 bytes (or
//! fails with anything but WouldBlock) without end (`event/source/unix/mio.rs`), so the view's
//! loop never gets back to its signal flag; harn must still stop within a few seconds of the
//! signal, with the exit code the loop would have given it (128 + the signal).

#![cfg(unix)]

mod common;

use common::{fixture, Fixture};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long harn may take to draw its first frame.
const START: Duration = Duration::from_secs(30);

/// How long harn may take to stop once its terminal is gone and the signal is in.
const STOP: Duration = Duration::from_secs(10);

/// How long the view runs after its first frame is drawn before the terminal goes away.
const SETTLE: Duration = Duration::from_millis(500);

/// In the header from the first frame on (`readonly_is_said_in_the_header_before_any_key`).
const DRAWN: &[u8] = b"read-only";

/// openpty → close-on-exec → spawn, one test at a time: a child the other test spawned in
/// between would inherit this master, and the terminal would never hang up.
static SPAWN: Mutex<()> = Mutex::new(());

struct Pty {
    master: File,
    slave: File,
}

/// An 80×24 pseudo-terminal; neither side leaks into a child.
fn pty() -> Pty {
    let (mut master, mut slave) = (-1, -1);
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: both out-pointers and the size are valid for the call; no name or termios.
    let made = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(size),
        )
    };
    assert_eq!(made, 0, "openpty: {}", io::Error::last_os_error());
    for fd in [master, slave] {
        // SAFETY: `fd` was just opened by openpty.
        let set = unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        assert_ne!(set, -1, "FD_CLOEXEC: {}", io::Error::last_os_error());
    }
    // SAFETY: openpty returned two open descriptors that nothing else owns.
    unsafe {
        Pty {
            master: File::from_raw_fd(master),
            slave: File::from_raw_fd(slave),
        }
    }
}

/// `harn --readonly` on the fixture with the pty's slave as stdin, stdout and stderr, leading a
/// session of its own. With `controlling`, the pty is that session's controlling terminal, as
/// under tmux: closing the master then sends harn SIGHUP. Without, nothing does.
fn spawn(f: &Fixture, controlling: bool) -> (Child, File) {
    let _one = SPAWN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Pty { master, slave } = pty();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_harn"));
    cmd.env("HOME", &f.home)
        .env("XDG_STATE_HOME", f.root.join("state"))
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ENABLE_TOOL_SEARCH")
        .env_remove("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS")
        .arg("--readonly")
        .arg("--dir")
        .arg(&f.project)
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    // A session of its own either way: without one, harn would share the test run's controlling
    // terminal, and crossterm restores raw mode on `/dev/tty` once stdin stops being a tty.
    // SAFETY: setsid, and login_tty = setsid, ioctl(TIOCSCTTY) and dup2: async-signal-safe.
    unsafe {
        cmd.pre_exec(move || {
            let made = if controlling {
                libc::login_tty(0)
            } else {
                libc::setsid()
            };
            if made == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().expect("run harn");
    (child, master)
}

/// Reads what harn writes for up to `within`, until `done` holds for all bytes read so far; the
/// bytes, with `true` when `done` held. Stops early when harn closes the terminal.
fn read(master: &mut File, within: Duration, done: impl Fn(&[u8]) -> bool) -> (Vec<u8>, bool) {
    let start = Instant::now();
    let mut seen = Vec::new();
    let mut buf = [0u8; 4096];
    while !done(&seen) {
        let Some(left) = within.checked_sub(start.elapsed()) else {
            return (seen, false);
        };
        let mut ready = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = libc::c_int::try_from(left.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: one valid pollfd for the length of the call.
        if unsafe { libc::poll(&mut ready, 1, ms) } <= 0 {
            continue;
        }
        match master.read(&mut buf) {
            Ok(0) => return (seen, false),
            Ok(n) => seen.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            // EIO: every slave descriptor is closed (harn exited).
            Err(_) => return (seen, false),
        }
    }
    (seen, true)
}

fn kill(child: &Child, signal: libc::c_int) {
    // SAFETY: a plain kill(2) on our own child, not yet reaped.
    unsafe {
        libc::kill(child.id() as libc::pid_t, signal);
    }
}

/// Waits up to `within` for `child`; on timeout it is killed and reaped, and None comes back.
fn wait(child: &mut Child, within: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    while start.elapsed() < within {
        if let Some(status) = child.try_wait().expect("wait for harn") {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    kill(child, libc::SIGKILL);
    let _ = child.wait();
    None
}

/// harn running on a pty, its first frame drawn and the view idle; then the pty's master closed:
/// the terminal is gone.
fn hung_up(f: &Fixture, controlling: bool) -> Child {
    let (mut child, mut master) = spawn(f, controlling);
    let (seen, drawn) = read(&mut master, START, |seen| {
        seen.windows(DRAWN.len()).any(|w| w == DRAWN)
    });
    if !drawn {
        let _ = wait(&mut child, Duration::ZERO);
        panic!(
            "no first frame within {START:?}: {:?}",
            String::from_utf8_lossy(&seen)
        );
    }
    // The rest of the first frame, then a few ticks: the view waits for a key when the master
    // closes, as it does when a tmux session is killed under it.
    let _ = read(&mut master, SETTLE, |_| false);
    drop(master);
    child
}

#[test]
fn sigterm_stops_harn_after_its_terminal_is_gone() {
    let f = fixture();
    let mut child = hung_up(&f, false);
    // Give the view's loop time to reach the read that never returns.
    std::thread::sleep(Duration::from_millis(300));
    // The close may land in the middle of a draw (one per tick): the write fails and harn stops
    // with an error on its own. That is not outliving the terminal either.
    if child.try_wait().expect("wait for harn").is_some() {
        return;
    }
    kill(&child, libc::SIGTERM);
    let status = wait(&mut child, STOP).expect("harn outlived its terminal after SIGTERM");
    assert_eq!(status.code(), Some(128 + libc::SIGTERM), "{status}");
}

#[test]
fn a_hung_up_controlling_terminal_stops_harn_with_sighup() {
    let f = fixture();
    let mut child = hung_up(&f, true);
    let status = wait(&mut child, STOP).expect("harn outlived its controlling terminal");
    // Unless the close landed in the middle of a draw (one per tick): the write failed first and
    // harn stopped with that error (exit 1).
    assert!(
        matches!(status.code(), Some(c) if c == 128 + libc::SIGHUP || c == 1),
        "{status}"
    );
}

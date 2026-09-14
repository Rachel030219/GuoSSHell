use std::{
    io::{self, Read, StdoutLock, Write},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const MODE_SEQUENCE: &[u8] = concat!(
    "\u{1b}]0;rshell-recovery-fixture\u{7}",
    "\u{1b}[?1049h",
    "\u{1b}[>1u",
    "\u{1b}[?1000h",
    "\u{1b}[?1006h",
    "\u{1b}[?1h",
    "\u{1b}[?25l",
    "fixture-界-e\u{301}"
)
.as_bytes();
const CLEAN_EXIT_SEQUENCE: &[u8] = concat!(
    "\u{1b}[?25h",
    "\u{1b}[?1l",
    "\u{1b}[?1006l",
    "\u{1b}[?1000l",
    "\u{1b}[<u",
    "\u{1b}[?1049l",
    "\u{1b}]0;rshell-recovery-fixture-clean\u{7}",
    "\r\nfixture-clean-exit\r\n"
)
.as_bytes();
const FIXTURE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy)]
enum Mode {
    Survive,
    ExitDirty,
}

enum InputEvent {
    Byte(u8),
    Closed,
}

fn main() {
    if run().is_err() {
        std::process::exit(2);
    }
}

fn run() -> io::Result<()> {
    let mode = parse_mode()?;
    let _raw_input = RawInput::enable()?;
    let stdout = io::stdout();
    let mut display = DisplayGuard::enter(stdout.lock())?;
    let events = input_events();
    let deadline = Instant::now() + FIXTURE_TIMEOUT;
    let mut interrupted = false;

    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "fixture timeout"))?;
        let event = events
            .recv_timeout(remaining)
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "fixture timeout"))?;
        match event {
            InputEvent::Byte(0x03) => match mode {
                Mode::ExitDirty => {
                    display.disarm();
                    return Ok(());
                }
                Mode::Survive if !interrupted => {
                    interrupted = true;
                    display.report_interrupt()?;
                }
                Mode::Survive => {}
            },
            InputEvent::Byte(byte) if interrupted && byte.eq_ignore_ascii_case(&b'q') => {
                return Ok(());
            }
            InputEvent::Byte(_) => {}
            InputEvent::Closed => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "fixture input",
                ));
            }
        }
    }
}

fn parse_mode() -> io::Result<Mode> {
    let mut arguments = std::env::args_os().skip(1);
    match (arguments.next().as_deref(), arguments.next()) {
        (Some(mode), None) if mode == "survive" => Ok(Mode::Survive),
        (Some(mode), None) if mode == "exit_dirty" => Ok(Mode::ExitDirty),
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, "fixture mode")),
    }
}

fn input_events() -> mpsc::Receiver<InputEvent> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        loop {
            let mut byte = [0_u8; 1];
            if stdin.read_exact(&mut byte).is_err() {
                let _ = sender.send(InputEvent::Closed);
                return;
            }
            if sender.send(InputEvent::Byte(byte[0])).is_err() {
                return;
            }
        }
    });
    receiver
}

struct DisplayGuard<'a> {
    stdout: StdoutLock<'a>,
    restore: bool,
}

impl<'a> DisplayGuard<'a> {
    fn enter(mut stdout: StdoutLock<'a>) -> io::Result<Self> {
        stdout.write_all(MODE_SEQUENCE)?;
        stdout.flush()?;
        Ok(Self {
            stdout,
            restore: true,
        })
    }

    fn report_interrupt(&mut self) -> io::Result<()> {
        self.stdout
            .write_all(b"\r\ninterrupt=03;survived=true\r\n")?;
        self.stdout.flush()
    }

    fn disarm(&mut self) {
        self.restore = false;
    }
}

impl Drop for DisplayGuard<'_> {
    fn drop(&mut self) {
        if self.restore {
            let _ = self.stdout.write_all(CLEAN_EXIT_SEQUENCE);
            let _ = self.stdout.flush();
        }
    }
}

#[cfg(unix)]
struct RawInput {
    original: libc::termios,
}

#[cfg(unix)]
impl RawInput {
    fn enable() -> io::Result<Self> {
        let mut current = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, current.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { current.assume_init() };
        let mut raw = unsafe { std::ptr::read(&original) };
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { original })
    }
}

#[cfg(unix)]
impl Drop for RawInput {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original);
        }
    }
}

#[cfg(windows)]
struct RawInput {
    handle: *mut std::ffi::c_void,
    original: u32,
}

#[cfg(windows)]
impl RawInput {
    fn enable() -> io::Result<Self> {
        type Handle = *mut std::ffi::c_void;
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(kind: u32) -> Handle;
            fn GetConsoleMode(console: Handle, mode: *mut u32) -> i32;
            fn SetConsoleMode(console: Handle, mode: u32) -> i32;
        }

        const STD_INPUT_HANDLE: u32 = (-10_i32) as u32;
        const PROCESSED_INPUT: u32 = 0x0001;
        const LINE_INPUT: u32 = 0x0002;
        const ECHO_INPUT: u32 = 0x0004;
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let mut original = 0;
        if handle.is_null() || unsafe { GetConsoleMode(handle, &mut original) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let raw = original & !(PROCESSED_INPUT | LINE_INPUT | ECHO_INPUT);
        if unsafe { SetConsoleMode(handle, raw) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle, original })
    }
}

#[cfg(windows)]
impl Drop for RawInput {
    fn drop(&mut self) {
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn SetConsoleMode(console: *mut std::ffi::c_void, mode: u32) -> i32;
        }
        unsafe {
            SetConsoleMode(self.handle, self.original);
        }
    }
}

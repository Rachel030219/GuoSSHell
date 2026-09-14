use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, BufRead, Write},
    path::PathBuf,
    process::{self, Command},
    thread,
    time::Duration,
};

fn main() -> io::Result<()> {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if let Some(duration) = option_value(&args, "--hold-open-ms") {
        let duration = duration.to_string_lossy().parse::<u64>().unwrap_or(3_000);
        thread::sleep(Duration::from_millis(duration));
        return Ok(());
    }
    let early_descendant = option_value(&args, "--spawn-descendant-before-marker-ms")
        .map(|duration| {
            Command::new(env::current_exe()?)
                .arg("--hold-open-ms")
                .arg(duration)
                .spawn()
        })
        .transpose()?;
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "PID:{}", process::id())?;
    if let Some(descendant) = early_descendant {
        writeln!(stdout, "DESCENDANT:{}", descendant.id())?;
        writeln!(stdout, "FIRST_USER_MARKER")?;
    }
    for (index, arg) in args.iter().enumerate() {
        writeln!(stdout, "ARG:{index}:{}", arg.to_string_lossy())?;
    }
    writeln!(stdout, "CWD:{}", env::current_dir()?.display())?;
    writeln!(
        stdout,
        "ENV:{}",
        env::var_os("RSHELL_FIXTURE_ENV")
            .unwrap_or_default()
            .to_string_lossy()
    )?;
    writeln!(
        stdout,
        "TERM:{}",
        env::var_os("TERM").unwrap_or_default().to_string_lossy()
    )?;

    if let Some(path) = option_value(&args, "--touch") {
        fs::write(PathBuf::from(path), b"spawned")?;
    }
    if let Some(code) = option_value(&args, "--exit") {
        let code = code.to_string_lossy().parse::<i32>().unwrap_or(70);
        writeln!(stdout, "BEFORE_EXIT:{code}")?;
        stdout.flush()?;
        process::exit(code);
    }

    let watch_resize_from = args
        .iter()
        .any(|argument| argument == "--watch-resize")
        .then(terminal_size)
        .transpose()?;
    if let Some((cols, rows)) = watch_resize_from {
        prepare_resize_watch()?;
        writeln!(stdout, "INITIAL_SIZE:{cols}x{rows}")?;
    }
    writeln!(stdout, "READY")?;
    if let Some(duration) = option_value(&args, "--spawn-inheriting-child-ms") {
        let child = Command::new(env::current_exe()?)
            .arg("--hold-open-ms")
            .arg(duration)
            .spawn()?;
        writeln!(stdout, "DESCENDANT:{}", child.id())?;
        writeln!(stdout, "DESCENDANT_READY")?;
    }
    stdout.flush()?;
    drop(stdout);

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        let command = line.trim_end_matches('\r');
        let mut stdout = io::stdout().lock();
        if command == "size" {
            let (cols, rows) = terminal_size()?;
            writeln!(stdout, "SIZE:{cols}x{rows}")?;
        } else if command == "quit" {
            writeln!(stdout, "CLEAN_EXIT")?;
            stdout.flush()?;
            return Ok(());
        } else if let Some(code) = command.strip_prefix("exit:") {
            let code = code.parse::<i32>().unwrap_or(71);
            writeln!(stdout, "BEFORE_EXIT:{code}")?;
            stdout.flush()?;
            process::exit(code);
        } else if let Some(payload) = command.strip_prefix("hello:") {
            writeln!(
                stdout,
                "PAYLOAD:\x1b[31mCOLOR\x1b[0m|WIDE:界🙂|ECHO:{payload}"
            )?;
            if args
                .iter()
                .any(|argument| argument == "--split-watch-marker")
            {
                stdout.flush()?;
                thread::sleep(Duration::from_millis(50));
            }
            if watch_resize_from.is_some() {
                writeln!(stdout, "WATCHING_SIZE")?;
            }
        } else if !command.is_empty() {
            writeln!(stdout, "UNKNOWN:{command}")?;
        }
        stdout.flush()?;
        if let Some(initial) = watch_resize_from {
            drop(stdout);
            report_next_size(initial)?;
            return Ok(());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_resize_watch() -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn report_next_size(initial: (u16, u16)) -> io::Result<()> {
    loop {
        thread::sleep(Duration::from_millis(10));
        let current = terminal_size()?;
        if current != initial {
            let mut stdout = io::stdout().lock();
            writeln!(stdout, "SIZE:{}x{}", current.0, current.1)?;
            stdout.flush()?;
            return Ok(());
        }
    }
}

#[cfg(windows)]
fn prepare_resize_watch() -> io::Result<()> {
    use std::ffi::c_void;

    type Handle = *mut c_void;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(kind: u32) -> Handle;
        fn GetConsoleMode(console: Handle, mode: *mut u32) -> i32;
        fn SetConsoleMode(console: Handle, mode: u32) -> i32;
    }

    const STD_INPUT_HANDLE: u32 = (-10_i32) as u32;
    const ENABLE_WINDOW_INPUT: u32 = 0x0008;
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let mut mode = 0;
    if input.is_null() || unsafe { GetConsoleMode(input, &mut mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { SetConsoleMode(input, mode | ENABLE_WINDOW_INPUT) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
fn report_next_size(_initial: (u16, u16)) -> io::Result<()> {
    use std::{ffi::c_void, mem::MaybeUninit};

    type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Coord {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    union InputEvent {
        window_size: Coord,
        alignment: [u32; 4],
    }

    #[repr(C)]
    struct InputRecord {
        event_type: u16,
        event: InputEvent,
    }

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(kind: u32) -> Handle;
        fn ReadConsoleInputW(
            input: Handle,
            records: *mut InputRecord,
            length: u32,
            read: *mut u32,
        ) -> i32;
    }

    const STD_INPUT_HANDLE: u32 = (-10_i32) as u32;
    const WINDOW_BUFFER_SIZE_EVENT: u16 = 0x0004;
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    loop {
        let mut record = MaybeUninit::<InputRecord>::uninit();
        let mut read = 0;
        if input.is_null()
            || unsafe { ReadConsoleInputW(input, record.as_mut_ptr(), 1, &mut read) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let record = unsafe { record.assume_init() };
        if read == 1 && record.event_type == WINDOW_BUFFER_SIZE_EVENT {
            let size = unsafe { record.event.window_size };
            let cols =
                u16::try_from(size.x).map_err(|_| io::Error::other("invalid resize columns"))?;
            let rows =
                u16::try_from(size.y).map_err(|_| io::Error::other("invalid resize rows"))?;
            let mut stdout = io::stdout().lock();
            writeln!(stdout, "SIZE:{cols}x{rows}")?;
            stdout.flush()?;
            return Ok(());
        }
    }
}

fn option_value<'a>(args: &'a [OsString], name: &str) -> Option<&'a OsString> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| &pair[1])
}

#[cfg(unix)]
fn terminal_size() -> io::Result<(u16, u16)> {
    #[repr(C)]
    #[derive(Default)]
    struct WinSize {
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    }

    #[cfg(target_os = "linux")]
    const TIOCGWINSZ: usize = 0x5413;
    #[cfg(not(target_os = "linux"))]
    const TIOCGWINSZ: usize = 0x4008_7468;

    unsafe extern "C" {
        fn ioctl(fd: i32, request: usize, ...) -> i32;
    }

    let mut size = WinSize::default();
    if unsafe { ioctl(1, TIOCGWINSZ, &mut size) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok((size.cols, size.rows))
}

#[cfg(windows)]
fn terminal_size() -> io::Result<(u16, u16)> {
    use std::{ffi::c_void, mem::MaybeUninit};

    type Handle = *mut c_void;

    #[repr(C)]
    struct Coord {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }

    #[repr(C)]
    struct ConsoleScreenBufferInfo {
        size: Coord,
        cursor_position: Coord,
        attributes: u16,
        window: SmallRect,
        maximum_window_size: Coord,
    }

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(kind: u32) -> Handle;
        fn GetConsoleScreenBufferInfo(output: Handle, info: *mut ConsoleScreenBufferInfo) -> i32;
    }

    const STD_OUTPUT_HANDLE: u32 = (-11_i32) as u32;
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let mut info = MaybeUninit::<ConsoleScreenBufferInfo>::uninit();
    if output.is_null() || unsafe { GetConsoleScreenBufferInfo(output, info.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let buffer = unsafe { info.assume_init() }.size;
    let cols = u16::try_from(buffer.x).map_err(|_| io::Error::other("invalid terminal columns"))?;
    let rows = u16::try_from(buffer.y).map_err(|_| io::Error::other("invalid terminal rows"))?;
    Ok((cols, rows))
}

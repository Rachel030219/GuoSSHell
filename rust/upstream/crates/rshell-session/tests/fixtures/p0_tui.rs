use std::io::{Read, Write};

fn main() -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(b"\x1b[?1049h\x1b[2J\x1b[H\x1b]2;rshell-p0-tui-active\x07")?;
    stdout.write_all("A界P0-TUI-ENTERED".as_bytes())?;
    stdout.flush()?;

    let mut input = [0u8; 1];
    loop {
        std::io::stdin().read_exact(&mut input)?;
        if input[0].eq_ignore_ascii_case(&b'q') {
            break;
        }
    }

    stdout.write_all(b"\x1b[?1049l\x1b]2;rshell-p0-tui-exited\x07\r\nP0-TUI-EXITED\r\n")?;
    stdout.flush()
}

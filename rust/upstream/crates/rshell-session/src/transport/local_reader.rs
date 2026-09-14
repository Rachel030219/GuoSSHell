use std::{
    io::{self, Read},
    thread::{self, JoinHandle},
};

use tokio::sync::mpsc;

/// Bounds unread PTY output while preserving backpressure to the child.
pub(super) const READER_CAPACITY: usize = 32;
const READ_CHUNK_SIZE: usize = 8 * 1024;

pub(super) enum ReaderEvent {
    Output(Vec<u8>),
    Eof,
    Failure,
}

pub(super) fn spawn_reader(
    reader: Box<dyn Read + Send>,
) -> io::Result<(mpsc::Receiver<ReaderEvent>, JoinHandle<()>)> {
    let (sender, receiver) = mpsc::channel(READER_CAPACITY);
    let thread = thread::Builder::new()
        .name("rshell-local-pty-reader".to_owned())
        .spawn(move || {
            let mut reader = reader;
            read_loop(&mut *reader, &sender);
        })?;
    Ok((receiver, thread))
}

fn read_loop(reader: &mut dyn Read, sender: &mpsc::Sender<ReaderEvent>) {
    let mut buffer = vec![0; READ_CHUNK_SIZE];
    loop {
        let event = match reader.read(&mut buffer) {
            Ok(0) => ReaderEvent::Eof,
            Ok(count) => ReaderEvent::Output(buffer[..count].to_vec()),
            Err(error) if expected_eof(&error) => ReaderEvent::Eof,
            Err(_) => ReaderEvent::Failure,
        };
        let terminal = !matches!(event, ReaderEvent::Output(_));
        if sender.blocking_send(event).is_err() || terminal {
            return;
        }
    }
}

fn expected_eof(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof
    ) {
        return true;
    }

    #[cfg(unix)]
    return error.raw_os_error() == Some(5); // EIO: the PTY slave closed.

    #[cfg(windows)]
    return matches!(error.raw_os_error(), Some(109 | 995)); // broken pipe / cancelled I/O

    #[cfg(not(any(unix, windows)))]
    false
}

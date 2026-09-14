use std::{io::Write, thread::JoinHandle, time::Duration};

use portable_pty::{Child, MasterPty, PtySize};
use rshell_core::{ExitStatus, SessionFailure, TerminalSize};
use tokio::sync::mpsc;

use crate::{TransportError, TransportEvent};

use super::local_reader::ReaderEvent;

mod lifecycle;

pub(super) use lifecycle::join_reader_thread;

const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(250);
const FORCE_EXIT_GRACE: Duration = Duration::from_millis(250);
const READER_CLOSE_GRACE: Duration = Duration::from_millis(100);
const READER_DRAIN_LIMIT: usize = 32;
#[cfg(unix)]
const READER_JOIN_GRACE: Duration = Duration::from_millis(1_250);
#[cfg(windows)]
const READER_JOIN_GRACE: Duration = Duration::from_millis(600);

pub(super) struct LocalRuntime {
    master: Option<Box<dyn MasterPty>>,
    #[cfg(unix)]
    process_group: Option<i32>,
    #[cfg(windows)]
    process_job: Option<rshell_platform::WindowsProcessJob>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    reader_rx: mpsc::Receiver<ReaderEvent>,
    reader_thread: Option<JoinHandle<()>>,
    pending_exit: Option<ExitStatus>,
    reader_eof: bool,
    exit_emitted: bool,
    shut_down: bool,
}

impl LocalRuntime {
    pub(super) fn new(
        master: Box<dyn MasterPty>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn Child + Send + Sync>,
        reader_rx: mpsc::Receiver<ReaderEvent>,
        reader_thread: JoinHandle<()>,
        #[cfg(windows)] process_job: rshell_platform::WindowsProcessJob,
    ) -> Self {
        #[cfg(unix)]
        let process_group = master.process_group_leader();
        Self {
            master: Some(master),
            #[cfg(unix)]
            process_group,
            #[cfg(windows)]
            process_job: Some(process_job),
            writer: Some(writer),
            child: Some(child),
            reader_rx,
            reader_thread: Some(reader_thread),
            pending_exit: None,
            reader_eof: false,
            exit_emitted: false,
            shut_down: false,
        }
    }

    pub(super) fn process_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(|child| child.process_id())
    }

    #[cfg(windows)]
    pub(super) fn process_tree_contains(&self, process_id: u32) -> Result<bool, TransportError> {
        self.process_job
            .as_ref()
            .ok_or_else(pty_error)?
            .contains_process(process_id)
            .map_err(|_| pty_error())
    }

    pub(super) async fn next_event(&mut self) -> Result<TransportEvent, TransportError> {
        if self.exit_emitted || self.shut_down {
            return Err(pty_error());
        }
        loop {
            match self.reader_rx.try_recv() {
                Ok(event) => {
                    if let Some(event) = self.handle_reader_event(event)? {
                        return Ok(event);
                    }
                    continue;
                }
                Err(mpsc::error::TryRecvError::Disconnected) => self.reader_eof = true,
                Err(mpsc::error::TryRecvError::Empty) => {}
            }

            self.poll_child()?;
            if self.reader_eof
                && let Some(status) = self.pending_exit.take()
            {
                self.join_reader()?;
                self.exit_emitted = true;
                return Ok(TransportEvent::Exit(status));
            }

            if self.reader_eof {
                tokio::time::sleep(CHILD_POLL_INTERVAL).await;
            } else {
                match tokio::time::timeout(CHILD_POLL_INTERVAL, self.reader_rx.recv()).await {
                    Ok(Some(event)) => {
                        if let Some(event) = self.handle_reader_event(event)? {
                            return Ok(event);
                        }
                    }
                    Ok(None) => self.reader_eof = true,
                    Err(_) => {}
                }
            }
        }
    }

    pub(super) fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        let writer = self.writer.as_mut().ok_or_else(pty_error)?;
        writer.write_all(bytes).map_err(|_| pty_error())?;
        writer.flush().map_err(|_| pty_error())
    }

    pub(super) fn resize(&mut self, size: TerminalSize) -> Result<(), TransportError> {
        let size = checked_pty_size(size)?;
        self.master
            .as_ref()
            .ok_or_else(pty_error)?
            .resize(size)
            .map_err(|_| pty_error())
    }

    fn handle_reader_event(
        &mut self,
        event: ReaderEvent,
    ) -> Result<Option<TransportEvent>, TransportError> {
        match event {
            ReaderEvent::Output(bytes) => Ok(Some(TransportEvent::Output(bytes))),
            ReaderEvent::Eof => {
                self.reader_eof = true;
                Ok(None)
            }
            ReaderEvent::Failure => Err(pty_error()),
        }
    }

    fn poll_child(&mut self) -> Result<(), TransportError> {
        if self.pending_exit.is_some() {
            return Ok(());
        }
        let Some(child) = self.child.as_mut() else {
            return Err(pty_error());
        };
        if let Some(status) = child.try_wait().map_err(|_| pty_error())? {
            self.pending_exit = Some(exit_status(status));
            self.writer.take();
            self.master.take();
        }
        Ok(())
    }
}

pub(super) fn checked_pty_size(size: TerminalSize) -> Result<PtySize, TransportError> {
    if size.cols == 0 || size.rows == 0 {
        return Err(TransportError::new(SessionFailure::Validation));
    }
    #[cfg(windows)]
    if size.cols > i16::MAX as u16 || size.rows > i16::MAX as u16 {
        return Err(TransportError::new(SessionFailure::Validation));
    }
    Ok(PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: u16::try_from(size.pixel_width)
            .map_err(|_| TransportError::new(SessionFailure::Validation))?,
        pixel_height: u16::try_from(size.pixel_height)
            .map_err(|_| TransportError::new(SessionFailure::Validation))?,
    })
}

fn exit_status(status: portable_pty::ExitStatus) -> ExitStatus {
    ExitStatus {
        code: i32::try_from(status.exit_code()).ok(),
        success: status.success(),
    }
}

fn pty_error() -> TransportError {
    TransportError::new(SessionFailure::Pty)
}

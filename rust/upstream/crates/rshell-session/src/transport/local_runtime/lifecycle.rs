use std::{thread::JoinHandle, time::Duration};

use tokio::sync::mpsc;

use crate::TransportError;

use super::{
    CHILD_POLL_INTERVAL, FORCE_EXIT_GRACE, LocalRuntime, READER_CLOSE_GRACE, READER_DRAIN_LIMIT,
    READER_JOIN_GRACE, ReaderEvent, SHUTDOWN_GRACE, pty_error,
};

impl LocalRuntime {
    pub(in super::super) async fn shutdown(&mut self) -> Result<(), TransportError> {
        if self.shut_down {
            return Ok(());
        }
        self.writer.take();
        #[cfg(windows)]
        let process_tree_terminated = self.terminate_process_tree().is_ok();
        #[cfg(not(windows))]
        let process_tree_terminated = true;

        let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
        let mut exited = self.child_has_exited().unwrap_or(false);
        while !exited && tokio::time::Instant::now() < deadline {
            discard_reader_events(&mut self.reader_rx);
            tokio::time::sleep(CHILD_POLL_INTERVAL).await;
            match self.child_has_exited() {
                Ok(child_exited) => exited = child_exited,
                Err(_) => break,
            }
        }
        if !exited {
            #[cfg(unix)]
            {
                let _ = self.signal_process_group(libc::SIGHUP);
            }
            if let Some(child) = self.child.as_mut() {
                let _ = child.kill();
            }
            let deadline = tokio::time::Instant::now() + FORCE_EXIT_GRACE;
            while !exited && tokio::time::Instant::now() < deadline {
                discard_reader_events(&mut self.reader_rx);
                tokio::time::sleep(CHILD_POLL_INTERVAL).await;
                match self.child_has_exited() {
                    Ok(child_exited) => exited = child_exited,
                    Err(_) => break,
                }
            }
        }

        #[cfg(unix)]
        {
            let _ = self.signal_process_group(libc::SIGKILL);
        }

        self.master.take();
        let reader_joined = self.join_reader_bounded().await.is_ok();
        let child_exited = exited || self.child_has_exited().unwrap_or(false);
        self.child.take();
        self.shut_down = true;
        if child_exited && reader_joined && process_tree_terminated {
            Ok(())
        } else {
            Err(pty_error())
        }
    }

    #[cfg(windows)]
    fn terminate_process_tree(&mut self) -> Result<(), TransportError> {
        self.process_job
            .as_mut()
            .map_or(Ok(()), |job| job.terminate().map_err(|_| pty_error()))?;
        self.process_job.take();
        Ok(())
    }

    fn child_has_exited(&mut self) -> Result<bool, TransportError> {
        let Some(child) = self.child.as_mut() else {
            return Ok(true);
        };
        child
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|_| pty_error())
    }

    pub(super) fn join_reader(&mut self) -> Result<(), TransportError> {
        if let Some(thread) = self.reader_thread.take() {
            thread.join().map_err(|_| pty_error())?;
        }
        Ok(())
    }

    async fn join_reader_bounded(&mut self) -> Result<(), TransportError> {
        let Some(thread) = self.reader_thread.take() else {
            return Ok(());
        };
        super::join_reader_thread(&mut self.reader_rx, thread, READER_JOIN_GRACE).await
    }

    #[cfg(unix)]
    fn signal_process_group(&self, signal: libc::c_int) -> Result<(), TransportError> {
        let Some(process_group) = self.process_group.filter(|group| *group > 1) else {
            return Ok(());
        };
        let result = unsafe { libc::kill(-process_group, signal) };
        if result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(pty_error())
        }
    }
}

pub(in super::super) async fn join_reader_thread(
    reader_rx: &mut mpsc::Receiver<ReaderEvent>,
    thread: JoinHandle<()>,
    grace: Duration,
) -> Result<(), TransportError> {
    let deadline = tokio::time::Instant::now() + grace;
    while !thread.is_finished() && tokio::time::Instant::now() < deadline {
        discard_reader_events(reader_rx);
        tokio::time::sleep(CHILD_POLL_INTERVAL).await;
    }
    if thread.is_finished() {
        reader_rx.close();
        return thread.join().map_err(|_| pty_error());
    }

    reader_rx.close();
    let close_deadline = tokio::time::Instant::now() + READER_CLOSE_GRACE;
    while !thread.is_finished() && tokio::time::Instant::now() < close_deadline {
        tokio::time::sleep(CHILD_POLL_INTERVAL).await;
    }
    if thread.is_finished() {
        let _ = thread.join();
    }
    Err(pty_error())
}

fn discard_reader_events(reader_rx: &mut mpsc::Receiver<ReaderEvent>) {
    for _ in 0..READER_DRAIN_LIMIT {
        if reader_rx.try_recv().is_err() {
            break;
        }
    }
}

impl Drop for LocalRuntime {
    fn drop(&mut self) {
        self.reader_rx.close();
        self.writer.take();
        #[cfg(windows)]
        let _ = self.terminate_process_tree();
        #[cfg(unix)]
        let _ = self.signal_process_group(libc::SIGHUP);
        if let Some(child) = self.child.as_mut()
            && !matches!(child.try_wait(), Ok(Some(_)))
        {
            let _ = child.kill();
            let deadline = std::time::Instant::now() + FORCE_EXIT_GRACE;
            while std::time::Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(CHILD_POLL_INTERVAL);
            }
        }
        #[cfg(unix)]
        let _ = self.signal_process_group(libc::SIGKILL);
        self.master.take();
        if let Some(thread) = self.reader_thread.take()
            && thread.is_finished()
        {
            let _ = thread.join();
        }
    }
}

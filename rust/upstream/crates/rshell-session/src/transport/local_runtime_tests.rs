use std::{sync::mpsc as std_mpsc, thread, time::Duration};

use rshell_core::SessionFailure;
use tokio::sync::mpsc;

use super::{local_reader::ReaderEvent, local_runtime::join_reader_thread};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_close_unblocks_a_sender_but_remains_a_pty_failure() {
    let (sender, mut receiver) = mpsc::channel(1);
    let (started_sender, started_receiver) = std_mpsc::channel();
    let thread = thread::spawn(move || {
        started_sender.send(()).unwrap();
        while sender.blocking_send(ReaderEvent::Output(vec![1])).is_ok() {}
    });
    started_receiver.recv().unwrap();

    let error = tokio::time::timeout(
        Duration::from_millis(500),
        join_reader_thread(&mut receiver, thread, Duration::from_millis(30)),
    )
    .await
    .expect("reader close cleanup must remain bounded")
    .expect_err("receiver-forced completion is not natural convergence");
    assert_eq!(error.failure(), SessionFailure::Pty);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn natural_reader_completion_before_the_deadline_succeeds() {
    let (sender, mut receiver) = mpsc::channel(1);
    let (started_sender, started_receiver) = std_mpsc::channel();
    let (release_sender, release_receiver) = std_mpsc::channel();
    let thread = thread::spawn(move || {
        started_sender.send(()).unwrap();
        release_receiver.recv().unwrap();
        drop(sender);
    });
    started_receiver.recv().unwrap();
    release_sender.send(()).unwrap();

    tokio::time::timeout(
        Duration::from_secs(2),
        join_reader_thread(&mut receiver, thread, Duration::from_millis(1_250)),
    )
    .await
    .expect("natural reader completion must remain bounded")
    .expect("natural reader completion must succeed");
}

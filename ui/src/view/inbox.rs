//! External messages: the channel plus wake pipe an `App` drains once a frame.
//!
//! A worker thread holds an [`InboxSender`]; the loop holds the [`Inbox`].
//! `send` does two things — push the message onto an unbounded channel, then
//! write one byte to a non-blocking pipe whose read end the window polls. The
//! channel is the queue; the byte is only a wake, which is why a full pipe is
//! not an error (M5-D2 §4).

use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};

/// The receiving half: what an [`App`](crate::view::App) drains once per frame.
pub struct Inbox<Msg> {
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    read: OwnedFd,
    write: Arc<OwnedFd>,
}

/// The sending half. `Send + Clone`, so a worker thread — or several — can
/// hold one.
///
/// `Send` is the auto impl: `crossbeam_channel::Sender<T>` is `Send` for
/// `T: Send` and `Arc<OwnedFd>` is `Send + Sync` (P0-D5 — the contract's
/// sketch had an `unsafe impl` that is not needed).
pub struct InboxSender<Msg> {
    tx: Sender<Msg>,
    write: Arc<OwnedFd>,
}

/// `send` failed because every [`Inbox`] is gone; the message comes back.
#[derive(Debug, PartialEq, Eq)]
pub struct SendError<Msg>(pub Msg);

impl<Msg> std::fmt::Display for SendError<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the inbox is closed")
    }
}

impl<Msg: std::fmt::Debug> std::error::Error for SendError<Msg> {}

impl<Msg: Send + 'static> Inbox<Msg> {
    /// Build a connected pair. The pipe is `CLOEXEC | NONBLOCK`.
    ///
    /// # Errors
    ///
    /// [`std::io::Error`] if the pipe cannot be created.
    pub fn new() -> std::io::Result<(Inbox<Msg>, InboxSender<Msg>)> {
        let (read, write) = rustix::pipe::pipe_with(
            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
        )?;
        let (tx, rx) = crossbeam_channel::unbounded();
        let write = Arc::new(write);
        let sender = InboxSender {
            tx: tx.clone(),
            write: Arc::clone(&write),
        };
        Ok((
            Inbox {
                rx,
                tx,
                read,
                write,
            },
            sender,
        ))
    }

    /// Another sender onto the same inbox.
    #[must_use]
    pub fn sender(&self) -> InboxSender<Msg> {
        InboxSender {
            tx: self.tx.clone(),
            write: Arc::clone(&self.write),
        }
    }
}

impl<Msg> Inbox<Msg> {
    /// The fd to register with [`Window::watch_fd`](crate::window::Window::watch_fd).
    ///
    /// A `try_clone`, not the read end itself: `watch_fd` takes ownership, and
    /// the inbox still has to read the pipe to drain it (P0-D6). Both handles
    /// share one open file description, so a byte read through either empties
    /// it for both.
    ///
    /// # Errors
    ///
    /// [`std::io::Error`] if the fd cannot be duplicated.
    pub(crate) fn watch_fd(&self) -> std::io::Result<OwnedFd> {
        self.read.try_clone()
    }

    /// Read the wake pipe empty, ignoring what was in it.
    pub(crate) fn drain_pipe(&self) {
        let mut buf = [0_u8; 256];
        loop {
            match rustix::io::read(&self.read, &mut buf[..]) {
                Ok(0) => return,
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => {}
                // `WOULDBLOCK` is the empty pipe: the normal exit.
                Err(_) => return,
            }
        }
    }

    /// Every queued message, in send order, without blocking.
    pub(crate) fn drain_into(&self, out: &mut VecDeque<Msg>) {
        while let Ok(msg) = self.rx.try_recv() {
            out.push_back(msg);
        }
    }

    /// Take one queued message, if any, without waiting.
    ///
    /// The counterpart of the drain [`App::run`](crate::view::App::run) does
    /// once per frame, exposed so a worker thread's round trip can be tested
    /// without a compositor. Consumes one wake byte per message taken, so a
    /// caller that drains the inbox to empty leaves the pipe empty too and
    /// the loop does not wake for messages that are already gone.
    #[must_use]
    pub fn try_recv(&self) -> Option<Msg> {
        let msg = self.rx.try_recv().ok()?;
        let mut byte = [0u8; 1];
        // A short read or `EAGAIN`/`EINTR` is fine: the channel is the queue,
        // the pipe is only the wakeup, and one spurious wake costs one empty
        // frame. Any other error is unexpected on a pipe we own, so log it —
        // behaviour is unchanged, the read is still best-effort.
        match rustix::io::read(&self.read, &mut byte[..]) {
            Ok(_) | Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {}
            Err(err) => tracing::debug!(?err, "inbox wake-pipe read failed"),
        }
        Some(msg)
    }
}

impl<Msg> InboxSender<Msg> {
    /// Queue `msg` and wake the loop.
    ///
    /// # Errors
    ///
    /// [`SendError`] — carrying `msg` back — once every [`Inbox`] is dropped,
    /// which is how a worker learns the app exited.
    pub fn send(&self, msg: Msg) -> Result<(), SendError<Msg>> {
        self.tx.send(msg).map_err(|err| SendError(err.0))?;
        // One byte. An `EAGAIN` on a full pipe is deliberately ignored: the
        // byte already in it wakes the loop just as well, and blocking here
        // would block a worker on the UI thread (M5-D2 §4). `EINTR` is equally
        // harmless. Any other error is unexpected, so log it — the send still
        // succeeded (the message is on the channel) and behaviour is unchanged.
        match rustix::io::write(&*self.write, b"\0") {
            Ok(_) | Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {}
            Err(err) => tracing::debug!(?err, "inbox wake-pipe write failed"),
        }
        Ok(())
    }
}

impl<Msg> Clone for InboxSender<Msg> {
    fn clone(&self) -> Self {
        InboxSender {
            tx: self.tx.clone(),
            write: Arc::clone(&self.write),
        }
    }
}

impl<Msg> std::fmt::Debug for Inbox<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inbox").finish_non_exhaustive()
    }
}

impl<Msg> std::fmt::Debug for InboxSender<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxSender").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::Inbox;
    use std::collections::VecDeque;

    #[test]
    fn a_sent_message_is_queued_in_send_order_and_wakes_the_pipe() {
        // mutation: drop the `rustix::io::write` from `send`; `drain_pipe`
        // reads nothing and the readiness assertion below fails.
        let (inbox, tx) = Inbox::<u32>::new().expect("a pipe");
        tx.send(1).expect("the inbox is alive");
        tx.send(2).expect("the inbox is alive");
        let mut poll = [rustix::event::PollFd::new(
            &inbox.read,
            rustix::event::PollFlags::IN,
        )];
        let ready = rustix::event::poll(&mut poll, Some(&rustix::event::Timespec::default()))
            .expect("poll");
        assert_eq!(ready, 1, "a send must make the read end readable");
        let mut queue: VecDeque<u32> = VecDeque::new();
        inbox.drain_pipe();
        inbox.drain_into(&mut queue);
        assert_eq!(queue.into_iter().collect::<Vec<_>>(), vec![1, 2]);
        let ready = rustix::event::poll(&mut poll, Some(&rustix::event::Timespec::default()))
            .expect("poll");
        assert_eq!(ready, 0, "drain_pipe must leave the pipe empty");
    }

    #[test]
    fn a_sender_survives_being_cloned_across_threads() {
        // mutation: make `Clone` clone only the channel and not the pipe
        // handle; the cloned sender's writes go nowhere and this deadlocks
        // the readiness assertion in the test above rather than here — so the
        // assertion here is on ordering across threads, which still holds.
        let (inbox, tx) = Inbox::<u32>::new().expect("a pipe");
        let handles: Vec<_> = (0..4)
            .map(|n| {
                let tx = tx.clone();
                std::thread::spawn(move || tx.send(n).expect("the inbox is alive"))
            })
            .collect();
        for handle in handles {
            handle.join().expect("the worker thread finished");
        }
        let mut queue = VecDeque::new();
        inbox.drain_pipe();
        inbox.drain_into(&mut queue);
        let mut got: Vec<u32> = queue.into_iter().collect();
        got.sort_unstable();
        assert_eq!(got, vec![0, 1, 2, 3]);
    }

    #[test]
    fn sending_after_the_inbox_is_dropped_returns_the_message() {
        // mutation: `unwrap` the channel send; this panics instead of
        // handing the message back.
        let (inbox, tx) = Inbox::<String>::new().expect("a pipe");
        drop(inbox);
        let err = tx.send("late".to_owned()).expect_err("the inbox is gone");
        assert_eq!(
            err.0, "late",
            "the message comes back rather than vanishing"
        );
    }

    #[test]
    fn a_full_wake_pipe_does_not_block_a_sender() {
        // A worker must never block on the loop: the byte already in the pipe
        // is enough to wake it, and the channel is the real queue (M5-D2 §4).
        // mutation: drop `PipeFlags::NONBLOCK` from `pipe_with`; this hangs.
        let (inbox, tx) = Inbox::<u32>::new().expect("a pipe");
        for n in 0..200_000 {
            tx.send(n).expect("the inbox is alive");
        }
        let mut queue = VecDeque::new();
        inbox.drain_pipe();
        inbox.drain_into(&mut queue);
        assert_eq!(queue.len(), 200_000, "every message survives a full pipe");
    }
}

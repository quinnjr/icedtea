//! Clipboard and primary selection, in the shape `harness/src/lib.rs` already
//! proves against this compositor: manager -> per-seat device -> `Dispatch`
//! impls for device/offer/source, with `event_created_child!` wiring the
//! offer.
//!
//! Drag and drop is M6 and is deliberately not wired here.

use std::cell::RefCell;
use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};
use std::rc::Rc;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source,
};
use wayland_client::{Connection, Dispatch, QueueHandle, event_created_child};
use wayland_protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_manager_v1, zwp_primary_selection_device_v1,
    zwp_primary_selection_offer_v1, zwp_primary_selection_source_v1,
};

use super::WindowState;

/// The only mime type M3 offers or accepts.
pub const TEXT_MIME: &str = "text/plain;charset=utf-8";

/// The most a single paste will read.
///
/// The writer is another client; without a cap, one that streams forever turns
/// a paste into an OOM.
pub(crate) const MAX_OFFER_BYTES: usize = 16 * 1024 * 1024;

/// Read a data offer's pipe to EOF, with a deadline.
///
/// Non-blocking reads plus `poll(2)`, not `read_to_end`: the other end is
/// another process and may never write, never close, or never stop.
#[must_use]
pub fn read_offer_fd(fd: OwnedFd, deadline: Duration) -> Option<String> {
    use rustix::event::{PollFlags, Timespec};

    let until = Instant::now() + deadline;
    let mut file = std::fs::File::from(fd);
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let now = Instant::now();
        if now >= until {
            tracing::warn!("a selection offer did not finish within its deadline");
            break;
        }
        let remaining = until - now;
        let borrowed = file.as_fd();
        let mut fds = [rustix::event::PollFd::new(&borrowed, PollFlags::IN)];
        let timespec = Timespec {
            tv_sec: remaining.as_secs().min(i64::MAX as u64) as i64,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        };
        match rustix::event::poll(&mut fds, Some(&timespec)) {
            Ok(0) => break,
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => break,
        }
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&chunk[..n]);
                if out.len() >= MAX_OFFER_BYTES {
                    out.truncate(MAX_OFFER_BYTES);
                    tracing::warn!(cap = MAX_OFFER_BYTES, "truncating an oversized selection");
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    // The mime type promises UTF-8; anything else is another client's bug and
    // is dropped rather than lossily mangled into the user's document.
    String::from_utf8(out).ok()
}

/// The offer/source bookkeeping `Dispatch` impls below write into and
/// [`Clipboard`]'s methods read from. Shared (`Rc<RefCell<..>>`) because the
/// `Dispatch` impls run on `&mut WindowState`, not on `Clipboard` itself --
/// `WindowState` holds a clone of the same `Rc` from the moment [`Clipboard`]
/// is built.
#[derive(Default)]
pub(crate) struct ClipboardShared {
    offer: Option<wl_data_offer::WlDataOffer>,
    offer_mimes: Vec<String>,
    source: Option<wl_data_source::WlDataSource>,
    payload: String,

    primary_offer: Option<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1>,
    primary_mimes: Vec<String>,
    primary_source: Option<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1>,
    primary_payload: String,
}

/// `wl_data_device` + `zwp_primary_selection_device_v1`, bound the moment the
/// compositor advertised both manager globals and `Window::open`'s roundtrips
/// saw a `wl_seat`.
///
/// Built lazily by [`super::Window::clipboard`] rather than eagerly by
/// `Window::open`: a headless/offscreen test harness that never touches the
/// clipboard should never pay for (or fail on) a global it does not need.
pub struct Clipboard {
    conn: Connection,
    qh: QueueHandle<WindowState>,
    manager: wl_data_device_manager::WlDataDeviceManager,
    device: wl_data_device::WlDataDevice,
    primary_manager:
        Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
    primary_device: Option<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1>,
    shared: Rc<RefCell<ClipboardShared>>,
}

impl Clipboard {
    /// # Panics
    ///
    /// If `manager` is `None` -- the compositor never advertised
    /// `wl_data_device_manager`, which is this fail-fast contract's own
    /// (`harness/src/lib.rs`'s `set_selection_text`) and contract §3.8's.
    pub(crate) fn new(
        conn: Connection,
        qh: QueueHandle<WindowState>,
        manager: Option<wl_data_device_manager::WlDataDeviceManager>,
        seat: &wayland_client::protocol::wl_seat::WlSeat,
        primary_manager: Option<
            zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
        >,
        shared: Rc<RefCell<ClipboardShared>>,
    ) -> Self {
        let manager = manager.expect("compositor did not advertise wl_data_device_manager");
        let device = manager.get_data_device(seat, &qh, ());
        let primary_device = primary_manager
            .as_ref()
            .map(|manager| manager.get_device(seat, &qh, ()));
        Self {
            conn,
            qh,
            manager,
            device,
            primary_manager,
            primary_device,
            shared,
        }
    }

    /// Own the clipboard: create a `wl_data_source` advertising
    /// [`TEXT_MIME`], hold `text` to answer its `Send`, and `set_selection` it
    /// with `serial`.
    pub fn copy(&mut self, text: &str, serial: u32) {
        let source = self.manager.create_data_source(&self.qh, ());
        source.offer(TEXT_MIME.to_string());
        self.device.set_selection(Some(&source), serial);
        {
            let mut shared = self.shared.borrow_mut();
            shared.payload = text.to_string();
            shared.source = Some(source);
        }
        let _ = self.conn.flush();
    }

    /// Read the current clipboard offer, if any, blocking on its pipe until
    /// EOF or `deadline`.
    #[must_use]
    pub fn paste(&mut self, deadline: Duration) -> Option<String> {
        let offer = self.shared.borrow().offer.clone()?;
        let (read, write) = std::io::pipe().ok()?;
        offer.receive(TEXT_MIME.to_string(), write.as_fd());
        let _ = self.conn.flush();
        drop(write);
        read_offer_fd(read.into(), deadline)
    }

    /// As [`Clipboard::copy`], but for the primary (middle-click) selection.
    /// A silent no-op if the compositor never advertised
    /// `zwp_primary_selection_device_manager_v1`: unlike the clipboard proper,
    /// primary selection is a convenience some compositors omit.
    pub fn set_primary(&mut self, text: &str, serial: u32) {
        let (Some(manager), Some(device)) = (&self.primary_manager, &self.primary_device) else {
            tracing::warn!("no zwp_primary_selection_device_manager_v1; set_primary is a no-op");
            return;
        };
        let source = manager.create_source(&self.qh, ());
        source.offer(TEXT_MIME.to_string());
        device.set_selection(Some(&source), serial);
        {
            let mut shared = self.shared.borrow_mut();
            shared.primary_payload = text.to_string();
            shared.primary_source = Some(source);
        }
        let _ = self.conn.flush();
    }

    /// As [`Clipboard::paste`], but for the primary selection.
    #[must_use]
    pub fn primary(&mut self, deadline: Duration) -> Option<String> {
        let offer = self.shared.borrow().primary_offer.clone()?;
        let (read, write) = std::io::pipe().ok()?;
        offer.receive(TEXT_MIME.to_string(), write.as_fd());
        let _ = self.conn.flush();
        drop(write);
        read_offer_fd(read.into(), deadline)
    }

    /// Whether a clipboard offer is currently available to [`Clipboard::paste`].
    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.shared.borrow().offer.is_some()
    }

    /// Whether a primary-selection offer is currently available to
    /// [`Clipboard::primary`].
    #[must_use]
    pub fn has_primary(&self) -> bool {
        self.shared.borrow().primary_offer.is_some()
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(shared) = state.clipboard_shared.as_ref() else {
            return;
        };
        match event {
            // A new offer is being introduced; its `offer(mime)` events
            // follow before the `selection` that names it -- reset so the
            // mime list reflects only this offer.
            wl_data_device::Event::DataOffer { .. } => shared.borrow_mut().offer_mimes.clear(),
            wl_data_device::Event::Selection { id } => shared.borrow_mut().offer = id,
            _ => {}
        }
    }

    // `data_offer` (opcode 0) introduces a server-created `wl_data_offer`.
    event_created_child!(WindowState, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event
            && let Some(shared) = state.clipboard_shared.as_ref()
        {
            shared.borrow_mut().offer_mimes.push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for WindowState {
    fn event(
        state: &mut Self,
        source: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(shared) = state.clipboard_shared.as_ref() else {
            return;
        };
        if let wl_data_source::Event::Send { mime_type, fd } = event
            && mime_type == TEXT_MIME
            && shared.borrow().source.as_ref() == Some(source)
        {
            let payload = shared.borrow().payload.clone();
            let mut f = std::fs::File::from(fd);
            let _ = std::io::Write::write_all(&mut f, payload.as_bytes());
        }
    }
}

impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for WindowState {
    fn event(
        _: &mut Self,
        _: &wl_data_device_manager::WlDataDeviceManager,
        _: wl_data_device_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1, ()>
    for WindowState
{
    fn event(
        _: &mut Self,
        _: &zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
        _: zwp_primary_selection_device_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(shared) = state.clipboard_shared.as_ref() else {
            return;
        };
        match event {
            zwp_primary_selection_device_v1::Event::DataOffer { .. } => {
                shared.borrow_mut().primary_mimes.clear();
            }
            zwp_primary_selection_device_v1::Event::Selection { id } => {
                shared.borrow_mut().primary_offer = id;
            }
            _ => {}
        }
    }

    // `data_offer` (opcode 0) introduces a server-created primary offer.
    event_created_child!(WindowState, zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => (zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()),
    ]);
}

impl Dispatch<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()> for WindowState {
    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
        event: zwp_primary_selection_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = event
            && let Some(shared) = state.clipboard_shared.as_ref()
        {
            shared.borrow_mut().primary_mimes.push(mime_type);
        }
    }
}

impl Dispatch<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, ()> for WindowState {
    fn event(
        state: &mut Self,
        source: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        event: zwp_primary_selection_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(shared) = state.clipboard_shared.as_ref() else {
            return;
        };
        if let zwp_primary_selection_source_v1::Event::Send { mime_type, fd } = event
            && mime_type == TEXT_MIME
            && shared.borrow().primary_source.as_ref() == Some(source)
        {
            let payload = shared.borrow().primary_payload.clone();
            let mut f = std::fs::File::from(fd);
            let _ = std::io::Write::write_all(&mut f, payload.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TEXT_MIME, read_offer_fd};
    use std::io::Write;
    use std::time::Duration;

    #[test]
    fn the_mime_type_is_the_one_gtk_and_the_harness_agree_on() {
        assert_eq!(TEXT_MIME, "text/plain;charset=utf-8");
    }

    #[test]
    fn an_offer_is_read_to_eof() {
        let (read, mut write) = std::io::pipe().expect("pipe");
        std::thread::spawn(move || {
            write.write_all(b"hello ").expect("write");
            write.write_all("wörld".as_bytes()).expect("write");
        });
        assert_eq!(
            read_offer_fd(read.into(), Duration::from_secs(2)).as_deref(),
            Some("hello wörld")
        );
    }

    #[test]
    fn a_writer_that_never_finishes_times_out_instead_of_hanging() {
        // The other end of a data offer is another client. It can be stopped
        // in a debugger, or malicious; a blocking read would hang the UI
        // thread forever.
        let (read, write) = std::io::pipe().expect("pipe");
        let started = std::time::Instant::now();
        let text = read_offer_fd(read.into(), Duration::from_millis(120));
        assert!(text.is_none() || text.as_deref() == Some(""), "{text:?}");
        assert!(started.elapsed() < Duration::from_secs(2), "it hung");
        drop(write);
    }

    #[test]
    fn a_non_utf8_offer_is_dropped_not_a_panic() {
        // The mime type says UTF-8; another client may still send anything.
        let (read, mut write) = std::io::pipe().expect("pipe");
        std::thread::spawn(move || {
            let _ = write.write_all(&[0xFF, 0xFE, 0x00, 0x80]);
        });
        assert_eq!(read_offer_fd(read.into(), Duration::from_secs(2)), None);
    }

    #[test]
    fn an_enormous_offer_is_truncated_rather_than_exhausting_memory() {
        let (read, mut write) = std::io::pipe().expect("pipe");
        std::thread::spawn(move || {
            let chunk = vec![b'x'; 64 * 1024];
            for _ in 0..(super::MAX_OFFER_BYTES / chunk.len() + 8) {
                if write.write_all(&chunk).is_err() {
                    break;
                }
            }
        });
        let text = read_offer_fd(read.into(), Duration::from_secs(5)).expect("some text");
        assert!(
            text.len() <= super::MAX_OFFER_BYTES,
            "read {} bytes",
            text.len()
        );
    }
}

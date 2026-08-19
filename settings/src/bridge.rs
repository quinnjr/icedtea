//! The worker→main-thread bridge. A D-Bus worker sends `T` over an
//! `async_channel`; [`attach`] drains it on the GTK main thread via a local
//! future, so `on_main` — which touches widgets — only ever runs there.

use gtk4::glib;

/// Drain `rx` on the GTK main thread, calling `on_main` per message. Returns the
/// paired sender for the worker to push onto.
pub fn channel<T: 'static>(mut on_main: impl FnMut(T) + 'static) -> async_channel::Sender<T> {
    let (tx, rx) = async_channel::unbounded::<T>();
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            on_main(msg);
        }
    });
    tx
}

//! Bridges the GTK main loop and a tokio runtime for network I/O.

use std::future::Future;
use std::sync::OnceLock;

pub fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Runs `f` on the tokio runtime and awaits it from the GTK main loop.
pub async fn run<F, T>(f: F) -> anyhow::Result<T>
where
    F: Future<Output = anyhow::Result<T>> + Send + 'static,
    T: Send + 'static,
{
    rt().spawn(f).await.map_err(|e| anyhow::anyhow!("task failed: {e}"))?
}

/// Spawns a future on the GTK main context.
pub fn spawn_local<F: Future<Output = ()> + 'static>(f: F) {
    gtk::glib::spawn_future_local(f);
}

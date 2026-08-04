pub mod window;
pub mod layout;
pub mod decoration;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    tracing::info!("icedtea compositor starting");
}

//! `gallery` — every widget the M3 toolkit ships, on one page.
//!
//! ```text
//! cargo run -p icedtea-ui --bin gallery -- [OPTIONS]
//!
//!   --theme <light|dark|hc>   Which bundled sheet to compile. Default: light.
//!   --theme-file <PATH>       A sheet on disk instead of a bundled one.
//!   --widget <NAME>           Render exactly one widget, alone, at the origin.
//!   --list                    Print every widget name, one per line, and exit.
//!   --probe-points            Print `<widget> <label> <x> <y>` and exit.
//!   --print-allocation        Print `<widget> <x> <y> <w> <h>` and exit.
//!   --size <WxH>              Surface size. Default: 1280x800.
//!   --scroll <PX>             Scroll the page before the first frame.
//!   --scale <N>               Output scale, for HiDPI probes. Default: 1.
//! ```
//!
//! `--list`, `--probe-points` and `--print-allocation` never touch Wayland.

use icedtea_ui::gallery::{Options, print_list};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let opts = match Options::parse(std::env::args().skip(1)) {
        Ok(opts) => opts,
        Err(err) => {
            eprintln!("gallery: {err}");
            std::process::exit(2);
        }
    };

    if opts.list {
        print_list();
        return;
    }

    let result = if opts.probe_points {
        icedtea_ui::gallery::print_probe_points(&opts)
    } else if opts.print_allocation {
        icedtea_ui::gallery::print_allocations(&opts)
    } else {
        // Task 6 fills this in.
        eprintln!("gallery: nothing to do yet");
        std::process::exit(2);
    };
    if let Err(err) = result {
        eprintln!("gallery: {err:?}");
        std::process::exit(1);
    }
}

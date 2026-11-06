//! `pixel-image-gui` application library.
//!
//! Keeping the application state and view in a library makes headless UI
//! dumps and future tests possible without opening a window.

/// Best-effort stderr logging that never panics.
///
/// Release builds use the Windows GUI subsystem, so the process may have no
/// console at all. The standard `eprintln!` panics when the handle is closed;
/// this macro just drops the message in that case.
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = ::std::writeln!(::std::io::stderr(), $($arg)*);
    }};
}

pub mod app;
pub mod model;
pub mod settings;
pub mod view;
pub mod worker;

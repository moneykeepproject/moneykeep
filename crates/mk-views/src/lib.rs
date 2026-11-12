//! MoneyKeep's views (ui-slint.md revision 2).
#![allow(clippy::float_arithmetic)]

pub mod sl {
    #![allow(
        clippy::all,
        clippy::pedantic,
        clippy::restriction,
        clippy::nursery,
        missing_debug_implementations,
        unused_qualifications
    )]
    slint::include_modules!();
}

mod convert;
pub mod phone;
mod window;

#[cfg(feature = "testing")]
pub mod testing;

/// The toolkit, for the app shells that drive the windows (event loop, timers).
pub use slint;
pub use window::Views;

//! Models, identifiers and failures that describe Orbiter's subject matter.
//!
//! Nothing here knows about Tauri, about the window, or about how anything is stored. These are
//! the types the application services are written in terms of, so a rule expressed here — that a
//! review authorises one exact artifact, that a device tag is not a UDID — holds everywhere rather
//! than being re-checked at each layer.

pub mod errors;
pub mod identifiers;

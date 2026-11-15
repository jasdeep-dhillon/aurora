//! What differs per OS: loading Valve's library, reading its RTTI class
//! names, spotting a running official client, and leaving the process.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

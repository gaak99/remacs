//! Lisp functions for making directory listings.

#[cfg(unix)]
include!("dired_unix.rs");

#[cfg(windows)]
include!("dired_windows.rs");

include!(concat!(env!("OUT_DIR"), "/dired_exports.rs"));

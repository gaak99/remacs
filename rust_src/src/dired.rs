//! Lisp functions for making directory listings.

#[cfg(unix)]
include!("dired_unix.in");

#[cfg(windows)]
include!("dired_windows.in");

include!(concat!(env!("OUT_DIR"), "/dired_exports.rs"));

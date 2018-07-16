//! Lisp functions for making directory listings.

#[cfg(unix)]
include!("dired_unix.rs__");

#[cfg(windows)]
include!("dired_windows.rs__");

include!(concat!(env!("OUT_DIR"), "/dired_exports.rs"));

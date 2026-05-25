//! Local IPC transport for the daemon. UDS on Linux/macOS, named pipe on
//! Windows. Both back the same JSON-RPC 2.0 wire protocol with
//! newline-delimited frames per ADR-019.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{default_socket_path, serve};

#[cfg(windows)]
pub use windows::{default_socket_path, serve};

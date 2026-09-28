//! Offline TouchDesigner project I/O orchestrated over Derivative's official
//! `toeexpand` / `toecollapse` tools.
//!
//! Reliability law (probe-proven against real TD):
//!
//! - Official-tool exit codes lie in both directions. Success is judged solely by
//!   filesystem evidence (expand dir + toc exist; packed file non-empty), never by
//!   exit status.
//! - `.toc` files are strict LF / no BOM. A CRLF toc makes `toecollapse` emit a
//!   silent 0-byte output file.
//! - Missing tools are a typed availability condition, never a panic; Derivative
//!   binaries are invoked where installed, never redistributed.
//! - Collision-sensitive callers can publish with an atomic no-replace rename
//!   where supported; multi-artifact publications still require caller rollback.
//!
//! Crate boundary: pure platform logic, no config-crate dependency — callers map
//! `[official_tools]` config into [`resolve::ToolSource`].

pub mod error;
pub mod ops;
pub mod palette;
pub mod publish;
pub mod resolve;
pub mod runner;
pub mod sidecar;
pub mod sniff;
pub mod stage;
pub mod toc;
pub mod wine;

pub use error::ProjectIoError;

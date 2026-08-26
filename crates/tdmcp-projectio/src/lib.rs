//! Offline TouchDesigner project I/O orchestrated over Derivative's official
//! `toeexpand` / `toecollapse` tools.
//!
//! Reliability law (V2-0 probe-proven, see `docs/SKILLS_CONTRACT_PROPOSAL.md` §6.1):
//!
//! - Official-tool exit codes lie in both directions. Success is judged solely by
//!   filesystem evidence (expand dir + toc exist; packed file non-empty), never by
//!   exit status.
//! - `.toc` files are strict LF / no BOM. A CRLF toc makes `toecollapse` emit a
//!   silent 0-byte output file.
//! - Missing tools are a typed availability condition, never a panic; Derivative
//!   binaries are invoked where installed, never redistributed.
//! - All mutation happens in staging directories and is published by atomic rename;
//!   failures clean up their partials.
//!
//! Crate boundary: pure platform logic, no config-crate dependency — callers map
//! `[official_tools]` config into [`resolve::ToolSource`].

pub mod error;
pub mod ops;
pub mod resolve;
pub mod runner;
pub mod sniff;
pub mod sidecar;
pub mod stage;
pub mod toc;

pub use error::ProjectIoError;

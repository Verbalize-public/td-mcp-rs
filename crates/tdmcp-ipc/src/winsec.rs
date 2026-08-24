//! Windows named-pipe security descriptor (quarantined Win32 FFI).
//!
//! This module is the ONLY `unsafe` carve-out in the workspace
//! (constitution amendment 2026-08-23; `RISKS.md` R8). The daemon may run
//! elevated; with Windows' default descriptor, a pipe created by an elevated
//! process denies write access to non-elevated clients of the same user (UAC
//! filtered token), so bridges fail to connect. Every pipe instance is
//! therefore created with an explicit SDDL descriptor granting generic
//! read/write to authenticated users.
//!
//! Invariant: callers only see safe functions; all raw FFI stays here and the
//! allocated security descriptor is always freed via `LocalFree`.

// RISKS.md R8 — quarantined Win32 FFI for pipe security descriptors.
#![allow(unsafe_code)]

mod raw {
    pub use windows_sys::Win32::Foundation::LocalFree;
    pub use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    pub use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
}

use std::ffi::OsStr;
use std::io;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

/// Least-privilege descriptor for the local bridge pipe: generic read/write
/// for authenticated users — no DACL-mutation or other rights.
const PIPE_SDDL: &str = "D:(A;;GRGW;;;AU)";

/// Create one named-pipe server instance like [`ServerOptions::create`], but
/// with the permissive descriptor from [`PIPE_SDDL`].
pub(crate) fn create_server(name: &str, first_pipe_instance: bool) -> io::Result<NamedPipeServer> {
    let sddl_w = wide(PIPE_SDDL);
    let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut sd_len = 0u32;

    // SAFETY(R8): writes through `sd`/`sd_len` only on success; both are
    // stack locals owned by this call.
    let ok = unsafe {
        raw::ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_w.as_ptr(),
            raw::SDDL_REVISION_1,
            &mut sd,
            &mut sd_len,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    let mut sa = raw::SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<raw::SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };

    // SAFETY(R8): `sa.lpSecurityDescriptor` must outlive the call only;
    // CreateNamedPipeW copies the descriptor into the kernel object.
    let created = unsafe {
        ServerOptions::new()
            .first_pipe_instance(first_pipe_instance)
            .create_with_security_attributes_raw(
                OsStr::new(name),
                (&mut sa as *mut raw::SECURITY_ATTRIBUTES).cast(),
            )
    };

    // SAFETY(R8): `sd` came from ConvertStringSD… above and is unused after.
    unsafe { raw::LocalFree(sd) };

    created
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

//! Answering a WebKitGTK authentication request with a credential. webkit2gtk-rs 2.0.2 binds everything about
//! the request except `webkit_authentication_request_authenticate` itself, so this is that one call - kept in
//! its own crate so the rest of felt-tray stays `forbid(unsafe_code)`.

use webkit2gtk::glib::translate::ToGlibPtr;
use webkit2gtk::{AuthenticationRequest, Credential};

/// Completes `request` with `credential`; `None` continues without one (the server then decides).
#[allow(unsafe_code)]
pub fn authenticate(request: &AuthenticationRequest, credential: Option<&Credential>) {
    let cred: *const webkit2gtk::ffi::WebKitCredential = credential.to_glib_none().0;
    // SAFETY: both pointers come from live wrappers borrowed for the whole call; WebKit copies the credential
    // (it's a boxed type) and takes no ownership of either.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage  audited: one FFI call, see SAFETY above
    unsafe {
        webkit2gtk::ffi::webkit_authentication_request_authenticate(
            request.to_glib_none().0,
            cred.cast_mut(),
        );
    }
}

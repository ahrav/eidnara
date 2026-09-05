use std::ffi::c_void;
use std::path::PathBuf;

use napi::{Env, Result, sys};

pub(crate) fn register_cleanup_marker(env: &Env, path: PathBuf) -> Result<()> {
    env.add_env_cleanup_hook(path, |path| {
        let _ = std::fs::write(path, b"clean");
    })?;
    Ok(())
}

/// An async cleanup hook must remove its own handle to signal completion.
unsafe extern "C" fn finish_probe_hook(
    handle: sys::napi_async_cleanup_hook_handle,
    _data: *mut c_void,
) {
    // SAFETY: handle was issued by napi_add_async_cleanup_hook for this hook.
    let _ = unsafe { sys::napi_remove_async_cleanup_hook(handle) };
}

/// If explicit removal fails, the hook removes its handle during environment cleanup.
pub(crate) fn probe_async_cleanup_hooks(env: &Env) -> Result<()> {
    let mut handle = std::ptr::null_mut();
    // SAFETY: `handle` is writable storage for the registration; the hook carries no data.
    let status = unsafe {
        sys::napi_add_async_cleanup_hook(
            env.raw(),
            Some(finish_probe_hook),
            std::ptr::null_mut(),
            &mut handle,
        )
    };
    if status != sys::Status::napi_ok {
        return Err(super::error("async cleanup hook registration failed"));
    }
    // SAFETY: handle was just issued for this env and has not been removed.
    let _ = unsafe { sys::napi_remove_async_cleanup_hook(handle) };
    Ok(())
}

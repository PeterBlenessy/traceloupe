//! Power-management assertion — keep the Mac awake while a long job runs.
//!
//! A Safety Scan can run for hours (a year of messages, ~60–100 s per chunk on
//! E4B). If the machine drops to idle sleep mid-scan the in-flight llama-server
//! request stalls; the 300 s read timeout then fails that chunk on wake, and an
//! unattended overnight scan quietly stalls. While a scan is in flight we hold a
//! `PreventUserIdleSystemSleep` power assertion and release it the moment the
//! scan finishes, stops, fails, or panics — the guard is an RAII value, so every
//! exit path out of the scan closure drops it.
//!
//! Only *system* idle sleep is prevented; the display is still free to sleep, so
//! the screen dims as usual while the CPU/GPU keep working (this is not
//! `caffeinate -d`). On non-macOS platforms it is a no-op.

/// A held power assertion. Kept alive for a scope; dropping it releases the
/// assertion. Constructing it can fail silently (no assertion held) — that only
/// means the OS may idle-sleep, never a scan error.
pub struct KeepAwake {
    #[cfg(target_os = "macos")]
    id: Option<u32>,
}

impl KeepAwake {
    /// Prevent system idle sleep until the returned guard is dropped. `reason`
    /// is a short human string macOS lists in `pmset -g assertions`.
    pub fn prevent_idle_sleep(reason: &str) -> Self {
        #[cfg(target_os = "macos")]
        {
            KeepAwake {
                id: macos::create(reason),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reason;
            KeepAwake {}
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(id) = self.id.take() {
            macos::release(id);
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_void};

    // IOPMAssertionID / IOReturn are 32-bit ints in <IOKit/pwr_mgt/IOPMLib.h>.
    type IOPMAssertionID = u32;
    type IOReturn = i32;
    const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;
    const K_IO_RETURN_SUCCESS: IOReturn = 0;
    // kCFStringEncodingUTF8 from <CoreFoundation/CFString.h>.
    const UTF8: u32 = 0x0800_0100;

    #[repr(C)]
    struct CFStringOpaque(c_void);
    type CFStringRef = *const CFStringOpaque;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            assertion_level: u32,
            assertion_name: CFStringRef,
            assertion_id: *mut IOPMAssertionID,
        ) -> IOReturn;
        fn IOPMAssertionRelease(assertion_id: IOPMAssertionID) -> IOReturn;
    }

    /// Build an owned `CFStringRef` from a Rust `&str` (caller must `CFRelease`
    /// it). Returns null if the string contains an interior NUL.
    fn cfstr(s: &str) -> CFStringRef {
        let Ok(c) = CString::new(s) else {
            return std::ptr::null();
        };
        // SAFETY: `c` lives for the duration of the call and CF copies its bytes.
        unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) }
    }

    pub fn create(reason: &str) -> Option<IOPMAssertionID> {
        // The documented assertion-type constant is the CFSTR of this literal.
        let atype = cfstr("PreventUserIdleSystemSleep");
        let aname = cfstr(reason);
        if atype.is_null() || aname.is_null() {
            // SAFETY: each pointer is either null (skipped) or a live CFString.
            unsafe {
                if !atype.is_null() {
                    CFRelease(atype as *const c_void);
                }
                if !aname.is_null() {
                    CFRelease(aname as *const c_void);
                }
            }
            return None;
        }
        let mut id: IOPMAssertionID = 0;
        // SAFETY: both args are valid non-null CFStringRefs and `id` is a valid
        // out-param for the duration of the call.
        let rc = unsafe {
            IOPMAssertionCreateWithName(atype, K_IOPM_ASSERTION_LEVEL_ON, aname, &mut id)
        };
        // SAFETY: both were created by CFStringCreateWithCString above; CF has
        // retained its own copies for the assertion, so releasing ours is safe.
        unsafe {
            CFRelease(atype as *const c_void);
            CFRelease(aname as *const c_void);
        }
        (rc == K_IO_RETURN_SUCCESS).then_some(id)
    }

    pub fn release(id: IOPMAssertionID) {
        // SAFETY: `id` was returned by a successful IOPMAssertionCreateWithName.
        unsafe {
            IOPMAssertionRelease(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_creates_and_drops_cleanly() {
        // On macOS this holds and releases a real assertion; elsewhere it is a
        // no-op. Either way, construct-then-drop must not panic.
        let guard = KeepAwake::prevent_idle_sleep("TraceLoupe test");
        drop(guard);
    }
}

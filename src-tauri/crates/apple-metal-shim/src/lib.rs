use std::ffi::c_void;

pub struct MetalDevice {
    raw: *mut c_void,
}

pub type ManuallyDropDevice = MetalDevice;

impl MetalDevice {
    /// # Safety
    ///
    /// The caller must pass a valid borrowed `MTLDevice` pointer. This shim only
    /// preserves the pointer for ScreenCaptureKit type compatibility; RChat does
    /// not use apple-metal APIs directly.
    pub unsafe fn from_raw_borrowed(raw: *mut c_void) -> Self {
        Self { raw }
    }

    pub fn as_raw_ptr(&self) -> *mut c_void {
        self.raw
    }
}

unsafe impl Send for MetalDevice {}
unsafe impl Sync for MetalDevice {}

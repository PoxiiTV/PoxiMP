//! macOS "exclusive" mode: CoreAudio hog mode plus the device's nominal sample rate set to the
//! file's. Streaming itself stays on cpal. Only these two properties are touched.
//!
//! UNVERIFIED: written and type-checked without a Mac to run it on.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::{null, NonNull};
use std::time::{Duration, Instant};

use objc2_core_audio::{
    kAudioDevicePropertyHogMode, kAudioDevicePropertyNominalSampleRate, kAudioHardwarePropertyTranslateUIDToDevice,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, AudioObjectGetPropertyData,
    AudioObjectID, AudioObjectPropertyAddress, AudioObjectPropertySelector, AudioObjectSetPropertyData,
};
use objc2_core_foundation::CFString;

/// The HAL applies a rate change asynchronously: wait this long for it to show.
const RATE_SETTLE: Duration = Duration::from_millis(500);

/// Held while playing. Dropping it gives the device back and restores its previous rate.
pub(crate) struct Hog {
    device: AudioObjectID,
    rate_before: f64,
}

impl Hog {
    /// `uid` = CoreAudio device UID (cpal's id without the host prefix). Returns the rate the
    /// device runs at now, or None when another process hogs it or the HAL refuses.
    pub fn take(uid: &str, rate: u32) -> Option<(Self, u32)> {
        let device = device_for_uid(uid)?;
        let pid = std::process::id() as i32;
        let owner: i32 = get(device, kAudioDevicePropertyHogMode)?;
        if owner != -1 && owner != pid {
            return None;
        }
        if owner != pid && (!set(device, kAudioDevicePropertyHogMode, &pid) || get::<i32>(device, kAudioDevicePropertyHogMode)? != pid) {
            return None;
        }
        // From here on, dropping `hog` releases the device again.
        let rate_before: f64 = get(device, kAudioDevicePropertyNominalSampleRate).unwrap_or(0.0);
        let hog = Self { device, rate_before };
        let want = f64::from(rate);
        if rate_before != want && set(device, kAudioDevicePropertyNominalSampleRate, &want) {
            let t0 = Instant::now();
            while get::<f64>(device, kAudioDevicePropertyNominalSampleRate) != Some(want) && t0.elapsed() < RATE_SETTLE {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let now = get::<f64>(device, kAudioDevicePropertyNominalSampleRate).unwrap_or(want);
        Some((hog, now.round() as u32))
    }
}

impl Drop for Hog {
    fn drop(&mut self) {
        // ponytail: a crash skips this; the HAL frees the hog with the process but the rate
        // stays changed (other apps get resampled). Restore it at startup if that bothers.
        if self.rate_before > 0.0 {
            set(self.device, kAudioDevicePropertyNominalSampleRate, &self.rate_before);
        }
        set(self.device, kAudioDevicePropertyHogMode, &-1i32);
    }
}

fn address(selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain }
}

/// Reads a fixed-size property (i32 pid, f64 rate, u32 id).
fn get<T: Copy + Default>(object: AudioObjectID, selector: AudioObjectPropertySelector) -> Option<T> {
    let addr = address(selector);
    let mut value = T::default();
    let mut size = size_of::<T>() as u32;
    // SAFETY: `value` is a valid, writable T and `size` says how big it is; the HAL writes at
    // most `size` bytes and we only use the value on success.
    let status = unsafe {
        AudioObjectGetPropertyData(object, NonNull::from(&addr), 0, null(), NonNull::from(&mut size), NonNull::from(&mut value).cast())
    };
    (status == 0 && size as usize == size_of::<T>()).then_some(value)
}

fn set<T>(object: AudioObjectID, selector: AudioObjectPropertySelector, value: &T) -> bool {
    let addr = address(selector);
    // SAFETY: `value` points at `size_of::<T>()` readable bytes for the duration of the call.
    let status = unsafe {
        AudioObjectSetPropertyData(object, NonNull::from(&addr), 0, null(), size_of::<T>() as u32, NonNull::from(value).cast())
    };
    status == 0
}

/// kAudioHardwarePropertyTranslateUIDToDevice: the qualifier is a CFStringRef to the UID.
fn device_for_uid(uid: &str) -> Option<AudioObjectID> {
    let cf = CFString::from_str(uid);
    let uid_ref: *const CFString = &*cf;
    let addr = address(kAudioHardwarePropertyTranslateUIDToDevice);
    let mut device: AudioObjectID = 0;
    let mut size = size_of::<AudioObjectID>() as u32;
    // SAFETY: the qualifier is a pointer to a live CFStringRef (`cf` outlives the call) and the
    // output is a writable AudioObjectID of the declared size.
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&addr),
            size_of::<*const CFString>() as u32,
            (&uid_ref as *const *const CFString).cast::<c_void>(),
            NonNull::from(&mut size),
            NonNull::from(&mut device).cast(),
        )
    };
    // kAudioObjectUnknown (0) = no such device.
    (status == 0 && device != 0).then_some(device)
}

//! WASAPI exclusive output (Windows bit-perfect). One render thread per open device: event
//! driven, integer PCM at the file's own rate and depth when the device takes it (else the
//! nearest format it does). The producer reopens it when the next file's format differs.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use wasapi::{calculate_period_100ns, DeviceEnumerator, Direction, SampleType, ShareMode, StreamMode, WasapiError, WaveFormat};

use crate::output::{exclusive_candidates, is_bit_perfect, to_int_le, Format, Opened, Output, Renderer, Shared, RING_SECS};
use crate::OutputInfo;

const AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED: u32 = 0x8889_0019;
/// "Allow applications to take exclusive control of this device" is off in Windows.
const AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED: u32 = 0x8889_000E;
const OPEN_TIMEOUT: Duration = Duration::from_secs(3);
const CHANNELS: usize = 2;

struct WasapiOutput {
    info: OutputInfo,
    stop: Arc<AtomicBool>,
    broken: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Output for WasapiOutput {
    fn info(&self) -> &OutputInfo {
        &self.info
    }

    fn info_mut(&mut self) -> &mut OutputInfo {
        &mut self.info
    }

    fn broken(&self) -> bool {
        self.broken.load(Relaxed)
    }
}

impl Drop for WasapiOutput {
    /// Stops the stream and releases the device (exclusivity ends with the client).
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Ok, or the `OutputInfo::fallback` code for why not.
type Ready = Result<(OutputInfo, rtrb::Producer<f32>), &'static str>;

fn hresult(e: &WasapiError) -> u32 {
    match e {
        WasapiError::Windows(w) => w.code().0 as u32,
        _ => 0,
    }
}

/// `endpoint` = WASAPI device id (cpal's id without the host prefix).
pub(crate) fn open(shared: &Arc<Shared>, endpoint: &str, device_id: String, f: Format) -> Result<Opened, &'static str> {
    let (tx, rx) = sync_channel::<Ready>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let broken = Arc::new(AtomicBool::new(false));
    let (s, b, sh, ep) = (stop.clone(), broken.clone(), shared.clone(), endpoint.to_string());
    let thread = thread::Builder::new()
        .name("poximp-wasapi".into())
        .spawn(move || {
            if let Err(e) = run(&sh, &ep, device_id, f, &tx, &s) {
                // Before Ready the opener gets the refusal; after it, the stream died.
                let why = if hresult(&e) == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED { "exclusiveNotAllowed" } else { "exclusiveFailed" };
                let _ = tx.try_send(Err(why));
                b.store(true, Relaxed);
            }
        })
        .map_err(|_| "exclusiveFailed")?;
    let mut out = WasapiOutput { info: placeholder(), stop, broken, thread: Some(thread) };
    // Dropping `out` on an error stops and joins the thread.
    match rx.recv_timeout(OPEN_TIMEOUT) {
        Ok(Ok((info, ring))) => {
            out.info = info;
            Ok((Box::new(out), ring))
        }
        Ok(Err(why)) => Err(why),
        Err(_) => Err("exclusiveFailed"),
    }
}

fn placeholder() -> OutputInfo {
    OutputInfo {
        device: String::new(),
        device_id: String::new(),
        sample_rate: 0,
        channels: 0,
        sample_format: String::new(),
        exclusive: true,
        bit_perfect: false,
        fallback: None,
    }
}

/// Opens the device, reports `Ready`, then renders until `stop` (or an error).
fn run(shared: &Arc<Shared>, endpoint: &str, device_id: String, f: Format, ready: &SyncSender<Ready>, stop: &AtomicBool) -> Result<(), WasapiError> {
    wasapi::initialize_mta().ok()?;
    // MMCSS "Pro Audio": exclusive buffers are ~10 ms, this thread must not wait behind others.
    // SAFETY: plain Win32 call with a static wide string; the handle lives as long as the thread.
    let _ = unsafe { windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut 0) };
    let device = DeviceEnumerator::new()?.get_device(endpoint)?;
    let mut client = device.get_iaudioclient()?;
    // Exclusive mode switched off for this device: no format will do, say why instead.
    let probe = WaveFormat::new(16, 16, &SampleType::Int, f.rate as usize, CHANNELS, None);
    if let Err(e) = client.is_supported(&probe, &ShareMode::Exclusive) {
        if hresult(&e) == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED {
            return Err(e);
        }
    }
    let (fmt, rate, store, valid) = exclusive_candidates(f)
        .into_iter()
        .find_map(|(r, s, v)| {
            let want = WaveFormat::new(s.into(), v.into(), &SampleType::Int, r as usize, CHANNELS, None);
            client.is_supported_exclusive_with_quirks(&want).ok().map(|fmt| (fmt, r, s, v))
        })
        .ok_or(WasapiError::UnsupportedFormat)?;
    let (default_period, _) = client.get_device_period()?;
    let period = client.calculate_aligned_period_near(default_period, Some(128), &fmt)?;
    if let Err(e) = client.initialize_client(&fmt, &Direction::Render, &StreamMode::EventsExclusive { period_hns: period }) {
        if hresult(&e) != AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED {
            return Err(e);
        }
        // The documented recovery: the aligned buffer size, a new client, initialize again.
        let frames = client.get_buffer_size()?;
        let period = calculate_period_100ns(i64::from(frames), i64::from(rate));
        client = device.get_iaudioclient()?;
        client.initialize_client(&fmt, &Direction::Render, &StreamMode::EventsExclusive { period_hns: period })?;
    }
    let event = client.set_get_eventhandle()?;
    let render = client.get_audiorenderclient()?;
    let frames = client.get_buffer_size()? as usize;
    let width = usize::from(store / 8);

    let cap = ((f64::from(rate) * RING_SECS) as usize) * 2;
    let (producer, consumer) = rtrb::RingBuffer::new(cap);
    shared.rate.store(rate, Relaxed);
    shared.flush.store(false, Relaxed);
    shared.frames.store(0, Relaxed);
    let mut renderer = Renderer::new(consumer, shared.clone(), rate, true);
    // Allocated once: the loop below never allocates.
    let mut samples = vec![0f32; frames * CHANNELS];
    let mut bytes = vec![0u8; frames * CHANNELS * width];
    let mut write = |n: usize| -> Result<(), WasapiError> {
        let (s, b) = (&mut samples[..n * CHANNELS], &mut bytes[..n * CHANNELS * width]);
        renderer.render(s, CHANNELS, |x| x);
        for (x, out) in s.iter().zip(b.chunks_exact_mut(width)) {
            to_int_le(*x, out);
        }
        render.write_to_device(n, b, None)
    };
    // Start with a full buffer (silence while the ring fills), as WASAPI asks.
    write(frames)?;
    client.start_stream()?;

    let bit_perfect = is_bit_perfect(f, rate, valid);
    let info = OutputInfo {
        device: device.get_friendlyname().unwrap_or_default(),
        device_id,
        sample_rate: rate,
        channels: CHANNELS as u16,
        sample_format: format!("i{valid}"),
        exclusive: true,
        bit_perfect,
        fallback: (!bit_perfect).then_some("formatChanged"),
    };
    if ready.send(Ok((info, producer))).is_err() {
        return Ok(());
    }
    let result = loop {
        if stop.load(Relaxed) {
            break Ok(());
        }
        if let Err(e) = event.wait_for_event(1000) {
            break Err(e);
        }
        // Event-driven exclusive: every event asks for one whole buffer.
        if let Err(e) = client.get_available_space_in_frames().and_then(|n| write((n as usize).min(frames))) {
            break Err(e);
        }
    };
    let _ = client.stop_stream();
    result
}

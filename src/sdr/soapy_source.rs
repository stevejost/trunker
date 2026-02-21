//! SoapySDR hardware device source for live IQ sample streaming.
//!
//! Wraps a SoapySDR `Device` and `RxStream` to provide an `Iterator`
//! over `Complex<f32>` samples from real SDR hardware.
//!
//! A dedicated reader thread drains the hardware into a bounded channel,
//! decoupling USB/driver timing from DSP processing. The channel depth
//! is controlled by `buffer_ms` (default 500 ms), giving the processing
//! pipeline headroom to absorb jitter without overflowing the driver's
//! tiny internal ring buffer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};

use num_complex::Complex;
use soapysdr::{Device, Direction, ErrorCode, RxStream};

use super::error::SdrError;

/// Receive channel index (single-channel devices).
const RX_CHANNEL: usize = 0;

/// Stream read timeout in microseconds (1 second).
const READ_TIMEOUT_US: i64 = 1_000_000;

/// Maximum consecutive non-timeout errors before the reader thread gives up.
const MAX_CONSECUTIVE_ERRORS: u32 = 50;

/// A live IQ sample source backed by a SoapySDR device.
///
/// Opens an SDR device, configures frequency/rate/gain, and yields
/// `Complex<f32>` samples via the `Iterator` trait. A dedicated reader
/// thread drains the hardware into a bounded channel so that processing
/// jitter does not cause driver overflows.
///
/// The stream stops when `running` is set to `false` (e.g. from a
/// Ctrl-C handler) or when the reader thread encounters a fatal error.
pub struct SoapySource {
    receiver: Option<Receiver<Result<Vec<Complex<f32>>, SdrError>>>,
    current_chunk: Vec<Complex<f32>>,
    position: usize,
    reader_thread: Option<JoinHandle<()>>,
    running: Arc<AtomicBool>,
    /// Total chunks successfully read from hardware by the reader thread.
    chunk_count: Arc<AtomicU64>,
    /// Total overflows (hardware + channel-full drops) in the reader thread.
    overflow_count: Arc<AtomicU64>,
    /// Device error captured when the reader thread reports a fatal failure.
    /// `None` means the stream ended normally (signal or EOF).
    device_error: Option<SdrError>,
}

impl std::fmt::Debug for SoapySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoapySource")
            .field("position", &self.position)
            .field("chunk_len", &self.current_chunk.len())
            .field(
                "thread_alive",
                &self
                    .reader_thread
                    .as_ref()
                    .is_some_and(|h| !h.is_finished()),
            )
            .field("chunk_count", &self.chunk_count.load(Ordering::Relaxed))
            .field(
                "overflow_count",
                &self.overflow_count.load(Ordering::Relaxed),
            )
            .finish()
    }
}

impl SoapySource {
    /// Open a SoapySDR device and configure it for receiving.
    ///
    /// Spawns a dedicated reader thread that drains the hardware into a
    /// bounded channel. The channel depth is `buffer_ms` milliseconds of
    /// samples, giving the processing pipeline headroom to absorb jitter.
    ///
    /// # Arguments
    /// * `device_args` -- device filter string (e.g. `"driver=rtlsdr"`)
    /// * `frequency_hz` -- center frequency in hertz
    /// * `sample_rate_hz` -- sample rate in hertz
    /// * `gain` -- manual gain in dB, or `None` for automatic gain control
    /// * `antenna` -- antenna port name, or `None` for default
    /// * `settings` -- device-specific key=value settings
    /// * `buffer_ms` -- channel buffer depth in milliseconds
    /// * `running` -- shared flag checked before each read; set to `false` to stop
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        device_args: &str,
        frequency_hz: u64,
        sample_rate_hz: u32,
        gain: Option<f64>,
        antenna: Option<&str>,
        settings: &[(String, String)],
        buffer_ms: u32,
        running: Arc<AtomicBool>,
    ) -> Result<Self, SdrError> {
        // Suppress stderr during init — SoapySDR and RTL-SDR drivers print
        // noisy informational messages (e.g. "Found Rafael Micro R820T tuner")
        // directly to stderr, which corrupts TUI displays when piped to a monitor.
        let (device, mut stream, mtu) = suppress_stderr(|| -> Result<_, SdrError> {
            let device = open_device(device_args)?;
            // Init order: sample_rate -> frequency -> gain -> antenna ->
            // stream -> activate. Setting sample rate first ensures the
            // hardware's bandwidth filter is configured before the PLL
            // locks to the target frequency.
            configure_sample_rate(&device, sample_rate_hz)?;
            configure_frequency(&device, frequency_hz)?;
            configure_gain(&device, gain)?;
            if let Some(ant) = antenna {
                configure_antenna(&device, ant)?;
            }
            let mut stream = create_stream(&device)?;
            let mtu = stream
                .mtu()
                .map_err(|e| SdrError::StreamCreate(e.message))?;
            stream
                .activate(None)
                .map_err(|e| SdrError::StreamActivate(e.message))?;
            Ok((device, stream, mtu))
        })?;

        // Apply device-specific settings (rfgain_sel, biasT_ctrl, etc.)
        // AFTER activation and OUTSIDE suppress_stderr. The SDRplay driver
        // only calls sdrplay_api_Update() when streamActive is true, and
        // if that Update fails the driver logs a warning but does NOT
        // return an error to SoapySDR — keeping this outside suppress_stderr
        // ensures those warnings are visible.
        apply_settings(&device, settings)?;
        log_device_state(&device, antenna);

        // Compute channel capacity from buffer_ms.
        let buffer_ms = buffer_ms.max(1);
        let capacity =
            ((buffer_ms as u64 * sample_rate_hz as u64) / (mtu as u64 * 1000)).max(2) as usize;

        tracing::info!(
            device_args,
            frequency_hz,
            sample_rate_hz,
            ?gain,
            mtu,
            buffer_ms,
            channel_capacity = capacity,
            "SoapySDR device opened"
        );

        // Discard the first ~200 ms of samples to let the hardware settle.
        // SDR devices (especially SDRplay) produce transient artifacts during
        // PLL lock and AGC convergence that can poison CQPSK feedback loops.
        let settle_samples = (sample_rate_hz as usize) / 5; // 200 ms
        let mut discarded = 0;
        let mut settle_buf = vec![Complex::new(0.0, 0.0); mtu];
        while discarded < settle_samples {
            match read_chunk(&mut stream, &mut settle_buf) {
                ReadResult::Data(n) => discarded += n,
                ReadResult::Retry => continue,
                ReadResult::Fatal => break,
            }
        }
        tracing::debug!(discarded, "settling samples discarded");

        let (sender, receiver) =
            mpsc::sync_channel::<Result<Vec<Complex<f32>>, SdrError>>(capacity);
        let thread_running = running.clone();
        let chunk_count = Arc::new(AtomicU64::new(0));
        let overflow_count = Arc::new(AtomicU64::new(0));
        let thread_chunks = chunk_count.clone();
        let thread_overflows = overflow_count.clone();

        let handle = thread::Builder::new()
            .name("sdr-reader".into())
            .spawn(move || {
                reader_loop(
                    stream,
                    mtu,
                    sender,
                    thread_running,
                    thread_chunks,
                    thread_overflows,
                );
            })
            .map_err(|e| SdrError::StreamCreate(format!("failed to spawn reader thread: {e}")))?;

        Ok(Self {
            receiver: Some(receiver),
            current_chunk: Vec::new(),
            position: 0,
            reader_thread: Some(handle),
            running,
            chunk_count,
            overflow_count,
            device_error: None,
        })
    }

    /// Total chunks successfully read from hardware since stream start.
    pub fn chunk_count(&self) -> u64 {
        self.chunk_count.load(Ordering::Relaxed)
    }

    /// Total overflows (hardware + channel-full drops) since stream start.
    pub fn overflow_count(&self) -> u64 {
        self.overflow_count.load(Ordering::Relaxed)
    }

    /// Shared handle to the chunk counter (survives source consumption).
    pub fn chunk_count_handle(&self) -> Arc<AtomicU64> {
        self.chunk_count.clone()
    }

    /// Shared handle to the overflow counter (survives source consumption).
    pub fn overflow_count_handle(&self) -> Arc<AtomicU64> {
        self.overflow_count.clone()
    }

    /// Return the device error if the stream ended due to a hardware failure.
    ///
    /// Returns `None` if the stream ended normally (signal or EOF).
    /// Check this after the iterator returns `None` to distinguish
    /// graceful shutdown from device errors.
    pub fn device_error(&self) -> Option<&SdrError> {
        self.device_error.as_ref()
    }
}

impl Iterator for SoapySource {
    type Item = Complex<f32>;

    fn next(&mut self) -> Option<Self::Item> {
        // Drain current chunk first.
        if self.position < self.current_chunk.len() {
            let sample = self.current_chunk[self.position];
            self.position += 1;
            return Some(sample);
        }

        // Need a new chunk from the reader thread.
        let receiver = self.receiver.as_ref()?;
        match receiver.recv() {
            Ok(Ok(chunk)) => {
                self.current_chunk = chunk;
                self.position = 1;
                Some(self.current_chunk[0])
            }
            Ok(Err(err)) => {
                // Reader thread sent a fatal device error.
                self.device_error = Some(err);
                None
            }
            Err(_) => {
                // Channel closed — reader thread exited (normal shutdown).
                None
            }
        }
    }
}

impl Drop for SoapySource {
    fn drop(&mut self) {
        // Signal the reader thread to stop.
        self.running.store(false, Ordering::Relaxed);
        // Drop the receiver to unblock a send() that may be blocked on a
        // full channel. Must happen before join() to avoid deadlock.
        self.receiver.take();
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}

/// Result of a single `stream.read()` call.
enum ReadResult {
    /// Successfully read `n` samples.
    Data(usize),
    /// Transient error (overflow, timeout, corruption) — caller should retry.
    Retry,
    /// Fatal error — caller should stop.
    Fatal,
}

/// Read one chunk of samples from the stream into the provided buffer.
fn read_chunk(stream: &mut RxStream<Complex<f32>>, buffer: &mut [Complex<f32>]) -> ReadResult {
    match stream.read(&mut [buffer], READ_TIMEOUT_US) {
        Ok(count) => ReadResult::Data(count),
        Err(e) if e.code == ErrorCode::Timeout => ReadResult::Retry,
        Err(e) if e.code == ErrorCode::Overflow => {
            // During settling, overflows are expected and harmless.
            tracing::debug!("SoapySDR overflow during settling (harmless)");
            ReadResult::Retry
        }
        Err(e) => {
            tracing::warn!(
                error = %e.message,
                code = ?e.code,
                "SoapySDR stream read error"
            );
            ReadResult::Fatal
        }
    }
}

/// Reader thread main loop. Reads MTU-sized chunks and sends them through
/// the channel until `running` is false or a fatal error occurs.
fn reader_loop(
    mut stream: RxStream<Complex<f32>>,
    mtu: usize,
    sender: SyncSender<Result<Vec<Complex<f32>>, SdrError>>,
    running: Arc<AtomicBool>,
    chunk_count: Arc<AtomicU64>,
    overflow_count: Arc<AtomicU64>,
) {
    let mut first_read = true;
    let mut consecutive_errors: u32 = 0;

    while running.load(Ordering::Relaxed) {
        let mut buffer = vec![Complex::new(0.0, 0.0); mtu];

        match stream.read(&mut [&mut buffer], READ_TIMEOUT_US) {
            Ok(count) => {
                consecutive_errors = 0;
                chunk_count.fetch_add(1, Ordering::Relaxed);

                if first_read {
                    first_read = false;
                    log_first_samples(&buffer, count);
                }

                buffer.truncate(count);
                match sender.try_send(Ok(buffer)) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        let n = overflow_count.fetch_add(1, Ordering::Relaxed) + 1;
                        tracing::warn!(
                            overflow_count = n,
                            "channel full: dropping samples to keep reader alive"
                        );
                    }
                    Err(TrySendError::Disconnected(_)) => break,
                }
            }
            Err(e) if e.code == ErrorCode::Timeout => {
                // No data within timeout window — just retry.
                continue;
            }
            Err(e) if e.code == ErrorCode::Overflow => {
                let n = overflow_count.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::warn!(
                    overflow_count = n,
                    "SoapySDR overflow: samples lost, symbol timing may be corrupted"
                );
                // Overflow is recoverable — the driver resets its buffer.
                consecutive_errors = 0;
            }
            Err(e) => {
                consecutive_errors += 1;
                tracing::warn!(
                    error = %e.message,
                    code = ?e.code,
                    consecutive_errors,
                    "SoapySDR stream read error (retrying)"
                );
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    tracing::error!(
                        "SoapySDR stream failed after {consecutive_errors} consecutive errors, giving up"
                    );
                    // Send the error through the channel so the consumer
                    // can distinguish device failure from normal EOF.
                    let _ = sender.send(Err(SdrError::StreamRead(format!(
                        "{} after {} consecutive errors",
                        e.message, consecutive_errors
                    ))));
                    break;
                }
            }
        }
    }

    // Deactivate the stream before exiting.
    if let Err(e) = stream.deactivate(None) {
        tracing::warn!(error = %e.message, "failed to deactivate SoapySDR stream");
    }

    let final_overflows = overflow_count.load(Ordering::Relaxed);
    if final_overflows > 0 {
        tracing::info!(overflow_count = final_overflows, "reader thread exiting");
    }
}

/// Open a SoapySDR device with the given argument string.
fn open_device(device_args: &str) -> Result<Device, SdrError> {
    Device::new(device_args).map_err(|e| SdrError::DeviceOpen {
        args: device_args.to_string(),
        detail: e.message,
    })
}

/// Apply device-specific settings via `write_setting()`, returning an error on failure.
///
/// These correspond to the "Other Settings" shown by `SoapySDRUtil --probe`,
/// such as `rfgain_sel`, `biasT_ctrl`, `hdr_ctrl`, etc.
fn apply_settings(device: &Device, settings: &[(String, String)]) -> Result<(), SdrError> {
    for (key, value) in settings {
        device
            .write_setting(key.as_str(), value.as_str())
            .map_err(|e| SdrError::Configure {
                parameter: format!("setting {key}={value}"),
                detail: e.message,
            })?;
    }
    Ok(())
}

/// Log the actual device state after all configuration is applied.
fn log_device_state(device: &Device, requested_antenna: Option<&str>) {
    let actual_freq = device
        .frequency(Direction::Rx, RX_CHANNEL)
        .map(|f| format!("{f}"))
        .unwrap_or_else(|_| "?".into());
    let actual_rate = device
        .sample_rate(Direction::Rx, RX_CHANNEL)
        .map(|r| format!("{r}"))
        .unwrap_or_else(|_| "?".into());
    let actual_gain = device
        .gain(Direction::Rx, RX_CHANNEL)
        .map(|g| format!("{g}"))
        .unwrap_or_else(|_| "?".into());
    let agc = device
        .gain_mode(Direction::Rx, RX_CHANNEL)
        .map(|m| if m { "on" } else { "off" })
        .unwrap_or("?");
    let actual_antenna = device
        .antenna(Direction::Rx, RX_CHANNEL)
        .unwrap_or_else(|_| "?".into());

    tracing::debug!(
        actual_freq,
        actual_rate,
        actual_gain,
        agc,
        actual_antenna,
        ?requested_antenna,
        "device state after configuration"
    );

    // Log available stream arguments (buffer sizing, etc.)
    if let Ok(args) = device.stream_args_info(Direction::Rx, RX_CHANNEL) {
        for arg in &args {
            tracing::debug!(
                key = %arg.key,
                name = arg.name.as_deref().unwrap_or(""),
                description = arg.description.as_deref().unwrap_or(""),
                value = %arg.value,
                "stream argument"
            );
        }
    }

    // Log available sample rate range
    if let Ok(rates) = device.get_sample_rate_range(Direction::Rx, RX_CHANNEL) {
        for range in &rates {
            tracing::debug!(
                min = range.minimum,
                max = range.maximum,
                "supported sample rate range"
            );
        }
    }

    // Log stream format negotiation
    if let Ok((native_fmt, fullscale)) = device.native_stream_format(Direction::Rx, RX_CHANNEL) {
        let available = device
            .stream_formats(Direction::Rx, RX_CHANNEL)
            .unwrap_or_default();
        tracing::debug!(
            requested = "CF32",
            native = %native_fmt,
            fullscale,
            available = ?available,
            "stream format"
        );
    }

    // Log each setting readback if available
    for key in &[
        "rfgain_sel",
        "biasT_ctrl",
        "hdr_ctrl",
        "rfnotch_ctrl",
        "dabnotch_ctrl",
        "agc_setpoint",
    ] {
        if let Ok(val) = device.read_setting(*key) {
            tracing::debug!(key, value = %val, "device setting");
        }
    }
}

/// Log diagnostic info about the first buffer of samples received.
fn log_first_samples(buffer: &[Complex<f32>], count: usize) {
    let samples = &buffer[..count.min(buffer.len())];
    let n = samples.len();
    if n == 0 {
        tracing::debug!("first buffer: empty");
        return;
    }
    let mut mag_sum = 0.0f64;
    let mut mag_max = 0.0f32;
    let mut zero_count = 0usize;
    for s in samples {
        let mag = (s.re * s.re + s.im * s.im).sqrt();
        mag_sum += mag as f64;
        if mag > mag_max {
            mag_max = mag;
        }
        if s.re == 0.0 && s.im == 0.0 {
            zero_count += 1;
        }
    }
    let mag_avg = mag_sum / n as f64;
    tracing::debug!(
        count = n,
        mag_avg = format!("{mag_avg:.6}"),
        mag_max = format!("{mag_max:.6}"),
        zero_count,
        first_re = format!("{:.6}", samples[0].re),
        first_im = format!("{:.6}", samples[0].im),
        "first buffer received"
    );
}

/// Select the antenna on the device's RX channel.
fn configure_antenna(device: &Device, antenna: &str) -> Result<(), SdrError> {
    device
        .set_antenna(Direction::Rx, RX_CHANNEL, antenna)
        .map_err(|e| SdrError::Configure {
            parameter: "antenna".to_string(),
            detail: e.message,
        })
}

/// Set the center frequency on the device's RX channel.
fn configure_frequency(device: &Device, frequency_hz: u64) -> Result<(), SdrError> {
    device
        .set_frequency(Direction::Rx, RX_CHANNEL, frequency_hz as f64, ())
        .map_err(|e| SdrError::Configure {
            parameter: "center_frequency".to_string(),
            detail: e.message,
        })
}

/// Set the sample rate on the device's RX channel.
///
/// Warns if the actual rate differs from the requested rate.
fn configure_sample_rate(device: &Device, sample_rate_hz: u32) -> Result<(), SdrError> {
    device
        .set_sample_rate(Direction::Rx, RX_CHANNEL, sample_rate_hz as f64)
        .map_err(|e| SdrError::Configure {
            parameter: "sample_rate".to_string(),
            detail: e.message,
        })?;

    let actual =
        device
            .sample_rate(Direction::Rx, RX_CHANNEL)
            .map_err(|e| SdrError::Configure {
                parameter: "sample_rate".to_string(),
                detail: e.message,
            })?;

    if (actual - sample_rate_hz as f64).abs() > 1.0 {
        tracing::warn!(
            requested = sample_rate_hz,
            actual = actual,
            "sample rate differs from requested value"
        );
    }
    Ok(())
}

/// Configure gain: manual (specific dB) or AGC.
fn configure_gain(device: &Device, gain: Option<f64>) -> Result<(), SdrError> {
    match gain {
        Some(gain_db) => {
            device
                .set_gain_mode(Direction::Rx, RX_CHANNEL, false)
                .map_err(|e| SdrError::Configure {
                    parameter: "gain_mode".to_string(),
                    detail: e.message,
                })?;
            device
                .set_gain(Direction::Rx, RX_CHANNEL, gain_db)
                .map_err(|e| SdrError::Configure {
                    parameter: "gain".to_string(),
                    detail: e.message,
                })?;
        }
        None => {
            device
                .set_gain_mode(Direction::Rx, RX_CHANNEL, true)
                .map_err(|e| SdrError::Configure {
                    parameter: "gain_mode".to_string(),
                    detail: e.message,
                })?;
        }
    }
    Ok(())
}

/// Create an RX stream for Complex<f32> samples on channel 0.
fn create_stream(device: &Device) -> Result<RxStream<Complex<f32>>, SdrError> {
    device
        .rx_stream::<Complex<f32>>(&[RX_CHANNEL])
        .map_err(|e| SdrError::StreamCreate(e.message))
}

/// Temporarily redirect stderr to `/dev/null`, run the closure, then restore it.
///
/// SoapySDR and hardware drivers (RTL-SDR, etc.) print informational messages
/// directly to stderr via C `fprintf`. When `p25 cc` is piped to `p25 monitor`,
/// these writes share the terminal and corrupt the TUI alternate screen.
fn suppress_stderr<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
    if saved >= 0 {
        let devnull = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY) };
        if devnull >= 0 {
            unsafe { libc::dup2(devnull, libc::STDERR_FILENO) };
            unsafe { libc::close(devnull) };
        }
    }

    let result = f();

    if saved >= 0 {
        unsafe { libc::dup2(saved, libc::STDERR_FILENO) };
        unsafe { libc::close(saved) };
    }

    result
}

/// List all SoapySDR devices visible on the system.
///
/// Prints device properties to stdout. If no devices are found,
/// prints a troubleshooting message to stderr.
pub fn list_devices() {
    match soapysdr::enumerate("") {
        Ok(devices) if devices.is_empty() => {
            eprintln!("No SoapySDR devices found.");
            eprintln!();
            eprintln!("Troubleshooting:");
            eprintln!("  - Is the SDR device plugged in?");
            eprintln!("  - Is the SoapySDR module installed? (e.g. soapysdr-module-rtlsdr)");
            eprintln!("  - Try: SoapySDRUtil --find");
        }
        Ok(devices) => {
            println!("Found {} SoapySDR device(s):", devices.len());
            for (index, device) in devices.iter().enumerate() {
                println!("  [{index}] {device}");
            }
        }
        Err(e) => {
            eprintln!("Error enumerating SoapySDR devices: {}", e.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a SoapySource backed by a pre-built channel for testing
    /// error propagation without real hardware.
    fn source_from_receiver(
        receiver: Receiver<Result<Vec<Complex<f32>>, SdrError>>,
    ) -> SoapySource {
        SoapySource {
            receiver: Some(receiver),
            current_chunk: Vec::new(),
            position: 0,
            reader_thread: None,
            running: Arc::new(AtomicBool::new(true)),
            chunk_count: Arc::new(AtomicU64::new(0)),
            overflow_count: Arc::new(AtomicU64::new(0)),
            device_error: None,
        }
    }

    #[test]
    fn open_nonexistent_device_returns_error() {
        let running = Arc::new(AtomicBool::new(true));
        let result = SoapySource::open(
            "driver=nonexistent_xyz_12345",
            852_350_000,
            2_400_000,
            None,
            None,
            &[],
            250, // buffer_ms
            running,
        );
        match result {
            Err(err) => {
                let msg = format!("{err}");
                assert!(msg.contains("nonexistent_xyz_12345"));
            }
            Ok(_) => panic!("expected error for nonexistent device"),
        }
    }

    #[test]
    fn iterator_yields_samples_from_channel() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let mut source = source_from_receiver(receiver);

        sender
            .send(Ok(vec![Complex::new(1.0, 2.0), Complex::new(3.0, 4.0)]))
            .unwrap();
        drop(sender);

        assert_eq!(source.next(), Some(Complex::new(1.0, 2.0)));
        assert_eq!(source.next(), Some(Complex::new(3.0, 4.0)));
        assert_eq!(source.next(), None);
        assert!(
            source.device_error().is_none(),
            "normal EOF should not set device_error"
        );
    }

    #[test]
    fn error_from_channel_sets_device_error() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let mut source = source_from_receiver(receiver);

        // Send one good chunk, then a fatal error.
        sender.send(Ok(vec![Complex::new(1.0, 0.0)])).unwrap();
        sender
            .send(Err(SdrError::StreamRead(
                "USB transfer failed after 50 consecutive errors".to_string(),
            )))
            .unwrap();
        drop(sender);

        // First sample succeeds.
        assert_eq!(source.next(), Some(Complex::new(1.0, 0.0)));
        // Next call hits the error.
        assert_eq!(source.next(), None);

        let err = source
            .device_error()
            .expect("device_error should be set after stream error");
        let msg = format!("{err}");
        assert!(
            msg.contains("USB transfer failed"),
            "error message should include description: {msg}"
        );
        assert!(
            msg.contains("50 consecutive errors"),
            "error message should include consecutive error count: {msg}"
        );
    }

    #[test]
    fn device_error_is_none_when_channel_closes_normally() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let mut source = source_from_receiver(receiver);

        // Drop sender without sending an error (normal shutdown).
        drop(sender);

        assert_eq!(source.next(), None);
        assert!(
            source.device_error().is_none(),
            "normal channel close should not set device_error"
        );
    }

    #[test]
    fn device_error_is_none_before_iteration() {
        let (_sender, receiver) = mpsc::sync_channel(4);
        let source = source_from_receiver(receiver);

        assert!(
            source.device_error().is_none(),
            "device_error should be None before iteration starts"
        );
    }
}

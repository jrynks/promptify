use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use promptify_core::audio::{CaptureBuffer, CapturedAudio};

const NO_INPUT: &str = "No microphone found. Connect one or set a default input device in system settings.";

pub fn default_input_name() -> Option<String> {
    let device = cpal::default_host().default_input_device()?;
    Some(device_name(&device))
}

pub fn default_input_id() -> Result<String, String> {
    let device = cpal::default_host().default_input_device().ok_or(NO_INPUT)?;
    device.id().map(|id| id.to_string()).map_err(|e| format!("Could not identify the default microphone: {e}"))
}

fn device_name(device: &cpal::Device) -> String {
    match device.description() {
        Ok(description) if !description.name().is_empty() => description.name().to_owned(),
        Ok(_) => {
            log::warn!("the default microphone has an empty device name");
            "System default microphone (name unavailable)".into()
        }
        Err(error) => {
            log::warn!("could not read the default microphone name: {error}");
            "System default microphone (name unavailable)".into()
        }
    }
}

/// Sent once by the capture thread: device ID, display name and buffer, or why it failed.
type Ready = Result<(Option<String>, String, Arc<Mutex<CaptureBuffer>>), String>;

/// An in-progress capture from the system default microphone.
pub struct Recording {
    pub device_id: Option<String>,
    stop_tx: mpsc::Sender<()>,
    thread: JoinHandle<Result<CapturedAudio, String>>,
    buffer: Arc<Mutex<CaptureBuffer>>,
}

/// Read access to a recording's audio while it is still being captured.
#[derive(Clone)]
pub struct LiveAudio(Arc<Mutex<CaptureBuffer>>);

impl LiveAudio {
    /// Copies the 16 kHz samples from `start` to the current end.
    pub fn copy_from(&self, start: usize) -> Vec<f32> {
        self.0.lock().unwrap().samples().get(start..).map(<[f32]>::to_vec).unwrap_or_default()
    }
}

impl Recording {
    /// Opens whatever the OS default input device is right now, so default-device changes apply
    /// to the next recording.
    pub fn start(max_samples: usize, on_level: impl Fn(f32) + Send + 'static) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::channel::<Ready>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        // cpal streams are not Send on every platform, so the stream lives on its own thread.
        let thread = std::thread::Builder::new()
            .name("mic-capture".into())
            .spawn(move || capture_thread(max_samples, on_level, ready_tx, stop_rx))
            .map_err(|e| format!("could not start capture thread: {e}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok((device_id, name, buffer))) => {
                log::info!("recording from default input device {name:?}");
                Ok(Self { device_id, stop_tx, thread, buffer })
            }
            Ok(Err(err)) => Err(err),
            Err(_) => {
                let _ = stop_tx.send(());
                Err("the microphone did not start in time".into())
            }
        }
    }

    pub fn live(&self) -> LiveAudio {
        LiveAudio(self.buffer.clone())
    }

    pub fn stop(self) -> Result<CapturedAudio, String> {
        let _ = self.stop_tx.send(());
        self.thread.join().map_err(|_| "capture thread panicked".to_string())?
    }
}

fn capture_thread(
    max_samples: usize,
    on_level: impl Fn(f32) + Send + 'static,
    ready_tx: mpsc::Sender<Ready>,
    stop_rx: mpsc::Receiver<()>,
) -> Result<CapturedAudio, String> {
    let fail = |msg: String| {
        let _ = ready_tx.send(Err(msg.clone()));
        Err(msg)
    };
    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        return fail(NO_INPUT.into());
    };
    let id = match device.id() {
        Ok(id) => Some(id.to_string()),
        Err(error) => {
            log::warn!("could not identify the recording microphone: {error}");
            None
        }
    };
    let name = device_name(&device);
    let supported = match device.default_input_config() {
        Ok(config) => config,
        Err(e) => return fail(format!("The default microphone is unavailable: {e}")),
    };
    let config = supported.config();
    let buffer = Arc::new(Mutex::new(CaptureBuffer::new(config.channels, config.sample_rate, max_samples)));
    let stream_error: Arc<Mutex<Option<String>>> = Arc::default();
    let (b, e) = (buffer.clone(), stream_error.clone());

    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config, b, e, on_level),
        SampleFormat::I16 => build::<i16>(&device, config, b, e, on_level),
        SampleFormat::U16 => build::<u16>(&device, config, b, e, on_level),
        SampleFormat::I32 => build::<i32>(&device, config, b, e, on_level),
        SampleFormat::U8 => build::<u8>(&device, config, b, e, on_level),
        SampleFormat::F64 => build::<f64>(&device, config, b, e, on_level),
        other => return fail(format!("Unsupported microphone sample format {other:?}")),
    };
    let stream = match stream {
        Ok(stream) => stream,
        Err(e) => return fail(format!("Could not open the microphone: {e}")),
    };
    if let Err(e) = stream.play() {
        return fail(format!("Could not start the microphone: {e}"));
    }
    let _ = ready_tx.send(Ok((id, name, buffer.clone())));

    let _ = stop_rx.recv();
    drop(stream);

    if let Some(err) = stream_error.lock().unwrap().take() {
        return Err(format!("The microphone stopped: {err}"));
    }
    Ok(buffer.lock().unwrap().take())
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    buffer: Arc<Mutex<CaptureBuffer>>,
    stream_error: Arc<Mutex<Option<String>>>,
    on_level: impl Fn(f32) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut scratch: Vec<f32> = Vec::new();
    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _| {
            scratch.clear();
            scratch.extend(data.iter().map(|s| s.to_sample::<f32>()));
            let level = buffer.lock().unwrap().push_interleaved(&scratch);
            on_level(level);
        },
        move |err| {
            log::warn!("microphone stream error: {err}");
            // Glitches and reroutes to the new default device keep the stream delivering audio.
            if matches!(err.kind(), cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied) {
                return;
            }
            stream_error.lock().unwrap().get_or_insert_with(|| err.to_string());
        },
        None,
    )
}

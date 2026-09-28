use crate::managers::history::HistoryManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{get_settings, write_settings, ModelUnloadTimeout};
use log::{debug, error};
use serde::Serialize;
use specta::Type;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

#[derive(Serialize, Type)]
pub struct ModelLoadStatus {
    is_loaded: bool,
    current_model: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub fn set_model_unload_timeout(app: AppHandle, timeout: ModelUnloadTimeout) {
    let mut settings = get_settings(&app);
    settings.model_unload_timeout = timeout;
    write_settings(&app, settings);
}

#[tauri::command]
#[specta::specta]
pub fn get_model_load_status(
    transcription_manager: State<TranscriptionManager>,
) -> Result<ModelLoadStatus, String> {
    Ok(ModelLoadStatus {
        is_loaded: transcription_manager.is_model_loaded(),
        current_model: transcription_manager.get_current_model(),
    })
}

#[tauri::command]
#[specta::specta]
pub fn unload_model_manually(
    transcription_manager: State<TranscriptionManager>,
) -> Result<(), String> {
    transcription_manager
        .unload_model()
        .map_err(|e| format!("Failed to unload model: {}", e))
}

/// Transcribe an audio file and paste the result into the active application.
/// Returns the transcribed text.
#[tauri::command]
#[specta::specta]
pub async fn transcribe_audio_file(
    app: AppHandle,
    file_path: String,
) -> Result<String, String> {
    debug!("transcribe_audio_file: {}", file_path);

    // Decode audio on a blocking thread (CPU-intensive, must not block Tokio).
    let samples = tokio::task::spawn_blocking(move || load_and_resample_to_16k(&file_path))
        .await
        .map_err(|e| format!("Audio decode task panicked: {}", e))?
        .map_err(|e| format!("Failed to load audio: {}", e))?;

    let tm = Arc::clone(&app.state::<Arc<TranscriptionManager>>());
    let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());
    // Ensure the model is loading (no-op if already loaded/loading).
    tm.initiate_model_load();

    // Whisper has a 30-second context window. Split longer audio into chunks
    // so we never hand the model more than it can handle at once.
    const CHUNK_SAMPLES: usize = 30 * 16_000; // 30 s at 16 kHz

    let samples_for_wav = samples.clone();
    let (text, file_name) = tokio::task::spawn_blocking(move || -> Result<(String, String), String> {
        // Transcribe in chunks. Re-initiate model load before each chunk so
        // "Unload Immediately" mode (which unloads after every transcribe()
        // call) doesn't leave the engine empty for subsequent chunks.
        let mut parts = Vec::new();
        for chunk in samples.chunks(CHUNK_SAMPLES) {
            tm.initiate_model_load();
            match tm.transcribe(chunk.to_vec()) {
                Ok(part) => {
                    let trimmed = part.trim().to_string();
                    if !trimmed.is_empty() {
                        parts.push(trimmed);
                    }
                }
                Err(e) => {
                    error!("transcribe_audio_file: chunk transcription failed, skipping: {}", e);
                }
            }
        }
        let text = parts.join(" ");

        // Save the resampled audio as WAV in the recordings directory so the
        // history entry can reference it (replay / retry). The history entry
        // is saved regardless of whether the WAV write succeeded.
        let file_name = format!("handy-import-{}.wav", chrono::Utc::now().timestamp());
        let wav_path = hm.recordings_dir().join(&file_name);
        if let Err(e) = crate::audio_toolkit::save_wav_file(&wav_path, &samples_for_wav) {
            error!("transcribe_audio_file: failed to save WAV to history: {}", e);
        }
        if let Err(e) = hm.save_entry(file_name.clone(), text.clone(), false, None, None) {
            error!("transcribe_audio_file: failed to save history entry: {}", e);
        }

        Ok((text, file_name))
    })
    .await
    .map_err(|e| format!("Transcription task panicked: {}", e))?
    .map_err(|e| format!("Transcription failed: {}", e))?;

    let _ = file_name; // WAV path used inside the blocking task above.
    let trimmed = text.trim().to_string();
    if !trimmed.is_empty() {
        let app_paste = app.clone();
        let text_paste = trimmed.clone();
        let _ = app.run_on_main_thread(move || {
            use tauri_plugin_clipboard_manager::ClipboardExt;
            let _ = app_paste.clipboard().write_text(&text_paste);
            let _ = crate::utils::paste(text_paste, app_paste);
        });
    }

    Ok(trimmed)
}

/// Decode any audio file supported by symphonia and resample to 16 kHz mono f32.
fn load_and_resample_to_16k(path: &str) -> anyhow::Result<Vec<f32>> {
    use rubato::{FftFixedIn, Resampler as _};
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow::anyhow!("No audio track found"))?;

    let track_id = track.id;
    // Prefer codec_params but fall back to the first decoded frame's spec,
    // because some M4A/AAC files don't populate these fields in the container headers.
    let hint_channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(0);
    let hint_rate = track.codec_params.sample_rate.unwrap_or(0) as usize;

    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut raw: Vec<f32> = Vec::new();
    let mut actual_channels = hint_channels;
    let mut actual_rate = hint_rate;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                // Confirm sample rate and channel count from the first real frame.
                if actual_channels == 0 {
                    actual_channels = decoded.spec().channels.count();
                }
                if actual_rate == 0 {
                    actual_rate = decoded.spec().rate as usize;
                }
                let mut buf =
                    SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
                buf.copy_interleaved_ref(decoded);
                raw.extend_from_slice(buf.samples());
            }
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }

    let channels = actual_channels.max(1);
    let orig_rate = if actual_rate > 0 { actual_rate } else { 44100 };

    // Downmix to mono.
    let mono: Vec<f32> = if channels == 1 {
        raw
    } else {
        raw.chunks_exact(channels)
            .map(|ch| ch.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    const TARGET_HZ: usize = 16_000;
    if orig_rate == TARGET_HZ {
        return Ok(mono);
    }

    // Resample using rubato FFT resampler with proper flushing and delay trimming.
    // Mirrors the algorithm used by FrameResampler::finish() to avoid zero-padding
    // artifacts and recover all audio held in the filter's delay line.
    const CHUNK: usize = 1024;
    let mut resampler = FftFixedIn::<f32>::new(orig_rate, TARGET_HZ, CHUNK, 1, 1)?;
    let mut out = Vec::with_capacity(mono.len() * TARGET_HZ / orig_rate + CHUNK);
    let mut in_count = 0usize;

    let mut pos = 0;
    while pos < mono.len() {
        let end = (pos + CHUNK).min(mono.len());
        let chunk = &mono[pos..end];
        pos = end;
        in_count += chunk.len();

        if chunk.len() == CHUNK {
            if let Ok(result) = resampler.process(&[chunk], None) {
                out.extend_from_slice(&result[0]);
            }
        } else {
            // Partial last chunk: process_partial handles its own internal padding
            // so we avoid feeding zeros through the FFT (Gibbs ringing).
            if let Ok(result) = resampler.process_partial(Some(&[chunk]), None) {
                out.extend_from_slice(&result[0]);
            }
        }
    }

    // Flush the resampler's internal delay line to recover all audio.
    // Output lags input by output_delay() samples; we need in*ratio + delay
    // total output frames before all real audio has emerged.
    let delay = resampler.output_delay();
    let expected = in_count * TARGET_HZ / orig_rate + delay;
    let mut rounds = 0;
    while out.len() < expected && rounds < 8 {
        rounds += 1;
        match resampler.process_partial::<&[f32]>(None, None) {
            Ok(flushed) => {
                let take = (expected - out.len()).min(flushed[0].len());
                out.extend_from_slice(&flushed[0][..take]);
            }
            Err(_) => break,
        }
    }

    // Trim the filter-startup (group-delay) samples from the beginning so
    // the output is time-aligned with the original audio.
    if delay > 0 && out.len() > delay {
        out.drain(..delay);
    }

    debug!(
        "Audio loaded: {}Hz {}ch → {} samples at 16kHz (delay={} trimmed)",
        orig_rate,
        channels,
        out.len(),
        delay
    );
    Ok(out)
}

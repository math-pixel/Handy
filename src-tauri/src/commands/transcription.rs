use crate::managers::transcription::TranscriptionManager;
use crate::settings::{get_settings, write_settings, ModelUnloadTimeout};
use log::debug;
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
    // Ensure the model is loading (no-op if already loaded/loading).
    tm.initiate_model_load();

    // Transcription blocks waiting for model load then runs inference — use blocking thread.
    let text = tokio::task::spawn_blocking(move || tm.transcribe(samples))
        .await
        .map_err(|e| format!("Transcription task panicked: {}", e))?
        .map_err(|e| format!("Transcription failed: {}", e))?;

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
    use rubato::{FftFixedIn, Resampler};
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
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(1);
    let orig_rate = track.codec_params.sample_rate.unwrap_or(44100) as usize;

    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut raw: Vec<f32> = Vec::new();
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

    // Resample using rubato FFT resampler.
    const CHUNK: usize = 1024;
    let mut resampler = FftFixedIn::<f32>::new(orig_rate, TARGET_HZ, CHUNK, 1, 1)?;
    let mut out = Vec::with_capacity(mono.len() * TARGET_HZ / orig_rate + CHUNK);
    let mut buf = vec![0.0_f32; CHUNK];

    let mut pos = 0;
    while pos < mono.len() {
        let end = (pos + CHUNK).min(mono.len());
        let len = end - pos;
        buf[..len].copy_from_slice(&mono[pos..end]);
        if len < CHUNK {
            buf[len..].fill(0.0);
        }
        pos += CHUNK;
        if let Ok(result) = resampler.process(&[&buf], None) {
            out.extend_from_slice(&result[0]);
        }
    }

    debug!(
        "Audio loaded: {}Hz {}ch → {} samples at 16kHz",
        orig_rate,
        channels,
        out.len()
    );
    Ok(out)
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use crate::managers::transcription::TranscriptionManager;

pub struct StreamingTranscriber {
    chunk_tx: mpsc::SyncSender<Vec<f32>>,
    stop_flag: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl StreamingTranscriber {
    /// `on_partial(text)` is called on each new partial result and is responsible
    /// for pasting the text into the active application (runs `run_on_main_thread`
    /// internally). It is invoked from a background transcription thread.
    pub fn new(
        tm: Arc<TranscriptionManager>,
        on_partial: Arc<dyn Fn(String) + Send + Sync + 'static>,
    ) -> Self {
        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<Vec<f32>>(64);
        let stop_flag = Arc::new(AtomicBool::new(false));

        let thread = {
            let stop_flag = stop_flag.clone();
            std::thread::spawn(move || run_streaming_loop(chunk_rx, stop_flag, tm, on_partial))
        };

        Self {
            chunk_tx,
            stop_flag,
            thread: Some(thread),
        }
    }

    /// Returns a closure that forwards speech chunks to the streaming loop.
    pub fn make_chunk_sender(&self) -> impl Fn(Vec<f32>) + Send + Sync + 'static {
        let tx = self.chunk_tx.clone();
        move |samples: Vec<f32>| {
            let _ = tx.try_send(samples);
        }
    }

    /// Signal the streaming loop to stop. Non-blocking — exits within ~250 ms.
    pub fn stop(self) {
        let Self {
            chunk_tx,
            stop_flag,
            thread,
        } = self;
        stop_flag.store(true, Ordering::Relaxed);
        drop(chunk_tx); // disconnect the channel so recv_timeout returns immediately
        drop(thread); // detach; the thread exits on its own
    }
}

/// Minimum new speech samples before triggering a partial transcription (2 s at 16 kHz).
const MIN_NEW_SAMPLES: usize = 16_000 * 2;

fn run_streaming_loop(
    chunk_rx: mpsc::Receiver<Vec<f32>>,
    stop_flag: Arc<AtomicBool>,
    tm: Arc<TranscriptionManager>,
    on_partial: Arc<dyn Fn(String) + Send + Sync + 'static>,
) {
    let mut accumulated: Vec<f32> = Vec::new();
    let mut last_transcribed_len: usize = 0;
    let is_transcribing = Arc::new(AtomicBool::new(false));

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        match chunk_rx.recv_timeout(Duration::from_millis(250)) {
            Ok(samples) => {
                accumulated.extend_from_slice(&samples);
                while let Ok(more) = chunk_rx.try_recv() {
                    accumulated.extend_from_slice(&more);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        let new_since_last = accumulated.len().saturating_sub(last_transcribed_len);
        let enough_total = accumulated.len() >= 16_000; // at least 1 s

        if new_since_last >= MIN_NEW_SAMPLES
            && enough_total
            && !is_transcribing.load(Ordering::Relaxed)
        {
            last_transcribed_len = accumulated.len();
            let snapshot = accumulated.clone();
            let tm_clone = Arc::clone(&tm);
            let on_partial_clone = on_partial.clone();
            let flag = is_transcribing.clone();

            flag.store(true, Ordering::Relaxed);
            std::thread::spawn(move || {
                match tm_clone.transcribe(snapshot) {
                    Ok(text) => {
                        let trimmed = text.trim().to_string();
                        if !trimmed.is_empty() {
                            on_partial_clone(trimmed);
                        }
                    }
                    Err(e) => {
                        log::debug!("Partial transcription skipped: {}", e);
                    }
                }
                flag.store(false, Ordering::Relaxed);
            });
        }
    }
}


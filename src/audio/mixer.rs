// ABOUTME: Real-time audio mixer performing sample mixing and channel routing.
// ABOUTME: Runs in audio callback thread with strict real-time constraints.

use crate::audio::bass_management::BassManagement;
use crate::audio::ducking::DuckingApplier;
use crate::audio::pitch_correction::PitchCorrector;
use crate::audio::streaming::SampleBuffer;
use crate::audio::types::DecodedBuffer;
use crate::config::{DEFAULT_MASTER_GAIN, DEFAULT_OUTPUT_CEILING_DB};
use ringbuf::HeapConsumer;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// Fade state for audio samples
#[derive(Debug, Clone, PartialEq)]
pub enum FadeState {
    /// No fading
    None,
    /// Fading in: (frames_elapsed, total_fade_frames)
    In { elapsed: usize, duration: usize },
    /// Fading out: (frames_elapsed, total_fade_frames)
    Out { elapsed: usize, duration: usize },
}

impl FadeState {
    /// Create a new fade in state
    pub fn fade_in(duration_ms: u32, sample_rate: u32) -> Self {
        let duration_frames = ((duration_ms as f32 / 1000.0) * sample_rate as f32) as usize;
        FadeState::In {
            elapsed: 0,
            duration: duration_frames,
        }
    }

    /// Create a new fade out state
    pub fn fade_out(duration_ms: u32, sample_rate: u32) -> Self {
        let duration_frames = ((duration_ms as f32 / 1000.0) * sample_rate as f32) as usize;
        FadeState::Out {
            elapsed: 0,
            duration: duration_frames,
        }
    }

    /// Calculate fade multiplier for the current position
    /// Returns a value between 0.0 and 1.0
    ///
    /// The fade ramp is linear. Unlike the loop crossfade (which sums two
    /// uncorrelated signals and so uses an equal-power curve to avoid a midpoint
    /// dip, D28), a fade is a single signal scaled toward or from silence, where
    /// a linear ramp over the typically short fade durations is adequate.
    pub fn multiplier(&self) -> f32 {
        match self {
            FadeState::None => 1.0,
            FadeState::In { elapsed, duration } => {
                if *duration == 0 {
                    return 1.0;
                }
                (*elapsed as f32 / *duration as f32).min(1.0)
            }
            FadeState::Out { elapsed, duration } => {
                if *duration == 0 {
                    return 0.0;
                }
                (1.0 - (*elapsed as f32 / *duration as f32)).max(0.0)
            }
        }
    }

    /// Advance fade state by one frame
    pub fn advance(&mut self) {
        match self {
            FadeState::In { elapsed, duration } => {
                if *elapsed < *duration {
                    *elapsed += 1;
                }
            }
            FadeState::Out { elapsed, duration } => {
                if *elapsed < *duration {
                    *elapsed += 1;
                }
            }
            FadeState::None => {}
        }
    }

    /// Check if fade is complete
    #[cfg(test)]
    pub fn is_complete(&self) -> bool {
        match self {
            FadeState::None => true,
            FadeState::In { elapsed, duration } => elapsed >= duration,
            FadeState::Out { elapsed, duration } => elapsed >= duration,
        }
    }
}

/// Active sample being played
pub struct ActiveSample {
    /// Unique internal sample ID (assigned by VoiceManager)
    pub id: u64,

    /// User-provided sample identifier for targeting commands
    pub sample_id: Option<String>,

    /// Voice ID this sample belongs to
    pub voice_id: String,

    /// Source file path (for targeting by filename)
    pub file_path: String,

    /// Audio buffer - either complete (cached) or streaming (loading)
    pub buffer: SampleBuffer,

    /// Current playback position (in frames, integer part)
    pub position: usize,

    /// Fractional part of playback position for sub-sample interpolation
    fractional_position: f64,

    /// Per-sample volume (0.0 - 1.0)
    pub volume: f32,

    /// Voice-level volume (0.0 - 1.0) - current smoothed value
    pub voice_volume: f32,

    /// Target voice volume for smooth ramping (0.0 - 1.0)
    pub target_voice_volume: f32,

    /// Channel routing: vec![(src_channel, dest_channel), ...]
    pub channel_map: Vec<(usize, usize)>,

    /// Optional per-route gain parallel to `channel_map` (D29). When shorter than
    /// `channel_map` (including empty, the default) a missing route reads as unity,
    /// so absent gains leave existing 1:1/sum routing unchanged. Lets a downmix that
    /// sums several source channels into one destination be attenuated to avoid
    /// clipping.
    channel_route_gains: Vec<f32>,

    /// Fade state (in/out/none)
    pub fade_state: FadeState,

    /// Playback speed multiplier (1.0 = normal, 2.0 = double speed, 0.5 = half speed)
    pub speed: f32,

    /// Pitch corrector for time-stretching without pitch change
    pub pitch_corrector: Option<PitchCorrector>,

    /// Loop playback continuously
    pub loop_mode: bool,

    /// Number of samples to crossfade at loop boundaries (0 = disabled)
    pub crossfade_samples: usize,

    /// Reusable scratch buffer for the pitch-correction mix path, so the audio
    /// callback does not heap-allocate a stretcher output buffer every block.
    /// Grows to the largest block seen, then is reused.
    pitch_scratch: Vec<f32>,

    /// Fractional carry for the pitch-correction input advance. Each pitch block
    /// consumes `output_frames * speed` input frames; only the integer part can be
    /// fed and the position advanced by it, so the fraction is carried here and
    /// each input slice begins exactly where the previous one ended (F11).
    pitch_input_accumulator: f64,

    /// Remaining stretcher tail frames to emit at EOF, or `None` when not draining.
    /// Set when the pitch path exhausts the source so the buffered tail is flushed
    /// instead of being truncated (F12); while it is `Some(n > 0)` the sample is
    /// kept alive. The stretcher's `flush` must be drained in a single call, so the
    /// whole tail is captured into `pitch_tail_buffer` at EOF and then emitted from
    /// it block by block.
    pitch_tail_remaining: Option<usize>,

    /// Holds the stretcher's drained tail (interleaved, source channels), captured
    /// by a single `flush` at EOF and then played out across blocks (F12). Sized
    /// once when pitch correction is enabled, so the per-block path never allocates.
    pitch_tail_buffer: Vec<f32>,

    /// Read cursor (in frames) into `pitch_tail_buffer` while draining the tail.
    pitch_tail_pos: usize,

    /// Frames left in the direct->stretched crossfade after a mid-playback enable,
    /// and its total length. Even with a pre-rolled stretcher, the first synthesis
    /// block fades in from zero, which would step down from the full-level direct
    /// playback (a click). For these frames the direct signal is faded out (read at
    /// `pitch_crossfade_direct_pos`) while the stretched signal fades in, masking
    /// the transition (F10). Zero length means no crossfade is active.
    pitch_crossfade_remaining: usize,
    pitch_crossfade_total: usize,
    pitch_crossfade_direct_pos: f64,

    /// Optional live-position publisher (Sprint W6 telemetry, DW3/DW12). When set
    /// and telemetry is enabled, the callback stores `position` into this atomic
    /// once per block — a single relaxed store, no alloc/lock — so the control
    /// thread can read live playback position without touching the mixer (D22a).
    /// The atomic is constructed off-RT and moved in with the sample, never
    /// allocated on the audio thread.
    pub position_publisher: Option<Arc<AtomicUsize>>,

    /// When the control thread enqueued this sample's AddSample command (Sprint 11,
    /// D50). Paired with `first_mix_latency`; `None` for samples built outside the
    /// daemon's play path (tests, benches), which therefore publish nothing.
    enqueued_at: Option<std::time::Instant>,

    /// First-mix latency sink (Sprint 11, D50): on the first block in which this
    /// sample mixes loaded audio, the callback stores the nanoseconds elapsed since
    /// `enqueued_at` — a single relaxed store into a pre-allocated atomic, no
    /// alloc/lock, mirroring `position_publisher`.
    first_mix_latency: Option<Arc<AtomicU64>>,

    /// Set once the first-mix latency has been published, so later blocks take a
    /// single-bool fast path.
    first_mix_published: bool,
}

impl ActiveSample {
    /// Create a new active sample with default stereo mapping and no sample id,
    /// loop, or crossfade. The convenience constructor for tests and the offline
    /// render/alloc harnesses (`audio::test_support`, the integration suites); the
    /// running daemon builds samples via `new_with_id`/`new_with_mapping`, which
    /// carry the command's id, loop, and crossfade. Used by the library crate's
    /// harness but only by `#[cfg(test)]` code in the binary, so the binary build
    /// would otherwise flag it dead.
    #[allow(dead_code)]
    pub fn new(
        id: u64,
        voice_id: String,
        buffer: impl Into<SampleBuffer>,
        volume: f32,
        voice_volume: f32,
        file_path: String,
    ) -> Self {
        let buffer = buffer.into();
        // Default channel mapping: 1:1 for available channels
        let channel_map = (0..buffer.channels()).map(|ch| (ch, ch)).collect();

        Self {
            id,
            sample_id: None,
            voice_id,
            file_path,
            buffer,
            position: 0,
            fractional_position: 0.0,
            volume: volume.clamp(0.0, 1.0),
            voice_volume,
            target_voice_volume: voice_volume,
            channel_map,
            channel_route_gains: Vec::new(),
            fade_state: FadeState::None,
            speed: 1.0,
            pitch_corrector: None,
            loop_mode: false,
            crossfade_samples: 0,
            pitch_scratch: Vec::new(),
            pitch_input_accumulator: 0.0,
            pitch_tail_remaining: None,
            pitch_tail_buffer: Vec::new(),
            pitch_tail_pos: 0,
            pitch_crossfade_remaining: 0,
            pitch_crossfade_total: 0,
            pitch_crossfade_direct_pos: 0.0,
            position_publisher: None,
            enqueued_at: None,
            first_mix_latency: None,
            first_mix_published: false,
        }
    }

    /// Create a new active sample with user-provided sample ID
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_id(
        id: u64,
        voice_id: String,
        buffer: impl Into<SampleBuffer>,
        volume: f32,
        voice_volume: f32,
        file_path: String,
        sample_id: Option<String>,
        loop_mode: bool,
        crossfade_samples: usize,
    ) -> Self {
        let buffer = buffer.into();
        // Default channel mapping: 1:1 for available channels
        let channel_map = (0..buffer.channels()).map(|ch| (ch, ch)).collect();

        Self {
            id,
            sample_id,
            voice_id,
            file_path,
            buffer,
            position: 0,
            fractional_position: 0.0,
            volume: volume.clamp(0.0, 1.0),
            voice_volume,
            target_voice_volume: voice_volume,
            channel_map,
            channel_route_gains: Vec::new(),
            fade_state: FadeState::None,
            speed: 1.0,
            pitch_corrector: None,
            loop_mode,
            crossfade_samples,
            pitch_scratch: Vec::new(),
            pitch_input_accumulator: 0.0,
            pitch_tail_remaining: None,
            pitch_tail_buffer: Vec::new(),
            pitch_tail_pos: 0,
            pitch_crossfade_remaining: 0,
            pitch_crossfade_total: 0,
            pitch_crossfade_direct_pos: 0.0,
            position_publisher: None,
            enqueued_at: None,
            first_mix_latency: None,
            first_mix_published: false,
        }
    }

    /// Create a new active sample with custom channel mapping
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_mapping(
        id: u64,
        voice_id: String,
        buffer: impl Into<SampleBuffer>,
        volume: f32,
        voice_volume: f32,
        channel_map: Vec<(usize, usize)>,
        file_path: String,
        sample_id: Option<String>,
        loop_mode: bool,
        crossfade_samples: usize,
    ) -> Self {
        Self {
            id,
            sample_id,
            voice_id,
            file_path,
            buffer: buffer.into(),
            position: 0,
            fractional_position: 0.0,
            volume: volume.clamp(0.0, 1.0),
            voice_volume,
            target_voice_volume: voice_volume,
            channel_map,
            channel_route_gains: Vec::new(),
            fade_state: FadeState::None,
            speed: 1.0,
            pitch_corrector: None,
            loop_mode,
            crossfade_samples,
            pitch_scratch: Vec::new(),
            pitch_input_accumulator: 0.0,
            pitch_tail_remaining: None,
            pitch_tail_buffer: Vec::new(),
            pitch_tail_pos: 0,
            pitch_crossfade_remaining: 0,
            pitch_crossfade_total: 0,
            pitch_crossfade_direct_pos: 0.0,
            position_publisher: None,
            enqueued_at: None,
            first_mix_latency: None,
            first_mix_published: false,
        }
    }

    /// Set the fade state for this sample
    pub fn set_fade(&mut self, fade_state: FadeState) {
        self.fade_state = fade_state;
    }

    /// Set the optional per-route downmix gains, parallel to `channel_map` (D29).
    /// Entries beyond the vector's length read as unity, so passing fewer gains than
    /// routes (or none) leaves those routes at unity.
    pub fn set_channel_route_gains(&mut self, gains: Vec<f32>) {
        self.channel_route_gains = gains;
    }

    /// The gain for channel-map route `route_idx`, or unity when no per-route gain
    /// was configured for it (D29).
    fn channel_route_gain(&self, route_idx: usize) -> f32 {
        self.channel_route_gains
            .get(route_idx)
            .copied()
            .unwrap_or(1.0)
    }

    /// Attach the latency probe (Sprint 11, D50): the instant the play path
    /// enqueued this sample and the pre-allocated sink for its first-mix latency.
    /// Called off-RT before the sample is moved onto the command ring.
    pub fn set_latency_probe(&mut self, enqueued_at: std::time::Instant, sink: Arc<AtomicU64>) {
        self.enqueued_at = Some(enqueued_at);
        self.first_mix_latency = Some(sink);
        self.first_mix_published = false;
    }

    /// Publish the first-mix latency (D50) if this sample is about to mix loaded
    /// audio for the first time. A one-bool fast path once published; a single
    /// relaxed store into the pre-allocated atomic on the publishing block. Called
    /// from `mix_audio` with `position` at the block's start, so "started" means
    /// the read cursor sits inside the decoded region — a still-streaming buffer
    /// that has not reached the cursor yet emits silence and does not count.
    fn publish_first_mix_if_started(&mut self) {
        if self.first_mix_published {
            return;
        }
        let Some(ref latency) = self.first_mix_latency else {
            return;
        };
        if self.position >= self.buffer.frames() {
            return;
        }
        if let Some(at) = self.enqueued_at {
            latency.store(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        self.first_mix_published = true;
    }

    /// Set the playback speed.
    ///
    /// Without pitch correction: supports -100.0 to 100.0 (negative = reverse)
    /// With pitch correction: supports 0.05 to 8.0 (no reverse)
    ///
    /// Returns true if the speed was set, false if it was rejected
    /// (e.g., negative speed with pitch correction enabled).
    pub fn set_speed(&mut self, speed: f32) -> bool {
        if self.pitch_corrector.is_some() {
            // Pitch correction: 0.05 to 8.0, no reverse
            if speed < 0.0 {
                tracing::warn!(
                    "Negative speed ({}) not supported with pitch correction, ignoring",
                    speed
                );
                return false;
            }
            self.speed = speed.clamp(0.05, 8.0);
            if let Some(ref mut pc) = self.pitch_corrector {
                pc.set_speed(self.speed);
            }
        } else {
            // No pitch correction: -100.0 to 100.0, but not zero
            if speed.abs() < 0.01 {
                // Treat very small speeds as minimum
                self.speed = if speed < 0.0 { -0.01 } else { 0.01 };
            } else {
                self.speed = speed.clamp(-100.0, 100.0);
            }
        }
        true
    }

    /// Enable pitch correction (preserves pitch when changing speed).
    /// Creates a new PitchCorrector if one doesn't exist.
    ///
    /// On the `None -> Some` transition the new stretcher has ~`input_latency`
    /// frames of warm-up state, so the first blocks would be silence — an audible
    /// gap when correction is enabled mid-playback. To avoid it, the stretcher is
    /// pre-rolled with the audio immediately *before* the current playback position
    /// (F10), priming its analysis buffers so the first mixed block already carries
    /// signal. The pre-roll reads a borrowed slice of the already-decoded buffer and
    /// produces no output, so it allocates nothing on the audio thread.
    pub fn enable_pitch_correction(&mut self) {
        if self.pitch_corrector.is_some() {
            return;
        }
        let mut pc = PitchCorrector::new(self.buffer.channels(), self.buffer.sample_rate());
        pc.set_speed(self.speed);
        self.preroll_corrector(&mut pc);
        // A fresh stretcher starts with no pending input fraction and no tail.
        self.pitch_input_accumulator = 0.0;
        self.pitch_tail_remaining = None;
        self.pitch_tail_pos = 0;
        // Size the tail buffer to one full flush (output_latency frames) up front so
        // the per-block drain at EOF never allocates on the audio thread. This sits
        // alongside the (already heap-allocating) stretcher construction, off the
        // per-block mix path.
        let tail_samples = pc.output_latency() * self.buffer.channels();
        if self.pitch_tail_buffer.len() < tail_samples {
            self.pitch_tail_buffer.resize(tail_samples, 0.0);
        }
        // Crossfade the (full-level) direct playback into the (fading-in) stretched
        // output over a few ms so the transition has no click. Only when enabling
        // mid-playback (there is prior audio to be continuous with); a first enable
        // at the very start has nothing to cross-fade from.
        if self.position > 0 {
            let xfade = (self.buffer.sample_rate() as usize / 200).max(1); // ~5 ms
            self.pitch_crossfade_remaining = xfade;
            self.pitch_crossfade_total = xfade;
            self.pitch_crossfade_direct_pos = self.precise_position();
        } else {
            self.pitch_crossfade_remaining = 0;
            self.pitch_crossfade_total = 0;
        }
        self.pitch_corrector = Some(pc);
    }

    /// Feed `pc` the audio leading up to `self.position` so it is primed and emits
    /// real signal on its first `process` (F10). The pre-roll window is two input
    /// latencies of preceding frames (the stretcher needs its full input+output
    /// pipeline primed); fewer are used near the start of the buffer. Pitch
    /// correction only runs on `Complete` buffers, so a streaming buffer (or a
    /// position with nothing before it) simply skips the pre-roll.
    fn preroll_corrector(&self, pc: &mut PitchCorrector) {
        let Some(decoded) = self.buffer.as_complete() else {
            return;
        };
        let channels = decoded.channels;
        if channels == 0 || self.position == 0 {
            return;
        }
        let pre_frames = (2 * pc.input_latency()).min(self.position);
        if pre_frames == 0 {
            return;
        }
        let start = (self.position - pre_frames) * channels;
        let end = self.position * channels;
        pc.preroll(&decoded.data[start..end], self.speed as f64);
    }

    /// Set speed and pitch correction mode together.
    /// This handles the correct order of operations: pitch correction mode
    /// must be changed before setting speed (negative speeds are rejected
    /// while pitch correction is enabled).
    ///
    /// Returns true if the speed was set, false if rejected.
    pub fn set_speed_with_mode(&mut self, speed: f32, pitch_correction: bool) -> bool {
        // Change pitch correction mode FIRST
        if pitch_correction {
            self.enable_pitch_correction();
        } else {
            self.disable_pitch_correction();
        }
        // Then set speed
        self.set_speed(speed)
    }

    /// Disable pitch correction (pitch follows speed).
    pub fn disable_pitch_correction(&mut self) {
        self.pitch_corrector = None;
        // Drop any pitch-path carry so a later re-enable starts clean and the
        // non-pitch path's `is_finished` is not held open by a stale tail.
        self.pitch_input_accumulator = 0.0;
        self.pitch_tail_remaining = None;
        self.pitch_tail_pos = 0;
        self.pitch_crossfade_remaining = 0;
        self.pitch_crossfade_total = 0;
    }

    /// Check if pitch correction is enabled.
    #[allow(dead_code)]
    pub fn has_pitch_correction(&self) -> bool {
        self.pitch_corrector.is_some()
    }

    /// Check if this sample has finished playing
    /// A sample is finished if it reached the end (or start for reverse) OR if fade out is complete
    /// Looping samples only finish when fade out is complete
    pub fn is_finished(&self) -> bool {
        // Fade out complete - always finishes, even for looping samples
        if let FadeState::Out { elapsed, duration } = self.fade_state {
            if elapsed >= duration {
                return true;
            }
        }

        // Looping samples never finish from buffer position
        if self.loop_mode {
            return false;
        }

        // A pitch-corrected sample that has reached EOF is kept alive until its
        // buffered tail has been fully drained (F12); only then may the position
        // check below mark it finished.
        if let Some(remaining) = self.pitch_tail_remaining {
            if remaining > 0 {
                return false;
            }
        }

        // For forward playback, finished when position >= frames
        // For reverse playback, finished when position is 0 (or we've gone negative)
        if self.speed >= 0.0 {
            if self.position >= self.buffer.frames() {
                return true;
            }
        } else {
            // Reverse playback: finished when we've reached/passed the start
            // The fractional position going negative indicates we've exhausted the buffer
            if self.position == 0 && self.fractional_position <= 0.0 {
                return true;
            }
        }

        false
    }

    /// Get the combined volume (sample volume * voice volume)
    #[allow(dead_code)]
    pub fn combined_volume(&self) -> f32 {
        self.volume * self.voice_volume
    }

    /// Set target voice volume for smooth ramping
    pub fn set_target_voice_volume(&mut self, target: f32) {
        self.target_voice_volume = target.clamp(0.0, 1.0);
    }

    /// Advance voice volume toward target by one frame.
    /// Uses linear ramping with a fixed rate that provides ~20ms transition at 44.1kHz.
    /// Returns true if volume is still ramping, false if at target.
    pub fn advance_voice_volume(&mut self) -> bool {
        const RAMP_RATE: f32 = 0.001; // ~22ms for full 0-1 transition at 44.1kHz

        if (self.voice_volume - self.target_voice_volume).abs() < RAMP_RATE {
            self.voice_volume = self.target_voice_volume;
            false
        } else if self.voice_volume < self.target_voice_volume {
            self.voice_volume += RAMP_RATE;
            true
        } else {
            self.voice_volume -= RAMP_RATE;
            true
        }
    }

    /// The frame count to wrap against when looping, or `None` if looping must
    /// not engage yet.
    ///
    /// A still-streaming buffer reports only the frames decoded so far, so
    /// wrapping against it would replay an ever-growing prefix (an audible buzz).
    /// Looping is therefore deferred until the buffer is `Complete`, when
    /// `frames()` is the true total; until then the sample plays forward and the
    /// callback emits silence past the loaded edge.
    fn loop_boundary(&self) -> Option<usize> {
        if self.loop_mode && self.buffer.is_complete() {
            let frames = self.buffer.frames();
            (frames > 0).then_some(frames)
        } else {
            None
        }
    }

    /// Whether a loop crossfade is engaged: looping against a complete buffer,
    /// a non-zero crossfade length, and a buffer long enough to hold a crossfade
    /// at both ends without overlap. This gates both the crossfade blend and the
    /// overlap-on-wrap offset, so they stay in lockstep.
    fn crossfade_active(&self) -> bool {
        match self.loop_boundary() {
            Some(buffer_frames) => {
                self.crossfade_samples > 0 && buffer_frames > self.crossfade_samples * 2
            }
            None => false,
        }
    }

    /// The frame a forward loop restarts from after wrapping. With an active
    /// crossfade the tail has already faded the head `[0..cf_samples]` in, so the
    /// next pass begins at `cf_samples` (a true overlap-add: the head is not
    /// replayed at full level). Without a crossfade, loops restart at 0.
    fn loop_restart_frame(&self) -> usize {
        if self.crossfade_active() {
            self.crossfade_samples
        } else {
            0
        }
    }

    /// Advance the playback position by the given number of output frames,
    /// accounting for playback speed (including negative for reverse).
    /// Handles looping by wrapping position to other end of buffer.
    /// Returns the effective number of source frames consumed (absolute value).
    fn advance_position(&mut self, output_frames: usize) -> usize {
        let advance = output_frames as f64 * self.speed as f64;
        let new_pos = self.position as f64 + self.fractional_position + advance;

        if let Some(buffer_frames) = self.loop_boundary() {
            // Handle looping
            if new_pos < 0.0 {
                // Reverse playback wrapped past start - loop to end
                let wrapped = new_pos % buffer_frames as f64 + buffer_frames as f64;
                self.position = wrapped as usize % buffer_frames;
                self.fractional_position = wrapped.fract();
            } else if new_pos >= buffer_frames as f64 {
                // Forward playback wrapped past end. With an active crossfade the
                // head [0..restart] has already been mixed into the tail, so the
                // next pass resumes at `restart` rather than 0 (true overlap-add);
                // the repeating section is then [restart, buffer_frames).
                let restart = self.loop_restart_frame() as f64;
                let period = buffer_frames as f64 - restart;
                let wrapped = restart + (new_pos - restart).rem_euclid(period);
                self.position = wrapped as usize;
                self.fractional_position = wrapped.fract();
            } else {
                self.position = new_pos as usize;
                self.fractional_position = new_pos.fract();
            }
        } else {
            // Non-looping behavior
            if new_pos < 0.0 {
                // Reverse playback reached the start
                self.position = 0;
                self.fractional_position = 0.0;
            } else {
                self.position = new_pos as usize;
                self.fractional_position = new_pos.fract();
            }
        }

        // Return effective frames consumed (absolute value)
        advance.abs().ceil() as usize
    }

    /// Get the current precise position as a float for interpolation
    fn precise_position(&self) -> f64 {
        self.position as f64 + self.fractional_position
    }
}

/// Active live input (microphone) being mixed
pub struct LiveInput {
    /// Voice ID this input belongs to (for ducking)
    pub voice_id: String,

    /// Ring buffer consumer for receiving audio samples
    pub consumer: HeapConsumer<f32>,

    /// Number of input channels
    pub input_channels: usize,

    /// Per-input volume (0.0 - 1.0)
    pub volume: f32,

    /// Whether this input is currently muted. When muted, `volume` is held at 0.0
    /// and the operator's pre-mute level is kept in `pre_mute_volume` (D34).
    muted: bool,

    /// The volume to restore on unmute — the level the input had when it was muted,
    /// so unmute returns to the calibrated value rather than a hardcoded 1.0 (D34).
    pre_mute_volume: f32,

    /// Voice-level volume (0.0 - 1.0) - current smoothed value
    pub voice_volume: f32,

    /// Target voice volume for smooth ramping (0.0 - 1.0)
    pub target_voice_volume: f32,

    /// Channel routing: vec![(src_channel, dest_channel), ...]
    pub channel_map: Vec<(usize, usize)>,
}

impl LiveInput {
    /// Create a new live input with the given routing
    pub fn new(
        voice_id: String,
        consumer: HeapConsumer<f32>,
        input_channels: usize,
        volume: f32,
        channel_map: Vec<(usize, usize)>,
    ) -> Self {
        Self {
            voice_id,
            consumer,
            input_channels,
            volume,
            muted: false,
            pre_mute_volume: volume,
            voice_volume: 1.0,
            target_voice_volume: 1.0,
            channel_map,
        }
    }

    /// Get the combined volume (input volume * voice volume)
    #[allow(dead_code)]
    pub fn combined_volume(&self) -> f32 {
        self.volume * self.voice_volume
    }

    /// Set this input's volume directly. An explicit level clears any muted state,
    /// so the value takes effect immediately and a later unmute does not revert it
    /// (D34). The value is clamped to a valid gain.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.muted = false;
    }

    /// Mute or unmute this input. Muting stores the current volume and zeroes it;
    /// unmuting restores the stored pre-mute volume rather than a hardcoded 1.0
    /// (D34). Guarded by `muted` so a repeated mute does not overwrite the stored
    /// level with 0.0, and a repeated unmute is a no-op.
    pub fn set_muted(&mut self, mute: bool) {
        if mute {
            if !self.muted {
                self.pre_mute_volume = self.volume;
                self.volume = 0.0;
                self.muted = true;
            }
        } else if self.muted {
            self.volume = self.pre_mute_volume;
            self.muted = false;
        }
    }

    /// Set target voice volume for smooth ramping
    #[allow(dead_code)]
    pub fn set_target_voice_volume(&mut self, target: f32) {
        self.target_voice_volume = target.clamp(0.0, 1.0);
    }

    /// Advance voice volume toward target by one frame.
    /// Uses linear ramping with a fixed rate that provides ~20ms transition at 44.1kHz.
    pub fn advance_voice_volume(&mut self) {
        const RAMP_RATE: f32 = 0.001; // ~22ms for full 0-1 transition at 44.1kHz

        if (self.voice_volume - self.target_voice_volume).abs() < RAMP_RATE {
            self.voice_volume = self.target_voice_volume;
        } else if self.voice_volume < self.target_voice_volume {
            self.voice_volume += RAMP_RATE;
        } else {
            self.voice_volume -= RAMP_RATE;
        }
    }
}

/// A windowed streamed source: a voice that plays a long, large, or live asset by
/// consuming decoded f32 frames from a bounded ring that a background task fills off
/// the audio thread, so it costs `O(window)` memory regardless of the asset's length.
///
/// It sits between the two existing voice types. Unlike [`ActiveSample`] its buffer
/// is a forward-only draining ring, so it supports no random-access feature (seek,
/// loop-crossfade, reverse, variable speed, or pitch correction); those are rejected
/// for streamed voices at the command layer. Unlike [`LiveInput`] it has an owner (a
/// producer task), a lifecycle (prebuffer -> play -> EOF/stop -> reap), fade in/out,
/// and a completion signal, so a finished cue retires itself and is reaped off-RT.
///
/// The per-frame ring consume and the hold-and-fade on underrun are shared with the
/// live-input path via [`mix_ring_voice_frame`].
pub struct StreamedSource {
    /// Unique internal id (assigned by VoiceManager), for stop/selection.
    pub id: u64,

    /// User-provided sample identifier for targeting commands.
    pub sample_id: Option<String>,

    /// Voice id this source belongs to (for ducking and voice volume).
    pub voice_id: String,

    /// Source file path or URL (for targeting by filename and logging).
    pub file_path: String,

    /// Ring-buffer consumer fed by the background decode task.
    pub consumer: HeapConsumer<f32>,

    /// Number of channels the producer writes (interleaved).
    pub input_channels: usize,

    /// Per-source volume (0.0 - 1.0).
    pub volume: f32,

    /// Voice-level volume (0.0 - 1.0) - current smoothed value.
    pub voice_volume: f32,

    /// Target voice volume for smooth ramping (0.0 - 1.0).
    pub target_voice_volume: f32,

    /// Channel routing: vec![(src_channel, dest_channel), ...].
    pub channel_map: Vec<(usize, usize)>,

    /// Fade state (in/out/none). A completed fade-out drives the source to silence
    /// and then to completion even if the ring still holds audio.
    pub fade_state: FadeState,

    /// Set by the producer task when the decoder reaches EOF and will not loop. The
    /// source is finished once this is set and the ring has drained.
    producer_done: Arc<AtomicBool>,

    /// Set off the audio thread (by the reaper, on stop or drop) to ask the producer
    /// task to stop early. Held so the source owns the flag for the producer's life.
    stop_flag: Arc<AtomicBool>,

    /// When the control thread enqueued this source's AddStreamedSource command
    /// (Sprint 11, D50). `None` outside the daemon's play path.
    enqueued_at: Option<std::time::Instant>,

    /// First-mix latency sink (Sprint 11, D50): on the first block in which this
    /// source pops real frames from its ring, the callback stores the nanoseconds
    /// elapsed since `enqueued_at` — one relaxed store into a pre-allocated atomic.
    first_mix_latency: Option<Arc<AtomicU64>>,

    /// Set once the first-mix latency has been published (one-bool fast path).
    first_mix_published: bool,
}

impl StreamedSource {
    /// Build a streamed source over `consumer`, the read end of the ring the producer
    /// task fills. `producer_done`/`stop_flag` are the shared handles the producer
    /// task also holds. Voice volume starts at unity and ramps toward
    /// `target_voice_volume`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: u64,
        voice_id: String,
        file_path: String,
        sample_id: Option<String>,
        consumer: HeapConsumer<f32>,
        input_channels: usize,
        volume: f32,
        channel_map: Vec<(usize, usize)>,
        producer_done: Arc<AtomicBool>,
        stop_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            id,
            sample_id,
            voice_id,
            file_path,
            consumer,
            input_channels,
            volume: volume.clamp(0.0, 1.0),
            voice_volume: 1.0,
            target_voice_volume: 1.0,
            channel_map,
            fade_state: FadeState::None,
            producer_done,
            stop_flag,
            enqueued_at: None,
            first_mix_latency: None,
            first_mix_published: false,
        }
    }

    /// Attach the latency probe (Sprint 11, D50): the instant the play path
    /// enqueued this source and the pre-allocated sink for its first-mix latency.
    /// Called off-RT before the source is moved onto the command ring.
    pub fn set_latency_probe(&mut self, enqueued_at: std::time::Instant, sink: Arc<AtomicU64>) {
        self.enqueued_at = Some(enqueued_at);
        self.first_mix_latency = Some(sink);
        self.first_mix_published = false;
    }

    /// Publish the first-mix latency (D50) after a block in which this source
    /// popped real frames from its ring. One-bool fast path once published; a
    /// single relaxed store on the publishing block.
    fn publish_first_mix_if_started(&mut self) {
        if self.first_mix_published {
            return;
        }
        let Some(ref latency) = self.first_mix_latency else {
            return;
        };
        if let Some(at) = self.enqueued_at {
            latency.store(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        self.first_mix_published = true;
    }

    /// Set the fade state for this source (fade-in on start, fade-out on stop).
    pub fn set_fade(&mut self, fade_state: FadeState) {
        self.fade_state = fade_state;
    }

    /// Ask the producer task to stop and stop feeding the ring. Called off the audio
    /// thread when the source is reaped.
    pub fn signal_stop(&self) {
        self.stop_flag.store(true, Ordering::Release);
    }

    /// Set target voice volume for smooth ramping.
    pub fn set_target_voice_volume(&mut self, target: f32) {
        self.target_voice_volume = target.clamp(0.0, 1.0);
    }

    /// Advance voice volume toward target by one frame (linear ramp, matching the
    /// other voices' ~22 ms transition at 44.1 kHz).
    pub fn advance_voice_volume(&mut self) {
        const RAMP_RATE: f32 = 0.001;

        if (self.voice_volume - self.target_voice_volume).abs() < RAMP_RATE {
            self.voice_volume = self.target_voice_volume;
        } else if self.voice_volume < self.target_voice_volume {
            self.voice_volume += RAMP_RATE;
        } else {
            self.voice_volume -= RAMP_RATE;
        }
    }

    /// Whether this source has finished and can be reaped off the audio thread. A
    /// completed fade-out finishes it even with audio still buffered; otherwise it
    /// finishes only once the producer has signalled EOF and the ring has drained
    /// below a full frame.
    pub fn is_finished(&self) -> bool {
        if let FadeState::Out { elapsed, duration } = self.fade_state {
            if elapsed >= duration {
                return true;
            }
        }
        self.producer_done.load(Ordering::Acquire)
            && self.consumer.len() < self.input_channels.max(1)
    }
}

impl Drop for StreamedSource {
    /// Dropping a source stops its producer thread, so a source that is reaped (or
    /// discarded anywhere off the audio thread) never leaves its decoder running on a
    /// ring no one reads. The audio thread only ever *moves* a finished source into
    /// the graveyard, so this drop runs off the real-time thread.
    fn drop(&mut self) {
        self.signal_stop();
    }
}

/// Pre-reserved capacity for the voice pool, so a Play never reallocates the
/// `active_samples` Vec on the audio thread. Generous headroom over the documented
/// "20+ simultaneous" target. (Exceeding it reallocates once — see docs/bugs.md.)
pub const MAX_VOICES: usize = 256;

/// Pre-reserved capacity for live inputs, so adding a microphone never reallocates
/// `live_inputs` on the audio thread.
pub const MAX_LIVE_INPUTS: usize = 16;

/// Pre-reserved capacity for windowed streamed sources, so adding one never
/// reallocates `streamed_sources` on the audio thread.
pub const MAX_STREAMED_SOURCES: usize = 64;

/// Convert a level in dBFS to a linear amplitude (0 dBFS == 1.0).
pub fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Fraction of the ceiling below which the limiter is perfectly transparent. The
/// soft knee occupies the top `(1 - SOFT_KNEE_RATIO)` of the range up to the
/// ceiling, so material below it passes through unchanged.
const SOFT_KNEE_RATIO: f32 = 0.95;

/// Soft-knee limiter. Below `SOFT_KNEE_RATIO * ceiling` the signal is unchanged;
/// above it, the excess is mapped through `tanh` so the output asymptotes to
/// `ceiling` (the join is C1-continuous, so there is no brickwall corner). The
/// magnitude is strictly below `ceiling`, so peaks never reach full scale.
fn soft_limit(x: f32, ceiling: f32) -> f32 {
    let threshold = ceiling * SOFT_KNEE_RATIO;
    let magnitude = x.abs();
    if magnitude <= threshold {
        x
    } else {
        let knee = ceiling - threshold;
        x.signum() * (threshold + knee * ((magnitude - threshold) / knee).tanh())
    }
}

/// The four Catmull-Rom taps `(p0, p1, p2, p3)` centered on the segment
/// `frame_n..frame_n+1`, where `frame_n` is in range and `read(i)` fetches the
/// sample at source frame `i` (already bounds-safe). Tap selection (D27):
/// - **Looping:** outer taps wrap around `buffer_frames` (the tap before frame 0
///   is the last frame; taps past the end wrap to the start), because a loop's
///   content is periodic.
/// - **Not looping:** an out-of-range outer tap is **linearly extrapolated** from
///   the two in-range center taps (`p0 = 2*p1 - p2`, `p3 = 2*p2 - p1`) rather than
///   clamped. Extrapolation preserves the straight-line continuation at the buffer
///   edges, so a ramp still interpolates exactly there (clamping would bend it).
fn cubic_taps(
    n: usize,
    buffer_frames: usize,
    loop_mode: bool,
    read: impl Fn(usize) -> f32,
) -> (f32, f32, f32, f32) {
    let p1 = read(n);
    let last = buffer_frames - 1;
    let (p0, p2, p3) = if loop_mode {
        let wrap = |off: isize| ((n as isize + off).rem_euclid(buffer_frames as isize)) as usize;
        (read(wrap(-1)), read(wrap(1)), read(wrap(2)))
    } else {
        let p2 = if n < last { read(n + 1) } else { p1 };
        let p0 = if n >= 1 { read(n - 1) } else { 2.0 * p1 - p2 };
        let p3 = if n + 1 < last {
            read(n + 2)
        } else {
            2.0 * p2 - p1
        };
        (p0, p2, p3)
    };
    (p0, p1, p2, p3)
}

/// Catmull-Rom cubic interpolation between the two center taps `p1` and `p2` at
/// fractional position `t` in `[0,1)`, using the outer taps `p0` and `p3` to shape
/// the curve. This is the non-pitch speed path's reconstruction filter (D27): it
/// has a far flatter passband than two-point linear interpolation, so resampling a
/// band-limited signal at a fractional ratio leaves much less spurious energy. It
/// is exact for inputs up to cubic (so a linear ramp resolves to `p1 + (p2-p1)*t`,
/// keeping the direction-independent two-point ramp identities the F9 tests pin).
fn cubic_interpolate(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a0 = 2.0 * p1;
    let a1 = p2 - p0;
    let a2 = 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3;
    let a3 = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
    0.5 * (a0 + t * (a1 + t * (a2 + t * a3)))
}

/// Equal-power crossfade between `a` (fading out) and `b` (fading in) as
/// `progress` goes 0.0 -> 1.0. The cos/sin law keeps `gain_a^2 + gain_b^2 == 1`,
/// so the summed power of two uncorrelated signals is constant through the
/// crossfade (no ~3 dB midpoint dip that a linear blend produces).
fn equal_power_blend(a: f32, b: f32, progress: f32) -> f32 {
    let angle = progress * std::f32::consts::FRAC_PI_2;
    a * angle.cos() + b * angle.sin()
}

/// The duck gain at frame `frame_idx` of a `frames`-long buffer, linearly
/// interpolated between the buffer's start and end duck multipliers `(start, end)`
/// (D2). The fade itself is advanced once per buffer (D1); this only reads, so
/// every sample/input/frame of a voice sees a consistent, smoothly-changing gain.
fn duck_frame_gain((start, end): (f32, f32), frame_idx: usize, frames: usize) -> f32 {
    if frames <= 1 {
        return start;
    }
    let t = frame_idx as f32 / frames as f32;
    start + (end - start) * t
}

/// Mixer state shared between engine and audio callback
pub struct MixerState {
    /// List of currently playing samples
    pub active_samples: Vec<ActiveSample>,

    /// List of active live inputs (microphones)
    pub live_inputs: Vec<LiveInput>,

    /// List of active windowed streamed sources (long, large, or live assets)
    pub streamed_sources: Vec<StreamedSource>,

    /// Number of output channels
    pub output_channels: usize,

    /// Applies pre-resolved ducking targets for automatic voice volume reduction
    pub ducking_applier: Option<DuckingApplier>,

    /// Bass management for LFE extraction and crossover filtering
    pub bass_management: Option<BassManagement>,

    /// Per-output-channel calibration gain, length `output_channels` (default 1.0).
    /// Resolved from `audio.channel_volumes` aliases->indices once at construction
    /// (off the RT thread) and applied as the final per-channel gain stage.
    pub channel_gains: Vec<f32>,

    /// Limiter ceiling as a linear amplitude. The bus peak is held at or below it.
    pub output_ceiling: f32,

    /// Linear gain applied to the whole bus before limiting.
    pub master_gain: f32,

    /// Count of samples that exceeded the ceiling and were limited. Incremented on
    /// the audio thread (lock-free), read by `/status`.
    pub clip_count: Arc<AtomicU64>,

    /// Opt-in telemetry gate (Sprint W6, DW3). Off by default. When set, the
    /// callback publishes each sample's live position into its `position_publisher`
    /// atomic (one relaxed store per sample per block); when clear it does no new
    /// work beyond a single relaxed load. The control thread shares this Arc and
    /// flips it (POST /telemetry); it never locks the RT state (D22a).
    pub telemetry_enabled: Arc<AtomicBool>,

    /// Per-output-channel peak meters (Sprint W7), length `output_channels`. When
    /// telemetry is on, the callback stores each channel's post-limiter peak (as
    /// f32 bits) once per block — relaxed, alloc-free. The control thread reads them
    /// for the state-event tick. Empty disables metering.
    pub output_meters: Arc<Vec<AtomicU32>>,
}

impl MixerState {
    /// Create a `MixerState` with the output stage at its defaults: unity
    /// per-channel gains, the default limiter ceiling and master gain, and a fresh
    /// clip counter. The convenience constructor for tests and the offline
    /// render/alloc harnesses (`audio::test_support`, the integration suites); the
    /// running daemon builds its `MixerState` inline in `main.rs` with the resolved
    /// gains, ducking applier, and bass management. Used by the library crate's
    /// harness but only by `#[cfg(test)]` code in the binary, so the binary build
    /// would otherwise flag it dead.
    #[allow(dead_code)]
    pub fn new(output_channels: usize) -> Self {
        Self {
            active_samples: Vec::with_capacity(MAX_VOICES),
            live_inputs: Vec::with_capacity(MAX_LIVE_INPUTS),
            streamed_sources: Vec::with_capacity(MAX_STREAMED_SOURCES),
            output_channels,
            ducking_applier: None,
            bass_management: None,
            channel_gains: vec![1.0; output_channels],
            output_ceiling: db_to_linear(DEFAULT_OUTPUT_CEILING_DB),
            master_gain: DEFAULT_MASTER_GAIN,
            clip_count: Arc::new(AtomicU64::new(0)),
            telemetry_enabled: Arc::new(AtomicBool::new(false)),
            output_meters: Arc::new((0..output_channels).map(|_| AtomicU32::new(0)).collect()),
        }
    }
}

/// Mix all active samples into the output buffer
///
/// This function runs in the audio callback thread and must never:
/// - Allocate memory
/// - Block on I/O
/// - Acquire locks
/// - Do expensive computation
pub fn mix_audio(output: &mut [f32], state: &mut MixerState) {
    // Zero the output buffer
    output.fill(0.0);

    let frames = output.len() / state.output_channels;

    // Advance every ducked voice's fade exactly ONCE for this buffer (D1), before
    // any sample/input reads it. Each voice's per-frame multiplier is then looked up
    // (non-advancing) and interpolated across the buffer (D2), so several samples on
    // one voice no longer race its fade ahead. Split the borrow: `ducking` reads the
    // applier immutably while the sample/input loops mutate their own fields.
    if let Some(ref mut applier) = state.ducking_applier {
        applier.advance_buffer(frames);
    }
    let ducking = state.ducking_applier.as_ref();
    let output_channels = state.output_channels;

    // Opt-in telemetry gate (Sprint W6, DW3): read once per block. When clear, the
    // per-sample position store below is skipped — no new RT work beyond this load.
    let publish_positions = state.telemetry_enabled.load(Ordering::Relaxed);

    // Mix each active sample into the output
    for sample in &mut state.active_samples {
        // This voice's duck multipliers at the buffer's start and end (D1); the mix
        // loop lerps between them per frame (D2). An unducked voice reads (1.0, 1.0).
        let duck = ducking.map_or((1.0, 1.0), |a| a.buffer_endpoints(&sample.voice_id));

        // First-mix latency probe (Sprint 11, D50): a one-bool fast path per
        // sample per block once published.
        sample.publish_first_mix_if_started();

        let position_advanced =
            mix_sample_into_output(sample, output, frames, output_channels, duck);

        // Advance playback position (accounting for speed). The pitch path advances
        // its own position by the exact input frames it fed the stretcher (F11), so
        // skip the generic speed-based advance for it to avoid double-counting.
        if !position_advanced {
            sample.advance_position(frames);
        }

        // Publish the post-advance live position (DW12): a single relaxed store into
        // a pre-allocated atomic moved in with the sample — no alloc, no lock. The
        // control thread reads it without ever touching the mixer (D22a).
        if publish_positions {
            if let Some(ref publisher) = sample.position_publisher {
                publisher.store(sample.position, Ordering::Relaxed);
            }
        }
    }

    // Mix each live input into the output
    for input in &mut state.live_inputs {
        // This input voice's duck multipliers at the buffer endpoints (D1/D2).
        let duck = ducking.map_or((1.0, 1.0), |a| a.buffer_endpoints(&input.voice_id));

        mix_live_input_into_output(input, output, frames, output_channels, duck);
    }

    // Mix each windowed streamed source into the output
    for source in &mut state.streamed_sources {
        // This voice's duck multipliers at the buffer endpoints (D1/D2).
        let duck = ducking.map_or((1.0, 1.0), |a| a.buffer_endpoints(&source.voice_id));

        mix_streamed_source_into_output(source, output, frames, output_channels, duck);
    }

    // Apply bass management (LFE extraction and crossover filtering)
    if let Some(ref mut bm) = state.bass_management {
        bm.process(output, state.output_channels);
    }

    // Final output stage, applied per frame in this fixed order:
    //   1. per-channel calibration gain (and the master bus gain),
    //   2. finite-guard: a non-finite sample (NaN/Inf from a corrupt source or a
    //      downstream overflow) becomes silence so it never reaches the DAC. This
    //      runs before the limiter because tanh(+Inf) == 1.0 is finite, so a +Inf
    //      would otherwise slip through the limiter as a ceiling-level value,
    //   3. soft-knee limiter toward the configured ceiling,
    //   4. a hard clamp at the ceiling as the last safety net.
    let channels = state.output_channels;
    let master_gain = state.master_gain;
    let ceiling = state.output_ceiling;
    let channel_gains = &state.channel_gains;
    let mut clip_events: u64 = 0;
    // Per-channel output peak for the meters (Sprint W7), tracked during the limiter
    // pass on a fixed-size stack array (no alloc) and published below if telemetry on.
    const MAX_METER_CHANNELS: usize = 64;
    let mut peaks = [0.0f32; MAX_METER_CHANNELS];
    for frame in output.chunks_exact_mut(channels) {
        for (ch, s) in frame.iter_mut().enumerate() {
            // `channel_gains` is sized to `output_channels` at construction, so this
            // index is valid in production; fall back to unity rather than ever
            // panicking in the audio callback if that invariant is somehow broken.
            let gain = channel_gains.get(ch).copied().unwrap_or(1.0);
            let gained = *s * master_gain * gain;
            if !gained.is_finite() {
                *s = 0.0;
                continue;
            }
            if gained.abs() > ceiling {
                clip_events += 1;
            }
            *s = soft_limit(gained, ceiling).clamp(-ceiling, ceiling);
            if ch < MAX_METER_CHANNELS {
                let a = s.abs();
                if a > peaks[ch] {
                    peaks[ch] = a;
                }
            }
        }
    }
    if clip_events > 0 {
        state.clip_count.fetch_add(clip_events, Ordering::Relaxed);
    }
    // Publish per-channel output peaks (gated; relaxed alloc-free stores, mirroring
    // the position publish). The control thread reads these for the state tick.
    if publish_positions {
        let meters = &state.output_meters;
        let n = channels.min(meters.len()).min(MAX_METER_CHANNELS);
        for (ch, slot) in meters.iter().take(n).enumerate() {
            slot.store(peaks[ch].to_bits(), Ordering::Relaxed);
        }
    }
}

/// Mix a single sample into the output buffer with linear interpolation for speed
/// control. Returns `true` if the sample advanced its own playback position (the
/// pitch path does so by the exact input frames fed), in which case the caller must
/// not also apply the generic speed-based advance.
#[must_use]
fn mix_sample_into_output(
    sample: &mut ActiveSample,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
    duck: (f32, f32),
) -> bool {
    // Use pitch-corrected path if pitch corrector is enabled
    // Note: pitch correction requires Complete buffers (direct data slice access)
    if sample.pitch_corrector.is_some() && sample.buffer.is_complete() {
        mix_sample_with_pitch_correction(sample, output, frames, output_channels, duck);
        return true;
    }

    let speed = sample.speed as f64;
    let is_reverse = speed < 0.0;
    let buffer_frames = sample.buffer.frames();
    let buffer_channels = sample.buffer.channels();
    // Looping only engages once the buffer is complete (see loop_boundary); a
    // still-streaming buffer plays forward and emits silence past the loaded edge
    // rather than wrapping against its growing prefix.
    let loop_mode = sample.loop_boundary().is_some();
    let sample_volume = sample.volume;
    // Frame the forward loop restarts from (past the overlapped head when a
    // crossfade is active); 0 for a plain loop. Constant for the whole block.
    let loop_restart = sample.loop_restart_frame() as f64;

    // Calculate current precise position (integer + fractional parts)
    let mut src_pos = sample.precise_position();

    for frame_idx in 0..frames {
        // Advance voice volume toward target (smooth ramping to avoid pops)
        sample.advance_voice_volume();

        // Handle looping wrap-around
        if loop_mode && buffer_frames > 0 {
            if src_pos < 0.0 {
                // Wrap from start to end
                src_pos = src_pos % buffer_frames as f64 + buffer_frames as f64;
            } else if src_pos >= buffer_frames as f64 {
                // Wrap from end to the restart frame (past the overlapped head
                // under an active crossfade), matching advance_position.
                let period = buffer_frames as f64 - loop_restart;
                src_pos = loop_restart + (src_pos - loop_restart).rem_euclid(period);
            }
        } else {
            // Check bounds based on direction (non-looping)
            if is_reverse {
                // For reverse playback, stop when we've gone past the start
                if src_pos < 0.0 {
                    break;
                }
            } else {
                // For forward playback, stop when we've reached the end
                if src_pos as usize >= buffer_frames {
                    break;
                }
            }
        }

        let src_frame = src_pos as usize;

        // Safety check: ensure we're within bounds
        if src_frame >= buffer_frames {
            if loop_mode {
                src_pos += speed;
                continue;
            }
            break;
        }

        // Calculate fade multiplier for this frame
        let fade_multiplier = sample.fade_state.multiplier();
        let ducking_multiplier = duck_frame_gain(duck, frame_idx, frames);
        let base_volume = sample_volume * sample.voice_volume;
        let final_volume = base_volume * fade_multiplier * ducking_multiplier;

        // Fractional position between src_frame and src_frame+1. The interpolation
        // taps are direction-independent: the output at position p is the same
        // curve through frame_n..frame_n+1 for forward and reverse alike (only the
        // loop crossfade below keys on direction).
        let frac = src_pos.fract() as f32;

        // Apply channel mapping and mix into output
        for (route_idx, &(src_ch, dest_ch)) in sample.channel_map.iter().enumerate() {
            // Bounds check
            if src_ch >= buffer_channels || dest_ch >= output_channels {
                continue;
            }

            let route_gain = sample.channel_route_gain(route_idx);
            let dest_idx = frame_idx * output_channels + dest_ch;

            // Get current sample value (returns silence for streaming buffers if unavailable)
            let sample_val = sample.buffer.get_sample_or_silence(src_frame, src_ch);

            // Cubic (Catmull-Rom) interpolation through the four taps centered on
            // src_frame..src_frame+1 (D27): taps wrap under looping and are linearly
            // extrapolated at the buffer edges otherwise (see cubic_taps). At an
            // integer position the value is exactly src_frame, so keep the cheap
            // exact path there.
            let interpolated_val = if frac > 0.001 {
                let (p0, p1, p2, p3) = cubic_taps(src_frame, buffer_frames, loop_mode, |i| {
                    sample.buffer.get_sample_or_silence(i, src_ch)
                });
                cubic_interpolate(p0, p1, p2, p3, frac)
            } else {
                sample_val
            };

            // Apply loop crossfade by blending samples from end and beginning
            let blended_val = if loop_mode
                && sample.crossfade_samples > 0
                && buffer_frames > sample.crossfade_samples * 2
            {
                let cf_samples = sample.crossfade_samples;

                if is_reverse {
                    // Reverse playback: crossfade when approaching start (frame 0)
                    if src_frame < cf_samples {
                        // Blend current position (near start) with end of buffer
                        // progress goes from 0.0 (at cf_samples-1) to 1.0 (at frame 0)
                        let progress = 1.0 - (src_frame as f32 / cf_samples as f32);
                        let blend_frame = buffer_frames - cf_samples + src_frame;
                        let blend_val = sample.buffer.get_sample_or_silence(blend_frame, src_ch);
                        equal_power_blend(interpolated_val, blend_val, progress)
                    } else {
                        interpolated_val
                    }
                } else {
                    // Forward playback: crossfade when approaching end
                    let crossfade_start = buffer_frames - cf_samples;
                    if src_frame >= crossfade_start {
                        // Blend current position (near end) with start of buffer
                        // progress goes from 0.0 (at crossfade_start) to 1.0 (at buffer_frames-1)
                        let frames_into_crossfade = src_frame - crossfade_start;
                        let progress = frames_into_crossfade as f32 / cf_samples as f32;
                        let blend_frame = frames_into_crossfade;
                        let blend_val = sample.buffer.get_sample_or_silence(blend_frame, src_ch);
                        equal_power_blend(interpolated_val, blend_val, progress)
                    } else {
                        interpolated_val
                    }
                }
            } else {
                interpolated_val
            };

            // Mix with combined volume, fade, and the per-route downmix gain applied
            output[dest_idx] += blended_val * final_volume * route_gain;
        }

        // Advance fade state
        sample.fade_state.advance();

        // Advance source position by speed (negative speed moves backward)
        src_pos += speed;
    }

    // The non-pitch path does not advance the position itself; the caller applies
    // the generic speed-based advance.
    false
}

/// Mix a sample with pitch correction using time-stretching.
/// This preserves pitch when speed != 1.0.
///
/// This path advances `sample.position` itself, by the exact number of input frames
/// fed to the stretcher (carrying the fractional remainder so successive input
/// slices abut, F11), and at EOF drains the stretcher's buffered tail across the
/// following blocks instead of truncating it (F12). The caller therefore must not
/// apply the generic speed-based advance to a pitch-corrected sample.
///
/// Note: This function requires a Complete buffer (not streaming).
fn mix_sample_with_pitch_correction(
    sample: &mut ActiveSample,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
    duck: (f32, f32),
) {
    // Pitch correction requires direct slice access, only available for Complete buffers
    let decoded_buffer = match sample.buffer.as_complete() {
        Some(buf) => buf,
        None => return, // Streaming buffer - skip pitch correction
    };

    let speed = sample.speed;
    let src_channels = decoded_buffer.channels;
    let output_samples = frames * src_channels;

    // Reuse the sample's pre-allocated scratch buffer for the stretcher output
    // (interleaved, same channel count as source) instead of heap-allocating one
    // every callback. Taken out by value so the mix loop below can keep mutating
    // other fields of `sample`; returned at the end of the function.
    let mut stretched = std::mem::take(&mut sample.pitch_scratch);
    if stretched.len() < output_samples {
        stretched.resize(output_samples, 0.0);
    }
    // Match the previous freshly-zeroed buffer so partially-filled output is silence.
    stretched[..output_samples].fill(0.0);

    // Produce this block's stretched output, either by feeding source input or, once
    // the source is exhausted, by emitting the drained tail. `produced` is false only
    // when there is genuinely nothing left to emit.
    let produced = if let Some(remaining) = sample.pitch_tail_remaining {
        // EOF reached on an earlier block: emit the next slice of the already-drained
        // tail (the stretcher's `flush` must be taken in one call, so it was captured
        // whole into `pitch_tail_buffer` at EOF and is now played out block by block).
        if remaining == 0 {
            false
        } else {
            let take = remaining.min(frames);
            let src_start = sample.pitch_tail_pos * src_channels;
            let copy_len = take * src_channels;
            stretched[..copy_len]
                .copy_from_slice(&sample.pitch_tail_buffer[src_start..src_start + copy_len]);
            sample.pitch_tail_pos += take;
            sample.pitch_tail_remaining = Some(remaining - take);
            true
        }
    } else {
        // Normal path: feed the integer number of input frames due this block.
        // `frames * speed` is the ideal (fractional) consumption; accumulate it and
        // feed only the whole part, carrying the remainder so the next slice starts
        // exactly where this one ends (F11).
        sample.pitch_input_accumulator += frames as f64 * speed as f64;
        let want = sample.pitch_input_accumulator.floor() as usize;
        let available = decoded_buffer.frames.saturating_sub(sample.position);

        // `want < available` keeps a strict margin so that the block which lands on
        // (or past) the last source frame takes the EOF path below — otherwise a
        // block that consumes *exactly* to the end would leave `position ==
        // buffer.frames()` with no tail armed, and `is_finished` would retire the
        // sample before its tail is drained (F12).
        if want < available {
            sample.pitch_input_accumulator -= want as f64;
            let input_start = sample.position * src_channels;
            let input_end = (sample.position + want) * src_channels;
            let input_slice = &decoded_buffer.data[input_start..input_end];
            if let Some(ref mut pc) = sample.pitch_corrector {
                pc.process(input_slice, &mut stretched[..output_samples]);
            }
            sample.position += want;
            true
        } else {
            // EOF this block: feed whatever input remains and produce this block's
            // output, then drain the stretcher's whole buffered tail (in one `flush`)
            // into the pre-sized tail buffer to be played out over the following
            // blocks (F12). The position advances to the end so `is_finished` reports
            // EOF once the tail has been fully emitted.
            let input_start = sample.position * src_channels;
            let input_end = decoded_buffer.frames * src_channels;
            let input_slice = &decoded_buffer.data[input_start..input_end];
            let tail = if let Some(ref mut pc) = sample.pitch_corrector {
                pc.process(input_slice, &mut stretched[..output_samples]);
                let tail_frames = pc.output_latency();
                let tail_samples = tail_frames * src_channels;
                pc.flush(&mut sample.pitch_tail_buffer[..tail_samples]);
                tail_frames
            } else {
                0
            };
            sample.position = decoded_buffer.frames;
            sample.pitch_input_accumulator = 0.0;
            sample.pitch_tail_pos = 0;
            sample.pitch_tail_remaining = Some(tail);
            true
        }
    };

    if produced {
        apply_pitch_block(
            sample,
            output,
            frames,
            output_channels,
            src_channels,
            duck,
            &stretched,
            &decoded_buffer,
            speed,
        );
    }

    // Return the scratch buffer to the sample for reuse on the next callback.
    sample.pitch_scratch = stretched;
}

/// Read a linearly-interpolated source sample for channel `src_ch` at the
/// fractional frame position `pos`, clamped to the buffer. Used for the direct
/// signal during the pitch-enable crossfade.
fn interpolated_source_sample(decoded: &DecodedBuffer, pos: f64, src_ch: usize) -> f32 {
    if decoded.frames == 0 {
        return 0.0;
    }
    let frame = (pos.floor() as usize).min(decoded.frames - 1);
    let frac = (pos - frame as f64) as f32;
    let a = decoded.data[frame * decoded.channels + src_ch];
    if frame + 1 < decoded.frames && frac > 0.0 {
        let b = decoded.data[(frame + 1) * decoded.channels + src_ch];
        a * (1.0 - frac) + b * frac
    } else {
        a
    }
}

/// Apply per-frame volume, fade, ducking and channel mapping for one block of
/// already-stretched interleaved samples, mixing the result into `output`.
///
/// While a pitch-enable crossfade is active, the direct (un-stretched) source is
/// faded out (read from `decoded` at the sample's direct cursor, advancing by
/// `speed`) and the stretched signal faded in, with an equal-power curve, so the
/// switch from direct to stretched playback has no click (F10).
#[allow(clippy::too_many_arguments)]
fn apply_pitch_block(
    sample: &mut ActiveSample,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
    src_channels: usize,
    duck: (f32, f32),
    stretched: &[f32],
    decoded: &DecodedBuffer,
    speed: f32,
) {
    let sample_volume = sample.volume;
    for frame_idx in 0..frames {
        // Advance voice volume toward target (smooth ramping to avoid pops)
        sample.advance_voice_volume();

        // Calculate fade multiplier for this frame
        let fade_multiplier = sample.fade_state.multiplier();
        let ducking_multiplier = duck_frame_gain(duck, frame_idx, frames);
        let base_volume = sample_volume * sample.voice_volume;
        let final_volume = base_volume * fade_multiplier * ducking_multiplier;

        // Equal-power crossfade gains for this frame. Outside the crossfade window
        // the stretched signal passes at unity and the direct contribution is zero.
        let (gain_stretched, gain_direct, direct_pos) = if sample.pitch_crossfade_remaining > 0 {
            let done = sample.pitch_crossfade_total - sample.pitch_crossfade_remaining;
            let progress = done as f32 / sample.pitch_crossfade_total as f32;
            let angle = progress * std::f32::consts::FRAC_PI_2;
            let pos = sample.pitch_crossfade_direct_pos;
            sample.pitch_crossfade_direct_pos += speed as f64;
            sample.pitch_crossfade_remaining -= 1;
            (angle.sin(), angle.cos(), Some(pos))
        } else {
            (1.0, 0.0, None)
        };

        // Apply channel mapping and mix into output
        for (route_idx, &(src_ch, dest_ch)) in sample.channel_map.iter().enumerate() {
            // Bounds check
            if src_ch >= src_channels || dest_ch >= output_channels {
                continue;
            }

            let route_gain = sample.channel_route_gain(route_idx);
            let src_idx = frame_idx * src_channels + src_ch;
            let dest_idx = frame_idx * output_channels + dest_ch;

            if src_idx < stretched.len() {
                let mut value = stretched[src_idx] * gain_stretched;
                if let Some(pos) = direct_pos {
                    value += interpolated_source_sample(decoded, pos, src_ch) * gain_direct;
                }
                output[dest_idx] += value * final_volume * route_gain;
            }
        }

        // Advance fade state
        sample.fade_state.advance();
    }
}

/// Number of frames over which a ring voice fades to silence when its ring buffer
/// underruns, instead of cutting hard to zero. Short (~1.3 ms at 48 kHz) so the
/// gap is barely audible, but long enough that the step per frame stays well below
/// a click (F3). The held frame is the last one read this block, faded out.
const UNDERRUN_FADE_FRAMES: usize = 64;

/// Consume one frame from a ring voice's `consumer` and mix it into `output` at
/// `frame_idx` through `channel_map`, scaled by `gain`. On an underrun (fewer than
/// `input_channels` samples queued) it does not cut to hard silence (an audible
/// click): it holds the last frame read this block (`last_frame`) and fades it
/// toward zero over `UNDERRUN_FADE_FRAMES` (`underrun_frames` carries the elapsed
/// count across the block), so the transition to silence has no hard step (F3).
///
/// Shared by the live-input and streamed-source mix paths; allocates nothing, never
/// blocks, and supports up to 16 input channels.
#[allow(clippy::too_many_arguments)]
fn mix_ring_voice_frame(
    consumer: &mut HeapConsumer<f32>,
    input_channels: usize,
    channel_map: &[(usize, usize)],
    output: &mut [f32],
    frame_idx: usize,
    output_channels: usize,
    gain: f32,
    last_frame: &mut [f32; 16],
    underrun_frames: &mut usize,
) {
    // Read one frame worth of samples, or — on underrun — hold the last frame and
    // fade it toward silence so there is no hard cut.
    let underrun = consumer.len() < input_channels;
    let fade_gain = if underrun {
        // Linear fade from the held frame to silence over UNDERRUN_FADE_FRAMES, then
        // flat silence. Before any frame was read this block last_frame is zero, so
        // this is silence with no step either way.
        let g = 1.0 - (*underrun_frames as f32 / UNDERRUN_FADE_FRAMES as f32);
        *underrun_frames += 1;
        g.max(0.0)
    } else {
        // Read the entire frame from the ring buffer and remember it as the frame to
        // hold should the next frame underrun.
        for slot in last_frame.iter_mut().take(input_channels.min(16)) {
            if let Some(sample) = consumer.pop() {
                *slot = sample;
            }
        }
        1.0
    };

    let final_gain = gain * fade_gain;
    for &(src_ch, dest_ch) in channel_map {
        if src_ch >= input_channels || dest_ch >= output_channels || src_ch >= 16 {
            continue;
        }
        let dest_idx = frame_idx * output_channels + dest_ch;
        output[dest_idx] += last_frame[src_ch] * final_gain;
    }
}

/// Mix a live input (microphone) into the output buffer.
///
/// The voice-volume ramp is advanced for every frame of the block — underrun frames
/// included — so it stays time-accurate rather than stalling at an underrun. The
/// per-frame ring consume and the hold-and-fade on underrun are in
/// [`mix_ring_voice_frame`] (F3).
fn mix_live_input_into_output(
    input: &mut LiveInput,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
    duck: (f32, f32),
) {
    let mut last_frame = [0.0f32; 16];
    let mut underrun_frames = 0usize;

    for frame_idx in 0..frames {
        // Advance voice volume toward target (smooth ramping to avoid pops). Done
        // every frame — including underrun frames — so the ramp stays time-accurate.
        input.advance_voice_volume();

        // input volume * voice volume * per-frame duck gain (D2); the underrun fade
        // gain is applied inside mix_ring_voice_frame.
        let ducking_multiplier = duck_frame_gain(duck, frame_idx, frames);
        let gain = input.volume * input.voice_volume * ducking_multiplier;

        mix_ring_voice_frame(
            &mut input.consumer,
            input.input_channels,
            &input.channel_map,
            output,
            frame_idx,
            output_channels,
            gain,
            &mut last_frame,
            &mut underrun_frames,
        );
    }
}

/// Mix a windowed streamed source into the output buffer.
///
/// Like the live-input path it consumes from a ring with a hold-and-fade on underrun
/// ([`mix_ring_voice_frame`]), but it also applies its fade state (fade-in on start,
/// fade-out on stop), advancing that fade once per frame, so a stopped cue ramps to
/// silence with no click.
fn mix_streamed_source_into_output(
    source: &mut StreamedSource,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
    duck: (f32, f32),
) {
    let mut last_frame = [0.0f32; 16];
    let mut underrun_frames = 0usize;

    for frame_idx in 0..frames {
        // Advance voice volume toward target every frame so the ramp stays
        // time-accurate across underruns.
        source.advance_voice_volume();

        // source volume * voice volume * per-frame duck gain (D2) * fade-in/out
        // multiplier; the underrun fade gain is applied inside mix_ring_voice_frame.
        let ducking_multiplier = duck_frame_gain(duck, frame_idx, frames);
        let fade_multiplier = source.fade_state.multiplier();
        let gain = source.volume * source.voice_volume * ducking_multiplier * fade_multiplier;

        mix_ring_voice_frame(
            &mut source.consumer,
            source.input_channels,
            &source.channel_map,
            output,
            frame_idx,
            output_channels,
            gain,
            &mut last_frame,
            &mut underrun_frames,
        );

        // Advance the fade once per frame (the duck fade is advanced once per buffer
        // in mix_audio; this fade is per-source, so it lives here).
        source.fade_state.advance();
    }

    // First-mix latency probe (Sprint 11, D50): published only when the block
    // popped at least one real frame from the ring (a fully underrun block is
    // silence, not a start).
    if underrun_frames < frames {
        source.publish_first_mix_if_started();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn create_test_buffer(frames: usize, channels: usize, value: f32) -> Arc<DecodedBuffer> {
        let data = vec![value; frames * channels];
        Arc::new(DecodedBuffer::new(data, channels, 48000))
    }

    const TEST_FILE: &str = "test.wav";

    #[test]
    fn test_single_sample_mixing() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20]; // 10 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // Should have mixed 0.5 into all channels
        for &s in &output {
            assert_eq!(s, 0.5);
        }
    }

    #[test]
    fn test_multiple_samples_additive() {
        let buffer1 = create_test_buffer(10, 2, 0.3);
        let buffer2 = create_test_buffer(10, 2, 0.4);

        let sample1 = ActiveSample::new(
            1,
            "test".to_string(),
            buffer1,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        let sample2 = ActiveSample::new(
            2,
            "test".to_string(),
            buffer2,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        let mut state = MixerState::new(2);
        state.active_samples.push(sample1);
        state.active_samples.push(sample2);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Should have added 0.3 + 0.4 ≈ 0.7 (with floating point tolerance)
        for &s in &output {
            assert!((s - 0.7).abs() < 0.0001);
        }
    }

    #[test]
    fn test_volume_control() {
        let buffer = create_test_buffer(10, 2, 1.0);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            0.5,
            1.0,
            TEST_FILE.to_string(),
        ); // 50% sample volume

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Should be 1.0 * 0.5 = 0.5
        for &s in &output {
            assert_eq!(s, 0.5);
        }
    }

    #[test]
    fn test_voice_volume() {
        let buffer = create_test_buffer(10, 2, 1.0);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            0.5,
            TEST_FILE.to_string(),
        ); // 50% voice volume

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Should be 1.0 * 1.0 * 0.5 = 0.5
        for &s in &output {
            assert_eq!(s, 0.5);
        }
    }

    #[test]
    fn test_combined_volume() {
        let buffer = create_test_buffer(10, 2, 1.0);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            0.5,
            0.4,
            TEST_FILE.to_string(),
        ); // 50% sample * 40% voice

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Should be 1.0 * 0.5 * 0.4 = 0.2
        for &s in &output {
            assert!((s - 0.2).abs() < 0.0001);
        }
    }

    #[test]
    fn test_play_volume_clamped_at_construction() {
        let buffer = create_test_buffer(10, 2, 1.0);

        // A Play with volume > 1.0 must be clamped to 1.0 at construction, matching
        // the runtime Volume command (D23/F4). All three constructors clamp.
        let s1 = ActiveSample::new(
            1,
            "v".to_string(),
            buffer.clone(),
            5.0,
            1.0,
            "f".to_string(),
        );
        assert_eq!(s1.volume, 1.0, "new() must clamp volume to 1.0");

        let s2 = ActiveSample::new_with_id(
            2,
            "v".to_string(),
            buffer.clone(),
            5.0,
            1.0,
            "f".to_string(),
            None,
            false,
            0,
        );
        assert_eq!(s2.volume, 1.0, "new_with_id() must clamp volume to 1.0");

        let s3 = ActiveSample::new_with_mapping(
            3,
            "v".to_string(),
            buffer,
            5.0,
            1.0,
            vec![(0, 0), (1, 1)],
            "f".to_string(),
            None,
            false,
            0,
        );
        assert_eq!(
            s3.volume, 1.0,
            "new_with_mapping() must clamp volume to 1.0"
        );
    }

    #[test]
    fn test_play_negative_volume_clamped_at_construction() {
        let buffer = create_test_buffer(10, 2, 1.0);
        let s = ActiveSample::new(1, "v".to_string(), buffer, -0.5, 1.0, "f".to_string());
        assert_eq!(s.volume, 0.0, "negative volume must clamp to 0.0");
    }

    #[test]
    fn test_saturation() {
        // Two 0.8 sources sum to 1.6. The soft-knee limiter holds the output at the
        // ceiling (the default -1.0 dBFS), not the old brickwall value of 1.0, and
        // records the over-ceiling event in the clip counter.
        let buffer1 = create_test_buffer(10, 2, 0.8);
        let buffer2 = create_test_buffer(10, 2, 0.8);

        let sample1 = ActiveSample::new(
            1,
            "test".to_string(),
            buffer1,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        let sample2 = ActiveSample::new(
            2,
            "test".to_string(),
            buffer2,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        let mut state = MixerState::new(2);
        state.active_samples.push(sample1);
        state.active_samples.push(sample2);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        let ceiling = state.output_ceiling;
        for &s in &output {
            assert!(
                s <= ceiling + 1e-6,
                "sample {s} must not exceed the ceiling"
            );
            assert!(s < 1.0, "soft limiter must not flat-top at 1.0, got {s}");
            // A constant 1.6 input asymptotes essentially onto the ceiling.
            assert!(
                (s - ceiling).abs() < 1e-3,
                "limited value {s} should sit at the ceiling {ceiling}"
            );
        }
        assert!(
            state.clip_count.load(Ordering::Relaxed) > 0,
            "clip counter must increment when the limiter acts"
        );
    }

    #[test]
    fn test_soft_limit_transfer_curve() {
        let ceiling = db_to_linear(-1.0);
        let threshold = ceiling * SOFT_KNEE_RATIO;

        // Transparent below the knee threshold.
        for &x in &[0.0f32, 0.1, 0.5, 0.7, 0.8, threshold] {
            assert_eq!(soft_limit(x, ceiling), x, "must be identity at {x}");
            assert_eq!(soft_limit(-x, ceiling), -x, "must be identity at -{x}");
        }

        // In the knee the curve compresses (output below the identity line) and
        // stays below the ceiling.
        let knee_input = (threshold + ceiling) / 2.0;
        let knee_output = soft_limit(knee_input, ceiling);
        assert!(
            knee_output < knee_input && knee_output > threshold,
            "knee output {knee_output} should compress {knee_input}"
        );
        assert!(
            knee_output < ceiling,
            "knee output must stay below the ceiling"
        );

        // Above the knee it asymptotes toward the ceiling and never exceeds it
        // (at f32 precision tanh saturates to 1.0, so the asymptote lands on the
        // ceiling rather than strictly below it).
        for &x in &[1.0f32, 1.6, 4.0, 100.0] {
            let y = soft_limit(x, ceiling);
            assert!(
                y <= ceiling,
                "soft_limit({x}) = {y} must not exceed {ceiling}"
            );
            assert!(
                y > threshold,
                "soft_limit({x}) = {y} should be near the ceiling"
            );
            assert_eq!(soft_limit(-x, ceiling), -y, "must be odd-symmetric at {x}");
        }
        // Monotonic: louder input maps to a higher (or equal) limited magnitude.
        assert!(soft_limit(1.6, ceiling) <= soft_limit(4.0, ceiling));
    }

    #[test]
    fn test_channel_mapping() {
        // Stereo buffer with different values per channel
        let data = vec![0.3, 0.7, 0.3, 0.7]; // 2 frames, 2 channels
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));

        // Map: L→1, R→3 (4-channel output)
        let channel_map = vec![(0, 1), (1, 3)];
        let sample = ActiveSample::new_with_mapping(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            channel_map,
            TEST_FILE.to_string(),
            None,
            false,
            0,
        );

        let mut state = MixerState::new(4);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 8]; // 2 frames * 4 channels
        mix_audio(&mut output, &mut state);

        // Frame 0
        assert_eq!(output[0], 0.0); // Channel 0: unmapped
        assert_eq!(output[1], 0.3); // Channel 1: L
        assert_eq!(output[2], 0.0); // Channel 2: unmapped
        assert_eq!(output[3], 0.7); // Channel 3: R

        // Frame 1
        assert_eq!(output[4], 0.0);
        assert_eq!(output[5], 0.3);
        assert_eq!(output[6], 0.0);
        assert_eq!(output[7], 0.7);
    }

    #[test]
    fn test_sample_completion() {
        let buffer = create_test_buffer(5, 2, 0.5);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        assert!(!sample.is_finished());

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 5 frames - should exactly finish
        let mut output = vec![0.0f32; 10];
        mix_audio(&mut output, &mut state);

        assert!(state.active_samples[0].is_finished());
    }

    #[test]
    fn test_partial_sample_playback() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        sample.position = 8; // Start near the end

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Request 5 frames, but only 2 remaining
        let mut output = vec![0.0f32; 10]; // 5 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // First 2 frames should have audio
        assert_eq!(output[0], 0.5);
        assert_eq!(output[1], 0.5);
        assert_eq!(output[2], 0.5);
        assert_eq!(output[3], 0.5);

        // Remaining should be silence
        for &s in &output[4..] {
            assert_eq!(s, 0.0);
        }
    }

    // === Loop Tests ===

    #[test]
    fn test_looping_sample_does_not_finish_at_end() {
        let buffer = create_test_buffer(5, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        sample.loop_mode = true;

        // Position at the end of buffer
        sample.position = 4;

        // A looping sample should not be marked as finished even at the end
        assert!(!sample.is_finished());

        // Move past the end
        sample.position = 5;
        assert!(!sample.is_finished());
    }

    #[test]
    fn test_looping_sample_wraps_forward() {
        // Create buffer with identifiable pattern: first frame = 0.1, last frame = 0.9
        let mut data = vec![0.1, 0.1]; // Frame 0
        data.extend(vec![0.2, 0.2]); // Frame 1
        data.extend(vec![0.3, 0.3]); // Frame 2
        data.extend(vec![0.4, 0.4]); // Frame 3
        data.extend(vec![0.5, 0.5]); // Frame 4
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));

        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        sample.loop_mode = true;
        sample.position = 3; // Start near end

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 4 frames - should wrap around to beginning
        let mut output = vec![0.0f32; 8]; // 4 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // Frame 0 from position 3 (0.4)
        assert!((output[0] - 0.4).abs() < 0.01);
        // Frame 1 from position 4 (0.5)
        assert!((output[2] - 0.5).abs() < 0.01);
        // Frame 2 from position 0 (wrapped, 0.1)
        assert!((output[4] - 0.1).abs() < 0.01);
        // Frame 3 from position 1 (0.2)
        assert!((output[6] - 0.2).abs() < 0.01);

        // Sample should not be finished
        assert!(!state.active_samples[0].is_finished());
    }

    #[test]
    fn test_looping_sample_finishes_on_fade_out() {
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        sample.loop_mode = true;

        // Not finished yet
        assert!(!sample.is_finished());

        // Apply a fade out that will complete immediately
        sample.fade_state = FadeState::Out {
            elapsed: 100,
            duration: 100,
        };

        // Now should be finished (fade out complete)
        assert!(sample.is_finished());
    }

    #[test]
    fn test_looping_reverse_wraps_to_end() {
        // Create buffer with identifiable pattern
        let mut data = vec![0.1, 0.1]; // Frame 0
        data.extend(vec![0.2, 0.2]); // Frame 1
        data.extend(vec![0.3, 0.3]); // Frame 2
        data.extend(vec![0.4, 0.4]); // Frame 3
        data.extend(vec![0.5, 0.5]); // Frame 4
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));

        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        sample.loop_mode = true;
        sample.speed = -1.0; // Reverse playback
        sample.position = 1; // Start near beginning

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 4 frames - should wrap around to end
        let mut output = vec![0.0f32; 8]; // 4 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // Frame 0 from position 1 (0.2)
        assert!((output[0] - 0.2).abs() < 0.01);
        // Frame 1 from position 0 (0.1)
        assert!((output[2] - 0.1).abs() < 0.01);
        // Frame 2 from position 4 (wrapped from end, 0.5)
        assert!((output[4] - 0.5).abs() < 0.01);
        // Frame 3 from position 3 (0.4)
        assert!((output[6] - 0.4).abs() < 0.01);

        // Sample should not be finished
        assert!(!state.active_samples[0].is_finished());
    }

    #[test]
    fn test_non_looping_sample_finishes_at_end() {
        let buffer = create_test_buffer(5, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        sample.loop_mode = false;

        sample.position = 5;
        assert!(sample.is_finished());
    }

    #[test]
    fn test_loop_mode_defaults_to_false() {
        let buffer = create_test_buffer(5, 2, 0.5);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        assert!(!sample.loop_mode);
    }

    #[test]
    fn test_mono_to_multichannel() {
        // Mono source to 8-channel output
        let data = vec![0.5; 10]; // 10 frames, 1 channel
        let buffer = Arc::new(DecodedBuffer::new(data, 1, 48000));

        // Route mono to channels 4 and 5
        let channel_map = vec![(0, 4), (0, 5)];
        let sample = ActiveSample::new_with_mapping(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            channel_map,
            TEST_FILE.to_string(),
            None,
            false,
            0,
        );

        let mut state = MixerState::new(8);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 80]; // 10 frames * 8 channels
        mix_audio(&mut output, &mut state);

        // Check first frame
        assert_eq!(output[0], 0.0); // Channel 0: silence
        assert_eq!(output[1], 0.0); // Channel 1: silence
        assert_eq!(output[2], 0.0); // Channel 2: silence
        assert_eq!(output[3], 0.0); // Channel 3: silence
        assert_eq!(output[4], 0.5); // Channel 4: mono signal
        assert_eq!(output[5], 0.5); // Channel 5: mono signal
        assert_eq!(output[6], 0.0); // Channel 6: silence
        assert_eq!(output[7], 0.0); // Channel 7: silence

        // Check second frame
        assert_eq!(output[12], 0.5); // Frame 1, Channel 4
        assert_eq!(output[13], 0.5); // Frame 1, Channel 5
    }

    #[test]
    fn test_quad_to_stereo_downmix() {
        // 4-channel source to stereo output (channels 0 and 1)
        let mut data = Vec::new();
        for _ in 0..10 {
            // 10 frames
            data.push(0.1); // Front left
            data.push(0.2); // Front right
            data.push(0.3); // Rear left
            data.push(0.4); // Rear right
        }
        let buffer = Arc::new(DecodedBuffer::new(data, 4, 48000));

        // Mix surround channels to stereo
        let channel_map = vec![
            (0, 0), // Front left → Left
            (1, 1), // Front right → Right
            (2, 0), // Rear left → Left (mix)
            (3, 1), // Rear right → Right (mix)
        ];
        let sample = ActiveSample::new_with_mapping(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            channel_map,
            TEST_FILE.to_string(),
            None,
            false,
            0,
        );

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20]; // 10 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // First frame: L should be 0.1 + 0.3 = 0.4, R should be 0.2 + 0.4 = 0.6
        assert!((output[0] - 0.4).abs() < 0.0001);
        assert!((output[1] - 0.6).abs() < 0.0001);
    }

    #[test]
    fn test_quad_to_stereo_downmix_with_route_gains() {
        // D29: per-route gains let a downmix be attenuated so summing N source
        // channels into one destination does not clip. Same quad->stereo map as
        // test_quad_to_stereo_downmix, but each route is scaled by 0.5, halving the
        // summed contribution: L = (0.1 + 0.3) * 0.5 = 0.2, R = (0.2 + 0.4) * 0.5 = 0.3.
        let mut data = Vec::new();
        for _ in 0..10 {
            data.push(0.1); // Front left
            data.push(0.2); // Front right
            data.push(0.3); // Rear left
            data.push(0.4); // Rear right
        }
        let buffer = Arc::new(DecodedBuffer::new(data, 4, 48000));

        let channel_map = vec![(0, 0), (1, 1), (2, 0), (3, 1)];
        let mut sample = ActiveSample::new_with_mapping(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            channel_map,
            TEST_FILE.to_string(),
            None,
            false,
            0,
        );
        sample.set_channel_route_gains(vec![0.5, 0.5, 0.5, 0.5]);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        assert!((output[0] - 0.2).abs() < 0.0001, "L got {}", output[0]);
        assert!((output[1] - 0.3).abs() < 0.0001, "R got {}", output[1]);
    }

    #[test]
    fn test_channel_route_gains_default_to_unity() {
        // Without explicit route gains the downmix must behave exactly as before
        // (each route at unity) — D29 must not change existing 1:1/sum routing.
        let buffer = create_test_buffer(4, 2, 0.5);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        // No route gains set: the lookup yields unity for every route.
        assert_eq!(sample.channel_route_gain(0), 1.0);
        assert_eq!(sample.channel_route_gain(7), 1.0);
    }

    #[test]
    fn test_sparse_channel_routing() {
        // Stereo to channels 6 and 9 (leaving gaps)
        let data = vec![0.3, 0.7, 0.3, 0.7]; // 2 frames, 2 channels
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));

        let channel_map = vec![(0, 6), (1, 9)];
        let sample = ActiveSample::new_with_mapping(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            channel_map,
            TEST_FILE.to_string(),
            None,
            false,
            0,
        );

        let mut state = MixerState::new(12);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 24]; // 2 frames * 12 channels
        mix_audio(&mut output, &mut state);

        // Frame 0
        for ch in 0..12 {
            let idx = ch;
            if ch == 6 {
                assert_eq!(output[idx], 0.3);
            } else if ch == 9 {
                assert_eq!(output[idx], 0.7);
            } else {
                assert_eq!(output[idx], 0.0);
            }
        }
    }

    #[test]
    fn test_out_of_bounds_channel_routing() {
        // Test that out-of-bounds channel mappings are safely ignored
        let data = vec![0.5, 0.5, 0.5, 0.5]; // 2 frames, 2 channels
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));

        // Map to invalid destinations
        let channel_map = vec![
            (0, 0),   // Valid
            (1, 100), // Out of bounds (only 4 output channels)
            (99, 2),  // Invalid source
        ];
        let sample = ActiveSample::new_with_mapping(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            channel_map,
            TEST_FILE.to_string(),
            None,
            false,
            0,
        );

        let mut state = MixerState::new(4);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 8]; // 2 frames * 4 channels
        mix_audio(&mut output, &mut state);

        // Only channel 0 should have audio (the valid mapping)
        assert_eq!(output[0], 0.5); // Frame 0, Ch 0
        assert_eq!(output[1], 0.0); // Frame 0, Ch 1
        assert_eq!(output[2], 0.0); // Frame 0, Ch 2
        assert_eq!(output[3], 0.0); // Frame 0, Ch 3
    }

    // === Fade Tests ===

    #[test]
    fn test_fade_state_creation() {
        let fade_in = FadeState::fade_in(1000, 48000); // 1 second at 48kHz
        assert_eq!(
            fade_in,
            FadeState::In {
                elapsed: 0,
                duration: 48000
            }
        );

        let fade_out = FadeState::fade_out(500, 44100); // 0.5 seconds at 44.1kHz
        assert_eq!(
            fade_out,
            FadeState::Out {
                elapsed: 0,
                duration: 22050
            }
        );
    }

    #[test]
    fn test_fade_in_multiplier() {
        // At start: 0/10 = 0.0
        let fade_start = FadeState::In {
            elapsed: 0,
            duration: 10,
        };
        assert_eq!(fade_start.multiplier(), 0.0);

        // At 30%: 3/10 = 0.3
        let fade_30 = FadeState::In {
            elapsed: 3,
            duration: 10,
        };
        assert!((fade_30.multiplier() - 0.3).abs() < 0.01);

        // At 50%: 5/10 = 0.5
        let fade_50 = FadeState::In {
            elapsed: 5,
            duration: 10,
        };
        assert_eq!(fade_50.multiplier(), 0.5);

        // At 100%: 10/10 = 1.0
        let fade_100 = FadeState::In {
            elapsed: 10,
            duration: 10,
        };
        assert_eq!(fade_100.multiplier(), 1.0);

        // Beyond 100%: clamped to 1.0
        let fade_over = FadeState::In {
            elapsed: 15,
            duration: 10,
        };
        assert_eq!(fade_over.multiplier(), 1.0);
    }

    #[test]
    fn test_fade_out_multiplier() {
        // At start: 1 - (0/10) = 1.0
        let fade_start = FadeState::Out {
            elapsed: 0,
            duration: 10,
        };
        assert_eq!(fade_start.multiplier(), 1.0);

        // At 30%: 1 - (3/10) = 0.7
        let fade_30 = FadeState::Out {
            elapsed: 3,
            duration: 10,
        };
        assert!((fade_30.multiplier() - 0.7).abs() < 0.01);

        // At 50%: 1 - (5/10) = 0.5
        let fade_50 = FadeState::Out {
            elapsed: 5,
            duration: 10,
        };
        assert_eq!(fade_50.multiplier(), 0.5);

        // At 100%: 1 - (10/10) = 0.0
        let fade_100 = FadeState::Out {
            elapsed: 10,
            duration: 10,
        };
        assert_eq!(fade_100.multiplier(), 0.0);

        // Beyond 100%: clamped to 0.0
        let fade_over = FadeState::Out {
            elapsed: 15,
            duration: 10,
        };
        assert_eq!(fade_over.multiplier(), 0.0);
    }

    #[test]
    fn test_fade_state_advance() {
        let mut fade = FadeState::In {
            elapsed: 0,
            duration: 5,
        };

        assert!(!fade.is_complete());
        fade.advance();
        assert_eq!(
            fade,
            FadeState::In {
                elapsed: 1,
                duration: 5
            }
        );

        for _ in 0..4 {
            fade.advance();
        }
        assert_eq!(
            fade,
            FadeState::In {
                elapsed: 5,
                duration: 5
            }
        );
        assert!(fade.is_complete());

        // Advancing past completion stays at max
        fade.advance();
        assert_eq!(
            fade,
            FadeState::In {
                elapsed: 5,
                duration: 5
            }
        );
    }

    #[test]
    fn test_fade_in_mixing() {
        // Create a 10-frame buffer at 48kHz
        let buffer = create_test_buffer(10, 2, 1.0);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Set 10-frame fade in (one frame per iteration)
        sample.set_fade(FadeState::In {
            elapsed: 0,
            duration: 10,
        });

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Process 10 frames
        let mut output = vec![0.0f32; 20]; // 10 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // First frame should be silent (0/10 = 0.0)
        assert_eq!(output[0], 0.0);
        assert_eq!(output[1], 0.0);

        // Frame at position 5 should be ~0.5 (5/10 = 0.5)
        let frame5_l = output[10]; // Frame 5, channel 0
        let frame5_r = output[11]; // Frame 5, channel 1
        assert!((frame5_l - 0.5).abs() < 0.1);
        assert!((frame5_r - 0.5).abs() < 0.1);

        // Last frame should be close to 1.0 (9/10 = 0.9)
        let last_l = output[18]; // Frame 9, channel 0
        let last_r = output[19]; // Frame 9, channel 1
        assert!((last_l - 0.9).abs() < 0.1);
        assert!((last_r - 0.9).abs() < 0.1);
    }

    #[test]
    fn test_fade_out_mixing() {
        // Create a 10-frame buffer
        let buffer = create_test_buffer(10, 2, 1.0);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Set 10-frame fade out
        sample.set_fade(FadeState::Out {
            elapsed: 0,
            duration: 10,
        });

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 20]; // 10 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // First frame is at the fade's full level (1 - 0/10 = 1.0); a full-scale
        // sample is held at the limiter ceiling rather than passed at 1.0.
        let ceiling = state.output_ceiling;
        assert!(output[0] <= ceiling + 1e-6 && output[0] > 0.85);
        assert!(output[1] <= ceiling + 1e-6 && output[1] > 0.85);
        assert_eq!(output[0], output[1]);

        // Frame at position 5 should be ~0.5 (1 - 5/10 = 0.5); below the knee, the
        // limiter is transparent.
        let frame5_l = output[10];
        let frame5_r = output[11];
        assert!((frame5_l - 0.5).abs() < 0.1);
        assert!((frame5_r - 0.5).abs() < 0.1);

        // Last frame should be close to 0.1 (1 - 9/10 = 0.1)
        let last_l = output[18];
        let last_r = output[19];
        assert!((last_l - 0.1).abs() < 0.1);
        assert!((last_r - 0.1).abs() < 0.1);
    }

    #[test]
    fn test_fade_out_completes_sample() {
        // Create a long buffer
        let buffer = create_test_buffer(100, 2, 1.0);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Set short fade out
        sample.set_fade(FadeState::Out {
            elapsed: 0,
            duration: 5,
        });

        assert!(!sample.is_finished());

        // Manually advance to completion
        for _ in 0..5 {
            sample.fade_state.advance();
        }

        // Sample should be marked as finished even though buffer has more frames
        assert!(sample.is_finished());
    }

    #[test]
    fn test_zero_duration_fade() {
        let fade_in = FadeState::In {
            elapsed: 0,
            duration: 0,
        };
        assert_eq!(fade_in.multiplier(), 1.0); // Instant full volume

        let fade_out = FadeState::Out {
            elapsed: 0,
            duration: 0,
        };
        assert_eq!(fade_out.multiplier(), 0.0); // Instant silence
    }

    // === LiveInput Tests ===

    fn create_test_ring_buffer_with_data(data: &[f32]) -> HeapConsumer<f32> {
        use ringbuf::HeapRb;
        let rb = HeapRb::<f32>::new(data.len() + 100);
        let (mut producer, consumer) = rb.split();
        producer.push_slice(data);
        consumer
    }

    #[test]
    fn test_live_input_basic_mixing() {
        // Create ring buffer with stereo data (10 frames)
        let data: Vec<f32> = (0..20)
            .map(|i| if i % 2 == 0 { 0.5 } else { 0.3 })
            .collect();
        let consumer = create_test_ring_buffer_with_data(&data);

        let live_input = LiveInput::new(
            "mic".to_string(),
            consumer,
            2, // stereo input
            1.0,
            vec![(0, 0), (1, 1)], // 1:1 mapping
        );

        let mut state = MixerState::new(2);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 20]; // 10 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // Check first frame
        assert_eq!(output[0], 0.5); // Left
        assert_eq!(output[1], 0.3); // Right

        // Check last frame
        assert_eq!(output[18], 0.5);
        assert_eq!(output[19], 0.3);
    }

    #[test]
    fn test_live_input_volume() {
        // Create ring buffer with mono data
        let data = vec![1.0f32; 10];
        let consumer = create_test_ring_buffer_with_data(&data);

        let live_input = LiveInput::new(
            "mic".to_string(),
            consumer,
            1,   // mono input
            0.5, // 50% volume
            vec![(0, 0)],
        );

        let mut state = MixerState::new(1);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 10]; // 10 frames * 1 channel
        mix_audio(&mut output, &mut state);

        for &s in &output {
            assert_eq!(s, 0.5); // 1.0 * 0.5 volume
        }
    }

    #[test]
    fn test_live_input_channel_routing() {
        // Create ring buffer with mono data
        let data = vec![0.7f32; 10];
        let consumer = create_test_ring_buffer_with_data(&data);

        // Route mono input to channels 2 and 3 (4-channel output)
        let live_input = LiveInput::new("mic".to_string(), consumer, 1, 1.0, vec![(0, 2), (0, 3)]);

        let mut state = MixerState::new(4);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 40]; // 10 frames * 4 channels
        mix_audio(&mut output, &mut state);

        // Check first frame
        assert_eq!(output[0], 0.0); // Channel 0
        assert_eq!(output[1], 0.0); // Channel 1
        assert_eq!(output[2], 0.7); // Channel 2
        assert_eq!(output[3], 0.7); // Channel 3
    }

    #[test]
    fn test_live_input_underrun() {
        // Create ring buffer with only 5 frames of data but request 10
        let data = vec![0.8f32; 10]; // 5 stereo frames
        let consumer = create_test_ring_buffer_with_data(&data);

        let live_input = LiveInput::new("mic".to_string(), consumer, 2, 1.0, vec![(0, 0), (1, 1)]);

        let mut state = MixerState::new(2);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 20]; // 10 frames requested
        mix_audio(&mut output, &mut state);

        // First 5 frames are the real samples at full level.
        for &s in &output[..10] {
            assert_eq!(s, 0.8);
        }

        // On underrun the input holds the last frame and fades it toward silence
        // over UNDERRUN_FADE_FRAMES rather than cutting hard to zero (F3). The
        // remaining 5 frames are therefore a gentle decay of the held 0.8, each a
        // small step below the previous one — never a hard 0.8 -> 0.0 cut.
        let mut prev = 0.8f32;
        for frame in 5..10 {
            let expected = 0.8 * (1.0 - (frame - 5) as f32 / UNDERRUN_FADE_FRAMES as f32);
            for ch in 0..2 {
                let s = output[frame * 2 + ch];
                assert!(
                    (s - expected).abs() < 1e-6,
                    "underrun frame {frame} ch {ch}: expected faded {expected}, got {s}"
                );
                assert!(
                    s <= prev + 1e-6 && (prev - s) < 0.05,
                    "underrun must decay smoothly, not cut: {prev} -> {s}"
                );
            }
            prev = expected;
        }
    }

    #[test]
    fn underrun_fades_to_silence_without_click_and_keeps_ramp_time_accurate() {
        // F3: when the ring underruns partway through a block, the input must fade
        // to silence (no hard cut from the last sample to zero) AND keep advancing
        // its voice-volume ramp for the silent frames, so the ramp stays
        // time-accurate (it must not stall at the underrun like the old `break` did).
        const FRAMES: usize = 256;
        const REAL_FRAMES: usize = 64; // ring underruns after this many frames
        const CHANNELS: usize = 2;

        // A constant-amplitude input, so the ONLY discontinuity in the rendered
        // output is at the underrun boundary (no waveform shape to confound the
        // click probe).
        let data = vec![0.8f32; REAL_FRAMES * CHANNELS];
        let consumer = create_test_ring_buffer_with_data(&data);
        let mut live_input = LiveInput::new(
            "mic".to_string(),
            consumer,
            CHANNELS,
            1.0,
            vec![(0, 0), (1, 1)],
        );
        // A voice-volume ramp in progress across the whole block.
        live_input.voice_volume = 0.0;
        live_input.target_voice_volume = 1.0;

        let mut state = MixerState::new(CHANNELS);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; FRAMES * CHANNELS];
        mix_audio(&mut output, &mut state);

        // Largest absolute step between consecutive samples of channel 0 — a simple
        // click probe (computed inline; the shared render-harness helper is only in
        // the library crate's module tree, not the binary's).
        let max_inter_sample_delta = |buf: &[f32]| {
            let mut prev: Option<f32> = None;
            let mut m = 0.0f32;
            for &v in buf.iter().step_by(CHANNELS) {
                if let Some(p) = prev {
                    m = m.max((v - p).abs());
                }
                prev = Some(v);
            }
            m
        };

        // (a) No hard discontinuity anywhere, including at the underrun boundary.
        // The old hard `break` cut from ~0.8 straight to 0.0 (a ~0.8 step); a fade
        // keeps every step well under a click threshold.
        let delta = max_inter_sample_delta(&output);
        assert!(
            delta < 0.05,
            "underrun produced an audible discontinuity: max_inter_sample_delta={delta}"
        );

        // The tail must actually reach silence (the held frame decays to zero, it
        // does not hold the last sample forever).
        for &s in &output[(FRAMES - 2) * CHANNELS..] {
            assert!(s.abs() < 1e-6, "underrun tail did not reach silence: {s}");
        }

        // (b) The voice-volume ramp advanced for EVERY frame of the block, silent
        // frames included — matching a full-block ramp, not one that stalled at the
        // underrun. Compute the full-block expectation with an independent ramp.
        let mut reference = LiveInput::new(
            "ref".to_string(),
            {
                use ringbuf::HeapRb;
                HeapRb::<f32>::new(1).split().1
            },
            1,
            1.0,
            vec![],
        );
        reference.voice_volume = 0.0;
        reference.target_voice_volume = 1.0;
        for _ in 0..FRAMES {
            reference.advance_voice_volume();
        }
        let actual = state.live_inputs[0].voice_volume;
        assert!(
            (actual - reference.voice_volume).abs() < 1e-6,
            "ramp desynced from real time across the underrun: got {actual}, \
             full-block expected {}",
            reference.voice_volume
        );
    }

    #[test]
    fn test_live_input_mixed_with_samples() {
        // Create a sample and a live input, verify they mix together
        let buffer = create_test_buffer(10, 2, 0.3);
        let sample = ActiveSample::new(
            1,
            "sfx".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        let data = vec![0.4f32; 20]; // 10 stereo frames
        let consumer = create_test_ring_buffer_with_data(&data);

        let live_input = LiveInput::new("mic".to_string(), consumer, 2, 1.0, vec![(0, 0), (1, 1)]);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Should be 0.3 (sample) + 0.4 (input) = 0.7
        for &s in &output {
            assert!((s - 0.7).abs() < 0.0001);
        }
    }

    #[test]
    fn test_live_input_voice_volume() {
        let data = vec![1.0f32; 10];
        let consumer = create_test_ring_buffer_with_data(&data);

        let mut live_input = LiveInput::new(
            "mic".to_string(),
            consumer,
            1,
            0.5, // 50% input volume
            vec![(0, 0)],
        );
        // Set both current and target volume to avoid ramping during test
        live_input.voice_volume = 0.6; // 60% voice volume
        live_input.target_voice_volume = 0.6;

        let mut state = MixerState::new(1);
        state.live_inputs.push(live_input);

        let mut output = vec![0.0f32; 10];
        mix_audio(&mut output, &mut state);

        // Combined: 1.0 * 0.5 * 0.6 = 0.3
        for &s in &output {
            assert!((s - 0.3).abs() < 0.0001);
        }
    }

    #[test]
    fn test_multiple_live_inputs() {
        // Two microphones mixing to same outputs
        let data1 = vec![0.2f32; 10];
        let consumer1 = create_test_ring_buffer_with_data(&data1);
        let live_input1 = LiveInput::new("mic1".to_string(), consumer1, 1, 1.0, vec![(0, 0)]);

        let data2 = vec![0.3f32; 10];
        let consumer2 = create_test_ring_buffer_with_data(&data2);
        let live_input2 = LiveInput::new("mic2".to_string(), consumer2, 1, 1.0, vec![(0, 0)]);

        let mut state = MixerState::new(1);
        state.live_inputs.push(live_input1);
        state.live_inputs.push(live_input2);

        let mut output = vec![0.0f32; 10];
        mix_audio(&mut output, &mut state);

        // Both inputs mix additively: 0.2 + 0.3 = 0.5
        for &s in &output {
            assert!((s - 0.5).abs() < 0.0001);
        }
    }

    // === Sample Identification Tests ===

    #[test]
    fn test_sample_stores_file_path() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            "/path/to/audio.wav".to_string(),
        );

        assert_eq!(sample.file_path, "/path/to/audio.wav");
    }

    #[test]
    fn test_sample_with_user_id() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new_with_id(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            "/path/to/audio.wav".to_string(),
            Some("my-sound-1".to_string()),
            false,
            0,
        );

        assert_eq!(sample.sample_id, Some("my-sound-1".to_string()));
        assert_eq!(sample.file_path, "/path/to/audio.wav");
    }

    #[test]
    fn test_sample_without_user_id() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            "/path/to/audio.wav".to_string(),
        );

        assert_eq!(sample.sample_id, None);
    }

    #[test]
    fn test_sample_with_mapping_stores_file_path() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new_with_mapping(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            vec![(0, 0), (1, 1)],
            "/path/to/audio.wav".to_string(),
            None,
            false,
            0,
        );

        assert_eq!(sample.file_path, "/path/to/audio.wav");
        assert_eq!(sample.sample_id, None);
    }

    #[test]
    fn test_sample_with_mapping_and_id() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new_with_mapping(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            vec![(0, 2), (1, 3)],
            "/path/to/surround.wav".to_string(),
            Some("surround-effect".to_string()),
            false,
            0,
        );

        assert_eq!(sample.file_path, "/path/to/surround.wav");
        assert_eq!(sample.sample_id, Some("surround-effect".to_string()));
        assert_eq!(sample.channel_map, vec![(0, 2), (1, 3)]);
    }

    // === Seek Tests ===

    #[test]
    fn test_seek_to_position() {
        // Create a buffer at 48000 Hz with 48000 frames (1 second)
        let data = vec![0.5f32; 48000 * 2]; // 1 second stereo
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Initial position is 0
        assert_eq!(sample.position, 0);

        // Seek to 500ms (should be frame 24000)
        let position_ms: u64 = 500;
        let target_frame = ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
        sample.position = target_frame;

        assert_eq!(sample.position, 24000);
    }

    #[test]
    fn test_seek_to_start() {
        let data = vec![0.5f32; 48000 * 2];
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Advance position
        sample.position = 10000;

        // Seek to 0ms
        let position_ms: u64 = 0;
        let target_frame = ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
        sample.position = target_frame;

        assert_eq!(sample.position, 0);
    }

    #[test]
    fn test_seek_clamps_to_buffer_end() {
        let data = vec![0.5f32; 48000 * 2]; // 1 second stereo at 48kHz
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Try to seek to 2 seconds (beyond buffer)
        let position_ms: u64 = 2000;
        let target_frame = ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
        sample.position = target_frame.min(sample.buffer.frames().saturating_sub(1));

        // Should be clamped to last valid frame
        assert_eq!(sample.position, 47999); // frames - 1
    }

    #[test]
    fn test_seek_with_different_sample_rates() {
        // Test seek calculation at 44100 Hz
        let data = vec![0.5f32; 44100 * 2]; // 1 second stereo at 44.1kHz
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 44100));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Seek to 1000ms (should be frame 44100)
        let position_ms: u64 = 1000;
        let target_frame = ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
        sample.position = target_frame.min(sample.buffer.frames().saturating_sub(1));

        // At 44100 Hz, 1000ms = 44100 frames, but clamped to 44099
        assert_eq!(sample.position, 44099);
    }

    #[test]
    fn test_seek_preserves_playback_state() {
        let data = vec![0.5f32; 48000 * 2];
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            0.7,
            0.8,
            TEST_FILE.to_string(),
        );
        sample.fade_state = FadeState::In {
            elapsed: 100,
            duration: 200,
        };

        // Seek to 250ms
        let position_ms: u64 = 250;
        let target_frame = ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
        sample.position = target_frame;

        // Volume and fade state should be preserved
        assert_eq!(sample.volume, 0.7);
        assert_eq!(sample.voice_volume, 0.8);
        assert!(matches!(
            sample.fade_state,
            FadeState::In {
                elapsed: 100,
                duration: 200
            }
        ));
        assert_eq!(sample.position, 12000); // 250ms at 48kHz
    }

    #[test]
    fn test_seek_then_mix() {
        // Create buffer with distinct values at different positions
        let mut data = vec![0.0f32; 100 * 2]; // 100 stereo frames
                                              // First 50 frames: 0.1
        data[..100].fill(0.1);
        // Last 50 frames: 0.9
        data[100..200].fill(0.9);
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Seek to frame 50 (the start of the 0.9 section)
        sample.position = 50;

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 10]; // 5 stereo frames
        mix_audio(&mut output, &mut state);

        // Seek landed in the loud (0.9) half, not the quiet (0.1) half. 0.9 is above
        // the limiter knee, so it reads back limited toward the ceiling but stays
        // clearly in the loud region (well above the 0.1 section).
        let ceiling = state.output_ceiling;
        for &s in &output {
            assert!(
                s > 0.5 && s <= ceiling + 1e-6,
                "seek should land in the loud region, got {s}"
            );
        }
    }

    // === Speed Control Tests ===

    #[test]
    fn test_speed_default_is_normal() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        assert_eq!(sample.speed, 1.0);
    }

    #[test]
    fn test_speed_double_plays_twice_as_fast() {
        // Create buffer with 100 frames of constant value
        let data = vec![0.5f32; 100 * 2]; // 100 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        sample.speed = 2.0;

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Request 50 output frames at 2x speed should consume 100 input frames
        let mut output = vec![0.0f32; 50 * 2];
        mix_audio(&mut output, &mut state);

        // After mixing 50 output frames at 2x, effective position should be 100
        // (meaning sample should be finished)
        assert!(state.active_samples[0].is_finished());
    }

    #[test]
    fn test_speed_half_plays_twice_as_slow() {
        let data = vec![0.5f32; 100 * 2];
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        sample.speed = 0.5;

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Request 50 output frames at 0.5x speed should consume 25 input frames
        let mut output = vec![0.0f32; 50 * 2];
        mix_audio(&mut output, &mut state);

        // After mixing 50 output frames at 0.5x, effective position should be 25
        // Sample should NOT be finished (only 1/4 through)
        assert!(!state.active_samples[0].is_finished());
        // Check position is roughly 25
        assert_eq!(state.active_samples[0].position, 25);
    }

    #[test]
    fn test_speed_interpolation() {
        // Create buffer with linear ramp: 0.0, 0.2, 0.4, 0.6, 0.8, 1.0
        let data = vec![0.0, 0.0, 0.2, 0.2, 0.4, 0.4, 0.6, 0.6, 0.8, 0.8, 1.0, 1.0]; // 6 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );
        sample.speed = 0.5;

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // At 0.5x speed, reading 2 output frames should cover 1 input frame
        // and we should see interpolated values
        let mut output = vec![0.0f32; 4]; // 2 stereo frames
        mix_audio(&mut output, &mut state);

        // First output frame: position 0 -> value 0.0
        assert!((output[0] - 0.0).abs() < 0.01);
        // Second output frame: position 0.5 -> interpolate between 0.0 and 0.2 = 0.1
        assert!((output[2] - 0.1).abs() < 0.01);
    }

    #[test]
    fn test_cubic_interpolate_reproduces_a_linear_ramp() {
        // Catmull-Rom is exact for inputs up to cubic, so on a straight line through
        // the four taps it must return the same line — this is what preserves the
        // ramp identities the F9 tests rely on (D27).
        let line = |x: f32| 0.3 + 0.7 * x; // p_k = line(k)
        let (p0, p1, p2, p3) = (line(-1.0), line(0.0), line(1.0), line(2.0));
        for &t in &[0.0f32, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
            let got = cubic_interpolate(p0, p1, p2, p3, t);
            let expected = line(t); // the value on the segment p1..p2 at fraction t
            assert!(
                (got - expected).abs() < 1e-6,
                "cubic must reproduce the line at t={t}: got {got}, expected {expected}"
            );
        }
    }

    #[test]
    fn test_cubic_taps_extrapolate_at_non_loop_edges() {
        // Non-looping: a missing outer tap is linearly extrapolated from the two
        // in-range center taps (not edge-clamped), so a ramp stays straight at the
        // buffer boundary. Buffer is the ramp [0, 1, 2, 3] (value == frame).
        let frames = 4;
        let read = |i: usize| i as f32;

        // First segment (n=0): p0 has no real frame; extrapolate to -1 so the four
        // taps are the straight line [-1, 0, 1, 2].
        let (p0, p1, p2, p3) = cubic_taps(0, frames, false, read);
        assert_eq!((p0, p1, p2, p3), (-1.0, 0.0, 1.0, 2.0));

        // Last segment (n=2): p3 has no real frame; extrapolate to 4 -> [1, 2, 3, 4].
        let (p0, p1, p2, p3) = cubic_taps(2, frames, false, read);
        assert_eq!((p0, p1, p2, p3), (1.0, 2.0, 3.0, 4.0));
    }

    #[test]
    fn test_cubic_taps_wrap_under_loop() {
        // Looping: outer taps wrap around the buffer (periodic content). Buffer
        // [0, 1, 2, 3]; at n=0 the tap before frame 0 wraps to the last frame (3),
        // and at n=3 the taps past the end wrap to 0 and 1.
        let frames = 4;
        let read = |i: usize| i as f32;

        let (p0, p1, p2, p3) = cubic_taps(0, frames, true, read);
        assert_eq!((p0, p1, p2, p3), (3.0, 0.0, 1.0, 2.0));

        let (p0, p1, p2, p3) = cubic_taps(3, frames, true, read);
        assert_eq!((p0, p1, p2, p3), (2.0, 3.0, 0.0, 1.0));
    }

    #[test]
    fn test_speed_set_via_method() {
        let buffer = create_test_buffer(10, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Without pitch correction: -100 to 100 range
        sample.set_speed(1.5);
        assert_eq!(sample.speed, 1.5);

        sample.set_speed(0.0); // Should clamp to minimum (0.01)
        assert!((sample.speed - 0.01).abs() < 0.001);

        sample.set_speed(150.0); // Should clamp to 100.0
        assert!((sample.speed - 100.0).abs() < 0.001);

        sample.set_speed(-50.0); // Negative for reverse
        assert_eq!(sample.speed, -50.0);

        sample.set_speed(-150.0); // Should clamp to -100.0
        assert!((sample.speed - (-100.0)).abs() < 0.001);
    }

    // === Pitch Correction Tests ===

    #[test]
    fn test_pitch_correction_enable_disable() {
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        assert!(!sample.has_pitch_correction());

        sample.enable_pitch_correction();
        assert!(sample.has_pitch_correction());

        sample.disable_pitch_correction();
        assert!(!sample.has_pitch_correction());
    }

    #[test]
    fn test_pitch_correction_updates_speed() {
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        sample.set_speed(2.0);
        sample.enable_pitch_correction();

        // Verify the pitch corrector was created with the current speed
        assert!(sample.pitch_corrector.is_some());
        if let Some(ref pc) = sample.pitch_corrector {
            assert_eq!(pc.speed(), 2.0);
        }

        // Now set speed again and verify pitch corrector is updated
        sample.set_speed(0.5);
        if let Some(ref pc) = sample.pitch_corrector {
            assert_eq!(pc.speed(), 0.5);
        }
    }

    #[test]
    fn test_pitch_correction_mixing() {
        // Create buffer with 200 frames of constant value
        let data = vec![0.5f32; 200 * 2]; // 200 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Enable pitch correction at 2x speed
        sample.set_speed(2.0);
        sample.enable_pitch_correction();

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Request 50 output frames
        let mut output = vec![0.0f32; 50 * 2];
        mix_audio(&mut output, &mut state);

        // With pitch correction, at 2x speed, we consume 100 input frames
        // to produce 50 output frames
        // Position should be 100 (the stretcher consumed that much)
        assert_eq!(state.active_samples[0].position, 100);
    }

    #[test]
    fn test_pitch_correction_slow_speed() {
        let data = vec![0.5f32; 100 * 2]; // 100 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Enable pitch correction at 0.5x speed
        sample.set_speed(0.5);
        sample.enable_pitch_correction();

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Request 50 output frames
        let mut output = vec![0.0f32; 50 * 2];
        mix_audio(&mut output, &mut state);

        // At 0.5x speed, we consume 25 input frames to produce 50 output frames
        assert_eq!(state.active_samples[0].position, 25);
    }

    #[test]
    fn test_pitch_correction_output_not_silent() {
        // Ensure the pitch-corrected path produces some output
        // Use a larger buffer because the stretcher has latency
        let data = vec![0.8f32; 48000 * 2]; // 48000 stereo frames (1 second at 48kHz)
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        sample.set_speed(1.5);
        sample.enable_pitch_correction();

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Run several passes to warm up the stretcher and accumulate output
        // (stretcher has latency, so first few frames may be silent)
        let mut all_output = Vec::new();
        for _ in 0..20 {
            let mut output = vec![0.0f32; 512 * 2];
            mix_audio(&mut output, &mut state);
            all_output.extend_from_slice(&output);
        }

        // After warming up, output should have some non-zero values
        // (may not be exactly 0.8 due to stretcher processing)
        let has_audio = all_output.iter().any(|&s| s.abs() > 0.01);
        assert!(has_audio, "Output should have audio content");
    }

    // === Reverse Playback Tests ===

    #[test]
    fn test_reverse_playback_basic() {
        // Create buffer with distinct values: frame 0 = 0.1, frame 1 = 0.3, frame 2 = 0.5, etc.
        let mut data = Vec::new();
        for i in 0..10 {
            let val = 0.1 + (i as f32 * 0.1); // 0.1, 0.2, 0.3, ...
            data.push(val); // L
            data.push(val); // R
        }
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Start at end of buffer for reverse playback
        sample.position = 9; // Last frame
        sample.set_speed(-1.0); // Reverse at normal speed

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 5 frames
        let mut output = vec![0.0f32; 10]; // 5 frames * 2 channels
        mix_audio(&mut output, &mut state);

        // First output is from position 9, the buffer's loudest frame (value 1.0).
        // A full-scale frame is held at the limiter ceiling, so it reads at the
        // ceiling rather than at 1.0.
        let ceiling = state.output_ceiling;
        assert!(
            (output[0] - ceiling).abs() < 1e-3,
            "First frame (the 1.0 frame) should read at the ceiling, got {}",
            output[0]
        );
        // Position should move backwards
        assert_eq!(state.active_samples[0].position, 4);
    }

    #[test]
    fn test_reverse_playback_finishes_at_start() {
        let data = vec![0.5f32; 20]; // 10 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Start at position 5, play backwards
        sample.position = 5;
        sample.set_speed(-1.0);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 10 frames (more than we have going backwards)
        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);

        // Sample should be finished (reached start)
        assert!(state.active_samples[0].is_finished());
    }

    #[test]
    fn test_reverse_double_speed() {
        let data = vec![0.5f32; 200]; // 100 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Start at position 50, play backwards at 2x
        sample.position = 50;
        sample.set_speed(-2.0);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 20 output frames at -2x speed should consume 40 source frames
        let mut output = vec![0.0f32; 40];
        mix_audio(&mut output, &mut state);

        // Position should be 50 - 40 = 10
        assert_eq!(state.active_samples[0].position, 10);
    }

    #[test]
    fn test_reverse_half_speed() {
        let data = vec![0.5f32; 200]; // 100 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Start at position 50, play backwards at 0.5x
        sample.position = 50;
        sample.set_speed(-0.5);

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix 20 output frames at -0.5x speed should consume 10 source frames
        let mut output = vec![0.0f32; 40];
        mix_audio(&mut output, &mut state);

        // Position should be 50 - 10 = 40
        assert_eq!(state.active_samples[0].position, 40);
    }

    #[test]
    fn test_reverse_with_volume_and_fade() {
        let data = vec![1.0f32; 20]; // 10 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            0.5,
            1.0,
            TEST_FILE.to_string(),
        );

        sample.position = 9;
        sample.set_speed(-1.0);
        sample.set_fade(FadeState::In {
            elapsed: 5,
            duration: 10,
        }); // 50% fade

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        let mut output = vec![0.0f32; 4]; // 2 frames
        mix_audio(&mut output, &mut state);

        // First frame: 1.0 * 0.5 (volume) * 0.5 (fade) = 0.25
        assert!(
            (output[0] - 0.25).abs() < 0.1,
            "Expected ~0.25, got {}",
            output[0]
        );
    }

    #[test]
    fn test_pitch_correction_rejects_negative_speed() {
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        sample.enable_pitch_correction();

        // Try to set negative speed - should be rejected
        let result = sample.set_speed(-1.0);
        assert!(
            !result,
            "set_speed should return false for negative speed with pitch correction"
        );

        // Speed should remain at 1.0 (the default)
        assert_eq!(sample.speed, 1.0);
    }

    #[test]
    fn test_pitch_correction_speed_limits() {
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        sample.enable_pitch_correction();

        // Test lower bound (0.05)
        sample.set_speed(0.01);
        assert!((sample.speed - 0.05).abs() < 0.001);

        // Test upper bound (8.0)
        sample.set_speed(20.0);
        assert!((sample.speed - 8.0).abs() < 0.001);

        // Test normal range
        sample.set_speed(3.5);
        assert_eq!(sample.speed, 3.5);
    }

    #[test]
    fn test_forward_then_reverse() {
        // Test switching between forward and reverse playback
        let data = vec![0.5f32; 200]; // 100 stereo frames
        let buffer = Arc::new(DecodedBuffer::new(data, 2, 48000));
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer.clone(),
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Start at position 50
        sample.position = 50;

        let mut state = MixerState::new(2);
        state.active_samples.push(sample);

        // Mix forward 10 frames
        let mut output = vec![0.0f32; 20];
        mix_audio(&mut output, &mut state);
        assert_eq!(state.active_samples[0].position, 60);

        // Switch to reverse
        state.active_samples[0].set_speed(-1.0);

        // Mix reverse 10 frames
        mix_audio(&mut output, &mut state);
        assert_eq!(state.active_samples[0].position, 50);
    }

    // === set_speed_with_mode Tests ===
    // These test the actual method that command handling uses

    #[test]
    fn test_set_speed_with_mode_pitch_corrected_to_reverse() {
        // Regression test: using set_speed_with_mode to switch from
        // pitch-corrected to negative speed in a single call
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Start with pitch correction enabled at 1.5x
        sample.set_speed_with_mode(1.5, true);
        assert!(sample.has_pitch_correction());
        assert_eq!(sample.speed, 1.5);

        // Switch to reverse with no pitch correction - this is what
        // the command handler does when receiving:
        // {"command": "speed", "message": {"speed": -1.5, "pitch_correction": false}}
        let result = sample.set_speed_with_mode(-1.5, false);

        // Should succeed in a single call
        assert!(
            result,
            "set_speed_with_mode should handle pitch-corrected to reverse"
        );
        assert_eq!(sample.speed, -1.5);
        assert!(!sample.has_pitch_correction());
    }

    #[test]
    fn test_set_speed_with_mode_reverse_to_pitch_corrected() {
        // Test switching from reverse playback to pitch-corrected
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Start in reverse
        sample.set_speed_with_mode(-2.0, false);
        assert_eq!(sample.speed, -2.0);
        assert!(!sample.has_pitch_correction());

        // Switch to pitch-corrected
        let result = sample.set_speed_with_mode(1.5, true);

        assert!(result);
        assert!(sample.has_pitch_correction());
        assert_eq!(sample.speed, 1.5);
    }

    #[test]
    fn test_set_speed_with_mode_negative_with_pitch_correction_rejected() {
        // Attempting negative speed WITH pitch_correction=true should fail
        // (the method enables pitch correction, then rejects negative speed)
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        sample.set_speed(1.0);
        let result = sample.set_speed_with_mode(-1.5, true);

        // Should fail - you can't have negative speed with pitch correction
        assert!(!result);
        // Pitch correction is enabled but speed was rejected
        assert!(sample.has_pitch_correction());
        // Speed remains at previous value
        assert_eq!(sample.speed, 1.0);
    }

    #[test]
    fn test_set_speed_with_mode_multiple_transitions() {
        // Test multiple mode transitions
        let buffer = create_test_buffer(100, 2, 0.5);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Forward normal -> Forward pitch-corrected
        sample.set_speed_with_mode(2.0, true);
        assert!(sample.has_pitch_correction());
        assert_eq!(sample.speed, 2.0);

        // Forward pitch-corrected -> Reverse normal
        sample.set_speed_with_mode(-1.0, false);
        assert!(!sample.has_pitch_correction());
        assert_eq!(sample.speed, -1.0);

        // Reverse normal -> Forward pitch-corrected
        sample.set_speed_with_mode(0.5, true);
        assert!(sample.has_pitch_correction());
        assert_eq!(sample.speed, 0.5);

        // Forward pitch-corrected -> Forward normal
        sample.set_speed_with_mode(3.0, false);
        assert!(!sample.has_pitch_correction());
        assert_eq!(sample.speed, 3.0);
    }

    // === Volume Ramping Tests ===

    #[test]
    fn test_voice_volume_ramping_smooth_transition() {
        // Verify that volume changes happen smoothly frame-by-frame
        let buffer = create_test_buffer(1000, 2, 1.0);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Change target volume from 1.0 to 0.5
        sample.set_target_voice_volume(0.5);

        // Track volume changes frame-by-frame
        let mut last_volume = sample.voice_volume;
        let mut frame_count = 0;

        // Ramp until we reach target
        while (sample.voice_volume - sample.target_voice_volume).abs() > 0.0001 {
            sample.advance_voice_volume();
            let current = sample.voice_volume;

            // Verify volume changed by at most RAMP_RATE (0.001)
            let delta = (current - last_volume).abs();
            assert!(
                delta <= 0.0011,
                "Volume changed too quickly: {} -> {} (delta {})",
                last_volume,
                current,
                delta
            );

            // Verify monotonic decrease
            assert!(
                current < last_volume,
                "Volume should decrease: {} -> {}",
                last_volume,
                current
            );

            last_volume = current;
            frame_count += 1;
        }

        // Should take roughly 500 frames to go from 1.0 to 0.5 (0.5 / 0.001 = 500)
        assert!(
            frame_count > 400 && frame_count < 600,
            "Unexpected frame count: {}",
            frame_count
        );
        assert!(
            (sample.voice_volume - 0.5).abs() < 0.01,
            "Final volume should be ~0.5"
        );
    }

    #[test]
    fn test_voice_volume_ramping_rapid_changes() {
        // Verify that rapid volume changes don't cause issues -
        // the system should smoothly transition to the newest target
        let buffer = create_test_buffer(2000, 2, 1.0);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Change target to 0.5 and start ramping
        sample.set_target_voice_volume(0.5);
        for _ in 0..100 {
            sample.advance_voice_volume();
        }
        // After 100 frames at 0.001/frame = 0.1 change
        // Volume should be 1.0 - 0.1 = 0.9 (still above 0.5)
        let vol_after_first = sample.voice_volume;
        assert!(
            vol_after_first < 1.0 && vol_after_first > 0.5,
            "Volume {} should be between 0.5 and 1.0",
            vol_after_first
        );

        // Interrupt: change target to 0.2 before reaching 0.5
        sample.set_target_voice_volume(0.2);
        assert_eq!(sample.target_voice_volume, 0.2);

        // Continue ramping toward 0.2
        for _ in 0..200 {
            sample.advance_voice_volume();
        }
        // After 200 more frames at 0.001/frame = 0.2 change
        // Volume should be ~0.9 - 0.2 = 0.7
        let vol_after_second = sample.voice_volume;
        assert!(
            vol_after_second < vol_after_first,
            "Volume should decrease: {} -> {}",
            vol_after_first,
            vol_after_second
        );

        // Interrupt again: change target to 0.9 (going back up)
        sample.set_target_voice_volume(0.9);

        // Volume should start increasing
        let vol_before_third = sample.voice_volume;
        for _ in 0..100 {
            sample.advance_voice_volume();
        }
        assert!(
            sample.voice_volume > vol_before_third,
            "Volume should increase toward 0.9: {} -> {}",
            vol_before_third,
            sample.voice_volume
        );

        // Key behavior: rapid target changes don't cause discontinuities -
        // the volume always smoothly ramps toward whatever the current target is
    }

    #[test]
    fn test_voice_volume_ramping_in_mix() {
        // Verify that mixing with volume ramping produces smooth output
        let buffer = create_test_buffer(100, 1, 1.0); // Mono, all 1.0 samples
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            1.0,
            TEST_FILE.to_string(),
        );

        // Set target to 0.5 - will ramp during mixing
        sample.set_target_voice_volume(0.5);

        let mut state = MixerState::new(1);
        state.active_samples.push(sample);

        // Mix 50 frames
        let mut output = vec![0.0f32; 50];
        mix_audio(&mut output, &mut state);

        // Verify output ramps smoothly - each sample should be slightly less than previous
        for i in 1..50 {
            let delta = output[i - 1] - output[i];
            // Delta should be roughly 0.001 (the ramp rate)
            assert!(
                delta > 0.0,
                "Output should decrease frame {} to {}: {} -> {}",
                i - 1,
                i,
                output[i - 1],
                output[i]
            );
            assert!(
                delta < 0.002,
                "Output change too large at frame {}: delta = {}",
                i,
                delta
            );
        }

        // First sample is at the start of the ramp (full volume on a full-scale
        // buffer), held at the limiter ceiling.
        assert!(output[0] > 0.85, "First sample should be near the ceiling");
        // Last sample should be lower
        assert!(
            output[49] < output[0],
            "Last sample should be lower than first"
        );
    }

    #[test]
    fn test_live_input_volume_ramping() {
        // Verify LiveInput also supports smooth volume ramping
        let data = vec![1.0f32; 100];
        let consumer = create_test_ring_buffer_with_data(&data);

        let mut input = LiveInput::new("mic".to_string(), consumer, 1, 1.0, vec![(0, 0)]);

        // Set target volume
        input.set_target_voice_volume(0.3);

        // Verify ramping works
        let initial = input.voice_volume;
        input.advance_voice_volume();
        assert!(
            input.voice_volume < initial,
            "Voice volume should decrease toward target"
        );

        // Continue ramping
        for _ in 0..1000 {
            input.advance_voice_volume();
        }
        assert!(
            (input.voice_volume - 0.3).abs() < 0.01,
            "Should reach target of 0.3"
        );
    }

    #[test]
    fn test_volume_ramping_no_change_when_at_target() {
        // Verify no ramping occurs when current == target
        let buffer = create_test_buffer(10, 2, 1.0);
        let mut sample = ActiveSample::new(
            1,
            "test".to_string(),
            buffer,
            1.0,
            0.7,
            TEST_FILE.to_string(),
        );

        // Current and target are both 0.7
        assert_eq!(sample.voice_volume, 0.7);
        assert_eq!(sample.target_voice_volume, 0.7);

        // Advancing should not change anything
        let result = sample.advance_voice_volume();
        assert!(!result, "Should return false when at target");
        assert_eq!(sample.voice_volume, 0.7);
    }

    // === StreamedSource Tests ===

    /// Build a streamed source over a ring pre-filled with `data` (interleaved),
    /// with the EOF flag set to `producer_done`. Reuses the live-input ring helper.
    fn streamed_with_data(data: &[f32], channels: usize, producer_done: bool) -> StreamedSource {
        let consumer = create_test_ring_buffer_with_data(data);
        let channel_map: Vec<(usize, usize)> = (0..channels).map(|c| (c, c)).collect();
        StreamedSource::new(
            7,
            "music".to_string(),
            "long.wav".to_string(),
            None,
            consumer,
            channels,
            1.0,
            channel_map,
            Arc::new(AtomicBool::new(producer_done)),
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn streamed_source_not_finished_with_data_and_no_eof() {
        // Audio queued and the producer has not signalled EOF: still playing.
        let src = streamed_with_data(&[0.1, 0.2, 0.3, 0.4], 2, false);
        assert!(!src.is_finished());
    }

    #[test]
    fn streamed_source_not_finished_at_eof_while_ring_has_data() {
        // EOF signalled, but a full frame is still queued: not finished yet.
        let src = streamed_with_data(&[0.1, 0.2], 2, true);
        assert!(!src.is_finished());
    }

    #[test]
    fn streamed_source_finished_at_eof_once_ring_drained() {
        // EOF signalled and the ring holds less than a frame: finished.
        let src = streamed_with_data(&[], 2, true);
        assert!(src.is_finished());
    }

    #[test]
    fn streamed_source_finished_when_fade_out_complete_even_with_data() {
        // A completed fade-out retires the source even though audio remains queued
        // and EOF has not been signalled (matches ActiveSample fade-out semantics).
        let mut src = streamed_with_data(&[0.5; 8], 2, false);
        src.set_fade(FadeState::Out {
            elapsed: 10,
            duration: 10,
        });
        assert!(src.is_finished());
    }

    #[test]
    fn streamed_source_mix_routes_audio_then_underrun_fades() {
        // Two stereo frames of 1.0 queued, no fade, unity gain. Past the queued audio
        // the ring underruns and the held last frame fades toward silence (F3), the
        // same hold-and-fade the live-input path uses.
        let mut src = streamed_with_data(&[1.0; 4], 2, true);
        let frames = 70;
        let mut output = vec![0.0f32; frames * 2];
        mix_streamed_source_into_output(&mut src, &mut output, frames, 2, (1.0, 1.0));

        // Frames 0..2 read real audio at unity.
        assert!((output[0] - 1.0).abs() < 1e-6);
        assert!((output[2] - 1.0).abs() < 1e-6);
        // Frame 2 is the first underrun: the held frame (1.0) at full gain (1 - 0/64).
        assert!((output[4] - 1.0).abs() < 1e-6);
        // Frame 3 underruns one step further: 1 - 1/64.
        assert!((output[6] - (1.0 - 1.0 / 64.0)).abs() < 1e-4);
        // The underrun began at frame 2, so by frame 2+64 = 66 the hold has reached
        // silence and stays there.
        assert!(output[66 * 2].abs() < 1e-6);
        assert!(output[69 * 2].abs() < 1e-6);
    }

    #[test]
    fn streamed_source_fade_out_ramps_to_silence() {
        // A fade-out scales the source down across the block, so the first frame is at
        // full level and a later frame is quieter — no hard cut.
        let mut src = streamed_with_data(&[1.0; 40], 2, true); // 20 stereo frames
        src.set_fade(FadeState::Out {
            elapsed: 0,
            duration: 10,
        });
        let mut output = vec![0.0f32; 20];
        mix_streamed_source_into_output(&mut src, &mut output, 10, 2, (1.0, 1.0));

        // Frame 0: 1 - 0/10 = 1.0; frame 5: 1 - 5/10 = 0.5; frame 9: 1 - 9/10 = 0.1.
        assert!((output[0] - 1.0).abs() < 1e-6);
        assert!((output[5 * 2] - 0.5).abs() < 1e-6);
        assert!((output[9 * 2] - 0.1).abs() < 1e-6);
    }

    #[test]
    fn mix_audio_includes_streamed_sources() {
        // A streamed source pushed into MixerState is mixed by mix_audio just like a
        // sample or a live input, through the output stage.
        let src = streamed_with_data(&[0.5; 8], 2, true); // 4 stereo frames at 0.5
        let mut state = MixerState::new(2);
        state.streamed_sources.push(src);

        let mut output = vec![0.0f32; 8]; // 4 frames * 2 channels
        mix_audio(&mut output, &mut state);

        for &s in &output {
            assert!((s - 0.5).abs() < 1e-4, "expected 0.5, got {s}");
        }
    }
}

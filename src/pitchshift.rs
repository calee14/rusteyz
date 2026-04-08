use rodio::Source;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Offline pitch shift — linear interpolation resampling.
// Meant to be applied to a Vec<f32> at startup, not on the audio thread.
// ---------------------------------------------------------------------------

pub fn pitch_shift_samples(samples: &[f32], factor: f32) -> Vec<f32> {
    if samples.is_empty() {
        return vec![];
    }

    let output_len = (samples.len() as f32 / factor) as usize;
    let mut out = Vec::with_capacity(output_len);
    let mut pos: f32 = 0.0;

    while pos < (samples.len() - 1) as f32 {
        let i = pos as usize;
        let frac = pos.fract();
        let s = samples[i] * (1.0 - frac) + samples[i + 1] * frac;
        out.push(s);
        pos += factor;
    }

    out
}

// ---------------------------------------------------------------------------
// Offline reverb — multi-tap delay + feedback.
// Apply to a Vec<f32> at startup so the audio thread just copies PCM.
// ---------------------------------------------------------------------------

pub fn apply_reverb(samples: &[f32], sample_rate: u32, mix: f32, feedback: f32) -> Vec<f32> {
    // Fixed delay taps: (delay_samples, gain)
    let taps: Vec<(usize, f32)> = vec![
        ((sample_rate as f32 * 0.029) as usize, 0.4),
        ((sample_rate as f32 * 0.053) as usize, 0.3),
        ((sample_rate as f32 * 0.079) as usize, 0.25),
        ((sample_rate as f32 * 0.110) as usize, 0.2),
    ];

    let max_delay = taps.iter().map(|(d, _)| *d).max().unwrap_or(0) + 1;

    // Output is longer than input so the reverb tail rings out naturally
    let tail = (sample_rate as f32 * 0.15) as usize;
    let total = samples.len() + tail;

    let mut buffer = vec![0.0f32; max_delay];
    let mut out = Vec::with_capacity(total);
    let mut write_pos = 0usize;
    let buf_len = buffer.len();

    for n in 0..total {
        let dry = if n < samples.len() { samples[n] } else { 0.0 };

        let wet: f32 = taps
            .iter()
            .map(|(delay, gain)| {
                let read_pos = (write_pos + buf_len - delay) % buf_len;
                buffer[read_pos] * gain
            })
            .sum();

        buffer[write_pos] = dry + wet * feedback;
        write_pos = (write_pos + 1) % buf_len;

        out.push(dry * (1.0 - mix) + wet * mix);
    }

    out
}

// ---------------------------------------------------------------------------
// Runtime wrappers — kept for completeness but not used in the hot path.
// PitchShift<S> and SimpleReverb<S> stream from a rodio Source at runtime.
// Only use these if you need fully dynamic DSP (e.g. variable pitch per note).
// ---------------------------------------------------------------------------

pub struct PitchShift<S> {
    input: S,
    factor: f32,
    position: f32,
    prev_sample: f32,
    next_sample: f32,
    consumed_first: bool,
}

impl<S: Source<Item = f32>> PitchShift<S> {
    pub fn new(input: S, factor: f32) -> Self {
        Self {
            input,
            factor,
            position: 0.0,
            prev_sample: 0.0,
            next_sample: 0.0,
            consumed_first: false,
        }
    }
}

impl<S: Source<Item = f32>> Iterator for PitchShift<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if !self.consumed_first {
            self.prev_sample = self.input.next()?;
            self.next_sample = self.input.next().unwrap_or(0.0);
            self.consumed_first = true;
        }

        let frac = self.position.fract();
        let out = self.prev_sample * (1.0 - frac) + self.next_sample * frac;

        self.position += self.factor;

        while self.position >= 1.0 {
            self.position -= 1.0;
            self.prev_sample = self.next_sample;
            self.next_sample = self.input.next().unwrap_or(0.0);
        }

        Some(out)
    }
}

impl<S: Source<Item = f32>> Source for PitchShift<S> {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> std::num::NonZero<u16> {
        self.input.channels()
    }
    fn sample_rate(&self) -> std::num::NonZero<u32> {
        self.input.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub struct SimpleReverb<S> {
    input: S,
    buffer: Vec<f32>,
    write_pos: usize,
    taps: Vec<(usize, f32)>,
    feedback: f32,
    mix: f32,
}

impl<S: Source<Item = f32>> SimpleReverb<S> {
    pub fn new(input: S, sample_rate: u32, mix: f32, feedback: f32) -> Self {
        let taps = vec![
            ((sample_rate as f32 * 0.029) as usize, 0.4),
            ((sample_rate as f32 * 0.053) as usize, 0.3),
            ((sample_rate as f32 * 0.079) as usize, 0.25),
            ((sample_rate as f32 * 0.110) as usize, 0.2),
        ];
        let max_delay = taps.iter().map(|(d, _)| *d).max().unwrap_or(0) + 1;
        Self {
            input,
            buffer: vec![0.0; max_delay],
            write_pos: 0,
            taps,
            feedback,
            mix,
        }
    }
}

impl<S: Source<Item = f32>> Iterator for SimpleReverb<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let dry = self.input.next()?;
        let buf_len = self.buffer.len();
        let wet: f32 = self
            .taps
            .iter()
            .map(|(delay, gain)| {
                let read_pos = (self.write_pos + buf_len - delay) % buf_len;
                self.buffer[read_pos] * gain
            })
            .sum();
        self.buffer[self.write_pos] = dry + wet * self.feedback;
        self.write_pos = (self.write_pos + 1) % buf_len;
        Some(dry * (1.0 - self.mix) + wet * self.mix)
    }
}

impl<S: Source<Item = f32>> Source for SimpleReverb<S> {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> std::num::NonZero<u16> {
        self.input.channels()
    }
    fn sample_rate(&self) -> std::num::NonZero<u32> {
        self.input.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}


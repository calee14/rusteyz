use rodio::Source;
use std::time::Duration;

pub struct SoftClip<S> {
    input: S,
}

impl<S> SoftClip<S> {
    pub fn new(input: S) -> Self {
        Self { input }
    }
}

impl<S> Iterator for SoftClip<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        self.input.next().map(|s| s.tanh())
    }
}

impl<S> Source for SoftClip<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.input.current_span_len()
    }
    fn channels(&self) -> std::num::NonZero<u16> {
        self.input.channels()
    }
    fn sample_rate(&self) -> std::num::NonZero<u32> {
        self.input.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }
}

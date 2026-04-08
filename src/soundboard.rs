use crate::{
    pitchshift::{apply_reverb, pitch_shift_samples},
    softclip::SoftClip,
};
use rand::{Rng, RngExt};
use rdev::{Event, EventType, Key, listen};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Source, buffer::SamplesBuffer};
use std::{collections::VecDeque, fs, io::Cursor, num::NonZero, sync::Arc, time::Instant};

// ---------------------------------------------------------------------------
// How many pitch variants to bake per sound at startup.
// Each variant is a slightly different pitch so rapid keypresses sound natural
// instead of robotic (stacked copies of the exact same waveform).
// ---------------------------------------------------------------------------
const PITCH_VARIANTS: usize = 5;

// Pitch factors for each variant — centred on 1.0 (no change), spread ±5%.
// Index 2 is the "neutral" sound; 0/1 are lower, 3/4 are higher.
const PITCH_FACTORS: [f32; PITCH_VARIANTS] = [0.95, 0.975, 1.0, 1.025, 1.05];

// Reverb settings baked into press sounds. Release sounds stay dry (they are
// very short and reverb on releases tends to sound muddy).
const REVERB_MIX: f32 = 0.18;
const REVERB_FEEDBACK: f32 = 0.15;

// ---------------------------------------------------------------------------
// HotKeyListener
// ---------------------------------------------------------------------------

enum HotKeyEvent {
    Mute,
    VolumeUp,
    VolumeDown,
}

struct HotKeyListener {
    pub max_size: usize,
    leader: bool,
    buffer: VecDeque<Key>,
}

impl HotKeyListener {
    fn new(max_size: usize) -> Self {
        Self {
            max_size,
            leader: false,
            buffer: VecDeque::new(),
        }
    }

    pub fn input_key(&mut self, key: Key) -> Option<HotKeyEvent> {
        if self.buffer.len() >= self.max_size {
            self.buffer.pop_front();
        }
        self.buffer.push_back(key);
        self.check_combinations()
    }

    fn check_combinations(&mut self) -> Option<HotKeyEvent> {
        if let Some(last_key) = self.buffer.back() {
            if matches!(last_key, Key::Alt) {
                self.leader = true;
                return None;
            }
        }

        if !self.leader {
            return None;
        }

        self.leader = false;

        match self.buffer.back().unwrap() {
            Key::KeyM => Some(HotKeyEvent::Mute),
            Key::KeyJ => Some(HotKeyEvent::VolumeDown),
            Key::KeyK => Some(HotKeyEvent::VolumeUp),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// PreloadedSound — a set of baked PCM variants, ready to copy to the mixer.
// ---------------------------------------------------------------------------

struct PreloadedSound {
    /// Each entry is one pitch variant: raw f32 PCM samples.
    variants: Vec<Arc<Vec<f32>>>,
    channels: u16,
    sample_rate: u32,
}

impl PreloadedSound {
    /// Decode a WAV file from raw bytes, apply pitch × reverb variants, store as PCM.
    fn from_bytes(raw: &[u8], with_reverb: bool) -> Self {
        // Decode once to f32 samples
        let decoder = Decoder::try_from(Cursor::new(raw.to_vec())).expect("failed to decode sound");
        let channels = decoder.channels().get();
        let sample_rate = decoder.sample_rate().get();
        // rodio 0.22 Decoder yields f32 natively
        let base_samples: Vec<f32> = decoder.collect();

        let variants = PITCH_FACTORS
            .iter()
            .map(|&factor| {
                // Pitch-shift offline
                let pitched = pitch_shift_samples(&base_samples, factor);

                // Optionally bake reverb
                let processed = if with_reverb {
                    apply_reverb(&pitched, sample_rate, REVERB_MIX, REVERB_FEEDBACK)
                } else {
                    pitched
                };

                Arc::new(processed)
            })
            .collect();

        Self {
            variants,
            channels,
            sample_rate,
        }
    }

    /// Pick a random variant and return a rodio SamplesBuffer ready to play.
    fn random_buffer(&self, rng: &mut impl Rng, volume: f32) -> SamplesBuffer {
        let idx = rng.random_range(0..self.variants.len());
        let samples: Vec<f32> = self.variants[idx].iter().map(|&s| s * volume).collect();
        SamplesBuffer::new(
            NonZero::new(self.channels).unwrap(),
            NonZero::new(self.sample_rate).unwrap(),
            samples,
        )
    }

    /// Duration of the longest variant in seconds (for active-sound tracking).
    fn duration_secs(&self) -> f32 {
        let max_len = self.variants.iter().map(|v| v.len()).max().unwrap_or(0);
        let frames = max_len as f32 / self.channels.max(1) as f32;
        frames / self.sample_rate as f32
    }
}

// ---------------------------------------------------------------------------
// SoundBoard
// ---------------------------------------------------------------------------

pub struct SoundBoard {
    volume: f32,
    is_muted: bool,
    press: PreloadedSound,
    release: PreloadedSound,
    spacebar_press: PreloadedSound,
    spacebar_release: PreloadedSound,
    handle: MixerDeviceSink,
    hot_key_listener: HotKeyListener,
    last_key: Option<Key>,
    rng: rand::rngs::ThreadRng,
    /// Tracks (expiry time) for each currently-playing sound.
    /// Used to scale volume down when many sounds overlap.
    active_sounds: Vec<Instant>,
    press_duration: f32,
    release_duration: f32,
    spacebar_press_duration: f32,
    spacebar_release_duration: f32,
}

impl SoundBoard {
    pub fn new() -> Self {
        println!("Preloading sounds — baking pitch variants + reverb…");

        let press_raw =
            fs::read("assets/milky_yellow_press.wav").expect("failed to read press sound");
        let release_raw =
            fs::read("assets/milky_yellow_release.wav").expect("failed to read release sound");
        let spacebar_press_raw = fs::read("assets/milky_yellow_space_press.wav")
            .expect("failed to read spacebar press sound");
        let spacebar_release_raw = fs::read("assets/milky_yellow_space_release.wav")
            .expect("failed to read spacebar release sound");

        // Press sounds get reverb baked in; release sounds stay dry.
        let press = PreloadedSound::from_bytes(&press_raw, true);
        let release = PreloadedSound::from_bytes(&release_raw, false);
        let spacebar_press = PreloadedSound::from_bytes(&spacebar_press_raw, true);
        let spacebar_release = PreloadedSound::from_bytes(&spacebar_release_raw, false);

        println!("Sounds ready ({PITCH_VARIANTS} pitch variants each).");

        let handle = DeviceSinkBuilder::open_default_sink().expect("open default audio device");

        let press_duration = press.duration_secs();
        let release_duration = release.duration_secs();
        let spacebar_press_duration = spacebar_press.duration_secs();
        let spacebar_release_duration = spacebar_release.duration_secs();

        Self {
            volume: 0.3,
            is_muted: false,
            press,
            release,
            spacebar_press,
            spacebar_release,
            handle,
            hot_key_listener: HotKeyListener::new(3),
            last_key: None,
            rng: rand::rng(),
            active_sounds: Vec::new(),
            press_duration,
            release_duration,
            spacebar_press_duration,
            spacebar_release_duration,
        }
    }

    /// Remove expired sounds and return a gain factor that keeps the summed
    /// signal roughly constant regardless of how many sounds overlap.
    /// Uses 1/sqrt(n) scaling — perceptually even and mathematically sound
    /// for uncorrelated signals.
    fn overlap_gain(&mut self) -> f32 {
        let now = Instant::now();
        self.active_sounds.retain(|&expiry| expiry > now);
        let n = (self.active_sounds.len() as f32 + 1.0).max(1.0); // +1 for the sound we're about to add
        1.0 / n.sqrt()
    }

    /// Register a new sound that will play for `duration` seconds.
    fn track_sound(&mut self, duration_secs: f32) {
        let expiry = Instant::now() + std::time::Duration::from_secs_f32(duration_secs);
        self.active_sounds.push(expiry);
    }

    pub fn start(mut self) {
        if let Err(error) = listen(move |event: Event| {
            if let EventType::KeyPress(key) = event.event_type {
                // Suppress key-repeat events
                if self.last_key == Some(key) {
                    return;
                }
                self.last_key = Some(key);

                // Handle hotkeys first
                if let Some(hotkey_event) = self.hot_key_listener.input_key(key) {
                    match hotkey_event {
                        HotKeyEvent::Mute => {
                            self.is_muted = !self.is_muted;
                            println!("Muted: {}", self.is_muted);
                        }
                        HotKeyEvent::VolumeUp => {
                            self.volume = (self.volume + 0.1).min(2.0);
                            println!("Volume: {:.1}", self.volume);
                        }
                        HotKeyEvent::VolumeDown => {
                            self.volume = (self.volume - 0.1).max(0.0);
                            println!("Volume: {:.1}", self.volume);
                        }
                    }
                }

                if self.is_muted {
                    return;
                }

                // Grab the duration (Copy) without borrowing self for long
                let (is_space, dur) = if key == Key::Space {
                    (true, self.spacebar_press_duration)
                } else {
                    (false, self.press_duration)
                };

                // Mutable work first — no outstanding borrows on self
                let gain = self.overlap_gain();
                self.track_sound(dur);
                let vol = self.volume * 0.25 * gain;

                // Now borrow the sound and rng; the borrow lives only for this line
                let sound = if is_space {
                    &self.spacebar_press
                } else {
                    &self.press
                };
                let buf = sound.random_buffer(&mut self.rng, vol);
                self.handle.mixer().add(SoftClip::new(buf));
            }

            if let EventType::KeyRelease(key) = event.event_type {
                self.last_key = None;

                if self.is_muted {
                    return;
                }

                let (is_space, dur) = if key == Key::Space {
                    (true, self.spacebar_release_duration)
                } else {
                    (false, self.release_duration)
                };

                let gain = self.overlap_gain();
                self.track_sound(dur);
                let vol = self.volume * 0.25 * gain;

                let sound = if is_space {
                    &self.spacebar_release
                } else {
                    &self.release
                };
                let buf = sound.random_buffer(&mut self.rng, vol);
                self.handle.mixer().add(SoftClip::new(buf));
            }
        }) {
            eprintln!("Error: {:?}", error);
        }
    }
}


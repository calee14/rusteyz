use crate::{
    pitchshift::{apply_reverb, pitch_shift_samples},
    softclip::SoftClip,
};
use rand::{Rng, RngExt};
use rdev::{Event, EventType, Key, listen};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Source, buffer::SamplesBuffer};
use std::{collections::VecDeque, fs, io::Cursor, num::NonZero, sync::Arc};

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
        }
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

                // Pick the right sound pool and grab a pre-baked variant
                let sound = if key == Key::Space {
                    &self.spacebar_press
                } else {
                    &self.press
                };

                // Volume is baked into the buffer; SoftClip just prevents clipping
                let buf = sound.random_buffer(&mut self.rng, self.volume * 0.25);
                self.handle.mixer().add(SoftClip::new(buf));
            }

            if let EventType::KeyRelease(key) = event.event_type {
                self.last_key = None;

                if self.is_muted {
                    return;
                }

                let sound = if key == Key::Space {
                    &self.spacebar_release
                } else {
                    &self.release
                };

                let buf = sound.random_buffer(&mut self.rng, self.volume * 0.25);
                self.handle.mixer().add(SoftClip::new(buf));
            }
        }) {
            eprintln!("Error: {:?}", error);
        }
    }
}


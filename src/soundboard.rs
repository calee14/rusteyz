use crate::{
    pitchshift::{self, PitchShift, SimpleReverb},
    softclip::SoftClip,
};
use rand::RngExt;
use rdev::{Event, EventType, Key, listen};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Source};
use std::{collections::VecDeque, fs, io::Cursor, ops::Deref, sync::Arc};

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
        // Check if we can activate leader
        if let Some(last_key) = self.buffer.back() {
            if matches!(last_key, Key::Alt) {
                self.leader = true;
                return None;
            }
        }

        // Trigger combos is leader is active
        if !self.leader {
            return None;
        }

        self.leader = false;

        // Check if events are detected
        let last_key = self.buffer.back().unwrap();
        match last_key {
            Key::KeyM => Some(HotKeyEvent::Mute),
            Key::KeyJ => Some(HotKeyEvent::VolumeDown),
            Key::KeyK => Some(HotKeyEvent::VolumeUp),
            _ => None,
        }
    }
}

pub struct SoundBoard {
    volume: f32,
    is_muted: bool,
    press_sound: Arc<Vec<u8>>,
    release_sound: Arc<Vec<u8>>,
    spacebar_press_sound: Arc<Vec<u8>>,
    spacebar_release_sound: Arc<Vec<u8>>,
    handle: MixerDeviceSink,
    hot_key_listner: HotKeyListener,
    last_key: Option<Key>,
}

impl SoundBoard {
    pub fn new() -> Self {
        // Preload keyboard click sound
        let press_bytes: Arc<Vec<u8>> =
            Arc::new(fs::read("assets/milky_yellow_press.wav").expect("failed to read sound file"));
        let release_bytes: Arc<Vec<u8>> = Arc::new(
            fs::read("assets/milky_yellow_release.wav").expect("failed to read sound file"),
        );

        // Preload keyboard spacebar click sound
        let spacebar_press_bytes: Arc<Vec<u8>> = Arc::new(
            fs::read("assets/milky_yellow_space_press.wav").expect("failed to read sound file"),
        );
        let spacebar_release_bytes: Arc<Vec<u8>> = Arc::new(
            fs::read("assets/milky_yellow_space_release.wav").expect("failed to read sound file"),
        );

        // Keep audio sink alive
        let handle = DeviceSinkBuilder::open_default_sink().expect("open default audio device");

        Self {
            volume: 0.3,
            is_muted: false,
            press_sound: press_bytes,
            release_sound: release_bytes,
            spacebar_press_sound: spacebar_press_bytes,
            spacebar_release_sound: spacebar_release_bytes,
            handle,
            hot_key_listner: HotKeyListener::new(3),
            last_key: None,
        }
    }

    pub fn start(mut self) {
        let press_sound = self.press_sound;
        let release_sound = self.release_sound;
        let spacebar_press_sound = self.spacebar_press_sound;
        let spacebar_release_sound = self.spacebar_release_sound;

        let handle = self.handle;
        if let Err(error) = listen(move |event: Event| {
            // A key was pressed. Play sound
            if let EventType::KeyPress(key) = event.event_type {
                let mut sound_to_play = press_sound.clone();
                if key == Key::Space {
                    sound_to_play = spacebar_press_sound.clone();
                }
                if self.last_key == Some(key) {
                    return;
                }
                println!("Playing sound because {:?} was pressed", key);

                self.last_key = Some(key);

                // Update hotkey listener
                if let Some(event) = self.hot_key_listner.input_key(key) {
                    println!("There is an event");
                    match event {
                        HotKeyEvent::Mute => self.is_muted = !self.is_muted,
                        HotKeyEvent::VolumeUp => {
                            self.volume = (self.volume + 0.1).min(2.0);
                            println!("Volume: {:?}", self.volume);
                        }
                        HotKeyEvent::VolumeDown => {
                            self.volume = (self.volume - 0.1).max(0.0);
                            println!("Volume: {:?}", self.volume);
                        }
                    }
                }

                let cursor = Cursor::new(sound_to_play.as_ref().clone());
                let source = Decoder::try_from(cursor).unwrap().amplify(0.25);

                if !self.is_muted {
                    handle
                        .mixer()
                        .add(SoftClip::new(source.amplify(self.volume)));
                }
            }
            if let EventType::KeyRelease(key) = event.event_type {
                let mut sound_to_play = release_sound.clone();
                if key == Key::Space {
                    sound_to_play = spacebar_release_sound.clone();
                }
                self.last_key = None;
                println!("Playing sound because {:?} was released", key);

                let cursor = Cursor::new(sound_to_play.as_ref().clone());
                let source = Decoder::try_from(cursor).unwrap().amplify(0.25);

                if !self.is_muted {
                    handle
                        .mixer()
                        .add(SoftClip::new(source.amplify(self.volume)));
                }
            }
        }) {
            eprintln!("Error: {:?}", error);
        }
    }
}

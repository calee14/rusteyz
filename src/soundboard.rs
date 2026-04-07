use std::{fs, io::Cursor, sync::Arc};

use rdev::{Event, EventType, listen};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink};

pub struct SoundBoard {
    volume: f32,
    is_muted: bool,
    sound: Arc<Vec<u8>>,
    handle: MixerDeviceSink,
}

impl SoundBoard {
    pub fn new() -> Self {
        // Preload keyboard click sound
        let japanese_black_bytes: Arc<Vec<u8>> =
            Arc::new(fs::read("assets/japanese_black.wav").expect("failed to read sound file"));

        // Keep audio sink alive
        let handle = DeviceSinkBuilder::open_default_sink().expect("open default audio device");

        Self {
            volume: 1.0,
            is_muted: false,
            sound: japanese_black_bytes,
            handle,
        }
    }

    pub fn start(self) {
        let sound = self.sound;
        let handle = self.handle;
        if let Err(error) = listen(move |event: Event| {
            // A key was pressed. Play sound
            if let EventType::KeyPress(key) = event.event_type {
                println!("Playing sound because {:?} was pressed", key);

                let cursor = Cursor::new(sound.as_ref().clone());
                let source = Decoder::try_from(cursor).unwrap();

                handle.mixer().add(source);
            }
        }) {
            eprintln!("Error: {:?}", error);
        }
    }
}

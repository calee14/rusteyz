use rdev::{Event, EventType, Key, listen};
use rodio::{Decoder, DeviceSinkBuilder, source::Source};
use std::{fs, io::Cursor, sync::Arc};

fn main() {
    // Preload keyboard click sound
    let japanese_black_bytes: Arc<Vec<u8>> =
        Arc::new(fs::read("assets/japanese_black.wav").expect("failed to read sound file"));

    // Keep audio sink alive
    let handle = DeviceSinkBuilder::open_default_sink().expect("open default audio device");

    if let Err(error) = listen(move |event: Event| {
        handle_key_pressed(event, &handle, &japanese_black_bytes);
    }) {
        eprintln!("Error: {:?}", error);
    }
}

fn handle_key_pressed(event: Event, handle: &rodio::MixerDeviceSink, sound: &Arc<Vec<u8>>) {
    // A key was pressed. Play sound
    if let EventType::KeyPress(key) = event.event_type {
        println!("Playing sound because {:?} was pressed", key);

        let cursor = Cursor::new(sound.as_ref().clone());
        let source = Decoder::try_from(cursor).unwrap();

        handle.mixer().add(source);
    }
}

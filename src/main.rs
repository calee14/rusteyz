use rdev::{Event, EventType, Key, listen};

fn main() {
    println!("Listening...");
    if let Err(error) = listen(handle_key_pressed) {
        eprintln!("Error: {:?}", error);
    }
}

fn handle_key_pressed(event: Event) {
    // A key was pressed. Play sound
    if let EventType::KeyPress(key) = event.event_type {
        println!("Playing sound because {:?} was pressed", key);
    }
}

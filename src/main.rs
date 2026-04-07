mod soundboard;

fn main() {
    let key_soundboard = soundboard::SoundBoard::new();
    key_soundboard.start();
}

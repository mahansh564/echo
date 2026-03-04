use std::sync::Mutex;

static TEST_MUTEX: Mutex<()> = Mutex::new(());

use super::*;

#[test]
fn config_defaults() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().expect("temp");
    std::env::set_var("HOME", dir.path());
    let config = load_config().expect("load");
    assert_eq!(config.mic_device, ":1");
    assert_eq!(config.hotkey, "cmd+shift+space");
    assert!(config.voice_enabled);
    assert_eq!(config.wake_word_phrase, "echo");
    assert_eq!(config.asr_backend, "sidecar");
    assert_eq!(config.asr_language, "en");
    assert_eq!(config.audio_sample_rate, 16_000);
}

#[test]
fn config_merges_user_values() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let dir = tempfile::tempdir().expect("temp");
    std::env::set_var("HOME", dir.path());
    let config_dir = dir.path().join(".echo");
    std::fs::create_dir_all(&config_dir).expect("mkdir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
hotkey = "cmd+shift+v"
model_endpoint = "http://localhost:1234"
voice_enabled = false
wake_word_phrase = "computer"
asr_backend = "http"
asr_sidecar_path = "/tmp/whisper-cli"
asr_model_path = "/tmp/ggml.bin"
asr_endpoint = "http://localhost:8081/inference"
"#,
    )
    .expect("write");
    let config = load_config().expect("load");
    assert_eq!(config.hotkey, "cmd+shift+v");
    assert_eq!(config.model_endpoint, "http://localhost:1234");
    assert!(!config.voice_enabled);
    assert_eq!(config.wake_word_phrase, "computer");
    assert_eq!(config.asr_backend, "http");
    assert_eq!(config.asr_sidecar_path, "/tmp/whisper-cli");
    assert_eq!(config.asr_model_path, "/tmp/ggml.bin");
    assert_eq!(config.asr_endpoint, "http://localhost:8081/inference");
}

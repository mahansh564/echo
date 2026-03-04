use anyhow::{anyhow, Result};
use std::{path::Path, process::Command, time::Duration};

use crate::config::EchoConfig;

#[derive(Debug, serde::Deserialize)]
struct WhisperJsonResponse {
    text: Option<String>,
}

pub async fn transcribe_wav(config: &EchoConfig, wav_bytes: Vec<u8>) -> Result<String> {
    match config.asr_backend.to_lowercase().as_str() {
        "http" => {
            transcribe_via_http(
                &config.asr_endpoint,
                &config.asr_language,
                config.asr_timeout_ms,
                wav_bytes,
            )
            .await
        }
        _ => transcribe_via_sidecar(config, wav_bytes).await,
    }
}

async fn transcribe_via_http(
    endpoint: &str,
    language: &str,
    timeout_ms: u64,
    wav_bytes: Vec<u8>,
) -> Result<String> {
    let response = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()?
        .post(endpoint)
        .query(&[("language", language)])
        .header("content-type", "audio/wav")
        .body(wav_bytes)
        .send()
        .await?
        .error_for_status()?;

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if content_type.contains("application/json") {
        let json: WhisperJsonResponse = response.json().await?;
        return Ok(json.text.unwrap_or_default().trim().to_string());
    }

    let text = response.text().await?;
    Ok(text.trim().to_string())
}

async fn transcribe_via_sidecar(config: &EchoConfig, wav_bytes: Vec<u8>) -> Result<String> {
    let sidecar_path = config.asr_sidecar_path.clone();
    let model_path = config.asr_model_path.clone();
    let language = config.asr_language.clone();

    tauri::async_runtime::spawn_blocking(move || {
        run_sidecar(&sidecar_path, &model_path, &language, wav_bytes)
    })
    .await
    .map_err(|e| anyhow!("sidecar task join error: {}", e))?
}

fn run_sidecar(
    sidecar_path: &str,
    model_path: &str,
    language: &str,
    wav_bytes: Vec<u8>,
) -> Result<String> {
    if sidecar_path.contains('/') && !Path::new(sidecar_path).exists() {
        return Err(anyhow!("ASR sidecar binary missing at {}", sidecar_path));
    }
    if !Path::new(model_path).exists() {
        return Err(anyhow!("ASR model missing at {}", model_path));
    }

    let dir = tempfile::tempdir()?;
    let input_path = dir.path().join("input.wav");
    let output_prefix = dir.path().join("output");
    let output_txt = dir.path().join("output.txt");

    std::fs::write(&input_path, wav_bytes)?;

    let output = Command::new(sidecar_path)
        .args([
            "-ng",
            "-m",
            model_path,
            "-f",
            input_path.to_string_lossy().as_ref(),
            "-l",
            language,
            "-nt",
            "-otxt",
            "-of",
            output_prefix.to_string_lossy().as_ref(),
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ASR sidecar failed: {}", stderr.trim()));
    }

    if output_txt.exists() {
        let text = std::fs::read_to_string(output_txt)?;
        return Ok(text.trim().to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_string();

    Ok(parsed)
}

use anyhow::{anyhow, Result};
use std::process::Command;

pub fn capture_wav_chunk(mic_device: &str, sample_rate: u32, duration_ms: u64) -> Result<Vec<u8>> {
    let device_spec = resolve_device_spec(mic_device)?;
    let duration_secs = format!("{:.3}", duration_ms as f64 / 1000.0);

    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-f",
            "avfoundation",
            "-i",
            &device_spec,
            "-ac",
            "1",
            "-ar",
            &sample_rate.to_string(),
            "-t",
            &duration_secs,
            "-f",
            "wav",
            "-",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "ffmpeg capture failed (device={}): {}",
            device_spec,
            stderr.trim()
        ));
    }

    if output.stdout.len() < 44 {
        return Err(anyhow!("captured wav data too small"));
    }
    if &output.stdout[0..4] != b"RIFF" || &output.stdout[8..12] != b"WAVE" {
        let preview = output
            .stdout
            .iter()
            .take(16)
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" ");
        return Err(anyhow!(
            "ffmpeg did not return WAV bytes (header preview: {})",
            preview
        ));
    }

    Ok(output.stdout)
}

fn resolve_device_spec(mic_device: &str) -> Result<String> {
    if mic_device.starts_with(':') {
        return Ok(mic_device.to_string());
    }

    let devices = list_audio_devices()?;
    if devices.is_empty() {
        return Err(anyhow!(
            "no avfoundation audio devices found; check macOS microphone permissions"
        ));
    }

    if mic_device == "default" {
        return Ok(format!(":{}", devices[0].0));
    }

    let wanted = mic_device.to_lowercase();
    if let Some((index, _)) = devices
        .iter()
        .find(|(_, name)| name.to_lowercase().contains(&wanted))
    {
        return Ok(format!(":{}", index));
    }

    Err(anyhow!(
        "audio device '{}' not found. Available devices: {}",
        mic_device,
        devices
            .iter()
            .map(|(idx, name)| format!("[{}] {}", idx, name))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn list_audio_devices() -> Result<Vec<(usize, String)>> {
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "avfoundation",
            "-list_devices",
            "true",
            "-i",
            "",
        ])
        .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut in_audio_section = false;
    let mut devices = Vec::new();
    for line in stderr.lines() {
        let line = line.trim();
        if line.contains("AVFoundation audio devices:") {
            in_audio_section = true;
            continue;
        }
        if line.contains("AVFoundation video devices:") {
            in_audio_section = false;
            continue;
        }
        if !in_audio_section {
            continue;
        }
        if let Some((idx, name)) = parse_device_line(line) {
            devices.push((idx, name));
        }
    }
    Ok(devices)
}

fn parse_device_line(line: &str) -> Option<(usize, String)> {
    let left = line.find('[')?;
    let right = line[left + 1..].find(']')? + left + 1;
    let idx = line[left + 1..right].parse::<usize>().ok()?;
    let name = line[right + 1..].trim();
    if name.is_empty() {
        return None;
    }
    Some((idx, name.to_string()))
}

pub fn rms_pcm16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq = samples
        .iter()
        .map(|v| {
            let f = *v as f32 / i16::MAX as f32;
            f * f
        })
        .sum::<f32>();
    (sum_sq / samples.len() as f32).sqrt()
}

pub fn decode_wav_pcm16_mono(bytes: &[u8]) -> Result<(u32, Vec<i16>)> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(anyhow!("invalid wav header"));
    }

    let mut cursor = 12usize;
    let mut sample_rate = None;
    let mut channels = None;
    let mut bits_per_sample = None;
    let mut audio_format = None;
    let mut data_offset = None;
    let mut data_size = None;

    while cursor + 8 <= bytes.len() {
        let chunk_id = &bytes[cursor..cursor + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[cursor + 4],
            bytes[cursor + 5],
            bytes[cursor + 6],
            bytes[cursor + 7],
        ]) as usize;
        cursor += 8;

        if cursor + chunk_size > bytes.len() {
            break;
        }

        match chunk_id {
            b"fmt " => {
                if chunk_size >= 16 {
                    audio_format = Some(u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]));
                    channels = Some(u16::from_le_bytes([bytes[cursor + 2], bytes[cursor + 3]]));
                    sample_rate = Some(u32::from_le_bytes([
                        bytes[cursor + 4],
                        bytes[cursor + 5],
                        bytes[cursor + 6],
                        bytes[cursor + 7],
                    ]));
                    bits_per_sample =
                        Some(u16::from_le_bytes([bytes[cursor + 14], bytes[cursor + 15]]));
                }
            }
            b"data" => {
                data_offset = Some(cursor);
                data_size = Some(chunk_size);
            }
            _ => {}
        }

        cursor += chunk_size;
        if chunk_size % 2 != 0 {
            cursor += 1;
        }
    }

    let sample_rate = sample_rate.ok_or_else(|| anyhow!("wav fmt chunk missing sample rate"))?;
    let channels = channels.ok_or_else(|| anyhow!("wav fmt chunk missing channels"))?;
    let bits_per_sample = bits_per_sample.ok_or_else(|| anyhow!("wav fmt chunk missing bits"))?;
    let audio_format = audio_format.ok_or_else(|| anyhow!("wav fmt chunk missing format"))?;

    if audio_format != 1 || channels != 1 || bits_per_sample != 16 {
        return Err(anyhow!("unsupported wav format (requires PCM16 mono)"));
    }

    let (data_offset, data_size) = match (data_offset, data_size) {
        (Some(offset), Some(size)) => (offset, size),
        _ => {
            // Some ffmpeg/driver combinations can emit malformed chunk metadata while still
            // writing valid PCM payload after a canonical 44-byte header.
            if bytes.len() <= 44 {
                return Err(anyhow!("wav data chunk missing"));
            }
            (44usize, bytes.len() - 44)
        }
    };
    let data = &bytes[data_offset..data_offset + data_size];

    let mut samples = Vec::with_capacity(data.len() / 2);
    for chunk in data.chunks_exact(2) {
        samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }

    Ok((sample_rate, samples))
}

pub fn trim_with_vad(
    samples: &[i16],
    sample_rate: u32,
    energy_threshold: f32,
    silence_stop_ms: u64,
    min_voiced_ms: u64,
) -> Vec<i16> {
    if samples.is_empty() {
        return Vec::new();
    }

    let frame_len = (sample_rate / 50).max(1) as usize;
    let silence_frames_required = (silence_stop_ms / 20).max(1) as usize;
    let min_voiced_samples = (sample_rate as u64 * min_voiced_ms / 1000) as usize;

    let mut speech_start: Option<usize> = None;
    let mut speech_end = samples.len();
    let mut last_voiced_end = 0usize;
    let mut voiced_samples = 0usize;
    let mut trailing_silent_frames = 0usize;

    for (frame_index, frame) in samples.chunks(frame_len).enumerate() {
        let energy = rms_pcm16(frame);
        let frame_start = frame_index * frame_len;
        let frame_end = (frame_start + frame.len()).min(samples.len());

        if energy >= energy_threshold {
            if speech_start.is_none() {
                speech_start = Some(frame_start);
            }
            last_voiced_end = frame_end;
            voiced_samples += frame.len();
            trailing_silent_frames = 0;
        } else if speech_start.is_some() {
            trailing_silent_frames += 1;
            if voiced_samples >= min_voiced_samples
                && trailing_silent_frames >= silence_frames_required
            {
                speech_end = last_voiced_end;
                break;
            }
        }
    }

    let Some(start) = speech_start else {
        return Vec::new();
    };
    let end = speech_end.max(start + 1).min(samples.len());
    samples[start..end].to_vec()
}

pub fn encode_wav_pcm16_mono(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
    let data_size = (samples.len() * 2) as u32;
    let riff_size = 36 + data_size;

    let mut out = Vec::with_capacity((44 + data_size) as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());

    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_size.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }

    out
}

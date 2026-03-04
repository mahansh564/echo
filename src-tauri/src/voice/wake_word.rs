use anyhow::{anyhow, Result};

pub fn validate_wake_word_model(path: &str) -> Result<()> {
    if std::path::Path::new(path).exists() {
        return Ok(());
    }
    Err(anyhow!("wake word model missing at {}", path))
}

pub fn is_wake_detected(transcript: &str, wake_phrase: &str) -> bool {
    let normalized_text = normalize(transcript);
    let normalized_wake = normalize(wake_phrase);
    !normalized_wake.is_empty() && normalized_text.contains(&normalized_wake)
}

pub fn extract_command_after_wake(transcript: &str, wake_phrase: &str) -> Option<String> {
    let normalized_wake = normalize(wake_phrase);
    if normalized_wake.is_empty() {
        return None;
    }

    let lowered = transcript.to_lowercase();
    let wake_lower = wake_phrase.to_lowercase();
    let idx = lowered.find(&wake_lower)?;
    let tail_start = idx.saturating_add(wake_phrase.len()).min(transcript.len());
    let tail =
        transcript[tail_start..].trim_matches(|c: char| c.is_whitespace() || c == ',' || c == '.');
    if tail.is_empty() {
        None
    } else {
        Some(tail.to_string())
    }
}

fn normalize(input: &str) -> String {
    input
        .to_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch.is_whitespace() {
                ch
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_phrase_matches_transcript() {
        assert!(is_wake_detected("Echo create task write docs", "echo"));
        assert!(is_wake_detected("hey, ECHO!", "echo"));
        assert!(!is_wake_detected("hello there", "echo"));
    }

    #[test]
    fn extracts_tail_command() {
        assert_eq!(
            extract_command_after_wake("Echo create task write docs", "echo"),
            Some("create task write docs".to_string())
        );
        assert_eq!(extract_command_after_wake("echo", "echo"), None);
    }
}

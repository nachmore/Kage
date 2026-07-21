use std::io::Read;
use std::path::Path;

pub(super) fn extract_text_content(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter(|item| item.get("type").and_then(|kind| kind.as_str()) == Some("text"))
            .filter_map(|item| item.get("text").and_then(|text| text.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub(super) fn is_system_message(text: &str) -> bool {
    text.starts_with("<identity>")
        || (text.starts_with("Follow these instructions") && text.len() > 5000)
}

/// Strip steering/context wrappers from a `.chat` user message.
pub(super) fn extract_user_text_from_chat(text: &str) -> String {
    let text = text.trim();
    if text.starts_with("<identity>") {
        return String::new();
    }

    let mut user_text = text.to_string();
    if let Some(index) = user_text.rfind("</user-rule>") {
        let trimmed = user_text[index + "</user-rule>".len()..]
            .trim_start_matches('`')
            .trim_start_matches(['\n', '\r'])
            .trim();
        if trimmed.is_empty() {
            return String::new();
        }
        user_text = trimmed.to_string();
    }
    if let Some(index) = user_text.rfind("</steering-reminder>") {
        let trimmed = user_text[index + "</steering-reminder>".len()..].trim();
        if trimmed.is_empty() {
            return String::new();
        }
        user_text = trimmed.to_string();
    }
    if user_text.starts_with("## Included Rules") || user_text.starts_with("<steering-reminder>") {
        return String::new();
    }
    if let Some(index) = user_text.find("<EnvironmentContext>") {
        user_text = user_text[..index].trim().to_string();
    }
    if user_text.is_empty()
        || user_text.starts_with("<identity>")
        || user_text.starts_with("Follow these instructions")
    {
        String::new()
    } else {
        user_text
    }
}

/// Read at most `max_bytes` from a file's head, with a lossy boundary fallback.
pub(super) fn read_file_head(path: &Path, max_bytes: usize) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() as usize <= max_bytes {
        let mut content = String::new();
        file.read_to_string(&mut content).ok()?;
        return Some(content);
    }

    let mut bytes = vec![0; max_bytes];
    file.read_exact(&mut bytes).ok()?;
    String::from_utf8(bytes.clone())
        .ok()
        .or_else(|| Some(String::from_utf8_lossy(&bytes).into_owned()))
}

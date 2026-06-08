// Functional core: prompt formatting and title re-emit sequences.

pub fn format_status_prompt(exit_code: i32) -> String {
    format!(
        "\r\n[title-proxy] child exited with code {exit_code}; space=retry q=quit"
    )
}

pub fn title_reemit_sequence(title: &str) -> Vec<u8> {
    format!("\x1b]2;{title}\x07").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_includes_exit_code() {
        let p = format_status_prompt(7);
        assert!(p.contains("code 7"));
        assert!(p.contains("space=retry q=quit"));
    }

    #[test]
    fn prompt_starts_with_crlf() {
        let p = format_status_prompt(0);
        assert!(p.starts_with("\r\n"));
    }

    #[test]
    fn title_reemit_sequence_format() {
        let seq = title_reemit_sequence("my-title");
        assert_eq!(seq, b"\x1b]2;my-title\x07");
    }
}

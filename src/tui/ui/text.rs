use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// Cut `text` to `max_chars` characters, ending with an ellipsis.
pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// AniList returns HTML even with `asHtml: false`. Strip tags, keep paragraph
/// breaks, decode the entities that show up in synopses.
pub fn plain_text(html: &str) -> String {
    let with_breaks = html
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</p>", "\n\n");

    let mut out = String::with_capacity(with_breaks.len());
    let mut in_tag = false;
    for c in with_breaks.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }

    out.replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&amp;", "&")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

/// Free-text line, used where a value may be missing.
pub fn dim(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(Color::DarkGray))
}

#[cfg(test)]
mod tests {
    use super::plain_text;

    #[test]
    fn strips_tags_and_entities() {
        assert_eq!(
            plain_text("<b>A</b> &amp; <i>B</i>&#039;s"),
            "A & B's".to_string()
        );
    }

    #[test]
    fn keeps_paragraph_breaks() {
        assert_eq!(plain_text("one<br>two</p>three"), "one\ntwo\n\nthree");
    }
}

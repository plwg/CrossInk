//! Renders Anki card templates to plain text for the e-reader.
//!
//! Supports the template features ordinary decks use: `{{Field}}`,
//! `{{FrontSide}}`, sections (`{{#Field}}...{{/Field}}`, `{{^Field}}`),
//! filters (`cloze:`, `text:`, `hint:`, `type:`, furigana), and cloze
//! deletions. Styling, images and sound cannot be shown and are dropped.

use std::collections::HashMap;

/// A note's fields by name, plus what the template needs to know about the card.
pub struct RenderContext<'a> {
    pub fields: &'a HashMap<String, String>,
    /// 0-based card ordinal; cloze number `ord + 1` is the active one.
    pub ord: u32,
    pub is_cloze: bool,
}

/// Question and answer of a card as plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedCard {
    pub front: String,
    pub back: String,
}

pub fn render_card(qfmt: &str, afmt: &str, ctx: &RenderContext) -> RenderedCard {
    let question = render(qfmt, ctx, true, "");
    let answer = render(afmt, ctx, false, &question);
    // Answers usually repeat the question, then `<hr id=answer>`; the device
    // shows the question separately, so keep only what follows.
    let answer_only = split_after_answer_rule(&answer).unwrap_or(&answer);
    RenderedCard {
        front: html_to_text(&question),
        back: html_to_text(answer_only),
    }
}

fn split_after_answer_rule(html: &str) -> Option<&str> {
    let lower = html.to_ascii_lowercase();
    let start = lower
        .find("<hr id=answer")
        .or_else(|| lower.find("<hr id=\"answer\""))?;
    let end = lower[start..].find('>')? + start + 1;
    Some(&html[end..])
}

/// Renders template HTML. `front_side` replaces `{{FrontSide}}`.
fn render(template: &str, ctx: &RenderContext, question: bool, front_side: &str) -> String {
    let mut out = String::new();
    render_into(template, ctx, question, front_side, &mut out);
    out
}

fn render_into(
    template: &str,
    ctx: &RenderContext,
    question: bool,
    front_side: &str,
    out: &mut String,
) {
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open + 2..].find("}}") else {
            out.push_str(&rest[open..]);
            return;
        };
        let tag = rest[open + 2..open + 2 + close].trim();
        rest = &rest[open + 2 + close + 2..];

        if let Some(name) = tag.strip_prefix('#').or_else(|| tag.strip_prefix('^')) {
            let name = name.trim();
            let (inner, after) = split_section(rest, name);
            let present = field_is_present(name, ctx);
            if present == tag.starts_with('#') {
                render_into(inner, ctx, question, front_side, out);
            }
            rest = after;
        } else if tag.starts_with('/') || tag.starts_with('!') {
            // Stray section end or comment.
        } else {
            out.push_str(&replacement(tag, ctx, question, front_side));
        }
    }
    out.push_str(rest);
}

/// Splits at the matching `{{/name}}`, allowing nested sections of the same name.
fn split_section<'t>(text: &'t str, name: &str) -> (&'t str, &'t str) {
    let mut depth = 0usize;
    let mut pos = 0;
    while let Some(open) = text[pos..].find("{{") {
        let start = pos + open;
        let Some(close) = text[start + 2..].find("}}") else {
            break;
        };
        let tag = text[start + 2..start + 2 + close].trim();
        let end = start + 2 + close + 2;
        let tag_name = |prefix: char| tag.strip_prefix(prefix).map(str::trim);
        if tag_name('#') == Some(name) || tag_name('^') == Some(name) {
            depth += 1;
        } else if tag_name('/') == Some(name) {
            if depth == 0 {
                return (&text[..start], &text[end..]);
            }
            depth -= 1;
        }
        pos = end;
    }
    (text, "")
}

fn field_is_present(name: &str, ctx: &RenderContext) -> bool {
    // Sections may name a cloze number, e.g. {{#c2}}.
    if let Some(n) = name.strip_prefix('c').and_then(|n| n.parse::<u32>().ok())
        && ctx.is_cloze
        && !ctx.fields.contains_key(name)
    {
        return ctx
            .fields
            .values()
            .any(|v| !clozes_numbered(v, n).is_empty());
    }
    ctx.fields
        .get(name)
        .is_some_and(|v| !html_to_text(v).trim().is_empty())
}

fn replacement(tag: &str, ctx: &RenderContext, question: bool, front_side: &str) -> String {
    let mut parts: Vec<&str> = tag.split(':').map(str::trim).collect();
    let field_name = parts.pop().unwrap_or_default();
    if field_name == "FrontSide" {
        return front_side.to_string();
    }
    let Some(value) = ctx.fields.get(field_name) else {
        return String::new();
    };
    let mut text = value.clone();
    // Filters apply right to left, nearest the field first.
    for filter in parts.iter().rev() {
        text = match *filter {
            "cloze" => render_cloze(&text, ctx.ord + 1, question),
            "text" => html_to_text(&text),
            // Typing answers is not possible on the device; hints are shown.
            "type" | "cloze-only" => String::new(),
            "furigana" => strip_ruby(&text, Ruby::Both),
            "kana" => strip_ruby(&text, Ruby::Reading),
            "kanji" => strip_ruby(&text, Ruby::Base),
            _ => text,
        };
    }
    text
}

/// The `(start, end)` byte ranges of `{{cN::...}}` deletions numbered `n`.
fn clozes_numbered(text: &str, n: u32) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut pos = 0;
    while let Some(open) = text[pos..].find("{{c") {
        let start = pos + open;
        let digits: String = text[start + 3..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let after_digits = start + 3 + digits.len();
        if digits.is_empty() || !text[after_digits..].starts_with("::") {
            pos = start + 3;
            continue;
        }
        let Some(end) = text[after_digits..]
            .find("}}")
            .map(|e| after_digits + e + 2)
        else {
            break;
        };
        if digits.parse::<u32>().ok() == Some(n) {
            found.push((start, end));
        }
        pos = end;
    }
    found
}

/// Replaces cloze deletions: the active number becomes `[...]` (or
/// `[hint]`) on the question; everything else shows its text.
fn render_cloze(text: &str, active: u32, question: bool) -> String {
    let mut out = String::new();
    let mut pos = 0;
    while let Some(open) = text[pos..].find("{{c") {
        let start = pos + open;
        let digits: String = text[start + 3..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let body_start = start + 3 + digits.len() + 2;
        let valid = !digits.is_empty() && text[start + 3 + digits.len()..].starts_with("::");
        let end = valid
            .then(|| text[body_start..].find("}}"))
            .flatten()
            .map(|e| body_start + e);
        let Some(end) = end else {
            out.push_str(&text[pos..start + 3]);
            pos = start + 3;
            continue;
        };
        out.push_str(&text[pos..start]);
        let body = &text[body_start..end];
        let (answer, hint) = match body.split_once("::") {
            Some((answer, hint)) => (answer, Some(hint)),
            None => (body, None),
        };
        if question && digits.parse::<u32>().ok() == Some(active) {
            out.push('[');
            out.push_str(hint.unwrap_or("..."));
            out.push(']');
        } else {
            out.push_str(answer);
        }
        pos = end + 2;
    }
    out.push_str(&text[pos..]);
    out
}

/// What to keep of a ruby annotation such as `漢字[かんじ]` or `你[nǐ]`.
#[derive(Clone, Copy)]
enum Ruby {
    /// `漢字(かんじ)`: e-ink cannot set the reading above, so it follows.
    Both,
    Reading,
    Base,
}

/// Anki furigana syntax `漢字[かんじ]`, as the `furigana:`, `kana:` and
/// `kanji:` filters show it.
fn strip_ruby(text: &str, keep: Ruby) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    let mut base = String::new();
    while let Some(c) = chars.next() {
        if c == '[' {
            let reading: String = chars.by_ref().take_while(|&c| c != ']').collect();
            match keep {
                Ruby::Both => {
                    out.push_str(&base);
                    out.push('(');
                    out.push_str(&reading);
                    out.push(')');
                }
                Ruby::Reading => out.push_str(&reading),
                Ruby::Base => out.push_str(&base),
            }
            base.clear();
        } else if c == ' ' {
            out.push_str(&base);
            base.clear();
        } else {
            base.push(c);
        }
    }
    out.push_str(&base);
    out
}

/// Turns card HTML into plain text: block tags become line breaks, images
/// become `[image]`, sound and other tags are dropped, entities decoded.
pub fn html_to_text(html: &str) -> String {
    let mut text = String::new();
    let mut rest = html;
    while let Some(open) = rest.find(['<', '[']) {
        text.push_str(&rest[..open]);
        rest = &rest[open..];
        if rest.starts_with("[sound:") {
            rest = rest.find(']').map_or("", |end| &rest[end + 1..]);
            continue;
        }
        if rest.starts_with('[') {
            text.push('[');
            rest = &rest[1..];
            continue;
        }
        let Some(close) = rest.find('>') else {
            text.push_str(rest);
            rest = "";
            break;
        };
        let tag = rest[1..close].trim().to_ascii_lowercase();
        rest = &rest[close + 1..];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        match name.as_str() {
            // <rp> holds fallback parentheses; <rt> gets its own below.
            "style" | "script" | "rp" if !tag.starts_with('/') => {
                let end_tag = format!("</{name}");
                let lower = rest.to_ascii_lowercase();
                rest = lower
                    .find(&end_tag)
                    .and_then(|i| rest[i..].find('>').map(|j| &rest[i + j + 1..]))
                    .unwrap_or("");
            }
            "br" | "div" | "p" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "hr" | "ul"
            | "ol" | "table" => text.push('\n'),
            "li" if !tag.starts_with('/') => text.push_str("\n• "),
            "img" => text.push_str("[image]"),
            // Ruby text (pinyin, furigana) follows its base in parentheses.
            "rt" => text.push(if tag.starts_with('/') { ')' } else { '(' }),
            "td" | "th" if !tag.starts_with('/') => text.push(' '),
            _ => {}
        }
    }
    text.push_str(rest);
    tidy_whitespace(&decode_entities(&text))
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest.find(';').filter(|&e| e <= 10);
        let decoded = end.and_then(|e| {
            let entity = &rest[1..e];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ => entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Collapses runs of spaces, trims each line and drops blank lines at the
/// ends and beyond one in a row.
fn tidy_whitespace(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() && lines.last().is_none_or(String::is_empty) {
            continue;
        }
        lines.push(collapsed);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn card(
        qfmt: &str,
        afmt: &str,
        f: &HashMap<String, String>,
        ord: u32,
        cloze: bool,
    ) -> RenderedCard {
        render_card(
            qfmt,
            afmt,
            &RenderContext {
                fields: f,
                ord,
                is_cloze: cloze,
            },
        )
    }

    #[test]
    fn basic_card_answer_excludes_question() {
        let f = fields(&[("Front", "der Hund"), ("Back", "the <b>dog</b>")]);
        let c = card(
            "{{Front}}",
            "{{FrontSide}}\n\n<hr id=answer>\n\n{{Back}}",
            &f,
            0,
            false,
        );
        assert_eq!(c.front, "der Hund");
        assert_eq!(c.back, "the dog");
    }

    #[test]
    fn answer_without_rule_is_kept_whole() {
        let f = fields(&[("Front", "a"), ("Back", "b")]);
        let c = card("{{Front}}", "{{Back}}", &f, 0, false);
        assert_eq!(c.back, "b");
    }

    #[test]
    fn sections_and_missing_fields() {
        let f = fields(&[("Front", "q"), ("Extra", ""), ("Note", "n")]);
        let q = "{{Front}}{{#Extra}} E:{{Extra}}{{/Extra}}{{^Extra}} none{{/Extra}}{{#Note}} N:{{Note}}{{/Note}}{{Missing}}";
        assert_eq!(card(q, "", &f, 0, false).front, "q none N:n");
    }

    #[test]
    fn nested_sections_of_same_name() {
        let f = fields(&[("A", "x")]);
        assert_eq!(
            card("{{#A}}1{{#A}}2{{/A}}3{{/A}}4", "", &f, 0, false).front,
            "1234"
        );
    }

    #[test]
    fn cloze_question_and_answer() {
        let f = fields(&[(
            "Text",
            "{{c1::Paris}} is the capital of {{c2::France::country}}.",
        )]);
        let q = "{{cloze:Text}}";
        let a = "{{cloze:Text}}";
        let first = card(q, a, &f, 0, true);
        assert_eq!(first.front, "[...] is the capital of France.");
        assert_eq!(first.back, "Paris is the capital of France.");
        let second = card(q, a, &f, 1, true);
        assert_eq!(second.front, "Paris is the capital of [country].");
    }

    #[test]
    fn cloze_number_sections() {
        let f = fields(&[("Text", "{{c2::x}}")]);
        assert_eq!(
            card("{{#c2}}yes{{/c2}}{{#c1}}no{{/c1}}", "", &f, 1, true).front,
            "yes"
        );
    }

    #[test]
    fn type_filter_and_hints() {
        let f = fields(&[("Front", "q"), ("Back", "a")]);
        let c = card("{{Front}}\n{{type:Back}}", "{{hint:Back}}", &f, 0, false);
        assert_eq!(c.front, "q");
        assert_eq!(c.back, "a");
    }

    #[test]
    fn furigana_filters() {
        let f = fields(&[("W", "日本[にほん] 語[ご]")]);
        assert_eq!(card("{{kana:W}}", "", &f, 0, false).front, "にほんご");
        assert_eq!(card("{{kanji:W}}", "", &f, 0, false).front, "日本語");
        assert_eq!(
            card("{{furigana:W}}", "", &f, 0, false).front,
            "日本(にほん)語(ご)"
        );
        let zh = fields(&[("W", "你[nǐ]好[hǎo]")]);
        assert_eq!(
            card("{{furigana:W}}", "", &zh, 0, false).front,
            "你(nǐ)好(hǎo)"
        );
    }

    #[test]
    fn html_ruby_and_chinese_text() {
        assert_eq!(
            html_to_text(
                "<ruby>你<rp>(</rp><rt>nǐ</rt><rp>)</rp></ruby><ruby>好<rt>hǎo</rt></ruby>"
            ),
            "你(nǐ)好(hǎo)"
        );
        assert_eq!(
            html_to_text("学习&nbsp;中文，<br>Learn Chinese。"),
            "学习 中文，\nLearn Chinese。"
        );
    }

    #[test]
    fn html_to_text_handles_blocks_entities_media() {
        let html = "<div>one&nbsp;&amp; two</div><div><br></div><div>three &lt;3 &#233; &#x263A;</div>\
                    <img src=\"a.png\">[sound:a.mp3]<style>.x{}</style><ul><li>a</li><li>b</li></ul>";
        assert_eq!(
            html_to_text(html),
            "one & two\n\nthree <3 é ☺\n[image]\n\n• a\n• b"
        );
    }

    #[test]
    fn html_to_text_keeps_plain_brackets_and_ampersands() {
        assert_eq!(
            html_to_text("a [b] & c &unknown; d"),
            "a [b] & c &unknown; d"
        );
    }
}

// SPDX-License-Identifier: Apache-2.0
//! `{{ path }}` placeholders in text (a run's headline, a mock fixture), replaced once, left to right: a substituted value
//! is never read again (a parameter holding `{{params.x}}` is text, not a template), and the result is capped.

/// The longest text a template renders to.
const MAX_LEN: usize = 64 * 1024;

/// Replaces each `{{ path }}` of `t` with `value(path)` (the path trimmed); an unclosed `{{` is kept as text.
pub fn render(t: &str, mut value: impl FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(s) = rest.find("{{") {
        let Some(e) = rest[s + 2..].find("}}") else { break };
        out.push_str(&rest[..s]);
        out.push_str(&value(rest[s + 2..s + 2 + e].trim()));
        rest = &rest[s + 4 + e..];
        if out.len() > MAX_LEN { rest = ""; break; }
    }
    out.push_str(rest);
    if out.len() > MAX_LEN {
        let mut cut = MAX_LEN;
        while !out.is_char_boundary(cut) { cut -= 1; }
        out.truncate(cut);
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_replaced_once() {
        let v = |p: &str| match p { "a" => "{{a}}".to_string(), "b" => "x".repeat(40_000), "n" => "1".into(), _ => String::new() };
        assert_eq!(render("id {{ n }} of {{n}}", v), "id 1 of 1");
        assert_eq!(render("{{a}}{{a}}", v), "{{a}}{{a}}", "a value is not a template");
        assert_eq!(render("open {{ n", v), "open {{ n");
        assert_eq!(render("{{}}!", v), "!");
        let long = render("{{b}}{{b}}{{b}}", v);
        assert!(long.len() <= MAX_LEN + '…'.len_utf8() && long.ends_with('…'));
    }
}

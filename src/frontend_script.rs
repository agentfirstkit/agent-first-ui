//! The other half of "an override may not ship code".
//!
//! [`crate::UiFrontend`] refuses a frontend *file* whose name says it is a
//! script. This refuses a script hidden inside one whose name does not — an
//! overridden HTML template with a `<script>` block, an `onclick=` attribute,
//! or a `javascript:` URL.
//!
//! It is still delivery rather than rendering: nothing here knows what a page
//! means, only that these bytes may not leave a frontend directory. The rule
//! has to be identical across Providers, because what it protects is identical
//! across Providers — each one binds its own behaviour to its own page with a
//! nonce, and that is only worth anything if a frontend cannot run code beside
//! it. This is one implementation of that rule, in the one place every
//! Provider inherits it from rather than reimplements it.
//!
//! Nothing above this line needs a server, which is why it is in the featureless
//! tier. Two copies of a security rule drift silently, and the only thing that
//! had kept a Provider on its own copy was that taking this crate at all used
//! to mean taking axum and tokio with it.

use crate::{Error, Result};

const FORBIDDEN_ELEMENTS: [&str; 5] = ["<script", "</script", "<iframe", "<object", "<embed"];

/// The ways a Jinja-family template switches HTML escaping off.
///
/// Read after whitespace is removed, so `{% filter safe %}`, `{{ x |   safe }}`
/// and `{% autoescape\nfalse %}` are all the same three needles.
///
/// Not a stray extra rule: every Provider that renders a frontend template
/// renders it with escaping forced on and the `safe` filter removed, because
/// the values it prints are domain values — a subject line, a table name, a
/// payee. A template that could turn escaping back off would make the frontend
/// the thing that decides whether those values are markup, which is exactly
/// what the rest of this file exists to prevent. It is one rule, so it is one
/// place.
const ESCAPE_HATCHES: [(&str, &str); 3] = [
    ("|safe", "the `safe` filter"),
    ("filtersafe", "a `{% filter safe %}` block"),
    ("autoescape", "an `{% autoescape %}` block"),
];

/// Refuse markup a frontend file may not supply.
///
/// `label` names the file for the person who has to fix it. This is the strict
/// reading, for text a person wrote: an executing URL scheme anywhere in the
/// file is a violation, whether or not it currently sits inside a tag, because
/// a template can move it into one, and a template directive that disables
/// escaping is a violation for the same reason.
pub fn reject_frontend_script(label: &str, markup: &str) -> Result<()> {
    let lower = markup.to_ascii_lowercase();
    let violation = forbidden_element(&lower)
        .or_else(|| dangerous_url_scheme(&lower))
        .or_else(|| escape_hatch(&lower))
        .or_else(|| tag_violation(&lower));
    refuse(label, violation)
}

/// The same refusal applied to a page a template already produced.
///
/// Deliberately narrower than [`reject_frontend_script`]: after rendering, the
/// text contains domain values — a SQL statement, a subject line — and
/// `data:` inside one of those is a string a person is meant to read, not a URL
/// a browser will fetch. So this reads elements and attributes, which is where
/// a rendered value could actually become behaviour, and leaves prose alone.
pub fn reject_rendered_script(label: &str, markup: &str) -> Result<()> {
    let lower = markup.to_ascii_lowercase();
    let violation = forbidden_element(&lower).or_else(|| tag_violation(&lower));
    refuse(label, violation)
}

fn refuse(label: &str, violation: Option<&'static str>) -> Result<()> {
    match violation {
        Some(violation) => Err(Error::FrontendScript {
            label: label.to_owned(),
            violation,
        }),
        None => Ok(()),
    }
}

fn forbidden_element(lower: &str) -> Option<&'static str> {
    FORBIDDEN_ELEMENTS
        .iter()
        .find(|needle| lower.contains(**needle))
        .copied()
}

fn escape_hatch(lower: &str) -> Option<&'static str> {
    let compact = lower
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    ESCAPE_HATCHES
        .iter()
        .find(|(needle, _)| compact.contains(needle))
        .map(|(_, violation)| *violation)
}

fn tag_violation(text: &str) -> Option<&'static str> {
    let mut remaining = text;
    while let Some(start) = remaining.find('<') {
        let tag_and_rest = &remaining[start..];
        let Some(end) = tag_and_rest.find('>') else {
            break;
        };
        let tag = &tag_and_rest[..=end];
        let violation = if contains_inline_event_handler(tag) {
            Some("inline event handler")
        } else {
            dangerous_url_scheme(tag)
        };
        if violation.is_some() {
            return violation;
        }
        remaining = &tag_and_rest[end + 1..];
    }
    None
}

/// A URL scheme that executes, found through the encodings that hide one.
///
/// Character references and interior whitespace are both legal in an attribute
/// value and both survive to the browser, so `java&#115;cript&colon;` is the
/// same URL as `javascript:` and has to be read as one.
fn dangerous_url_scheme(text: &str) -> Option<&'static str> {
    let decoded = decode_html_character_references(text);
    let compact = decoded
        .chars()
        .filter(|character| !character.is_whitespace() && !character.is_control())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if compact.contains("javascript:") {
        Some("javascript:")
    } else if compact.contains("vbscript:") {
        Some("vbscript:")
    } else if compact.contains("data:") {
        Some("data:")
    } else {
        None
    }
}

fn decode_html_character_references(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < text.len() {
        let remaining = &text[index..];
        if let Some((character, consumed)) = decode_numeric_character_reference(remaining) {
            output.push(character);
            index += consumed;
            continue;
        }
        let named = [
            ("&colon;", ':'),
            ("&colon", ':'),
            ("&tab;", '\t'),
            ("&tab", '\t'),
            ("&newline;", '\n'),
            ("&newline", '\n'),
        ]
        .into_iter()
        .find(|(entity, _)| {
            remaining
                .get(..entity.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(entity))
        });
        if let Some((entity, character)) = named {
            output.push(character);
            index += entity.len();
            continue;
        }
        let Some(character) = remaining.chars().next() else {
            break;
        };
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn decode_numeric_character_reference(text: &str) -> Option<(char, usize)> {
    let (digits_start, radix) = if text
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("&#x"))
    {
        (3, 16)
    } else if text.starts_with("&#") {
        (2, 10)
    } else {
        return None;
    };
    let digits_len = text[digits_start..]
        .bytes()
        .take_while(|byte| match radix {
            16 => byte.is_ascii_hexdigit(),
            _ => byte.is_ascii_digit(),
        })
        .count();
    if digits_len == 0 {
        return None;
    }
    let digits_end = digits_start + digits_len;
    let value = u32::from_str_radix(&text[digits_start..digits_end], radix).ok()?;
    let character = char::from_u32(value)?;
    let consumed = digits_end + usize::from(text[digits_end..].starts_with(';'));
    Some((character, consumed))
}

fn contains_inline_event_handler(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if !bytes[index].is_ascii_whitespace() && !matches!(bytes[index], b'/' | b'<') {
            index += 1;
            continue;
        }
        let mut cursor = index + 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor + 2 > bytes.len() || &bytes[cursor..cursor + 2] != b"on" {
            index = cursor;
            continue;
        }
        cursor += 2;
        let name_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || matches!(bytes[cursor], b'-' | b'_'))
        {
            cursor += 1;
        }
        if cursor == name_start {
            index = cursor;
            continue;
        }
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if bytes.get(cursor) == Some(&b'=') {
            return true;
        }
        index = cursor;
    }
    false
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_markup_passes() {
        reject_frontend_script(
            "page",
            "<main class=\"x\"><p>hello</p><a href=\"/x\">go</a></main>",
        )
        .unwrap();
        // A semantic hook is exactly what a Provider asks an override to write,
        // so declaring one must not read as behaviour.
        reject_frontend_script(
            "page",
            "<button data-provider-decision=\"approve\">Run</button>",
        )
        .unwrap();
    }

    #[test]
    fn every_way_an_override_could_run_code_is_refused() {
        for markup in [
            "<script>alert(1)</script>",
            "<iframe src=\"/x\"></iframe>",
            "<object data=\"x\"></object>",
            "<embed src=\"x\">",
            "<button onclick=\"go()\">x</button>",
            "<button  ON-Click = 'go()'>x</button>",
            "<a href=\"javascript:alert(1)\">x</a>",
            // The encodings that hide the same URL from a plain substring test.
            "<a href=\"java&#115;cript&colon;alert(1)\">x</a>",
            "<a href=\"jav\tascript:alert(1)\">x</a>",
            "<a href=\"vbscript:x\">x</a>",
            "<a href=\"data:text/html,<script>\">x</a>",
        ] {
            let error = reject_frontend_script("page", markup)
                .unwrap_err()
                .to_string();
            assert!(error.contains("page"), "{markup}: {error}");
        }
    }

    /// Turning escaping off is how a template makes a domain value into markup
    /// without containing any markup itself, so it is refused by the same call.
    #[test]
    fn a_template_may_not_switch_html_escaping_off() {
        for markup in [
            "{{ value | safe }}",
            "{{ value |   safe }}",
            "{% filter safe %}{{ value }}{% endfilter %}",
            "{% autoescape\nfalse %}{{ value }}{% endautoescape %}",
            "{% AutoEscape false %}{{ value }}{% endautoescape %}",
        ] {
            let error = reject_frontend_script("page", markup)
                .unwrap_err()
                .to_string();
            assert!(error.contains("page"), "{markup}: {error}");
        }
        // The words themselves are ordinary prose in a rendered page — a query
        // that mentions `autoescape` is a value a person is meant to read.
        reject_rendered_script("page", "<pre>select 'autoescape', x | safe</pre>").unwrap();
    }

    /// A rendered page carries domain values, and a person's own text is not a
    /// URL. `data:` in a SQL statement must not be read as one — while the same
    /// bytes inside an attribute still are.
    #[test]
    fn a_rendered_value_is_read_as_prose_but_an_attribute_is_still_read_as_a_url() {
        reject_rendered_script("page", "<pre>select 'data:image/png;base64,AA'</pre>").unwrap();
        assert!(reject_frontend_script("page", "select 'data:image/png'").is_err());
        assert!(reject_rendered_script("page", "<a href=\"data:text/html,x\">x</a>").is_err());
    }
}

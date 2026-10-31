//! The other half of "an override may not ship code".
//!
//! [`crate::UiFrontend`] refuses a frontend *file* whose name says it is a
//! script. This refuses a script hidden inside one whose name does not — an
//! overridden HTML template with a `<script>` block, an `onclick=` attribute,
//! or a `javascript:` URL — and the markup that runs no script but still sends
//! the person, a request, or the page's address somewhere: a `<meta>` refresh
//! or referrer policy, a `referrerpolicy` attribute, a form that posts by
//! itself, a `<base>` that re-points every relative URL.
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
//! tier. Two copies of a security rule drift silently, and a Provider whose UI
//! is compiled out should not have to take axum and tokio to share this one.

use crate::{Error, Result};
use scraper::{Html, Node};

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
        .or_else(|| document_violation(&Html::parse_document(markup)));
    refuse(label, violation)
}

/// The same refusal applied to a page a template already produced.
///
/// Deliberately narrower than [`reject_frontend_script`]: after rendering, the
/// text contains domain values — a SQL statement, a subject line — and
/// `data:` inside one of those is a string a person is meant to read, not a URL
/// a browser will fetch. So this reads elements and attributes, which is where
/// a rendered value could actually become behaviour, and leaves prose alone.
/// A URL scheme counts only in an attribute a browser loads or navigates to:
/// a `title` or `aria-label` is text, wherever it sits.
pub fn reject_rendered_script(label: &str, markup: &str) -> Result<()> {
    inspect_rendered_page(label, markup).map(drop)
}

/// Parse and inspect once so [`crate::UiPage`] can reuse the browser-shaped
/// tree for its element and marker contracts.
pub(crate) fn inspect_rendered_page(label: &str, markup: &str) -> Result<Html> {
    let document = Html::parse_document(markup);
    refuse(label, document_violation(&document))?;
    Ok(document)
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

/// Elements that run code, load another document, or re-point every relative
/// URL on the page.
const EXECUTABLE_ELEMENTS: [&str; 8] = [
    "script", "iframe", "object", "embed", "frame", "frameset", "applet", "base",
];

/// Attributes that make a form send a request by itself. A page's forms are
/// read by its own runtime; one that posts on its own goes to whatever route
/// it names, past the decision a button is labelled with.
const REQUEST_ATTRIBUTES: [&str; 4] = ["action", "formaction", "method", "formmethod"];

/// Attributes whose value a browser loads or navigates to, including the SVG
/// animation attributes that can set an `href`. Only these are read as URLs:
/// `title="Metadata: Q3"` is text a person reads.
const URL_ATTRIBUTES: [&str; 20] = [
    "href",
    "xlink:href",
    "src",
    "srcset",
    "poster",
    "data",
    "ping",
    "background",
    "cite",
    "longdesc",
    "manifest",
    "icon",
    "lowsrc",
    "dynsrc",
    "codebase",
    "archive",
    "to",
    "from",
    "by",
    "values",
];

fn document_violation(document: &Html) -> Option<&'static str> {
    for node in document.tree.nodes() {
        let Node::Element(element) = node.value() else {
            continue;
        };
        if EXECUTABLE_ELEMENTS.contains(&element.name()) {
            return Some("executable element");
        }
        // A refresh navigates without a click, and a referrer policy would
        // undo the `no-referrer` that keeps this page's credentialed URL here.
        if element.name() == "meta"
            && (element.attr("http-equiv").is_some_and(|equiv| {
                let equiv = equiv.trim();
                equiv.eq_ignore_ascii_case("refresh") || equiv.eq_ignore_ascii_case("set-cookie")
            }) || element
                .attr("name")
                .is_some_and(|name| name.trim().eq_ignore_ascii_case("referrer")))
        {
            return Some("a `<meta>` that navigates or sets page policy");
        }
        for (name, value) in element.attrs() {
            let name = name.to_ascii_lowercase();
            if name
                .strip_prefix("on")
                .is_some_and(|suffix| !suffix.is_empty())
            {
                return Some("inline event handler");
            }
            // `method="dialog"` closes a `<dialog>` and sends nothing.
            if REQUEST_ATTRIBUTES.contains(&name.as_str())
                && !(name.ends_with("method") && value.trim().eq_ignore_ascii_case("dialog"))
            {
                return Some("a form that sends a request by itself");
            }
            if name == "referrerpolicy" {
                return Some("a `referrerpolicy` override");
            }
            if URL_ATTRIBUTES.contains(&name.as_str())
                && let Some(violation) = dangerous_url_scheme(value)
            {
                return Some(violation);
            }
        }
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
            // A quoted `>` does not end a tag for the browser.  The old
            // hand-written scanner stopped there and missed the handler.
            "<button title=\">\" onclick=\"go()\">x</button>",
            "<a href=\"javascript:alert(1)\">x</a>",
            // The encodings that hide the same URL from a plain substring test.
            "<a href=\"java&#115;cript&colon;alert(1)\">x</a>",
            "<a href=\"jav\tascript:alert(1)\">x</a>",
            "<a href=\"vbscript:x\">x</a>",
            "<a href=\"data:text/html,<script>\">x</a>",
            "<frameset><frame src=\"/x\"></frameset>",
            "<base href=\"https://elsewhere.example/\">",
            "<svg><a><animate attributeName=\"href\" values=\"javascript:alert(1)\"/></a></svg>",
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
        // Text in an attribute that is not a URL stays text.
        reject_rendered_script(
            "page",
            "<p title=\"Metadata: Q3\" aria-label=\"javascript: a primer\">x</p>",
        )
        .unwrap();
    }

    /// Markup that runs no script but still sends the person, a request or
    /// this page's address somewhere: a refresh, a referrer override, a form
    /// that posts by itself.
    #[test]
    fn markup_that_navigates_posts_or_leaks_the_referrer_is_refused() {
        for markup in [
            "<meta http-equiv=\"refresh\" content=\"0;url=https://elsewhere.example/\">",
            "<meta name=\"referrer\" content=\"unsafe-url\">",
            "<a referrerpolicy=\"unsafe-url\" href=\"https://elsewhere.example/\">x</a>",
            "<form method=\"post\" action=\"refuse\"><button>Approve</button></form>",
            "<form><button formaction=\"refuse\">Approve</button></form>",
        ] {
            assert!(reject_rendered_script("page", markup).is_err(), "{markup}");
        }
        // A form the page's runtime reads, a dialog's own form, and the
        // metadata a page needs — a policy `<meta>` can only narrow a policy.
        reject_rendered_script(
            "page",
            "<meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">\
             <meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\">\
             <meta http-equiv=\"Content-Security-Policy\" content=\"img-src 'self'\">\
             <form data-provider-form><button data-provider-decision=\"approve\">Approve</button></form>\
             <dialog><form method=\"dialog\"><button>Close</button></form></dialog>",
        )
        .unwrap();
        assert!(
            reject_rendered_script("page", "<meta http-equiv=\"Set-Cookie\" content=\"a=b\">")
                .is_err()
        );
    }
}

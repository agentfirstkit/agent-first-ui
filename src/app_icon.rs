use crate::{Error, Result};

/// The conventional frontend asset a Provider uses for its application icon.
///
/// It lives in the frontend overlay rather than in AFUI's window launcher:
/// the page whose identity the window carries owns the icon, just as it owns
/// its templates and stylesheet. A replacement frontend may therefore supply
/// `assets/app-icon.svg`, and the exact bytes it supplies are already covered
/// by that frontend's trusted fingerprint.
pub const APP_ICON_ASSET_PATH: &str = "assets/app-icon.svg";

const MAX_APP_ICON_BYTES: usize = 256 * 1024;

/// One Provider-owned SVG application icon, after its frontend overlay has
/// been resolved.
///
/// SVG is intentional here: it is editable as part of a frontend template,
/// scales cleanly from a browser favicon to a desktop Dock icon, and can be
/// embedded in a published crate without a binary asset generation step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiAppIcon(String);

impl UiAppIcon {
    /// Validate Provider-owned SVG before a session or window starts.
    pub fn from_svg(svg: impl Into<String>) -> Result<Self> {
        let svg = svg.into();
        let trimmed = svg.trim_start();
        if svg.len() > MAX_APP_ICON_BYTES {
            return Err(Error::InvalidAppIcon {
                reason: "the SVG is larger than 256 KiB",
            });
        }
        if svg.contains('\0') {
            return Err(Error::InvalidAppIcon {
                reason: "the SVG contains a NUL byte",
            });
        }
        if !contains_svg_root(trimmed) {
            return Err(Error::InvalidAppIcon {
                reason: "the document has no SVG root element",
            });
        }
        Ok(Self(svg))
    }

    #[must_use]
    pub fn as_svg(&self) -> &str {
        &self.0
    }
}

fn contains_svg_root(source: &str) -> bool {
    source.match_indices("<svg").any(|(offset, _)| {
        source[offset + 4..]
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_whitespace() || character == '>')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_svg_icon_may_include_an_xml_declaration() {
        let icon = UiAppIcon::from_svg(
            "<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
        )
        .unwrap();
        assert!(icon.as_svg().contains("<svg"));
    }

    #[test]
    fn text_that_only_mentions_svg_is_not_an_icon() {
        assert!(UiAppIcon::from_svg("not <svg-shaped> markup").is_err());
    }
}

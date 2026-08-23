use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use agent_first_slug::{SlugValidationPolicy, validate_slug};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::{APP_ICON_ASSET_PATH, Error, Result, UiAppIcon};

pub const SAFE_MODE_ENV: &str = "AFUI_SAFE_MODE";
pub const CONFIG_DIR_ENV: &str = "AFUI_CONFIG_DIR";

/// The sentence every "this frontend will not load" error ends with.
///
/// Providers attach it as the hint on their own error type. It exists as one
/// constant because the way out has to be the same sentence everywhere: a
/// person reading it has already lost the UI they expected, and a second
/// vocabulary for the escape hatch is the last thing that helps.
pub const SAFE_MODE_HINT: &str = "Fix the frontend with `afui frontend check`, or set \
     AFUI_SAFE_MODE=1 to ignore every user frontend and open the Provider's built-in UI.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontendScope {
    Workspace,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontendManifest {
    pub frontend_id: String,
    pub ui_api_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontendLocation {
    pub scope: FrontendScope,
    pub root: PathBuf,
    pub manifest: FrontendManifest,
}

pub fn safe_mode_from_env() -> bool {
    std::env::var_os(SAFE_MODE_ENV).is_some_and(|value| {
        let value = value.to_string_lossy();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

pub fn frontend_root(
    scope: FrontendScope,
    workspace_root: &Path,
    provider_id: &str,
    ui_kind: &str,
) -> Result<PathBuf> {
    validate_frontend_identifier("provider_id", provider_id)?;
    validate_frontend_identifier("ui_kind", ui_kind)?;
    let base = match scope {
        FrontendScope::Workspace => workspace_root.join(".afui"),
        FrontendScope::Global => global_afui_root()?,
    };
    Ok(base.join("frontends").join(provider_id).join(ui_kind))
}

pub fn discover_frontend(
    workspace_root: &Path,
    provider_id: &str,
    ui_kind: &str,
    safe_mode: bool,
) -> Result<Option<FrontendLocation>> {
    if safe_mode {
        return Ok(None);
    }
    for scope in [FrontendScope::Workspace, FrontendScope::Global] {
        let root = frontend_root(scope, workspace_root, provider_id, ui_kind)?;
        if root.join("frontend.json").is_file() {
            if scope == FrontendScope::Workspace
                && !workspace_frontend_is_enabled(&root, provider_id, ui_kind)?
            {
                continue;
            }
            return load_frontend(scope, root).map(Some);
        }
    }
    Ok(None)
}

fn workspace_frontend_is_enabled(root: &Path, provider_id: &str, ui_kind: &str) -> Result<bool> {
    #[derive(Deserialize)]
    struct TrustStore {
        #[serde(default)]
        frontends: BTreeMap<String, TrustEntry>,
    }
    #[derive(Deserialize)]
    struct TrustEntry {
        fingerprint: String,
    }
    let trust_path = global_afui_root()?.join("trust.json");
    if !trust_path.is_file() {
        return Ok(false);
    }
    let bytes = fs::read(trust_path).map_err(Error::Io)?;
    let trust: TrustStore =
        serde_json::from_slice(&bytes).map_err(|error| Error::FrontendManifest {
            message: format!("invalid trust.json: {error}"),
        })?;
    let key = format!("workspace:{provider_id}:{ui_kind}");
    let Some(entry) = trust.frontends.get(&key) else {
        return Ok(false);
    };
    Ok(entry.fingerprint == frontend_fingerprint(root)?)
}

pub(crate) fn frontend_fingerprint(root: &Path) -> Result<String> {
    let mut files = BTreeMap::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(entry.path()).map_err(Error::Io)?;
        files.insert(relative, blake3::hash(&bytes).to_hex().to_string());
    }
    let mut hasher = blake3::Hasher::new();
    for (path, hash) in files {
        hasher.update(path.as_bytes());
        hasher.update(&[0]);
        hasher.update(hash.as_bytes());
        hasher.update(&[0]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn load_frontend(scope: FrontendScope, root: PathBuf) -> Result<FrontendLocation> {
    let bytes = fs::read(root.join("frontend.json")).map_err(Error::Io)?;
    let manifest: FrontendManifest =
        serde_json::from_slice(&bytes).map_err(|error| Error::FrontendManifest {
            message: error.to_string(),
        })?;
    validate_frontend_identifier("frontend_id", &manifest.frontend_id)?;
    if manifest.ui_api_version.trim().is_empty() {
        return Err(Error::FrontendManifest {
            message: "ui_api_version must not be empty".to_owned(),
        });
    }
    Ok(FrontendLocation {
        scope,
        root,
        manifest,
    })
}

pub fn global_afui_root() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os(CONFIG_DIR_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    dirs::config_dir()
        .map(|root| root.join("afui"))
        .ok_or(Error::ConfigDirectoryUnavailable)
}

pub fn validate_frontend_identifier(field: &'static str, value: &str) -> Result<()> {
    validate_identifier(field, value)
}

/// The one grammar a `provider_id` and a `ui_kind` have to satisfy.
///
/// It lives here rather than beside [`crate::UiSession`] because a frontend
/// directory is named by these two strings, so the rule has to exist in a build
/// that has no sessions in it at all. A session validates the same names
/// through this function: one grammar, because a session and the frontend
/// serving it are addressed by the same segments.
pub(crate) fn validate_identifier(field: &'static str, value: &str) -> Result<()> {
    let mut characters = value.chars();
    let starts_correctly = characters
        .next()
        .is_some_and(|character| character.is_ascii_lowercase());
    let rest_is_valid = characters.all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-'
    });
    let is_url_segment = validate_slug(value, SlugValidationPolicy::UrlPathSegment).is_ok();
    if is_url_segment && starts_correctly && rest_is_valid && value.len() <= 64 {
        Ok(())
    } else {
        Err(Error::InvalidIdentifier {
            field,
            value: value.to_owned(),
        })
    }
}

// ═══════════════════════════════════════════
// Delivery: which bytes a Provider UI is built from
// ═══════════════════════════════════════════

/// File extensions an override may never supply, whatever it calls the file.
///
/// This is the one place "no overridden JavaScript" is enforced rather than
/// described. A Provider's behaviour is its own — typically a nonce-bound
/// trusted runtime its controls are wired to — and that
/// only means anything if a person's frontend cannot ship code of its own
/// beside it. Refusing by extension is deliberately blunt: it is a delivery
/// rule about which bytes may leave this crate, not an attempt to read a
/// language.
const SCRIPT_EXTENSIONS: [&str; 5] = ["js", "mjs", "cjs", "wasm", "jsx"];

/// Where one Provider UI's files come from: a person's override, or nowhere.
///
/// This is the delivery half of a frontend, and it is deliberately only that.
/// It resolves whether a trusted override exists for a `provider_id` +
/// `ui_kind`, hands back a named file when the override supplies one, serves
/// the override's static assets, and turns "unloadable" or "wrong
/// `ui_api_version`" into an error that names safe mode. It never renders,
/// never templates, and never sees a domain value: the Provider reads
/// [`UiFrontend::text`] for a file it knows the meaning of, falls back to its
/// own built-in when the answer is `None`, and does the rendering itself.
///
/// The overlay is per file, not per directory. Replacing a directory would
/// make a person who wants to change one page responsible for shipping every
/// other file the Provider ships — a full set of language packs, say — so a
/// one-page customisation would silently delete i18n.
/// Every trusted file, read once, at the moment trust was decided.
///
/// The fingerprint is computed over the whole directory at resolve; serving
/// then used to `fs::read` the path again per request, so a long-running
/// process kept serving templates, CSS and assets that had been edited *after*
/// the check — while the documentation said any later edit revoked trust. It
/// also meant `reject_frontend_symlinks` checked one object and the read opened
/// another.
///
/// Holding the bytes settles both: what is served is exactly what was
/// fingerprinted, and there is no second path lookup to race. A frontend is
/// templates and small assets by design, and it is already read end-to-end to
/// compute the fingerprint, so this costs one retained copy of something this
/// process just hashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontendSnapshot {
    files: BTreeMap<String, Arc<[u8]>>,
}

impl FrontendSnapshot {
    fn read(root: &Path) -> Result<Self> {
        let mut files = BTreeMap::new();
        for entry in WalkDir::new(root).follow_links(false) {
            let entry =
                entry.map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?;
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?
                .to_string_lossy()
                .replace('\\', "/");
            // Said out loud, at resolve, rather than left as a file that
            // silently is not part of the frontend: a fingerprint covers files,
            // not the places they point at.
            if entry.file_type().is_symlink() {
                return Err(Error::FrontendFileName {
                    name: relative,
                    reason: "it is a symlink, and a trusted fingerprint covers files rather than the places they point at",
                });
            }
            if !entry.file_type().is_file() {
                continue;
            }
            files.insert(
                relative,
                Arc::from(fs::read(entry.path()).map_err(Error::Io)?),
            );
        }
        Ok(Self { files })
    }

    fn get(&self, relative: &str) -> Option<&Arc<[u8]>> {
        self.files.get(relative)
    }

    fn under(&self, prefix: &str) -> Vec<String> {
        let prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.trim_end_matches('/'))
        };
        self.files
            .keys()
            .filter(|name| name.starts_with(&prefix))
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct UiFrontend {
    provider_id: String,
    ui_kind: String,
    ui_api_version: String,
    location: Option<FrontendLocation>,
    /// The trusted bytes, captured when trust was decided.
    snapshot: Option<FrontendSnapshot>,
}

impl UiFrontend {
    /// Resolve the frontend serving this `ui_kind`, with safe mode read from
    /// the environment.
    ///
    /// `ui_api_version` is the Provider's own contract number — the shape of
    /// the document its templates render against, the hooks its runtime binds,
    /// the files it will ask for. AFUI does not know what any of that means and
    /// only checks that the frontend claims the same string.
    pub fn resolve(
        workspace_root: &Path,
        provider_id: &str,
        ui_kind: &str,
        ui_api_version: &str,
    ) -> Result<Self> {
        Self::resolve_with_safe_mode(
            workspace_root,
            provider_id,
            ui_kind,
            ui_api_version,
            safe_mode_from_env(),
        )
    }

    /// The same lookup with safe mode named rather than read.
    ///
    /// Safe mode wins before any frontend file is opened, so a frontend broken
    /// badly enough to stop the Provider cannot also be what the Provider has
    /// to read in order to recover.
    pub fn resolve_with_safe_mode(
        workspace_root: &Path,
        provider_id: &str,
        ui_kind: &str,
        ui_api_version: &str,
        safe_mode: bool,
    ) -> Result<Self> {
        let built_in = |location, snapshot| Self {
            provider_id: provider_id.to_owned(),
            ui_kind: ui_kind.to_owned(),
            ui_api_version: ui_api_version.to_owned(),
            location,
            snapshot,
        };
        let location = discover_frontend(workspace_root, provider_id, ui_kind, safe_mode).map_err(
            |error| Error::FrontendUnreadable {
                provider_id: provider_id.to_owned(),
                ui_kind: ui_kind.to_owned(),
                message: error.to_string(),
            },
        )?;
        let Some(location) = location else {
            return Ok(built_in(None, None));
        };
        if location.manifest.ui_api_version != ui_api_version {
            return Err(Error::FrontendIncompatible {
                provider_id: provider_id.to_owned(),
                ui_kind: ui_kind.to_owned(),
                frontend_id: location.manifest.frontend_id,
                declared: location.manifest.ui_api_version,
                implemented: ui_api_version.to_owned(),
            });
        }
        // Read here, while the fingerprint that authorized it is still the
        // truth about this directory.
        let snapshot =
            FrontendSnapshot::read(&location.root).map_err(|error| Error::FrontendUnreadable {
                provider_id: provider_id.to_owned(),
                ui_kind: ui_kind.to_owned(),
                message: error.to_string(),
            })?;
        Ok(built_in(Some(location), Some(snapshot)))
    }

    /// The Provider's own files, consulting no frontend at all.
    ///
    /// What safe mode resolves to, named directly. A Provider uses it when it
    /// has already decided not to read a person's frontend — a preview of its
    /// own page, a test of its own templates — and wants that decision to be
    /// visible rather than implied by an environment variable.
    #[must_use]
    pub fn builtin(provider_id: &str, ui_kind: &str, ui_api_version: &str) -> Self {
        Self {
            provider_id: provider_id.to_owned(),
            ui_kind: ui_kind.to_owned(),
            ui_api_version: ui_api_version.to_owned(),
            location: None,
            snapshot: None,
        }
    }

    /// The `frontend_id` of the override serving this UI, or `None` for the
    /// Provider's built-in.
    ///
    /// A Provider that puts this in its readiness event answers the one
    /// question a silent trust gate leaves open: whether the frontend a person
    /// installed is actually the thing they are looking at.
    #[must_use]
    pub fn frontend_id(&self) -> Option<&str> {
        self.location
            .as_ref()
            .map(|location| location.manifest.frontend_id.as_str())
    }

    #[must_use]
    pub fn is_builtin(&self) -> bool {
        self.location.is_none()
    }

    #[must_use]
    pub fn location(&self) -> Option<&FrontendLocation> {
        self.location.as_ref()
    }

    #[must_use]
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    #[must_use]
    pub fn ui_kind(&self) -> &str {
        &self.ui_kind
    }

    #[must_use]
    pub fn ui_api_version(&self) -> &str {
        &self.ui_api_version
    }

    /// This file's bytes from the override, or `None` for the Provider's own.
    ///
    /// `None` is the overlay: it means "the override did not supply this one",
    /// not "there is no override". The caller uses its built-in for that file
    /// and keeps reading the override for the rest.
    pub fn file(&self, relative: &str) -> Result<Option<Vec<u8>>> {
        let Some(snapshot) = &self.snapshot else {
            return Ok(None);
        };
        // Served from what was trusted, not from the path it came from: the
        // directory may have changed since, and a changed directory is not this
        // frontend.
        let relative = frontend_relative_path(relative)?;
        let key = relative.to_string_lossy().replace('\\', "/");
        Ok(snapshot.get(&key).map(|bytes| bytes.to_vec()))
    }

    /// The same overlay for a file the Provider reads as text.
    pub fn text(&self, relative: &str) -> Result<Option<String>> {
        let Some(bytes) = self.file(relative)? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| Error::FrontendUnreadable {
                provider_id: self.provider_id.clone(),
                ui_kind: self.ui_kind.clone(),
                message: format!("{relative} is not valid UTF-8: {error}"),
            })
    }

    /// This frontend's application icon, falling back to the Provider's own.
    ///
    /// The override path is fixed across Providers so the icon is part of the
    /// same frontend package as its templates and stylesheet. A Provider still
    /// supplies the built-in SVG: AFUI owns the overlay contract, not anybody
    /// else's identity.
    pub fn app_icon(&self, builtin_svg: &str) -> Result<UiAppIcon> {
        let (svg, from_override) = match self.text(APP_ICON_ASSET_PATH)? {
            Some(svg) => (svg, true),
            None => (builtin_svg.to_owned(), false),
        };
        UiAppIcon::from_svg(svg).map_err(|error| {
            if from_override {
                Error::FrontendUnreadable {
                    provider_id: self.provider_id.clone(),
                    ui_kind: self.ui_kind.clone(),
                    message: format!("{APP_ICON_ASSET_PATH} is invalid: {error}"),
                }
            } else {
                error
            }
        })
    }

    /// List regular override files below one frontend-relative directory.
    ///
    /// The result is sorted and uses `/` separators.  Built-in frontends and
    /// missing directories return an empty list.  Every returned path has gone
    /// through the same traversal, script-extension, and symlink checks as
    /// [`Self::file`], so a caller can safely feed the names back into
    /// [`Self::text`].
    pub fn files_under(&self, relative: &str) -> Result<Vec<String>> {
        let Some(snapshot) = &self.snapshot else {
            return Ok(Vec::new());
        };
        let relative = frontend_relative_path(relative)?;
        // Names come from the snapshot, so a symlink or a file that appeared
        // after trust was decided is not in this list at all — there is nothing
        // to reject at read time because nothing outside the trusted set is
        // reachable.
        let mut files = snapshot.under(&relative.to_string_lossy().replace('\\', "/"));
        files.sort();
        Ok(files)
    }
}

/// Serving an override's static files needs a router, and a router needs a
/// server. The overlay above does not: a Provider reading its own templates
/// through [`UiFrontend::text`] never mounts anything.
#[cfg(feature = "session")]
mod assets {
    use axum::Router;
    use axum::extract::Path as UrlPath;
    use axum::http::{HeaderValue, StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;

    use super::{Path, UiFrontend};

    /// What an override asset may be, and what a browser is told it is.
    ///
    /// An allowlist rather than a guess: an override serves presentation, and
    /// anything whose type this crate cannot name is refused instead of being
    /// sent with a type a browser might sniff past.
    const ASSET_CONTENT_TYPES: [(&str, &str); 12] = [
        ("css", "text/css; charset=utf-8"),
        ("svg", "image/svg+xml"),
        ("png", "image/png"),
        ("jpg", "image/jpeg"),
        ("jpeg", "image/jpeg"),
        ("gif", "image/gif"),
        ("webp", "image/webp"),
        ("ico", "image/x-icon"),
        ("woff", "font/woff"),
        ("woff2", "font/woff2"),
        ("ttf", "font/ttf"),
        ("otf", "font/otf"),
    ];

    impl UiFrontend {
        /// Serve the override's `assets/` tree, and nothing else.
        ///
        /// Mount it wherever the Provider's page refers to its own assets from
        /// — `Router::nest("/assets", frontend.assets_router())`. With no
        /// override installed every path is a 404, so a Provider mounts it
        /// unconditionally and does not branch on whether one exists.
        pub fn assets_router(&self) -> Router {
            let frontend = self.clone();
            Router::new().route(
                "/{*path}",
                get(move |UrlPath(path): UrlPath<String>| {
                    let frontend = frontend.clone();
                    async move { frontend.asset_response(&path) }
                }),
            )
        }

        /// One file out of `assets/`, named to a browser or refused.
        ///
        /// `pub(crate)` because AFUI's own page dispatches its routes by hand —
        /// it has already taken a credential off the front of the path, so
        /// there is no sub-router boundary to nest [`Self::assets_router`]
        /// under. Sharing this rather than writing a second copy is what keeps
        /// one answer to "which types may be named to a browser".
        pub(crate) fn asset_response(&self, relative: &str) -> Response {
            let name = format!("assets/{relative}");
            let Some(content_type) = asset_content_type(relative) else {
                return (
                    StatusCode::FORBIDDEN,
                    "an override may serve stylesheets, images and fonts here; this file's type \
                     is not one AFUI will name to a browser\n",
                )
                    .into_response();
            };
            match self.file(&name) {
                Ok(Some(bytes)) => (
                    [(header::CONTENT_TYPE, HeaderValue::from_static(content_type))],
                    bytes,
                )
                    .into_response(),
                Ok(None) => (StatusCode::NOT_FOUND, "no such frontend asset\n").into_response(),
                Err(error) => (StatusCode::FORBIDDEN, format!("{error}\n")).into_response(),
            }
        }
    }

    fn asset_content_type(relative: &str) -> Option<&'static str> {
        let extension = Path::new(relative)
            .extension()
            .and_then(|extension| extension.to_str())?
            .to_ascii_lowercase();
        ASSET_CONTENT_TYPES
            .iter()
            .find(|(candidate, _)| *candidate == extension)
            .map(|(_, content_type)| *content_type)
    }
}

/// Turn a Provider-supplied file name into a path that cannot leave the
/// frontend.
///
/// Every component must be an ordinary name: no absolute root, no `..`, no
/// drive prefix, no empty segment. A Provider asks for names it chose itself,
/// but a URL path reaches this through [`UiFrontend::assets_router`], so the
/// check belongs here rather than in each caller.
fn frontend_relative_path(relative: &str) -> Result<PathBuf> {
    let refuse = |reason: &'static str| Error::FrontendFileName {
        name: relative.to_owned(),
        reason,
    };
    if relative.is_empty() {
        return Err(refuse("it is empty"));
    }
    if relative.contains('\\') {
        return Err(refuse("it contains a backslash"));
    }
    let mut path = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::ParentDir => return Err(refuse("it climbs out of the frontend with `..`")),
            Component::RootDir | Component::Prefix(_) => {
                return Err(refuse("it is an absolute path"));
            }
            Component::CurDir => {}
        }
    }
    if path.as_os_str().is_empty() {
        return Err(refuse("it names no file"));
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if extension.is_some_and(|extension| SCRIPT_EXTENSIONS.contains(&extension.as_str())) {
        return Err(refuse(
            "an override may not supply JavaScript; a Provider's behaviour is its own, and its \
             page binds it with a nonce the frontend cannot forge",
        ));
    }
    Ok(path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("afui-frontend-{name}-{stamp}"))
    }

    /// A frontend at the global scope, which needs no trust record — the
    /// per-file overlay is what these tests are about, not the trust gate.
    fn global_frontend(root: &Path, provider_id: &str, ui_kind: &str, version: &str) -> PathBuf {
        let directory = root.join("frontends").join(provider_id).join(ui_kind);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("frontend.json"),
            format!(r#"{{"frontend_id":"custom","ui_api_version":"{version}"}}"#),
        )
        .unwrap();
        directory
    }

    use crate::ENV_LOCK as CONFIG_DIR;

    fn resolve(
        config: &Path,
        ui_api_version: &str,
    ) -> (std::sync::MutexGuard<'static, ()>, Result<UiFrontend>) {
        let guard = CONFIG_DIR.lock().unwrap_or_else(|error| error.into_inner());
        // SAFETY: the guard makes this the only thread reading or writing the
        // variable for the length of the test.
        unsafe { std::env::set_var(CONFIG_DIR_ENV, config) };
        let resolved = UiFrontend::resolve_with_safe_mode(
            Path::new("/nonexistent"),
            "afdemo",
            "demo",
            ui_api_version,
            false,
        );
        (guard, resolved)
    }

    #[test]
    fn a_file_the_override_does_not_supply_falls_back_rather_than_erroring() {
        let root = temp_root("overlay");
        let directory = global_frontend(&root, "afdemo", "demo", "1");
        fs::write(directory.join("page.html.j2"), "mine").unwrap();
        let (_guard, frontend) = resolve(&root, "1");
        let frontend = frontend.unwrap();
        assert_eq!(frontend.frontend_id(), Some("custom"));
        assert_eq!(
            frontend.text("page.html.j2").unwrap().as_deref(),
            Some("mine")
        );
        // The whole point of the overlay: one replaced file does not make the
        // rest of the Provider's files disappear.
        assert_eq!(frontend.text("style.css").unwrap(), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn no_override_reads_as_built_in_for_every_file() {
        let root = temp_root("builtin");
        fs::create_dir_all(&root).unwrap();
        let (_guard, frontend) = resolve(&root, "1");
        let frontend = frontend.unwrap();
        assert!(frontend.is_builtin());
        assert_eq!(frontend.frontend_id(), None);
        assert_eq!(frontend.text("page.html.j2").unwrap(), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_app_icon_is_a_per_file_frontend_overlay() {
        let root = temp_root("app-icon");
        let directory = global_frontend(&root, "afdemo", "demo", "1");
        fs::create_dir_all(directory.join("assets")).unwrap();
        fs::write(
            directory.join(APP_ICON_ASSET_PATH),
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"1\"/></svg>",
        )
        .unwrap();
        let (_guard, frontend) = resolve(&root, "1");
        let icon = frontend
            .unwrap()
            .app_icon("<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>")
            .unwrap();
        assert!(icon.as_svg().contains("<circle"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_incompatible_version_is_an_error_that_names_safe_mode() {
        let root = temp_root("incompatible");
        global_frontend(&root, "afdemo", "demo", "99");
        let (_guard, resolved) = resolve(&root, "1");
        let error = resolved.unwrap_err();
        let message = error.to_string();
        assert!(message.contains("ui_api_version 99"), "{message}");
        assert!(message.contains("built-in"), "{message}");
        assert!(matches!(error, Error::FrontendIncompatible { .. }));
        // …and safe mode is the way out, checked before the manifest is read.
        let safe = UiFrontend::resolve_with_safe_mode(
            Path::new("/nonexistent"),
            "afdemo",
            "demo",
            "1",
            true,
        )
        .unwrap();
        assert!(safe.is_builtin());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_unparseable_manifest_is_an_error_rather_than_a_quiet_built_in() {
        let root = temp_root("unreadable");
        let directory = global_frontend(&root, "afdemo", "demo", "1");
        fs::write(directory.join("frontend.json"), "{ not json").unwrap();
        let (_guard, resolved) = resolve(&root, "1");
        let error = resolved.unwrap_err();
        assert!(matches!(error, Error::FrontendUnreadable { .. }));
        assert!(error.to_string().contains("built-in"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_override_may_not_supply_javascript_or_escape_its_own_directory() {
        let root = temp_root("refusals");
        let directory = global_frontend(&root, "afdemo", "demo", "1");
        fs::write(directory.join("app.js"), "alert(1)").unwrap();
        let (_guard, frontend) = resolve(&root, "1");
        let frontend = frontend.unwrap();
        for (name, needle) in [
            ("app.js", "JavaScript"),
            ("../frontend.json", ".."),
            ("/etc/passwd", "absolute"),
            ("", "empty"),
        ] {
            let error = frontend.file(name).unwrap_err();
            assert!(error.to_string().contains(needle), "{name}: {error}");
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn what_is_served_is_what_was_fingerprinted() {
        // A resolved frontend used to re-read each file from disk per request,
        // so a long-running process served whatever the directory held *now* —
        // while the README said any later edit revoked trust. It serves the
        // bytes that were trusted.
        let root = temp_root("snapshot");
        let directory = global_frontend(&root, "afdemo", "demo", "1");
        fs::write(directory.join("style.css"), "body{color:trusted}").unwrap();
        let (_guard, frontend) = resolve(&root, "1");
        let frontend = frontend.unwrap();
        assert_eq!(
            frontend.text("style.css").unwrap().as_deref(),
            Some("body{color:trusted}")
        );

        fs::write(directory.join("style.css"), "body{color:edited-later}").unwrap();
        assert_eq!(
            frontend.text("style.css").unwrap().as_deref(),
            Some("body{color:trusted}"),
            "an edit after trust was decided must not reach a browser"
        );

        // And a file that did not exist when trust was decided is not part of
        // this frontend at all.
        fs::write(directory.join("late.css"), "body{}").unwrap();
        assert_eq!(frontend.text("late.css").unwrap(), None);
        assert!(frontend.file("late.css").unwrap().is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_symlinked_override_file_is_refused_because_the_fingerprint_did_not_cover_it() {
        let root = temp_root("symlink");
        let directory = global_frontend(&root, "afdemo", "demo", "1");
        let outside = root.join("outside.css");
        fs::write(&outside, "body{}").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, directory.join("style.css")).unwrap();
        #[cfg(not(unix))]
        let _ = directory;
        // Refused when trust is decided, not when the file is asked for: the
        // fingerprint is over the whole directory, and a directory holding a
        // link is not a directory that fingerprint honestly describes. The
        // message names the file so it can be removed.
        let (_guard, frontend) = resolve(&root, "1");
        #[cfg(unix)]
        {
            let error = frontend.expect_err("a linked override must not resolve");
            let message = error.to_string();
            assert!(message.contains("symlink"), "{message}");
            assert!(message.contains("style.css"), "{message}");
        }
        #[cfg(not(unix))]
        let _ = frontend;
        let _ = fs::remove_dir_all(root);
    }
}

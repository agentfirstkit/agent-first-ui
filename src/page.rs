//! The other half of a frontend: turning its files into a page.
//!
//! [`crate::UiFrontend`] resolves which bytes a template comes from — a
//! person's override, or a Provider's own. What every Provider still had to
//! write for itself was everything after that: the MiniJinja policy strict
//! enough that a broken template fails loudly, the render, the guard against
//! a rendered value smuggling markup, the check that the elements a runtime
//! binds to actually exist, and the splice point that runtime is injected
//! through. Those steps form one safety boundary: if pages reimplement them,
//! the first page that skips or reorders a guard becomes the page whose
//! override can run code.
//!
//! [`UiPage`] is that assembly, done once. It never reads a document's
//! fields and never chooses what a page contains — only that the page an
//! override produced is safe to serve and complete enough for a Provider's
//! own runtime to bind to. A Provider still owns the template markup, the
//! shape of the document a template renders against, and the runtime script
//! itself; this only checks that the three agree.
//!
//! Nothing here binds a port or mints a credential, which is why this module
//! sits behind its own feature rather than `session`'s: a Provider whose
//! human UI is a compiled-out option still has a template to check.

use std::collections::HashSet;

use minijinja::functions::Function;
use minijinja::value::{FunctionArgs, FunctionResult, Value};
use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use scraper::{Html, Node};
use serde::Serialize;

use crate::frontend::UiFrontend;
use crate::frontend_script::{inspect_rendered_page, reject_frontend_script};
use crate::reserved::{APP_ICON_PATH, BASE_STYLE_PATH, SESSION_END_PATH};
use crate::{Error, Result};

/// The reserved template global carrying AFUI's own routes.
///
/// Held in the same protected position as `document`: an extension or a
/// frontend that could shadow it could point the page's icon, channel or end
/// route somewhere else, which is exactly the kind of decision a replaceable
/// template does not get to make.
const AFUI_GLOBAL: &str = "afui";

/// Namespace for assembling one Provider page. Carries no state of its own —
/// [`UiPage::builder`] is what returns something to call.
pub struct UiPage;

impl UiPage {
    /// Start assembling the page this frontend serves.
    #[must_use]
    pub fn builder(frontend: &UiFrontend) -> UiPageBuilder<'_> {
        UiPageBuilder {
            frontend,
            entry: None,
            entry_lookup: None,
            fallback: None,
            templates: Vec::new(),
            required_elements: Vec::new(),
            required_attribute_values: Vec::new(),
            required_attributes: Vec::new(),
            runtime_marker: None,
            runtime: None,
            extensions: Vec::new(),
            document_at_root: false,
        }
    }
}

/// One page's worth of templates and the contract its runtime depends on.
///
pub struct UiPageBuilder<'a> {
    frontend: &'a UiFrontend,
    entry: Option<String>,
    entry_lookup: Option<String>,
    fallback: Option<String>,
    templates: Vec<(String, String, Option<String>)>,
    required_elements: Vec<String>,
    required_attribute_values: Vec<(String, Vec<String>)>,
    required_attributes: Vec<String>,
    runtime_marker: Option<String>,
    runtime: Option<String>,
    extensions: Vec<UiPageExtension<'a>>,
    document_at_root: bool,
}

type UiPageExtension<'a> =
    Box<dyn for<'environment> FnOnce(&mut UiPageEnvironment<'environment>) -> Result<()> + 'a>;

mod sealed {
    pub trait PlainValue {}
    pub trait PlainFilterResult {}

    macro_rules! plain_scalars {
        ($($type:ty),+ $(,)?) => {
            $(impl PlainValue for $type {})+
        };
    }

    plain_scalars!(
        (),
        bool,
        char,
        String,
        i8,
        i16,
        i32,
        i64,
        i128,
        isize,
        u8,
        u16,
        u32,
        u64,
        u128,
        usize,
        f32,
        f64,
    );

    impl<T: PlainValue> PlainValue for Option<T> {}
    impl<T: PlainValue> PlainValue for Vec<T> {}
    impl<T: PlainValue, const N: usize> PlainValue for [T; N] {}
    impl PlainValue for super::UiPageSerializedValue {}

    impl<T: PlainValue> PlainFilterResult for T {}
    impl<T: PlainValue> PlainFilterResult for std::result::Result<T, minijinja::Error> {}
}

/// A template value that cannot carry MiniJinja's bypass-autoescaping bit.
///
/// Scalars and collections of these values are supported.  MiniJinja's raw
/// [`Value`] is deliberately not: `Value::from_safe_string` would let a
/// frontend turn domain data back into markup through a trusted extension.
///
/// ```compile_fail
/// fn accepts_plain_value<T: agent_first_ui::UiPageValue>() {}
/// accepts_plain_value::<minijinja::Value>();
/// ```
pub trait UiPageValue: Into<Value> + sealed::PlainValue {}

impl<T> UiPageValue for T where T: Into<Value> + sealed::PlainValue {}

/// A structured filter/global value rebuilt through JSON so MiniJinja safety
/// provenance cannot cross the page-extension boundary.
///
/// Use this when a filter needs to return a sequence or map selected from its
/// input document. Serializing through JSON deliberately drops engine-only
/// state such as the `Value::from_safe_string` bit while preserving the data
/// shape a template can iterate or inspect.
#[derive(Debug)]
pub struct UiPageSerializedValue(Value);

impl UiPageSerializedValue {
    /// Rebuild a serializable value without MiniJinja safety provenance.
    pub fn from_serialize(value: impl Serialize) -> std::result::Result<Self, minijinja::Error> {
        let value = serde_json::to_value(value).map_err(|error| {
            minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "a UiPage extension returned a value that could not be serialized",
            )
            .with_source(error)
        })?;
        Ok(Self(Value::from_serialize(value)))
    }

    /// Preserve a filter's “not found” result without exposing raw `Value`.
    #[must_use]
    pub fn undefined() -> Self {
        Self(Value::UNDEFINED)
    }
}

impl From<UiPageSerializedValue> for Value {
    fn from(value: UiPageSerializedValue) -> Self {
        value.0
    }
}

/// A filter result that cannot carry MiniJinja's bypass-autoescaping bit.
///
/// Plain values and `Result<plain value, minijinja::Error>` are supported.
/// Raw [`Value`] results are intentionally excluded for the same reason as
/// [`UiPageValue`].
///
/// ```compile_fail
/// fn accepts_plain_result<T: agent_first_ui::UiPageFilterResult>() {}
/// accepts_plain_result::<minijinja::Value>();
/// ```
pub trait UiPageFilterResult: FunctionResult + sealed::PlainFilterResult {}

impl<T> UiPageFilterResult for T where T: FunctionResult + sealed::PlainFilterResult {}

/// The deliberately small extension surface for a page's template engine.
///
/// Provider code may add plain-value filters and globals.  It cannot add
/// templates, change undefined-value behaviour, disable auto-escaping, or
/// restore MiniJinja's `safe` filter; those remain one invariant owned by
/// [`UiPage`].
pub struct UiPageEnvironment<'a> {
    frontend: &'a UiFrontend,
    environment: &'a mut Environment<'static>,
}

impl UiPageEnvironment<'_> {
    /// Add a trusted Rust filter.  The reserved `safe` name is refused even
    /// though the page reasserts its policy after all extensions run.
    pub fn add_filter<F, Rv, Args>(&mut self, name: impl Into<String>, filter: F) -> Result<()>
    where
        F: Function<Rv, Args>,
        Rv: UiPageFilterResult,
        Args: for<'value> FunctionArgs<'value>,
    {
        let name = name.into();
        if name == "safe" {
            return Err(incomplete(
                self.frontend,
                "a UiPage extension may not register the reserved `safe` filter",
            ));
        }
        self.environment.add_filter(name, filter);
        Ok(())
    }

    /// Add a trusted global value.  `document` is supplied by [`UiPage`] at
    /// render time, `afui` carries AFUI's own reserved routes, and neither can
    /// be shadowed by an extension.
    pub fn add_global(&mut self, name: impl Into<String>, value: impl UiPageValue) -> Result<()> {
        let name = name.into();
        if name == "document" {
            return Err(incomplete(
                self.frontend,
                "a UiPage extension may not register the reserved `document` global",
            ));
        }
        if name == AFUI_GLOBAL {
            return Err(incomplete(
                self.frontend,
                "a UiPage extension may not register the reserved `afui` global",
            ));
        }
        self.environment.add_global(name, value);
        Ok(())
    }
}

impl<'a> UiPageBuilder<'a> {
    /// The entry template's name, relative to the frontend directory. Also
    /// the name it renders under and the label its errors are reported
    /// against.
    #[must_use]
    pub fn entry(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.entry_lookup = Some(name.clone());
        self.entry = Some(name);
        self
    }

    /// Name an entry template separately from its path in the frontend.  This
    /// supports trees whose on-disk layout contains a locale directory while
    /// includes use locale-independent logical names.
    #[must_use]
    pub fn entry_at(mut self, name: impl Into<String>, frontend_path: impl Into<String>) -> Self {
        self.entry = Some(name.into());
        self.entry_lookup = Some(frontend_path.into());
        self
    }

    /// The Provider's own source for the entry template, used when no
    /// override supplies one.
    #[must_use]
    pub fn fallback(mut self, source: impl Into<String>) -> Self {
        self.fallback = Some(source.into());
        self
    }

    /// One more template the entry may `{% include %}`. May be called
    /// repeatedly. Resolved the same way as the entry: the override's file
    /// under this name if it supplied one, `fallback` otherwise.
    #[must_use]
    pub fn template(mut self, name: impl Into<String>, fallback: impl Into<String>) -> Self {
        let name = name.into();
        self.templates
            .push((name.clone(), name, Some(fallback.into())));
        self
    }

    /// Register a logical template name whose override lives at a different
    /// frontend-relative path.
    #[must_use]
    pub fn template_at(
        mut self,
        name: impl Into<String>,
        frontend_path: impl Into<String>,
        fallback: impl Into<String>,
    ) -> Self {
        self.templates
            .push((name.into(), frontend_path.into(), Some(fallback.into())));
        self
    }

    /// Register a template whose name and source both come from the selected
    /// frontend.  This is for open-ended partial directories: if the file
    /// disappears before render, the page fails rather than silently growing
    /// a different built-in template.
    #[must_use]
    pub fn override_template(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.templates.push((name.clone(), name, None));
        self
    }

    /// Register an override-only logical template at a separate frontend path.
    #[must_use]
    pub fn override_template_at(
        mut self,
        name: impl Into<String>,
        frontend_path: impl Into<String>,
    ) -> Self {
        self.templates
            .push((name.into(), frontend_path.into(), None));
        self
    }

    /// Register every override file under `directory` that this builder does
    /// not already name.
    ///
    /// A frontend may break its page into partials the Provider has never
    /// heard of, and `{% include %}` them — that is what makes a real
    /// restructure possible rather than a reskin. Each one still has to be
    /// registered, or the include fails to resolve.
    ///
    /// Registered as override-only, so a partial that disappears between
    /// discovery and render fails the page rather than silently resolving to
    /// some built-in template of the same name. Templates this builder already
    /// registered are skipped: those are the contract, and they keep their own
    /// fallbacks.
    ///
    /// `suffix` is the extension a template file carries; anything else under
    /// the directory is ignored rather than registered as a template that
    /// would never parse.
    pub fn override_templates_under(mut self, directory: &str, suffix: &str) -> Result<Self> {
        let mut registered: HashSet<&str> = self
            .templates
            .iter()
            .map(|(_, frontend_path, _)| frontend_path.as_str())
            .collect();
        if let Some(entry) = &self.entry_lookup {
            registered.insert(entry.as_str());
        }
        let discovered: Vec<String> = self
            .frontend
            .files_under(directory)?
            .into_iter()
            .filter(|name| name.ends_with(suffix) && !registered.contains(name.as_str()))
            .collect();
        for name in discovered {
            self = self.override_template(name);
        }
        Ok(self)
    }

    /// Every element id the Provider's own runtime binds to. A rendered page
    /// missing one is reported by name rather than opened as a page that
    /// silently does nothing.
    #[must_use]
    pub fn requires_elements(mut self, ids: &'static [&'static str]) -> Self {
        self.required_elements = ids.iter().map(|id| (*id).to_owned()).collect();
        self
    }

    /// Take required element ids from a flat serializable object whose values
    /// are strings.  This lets one typed document object be both the template
    /// input and the runtime contract, without maintaining a second list that
    /// can drift from it.
    pub fn requires_element_map(mut self, ids: &impl Serialize) -> Result<Self> {
        let value = serde_json::to_value(ids).map_err(|error| {
            incomplete(
                self.frontend,
                &format!("the required-element map could not be serialized: {error}"),
            )
        })?;
        let object = value.as_object().ok_or_else(|| {
            incomplete(
                self.frontend,
                "the required-element map must serialize as a flat object",
            )
        })?;
        let mut seen = HashSet::new();
        let mut required = Vec::with_capacity(object.len());
        for (field, value) in object {
            let id = value.as_str().filter(|id| !id.is_empty()).ok_or_else(|| {
                incomplete(
                    self.frontend,
                    &format!("required-element field `{field}` must contain a non-empty string"),
                )
            })?;
            if !seen.insert(id.to_owned()) {
                return Err(incomplete(
                    self.frontend,
                    &format!("required-element id `{id}` appears more than once"),
                ));
            }
            required.push(id.to_owned());
        }
        self.required_elements = required;
        Ok(self)
    }

    /// Every value of one attribute the Provider's own runtime binds to.
    ///
    /// The element check above answers "is the thing my runtime writes into
    /// there?". This answers the other half: "does the page actually declare
    /// the choices my runtime knows how to act on?" — a decide page whose
    /// template dropped its refuse control opens as a question nobody can
    /// answer, and a page that wrote `approve-all` where the runtime expects
    /// `approve` opens with a control bound to nothing.
    ///
    /// Matched by the same rules as an element id and for the same reason: the
    /// whole attribute value, not a prefix, and only on an element a browser
    /// would actually render — a declaration inside `<template>` or a comment
    /// is inert, and a page whose only refuse control is inert is a page with
    /// no refuse control.
    ///
    /// At least one carrier per value rather than exactly one: a runtime binds
    /// every control it finds, so a long page with an approve button at the
    /// top and another at the bottom is a page that works.
    ///
    /// AFUI still does not learn what any of these mean. It checks that the
    /// template and the runtime agree on a vocabulary; the vocabulary, and
    /// everything each word does, remain the Provider's.
    #[must_use]
    pub fn requires_attribute_values(
        mut self,
        attribute: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.required_attribute_values.push((
            attribute.into(),
            values.into_iter().map(Into::into).collect(),
        ));
        self
    }

    /// Every attribute the Provider's own runtime looks for, by name alone.
    ///
    /// The sibling of [`Self::requires_attribute_values`] for a contract whose
    /// vocabulary is the attribute names themselves rather than one
    /// attribute's values — a page that marks several distinct regions, each
    /// with its own `data-*` hook. Same discipline: an active element only, so
    /// a hook that survives in a comment or inside `<template>` does not count
    /// as one a runtime can find.
    #[must_use]
    pub fn requires_attributes(
        mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.required_attributes
            .extend(names.into_iter().map(Into::into));
        self
    }

    /// The declarations a generated decision runtime binds, read back as the
    /// page's contract.
    ///
    /// One list rather than two: a Provider that adds a decision to its
    /// runtime cannot forget to add it here, and a page that declares one the
    /// runtime never learned is caught before a window opens.
    #[must_use]
    pub fn requires_decisions(self, runtime: &crate::UiDecisionRuntime) -> Self {
        let attribute = runtime.attribute().to_owned();
        let values: Vec<String> = runtime.declarations().map(str::to_owned).collect();
        self.requires_attribute_values(attribute, values)
    }

    /// Where the rendered page makes room for the Provider's runtime. Must
    /// appear in the rendered output exactly once, whether or not a runtime
    /// is actually spliced in — see [`Self::runtime`].
    #[must_use]
    pub fn runtime_marker(mut self, marker: impl Into<String>) -> Self {
        self.runtime_marker = Some(marker.into());
        self
    }

    /// The Provider's own script tag for this render, or `None` for a page
    /// with no runtime to load. Either way the marker still has to appear
    /// exactly once — `None` replaces it with nothing rather than skipping
    /// the check, so a page that quietly dropped the marker is still caught
    /// even on a render that was never going to inject anything.
    #[must_use]
    pub fn runtime(mut self, runtime: Option<String>) -> Self {
        self.runtime = runtime;
        self
    }

    /// Add trusted filters or globals through the restricted extension
    /// surface.  The closure runs before templates are registered and the
    /// fixed escaping policy is reasserted afterwards.
    #[must_use]
    pub fn extend_environment<F>(mut self, extension: F) -> Self
    where
        F: for<'environment> FnOnce(&mut UiPageEnvironment<'environment>) -> Result<()> + 'a,
    {
        self.extensions.push(Box::new(extension));
        self
    }

    /// Render the serialized document as the template's root context instead
    /// of under the default `document` name.  This is useful for an established
    /// template contract whose top-level fields are already public; new pages
    /// generally benefit from the explicit `document.*` namespace.
    #[must_use]
    pub fn document_at_root(mut self) -> Self {
        self.document_at_root = true;
        self
    }

    /// Render, check, and splice — in that order because the order is the
    /// safety property. Checking a page that already has the runtime spliced
    /// in would flag the Provider's own trusted script as a violation the
    /// moment it looked like the markup it is refusing everywhere else.
    pub fn render(self, document: &impl Serialize) -> Result<String> {
        let frontend = self.frontend;
        let entry = self.entry.ok_or_else(|| {
            incomplete(
                frontend,
                "UiPage::builder has no entry template; call `.entry(...)`",
            )
        })?;
        let entry_lookup = self.entry_lookup.ok_or_else(|| {
            incomplete(
                frontend,
                "UiPage::builder has no entry lookup path; call `.entry(...)`",
            )
        })?;
        let fallback = self.fallback.ok_or_else(|| {
            incomplete(
                frontend,
                "UiPage::builder has no fallback source; call `.fallback(...)`",
            )
        })?;
        let marker = self.runtime_marker.ok_or_else(|| {
            incomplete(
                frontend,
                "UiPage::builder has no runtime marker; call `.runtime_marker(...)`",
            )
        })?;

        // 1-2: resolve every template's source. The entry is one of these,
        // not a special case — an override's bytes are checked at the source
        // level, a Provider's own fallback is not, because the fallback is
        // where a trusted runtime comes from in the first place.
        let mut sources = Vec::with_capacity(self.templates.len() + 1);
        sources.push((
            entry.clone(),
            resolve_source(frontend, &entry_lookup, Some(&fallback))?,
        ));
        for (name, frontend_path, template_fallback) in &self.templates {
            sources.push((
                name.clone(),
                resolve_source(frontend, frontend_path, template_fallback.as_deref())?,
            ));
        }

        // 3: the one escaping policy every Provider page renders under.
        // Removing the `safe` filter is what stops a template from turning
        // itself back off; without it the strict/auto-escape pair above would
        // be one filter call away from doing nothing.
        let mut environment = Environment::new();
        environment.set_undefined_behavior(UndefinedBehavior::SemiStrict);
        environment.set_auto_escape_callback(|_| AutoEscape::Html);
        environment.remove_filter("safe");

        for extension in self.extensions {
            extension(&mut UiPageEnvironment {
                frontend,
                environment: &mut environment,
            })?;
        }
        // Extensions never receive the raw environment, but reassert these
        // invariants here as defense in depth around future registrar growth.
        environment.set_undefined_behavior(UndefinedBehavior::SemiStrict);
        environment.set_auto_escape_callback(|_| AutoEscape::Html);
        environment.remove_filter("safe");
        // Registered after the extensions rather than before, so this wins
        // outright even if a future registrar stopped refusing the name. Every
        // path is relative to the session root the page was served from, which
        // is what makes them resolve under the session's own credential.
        environment.add_global(AFUI_GLOBAL, afui_paths());

        // 4: register every template, then render the entry.
        for (name, source) in sources {
            environment
                .add_template_owned(name, source)
                .map_err(|error| render_error(frontend, &entry, &error))?;
        }
        let template = environment
            .get_template(&entry)
            .map_err(|error| render_error(frontend, &entry, &error))?;
        let value = minijinja::Value::from_serialize(document);
        let page = if self.document_at_root {
            template.render(value)
        } else {
            template.render(minijinja::context! { document => value })
        }
        .map_err(|error| render_error(frontend, &entry, &error))?;

        // 5: the rendered guard runs before anything is injected. Running it
        // after would fail the Provider's own runtime on its own marker.
        let parsed = inspect_rendered_page("rendered page", &page)?;

        // 6: the marker names exactly one splice point. `replace` below
        // touches every occurrence, so a template that wrote it twice would
        // otherwise get its runtime loaded twice.
        let comment = marker_comment(&marker).ok_or_else(|| {
            incomplete(
                frontend,
                "the runtime marker must be one complete HTML comment",
            )
        })?;
        // `replacen` works on the source string, while the completeness check
        // works on the parsed tree.  Require one occurrence in both views so
        // an inert copy in an attribute or `<template>` cannot appear earlier
        // in the source and steal the splice from the one active comment.
        let source_occurrences = page.match_indices(&marker).count();
        if source_occurrences != 1 {
            return Err(incomplete(
                frontend,
                &marker_source_count_message(&marker, source_occurrences),
            ));
        }
        let occurrences = marker_occurrences(&parsed, comment);
        if occurrences != 1 {
            return Err(incomplete(
                frontend,
                &marker_count_message(&marker, occurrences),
            ));
        }

        // 7: every hook the Provider's own runtime binds to — elements by
        // exact id (`id="tree"` matches, `id="tree-empty"` does not),
        // declarations by whole attribute value, and attribute names on their
        // own. Same matching discipline for all three: an active element, never
        // one inside a `<template>` or a comment.
        //
        // Collected rather than returned at the first one, because the person
        // reading this is writing a page against a contract they cannot see
        // from here. Stopping at the first miss turns "this page is missing
        // seven hooks" into seven renders that each name one — and the fix for
        // all seven is the same sitting.
        let mut missing = Vec::new();
        for id in self.required_elements {
            if !has_element_id(&parsed, &id) {
                missing.push(format!("no element with id `{id}`"));
            }
        }
        for (attribute, values) in self.required_attribute_values {
            for value in values {
                if !has_attribute_value(&parsed, &attribute, &value) {
                    missing.push(format!(
                        "no active element declaring `{attribute}=\"{value}\"`"
                    ));
                }
            }
        }
        for attribute in self.required_attributes {
            if !has_attribute(&parsed, &attribute) {
                missing.push(format!("no active element carrying `{attribute}`"));
            }
        }
        if !missing.is_empty() {
            return Err(incomplete(
                frontend,
                &format!(
                    "a runtime bound to what is missing would open and do nothing — {}",
                    missing.join("; ")
                ),
            ));
        }

        // 8: only now does the marker become the Provider's own script.
        Ok(page.replacen(&marker, self.runtime.as_deref().unwrap_or(""), 1))
    }
}

/// One template's source: the override's file under `name` if it supplied
/// one, `fallback` otherwise. An override's bytes are checked at the source
/// level here — before they ever reach MiniJinja — so a smuggled `<script>`
/// or a `{% filter safe %}` block is refused by name rather than by a render
/// failure that does not say which file it came from.
fn resolve_source(frontend: &UiFrontend, name: &str, fallback: Option<&str>) -> Result<String> {
    match frontend.text(name)? {
        Some(source) => {
            reject_frontend_script(name, &source)?;
            Ok(source)
        }
        None => fallback.map(str::to_owned).ok_or_else(|| {
            incomplete(
                frontend,
                &format!("frontend template `{name}` was discovered but is no longer present"),
            )
        }),
    }
}

/// What a template needs from AFUI to reference AFUI's own routes.
///
/// Templates need only the routes they may render directly. The live runtime
/// is deliberately absent: Provider JavaScript reaches it through
/// `afui.connect()` rather than naming AFUI's transport path.
///
/// These are the one kind of value that crosses into a template already
/// escaped, and the reason is that there is nothing here to escape: each is a
/// `&'static str` literal in this crate's own source, matching
/// `[a-z0-9_./-]+`, with no input of any kind reaching it. Left to the HTML
/// escaper the separators would render as `&#x2f;`, which every browser
/// resolves identically but which no one reading view-source, or grepping a
/// rendered page, would recognise as the route they wrote. The invariant this
/// does not touch is the one that matters: no *document* field and no
/// *frontend* byte becomes markup, ever.
fn afui_paths() -> Value {
    Value::from(
        [
            ("app_icon_path", APP_ICON_PATH),
            ("base_style_path", BASE_STYLE_PATH),
            ("end_path", SESSION_END_PATH),
        ]
        .into_iter()
        .map(|(name, path)| (name, Value::from_safe_string(path.to_owned())))
        .collect::<std::collections::BTreeMap<_, _>>(),
    )
}

fn has_element_id(page: &Html, id: &str) -> bool {
    page.tree.nodes().any(|node| {
        node.value()
            .as_element()
            .is_some_and(|element| element.id() == Some(id))
            && !node.ancestors().any(|ancestor| {
                ancestor
                    .value()
                    .as_element()
                    .is_some_and(|element| element.name() == "template")
            })
    })
}

/// Whether an element a browser would render carries exactly this attribute
/// value. Inert copies — inside `<template>`, or in a comment the parser never
/// turned into an element — do not count, for the same reason they do not
/// count for an element id: a runtime binds what is on the page.
fn has_attribute_value(page: &Html, attribute: &str, value: &str) -> bool {
    page.tree.nodes().any(|node| {
        node.value()
            .as_element()
            .and_then(|element| element.attr(attribute))
            .is_some_and(|declared| declared == value)
            && !node.ancestors().any(|ancestor| {
                ancestor
                    .value()
                    .as_element()
                    .is_some_and(|element| element.name() == "template")
            })
    })
}

/// Whether an element a browser would render carries this attribute at all,
/// whatever its value.
fn has_attribute(page: &Html, attribute: &str) -> bool {
    page.tree.nodes().any(|node| {
        node.value()
            .as_element()
            .is_some_and(|element| element.attr(attribute).is_some())
            && !node.ancestors().any(|ancestor| {
                ancestor
                    .value()
                    .as_element()
                    .is_some_and(|element| element.name() == "template")
            })
    })
}

fn marker_comment(marker: &str) -> Option<&str> {
    marker.strip_prefix("<!--")?.strip_suffix("-->")
}

fn marker_occurrences(page: &Html, marker: &str) -> usize {
    page.tree
        .nodes()
        .filter(|node| {
            matches!(node.value(), Node::Comment(comment) if &**comment == marker)
                && !node.ancestors().any(|ancestor| {
                    ancestor
                        .value()
                        .as_element()
                        .is_some_and(|element| element.name() == "template")
                })
        })
        .count()
}

fn marker_count_message(marker: &str, occurrences: usize) -> String {
    if occurrences == 0 {
        format!(
            "the rendered page does not contain `{marker}`, so there is nowhere to load a \
             runtime into"
        )
    } else {
        format!(
            "the rendered page contains `{marker}` {occurrences} times; it must appear exactly \
             once, or a runtime would be loaded more than once"
        )
    }
}

fn marker_source_count_message(marker: &str, occurrences: usize) -> String {
    format!(
        "the rendered page source contains `{marker}` {occurrences} times; it must appear exactly \
         once, as the active HTML comment where the runtime is loaded"
    )
}

fn incomplete(frontend: &UiFrontend, what: &str) -> Error {
    Error::PageIncomplete {
        frontend_id: frontend.frontend_id().map(str::to_owned),
        what: what.to_owned(),
    }
}

/// A MiniJinja failure, named to the template it came from. MiniJinja's own
/// top-level message is often only "template error" — the useful part is in
/// the `source()` chain underneath it, so this unwraps every layer rather
/// than reporting the outermost one.
fn render_error(frontend: &UiFrontend, template: &str, error: &minijinja::Error) -> Error {
    let mut detail = error.to_string();
    let mut source: Option<&dyn std::error::Error> = std::error::Error::source(error);
    while let Some(next) = source {
        detail.push_str(": ");
        detail.push_str(&next.to_string());
        source = next.source();
    }
    Error::PageRender {
        frontend_id: frontend.frontend_id().map(str::to_owned),
        template: template.to_owned(),
        detail,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde::Serialize;

    use super::*;
    use crate::frontend::CONFIG_DIR_ENV;
    use crate::{ENV_LOCK, Error};

    #[derive(Serialize)]
    struct EmptyDocument {}

    fn builtin_frontend() -> UiFrontend {
        UiFrontend::builtin("reviewer", "console", "1")
    }

    /// A page with every builder call satisfied, no override involved.
    #[test]
    fn a_complete_page_gets_its_runtime_spliced_in_and_nothing_else() {
        let frontend = builtin_frontend();
        let page = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<main><p id=\"status\">ok</p><!-- marker --></main>")
            .requires_elements(&["status"])
            .runtime_marker("<!-- marker -->")
            .runtime(Some("<script>run()</script>".to_owned()))
            .render(&EmptyDocument {})
            .unwrap();
        assert!(page.contains("<script>run()</script>"));
        assert!(!page.contains("<!-- marker -->"));
    }

    /// `None` still requires the marker to be present — it is replaced with
    /// nothing, not skipped.
    #[test]
    fn no_runtime_still_requires_the_marker_and_replaces_it_with_nothing() {
        let frontend = builtin_frontend();
        let page = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<main><!-- marker --></main>")
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap();
        assert_eq!(page, "<main></main>");
    }

    /// A missing element id is reported by name, not as a generic failure.
    #[test]
    fn a_missing_required_element_is_named_in_the_error() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<main id=\"tree\"><!-- marker --></main>")
            .requires_elements(&["tree", "status"])
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }));
        assert!(error.to_string().contains("`status`"), "{error}");
    }

    /// Everything a page is missing is named in one failure, across all three
    /// kinds of hook.
    ///
    /// Somebody writing a replacement page cannot read the contract from
    /// where they are standing — they find it by rendering. Naming one miss
    /// at a time turns a page that is missing five hooks into five renders,
    /// each of which sends them back for a fix they could have made in the
    /// same sitting as the others.
    #[test]
    fn every_missing_hook_is_named_in_one_failure() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<main id=\"tree\"><!-- marker --></main>")
            .requires_elements(&["tree", "status", "viewer"])
            .requires_attribute_values("data-decision", ["approve", "refuse"])
            .requires_attributes(["data-live"])
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
        let reported = error.to_string();
        for hook in [
            "`status`",
            "`viewer`",
            "data-decision=\\\"approve\\\"",
            "data-decision=\\\"refuse\\\"",
            "`data-live`",
        ] {
            let hook = hook.replace("\\\"", "\"");
            assert!(reported.contains(&hook), "`{hook}` missing from {reported}");
        }
        assert!(!reported.contains("`tree`"), "{reported}");
    }

    /// A required id must match exactly — a page that only has a longer id
    /// sharing the same prefix has not satisfied the contract.
    #[test]
    fn a_required_element_id_is_matched_exactly_not_as_a_prefix() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<main id=\"tree-empty\"><!-- marker --></main>")
            .requires_elements(&["tree"])
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }));
    }

    /// Attribute names and comments are inert text, not element identities.
    #[test]
    fn data_attributes_and_comments_do_not_satisfy_an_element_contract() {
        let frontend = builtin_frontend();
        for fallback in [
            "<main data-id=\"status\"><!-- marker --></main>",
            "<main><!-- id=\"status\" --><!-- marker --></main>",
        ] {
            let error = UiPage::builder(&frontend)
                .entry("page.html.j2")
                .fallback(fallback)
                .requires_elements(&["status"])
                .runtime_marker("<!-- marker -->")
                .runtime(None)
                .render(&EmptyDocument {})
                .unwrap_err();
            assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
        }
    }

    #[test]
    fn an_element_inside_an_inert_template_does_not_satisfy_the_runtime_contract() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<template><p id=\"status\"></p></template><!-- marker -->")
            .requires_elements(&["status"])
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
        assert!(error.to_string().contains("`status`"), "{error}");
    }

    #[derive(Serialize)]
    struct ElementMap {
        status: &'static str,
        decision: &'static str,
    }

    #[test]
    fn a_serialized_element_map_is_the_runtime_contract() {
        let frontend = builtin_frontend();
        let ids = ElementMap {
            status: "status",
            decision: "decision",
        };
        let page = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback(
                "<main><p id=\"status\"></p><button id=\"decision\"></button><!-- marker --></main>",
            )
            .requires_element_map(&ids)
            .unwrap()
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap();
        assert!(page.contains("id=\"decision\""));
    }

    /// A page that dropped one of its decide controls opens as a question
    /// nobody can answer, so it is refused and the missing declaration named.
    #[test]
    fn a_missing_declaration_is_named_in_the_error() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<button data-decision=\"approve\">yes</button><!-- marker -->")
            .requires_attribute_values("data-decision", ["approve", "refuse"])
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
        assert!(error.to_string().contains("refuse"), "{error}");
    }

    /// The one failure this check exists to catch that a looser match would
    /// let through: a declaration the runtime maps to nothing looks like a
    /// control and does nothing.
    #[test]
    fn a_declaration_is_matched_whole_not_as_a_prefix() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<button data-decision=\"approve-all\">yes</button><!-- marker -->")
            .requires_attribute_values("data-decision", ["approve"])
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
    }

    /// Inert declarations do not count, for the same reason inert elements do
    /// not: a runtime binds what a browser rendered.
    #[test]
    fn a_declaration_inside_an_inert_template_or_a_comment_does_not_count() {
        let frontend = builtin_frontend();
        for fallback in [
            "<template><button data-decision=\"approve\"></button></template><!-- marker -->",
            "<!-- <button data-decision=\"approve\"></button> --><!-- marker -->",
        ] {
            let error = UiPage::builder(&frontend)
                .entry("page.html.j2")
                .fallback(fallback)
                .requires_attribute_values("data-decision", ["approve"])
                .runtime_marker("<!-- marker -->")
                .runtime(None)
                .render(&EmptyDocument {})
                .unwrap_err();
            assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
        }
    }

    /// A hook that only survives in a comment is a hook no runtime can find.
    #[test]
    fn a_required_attribute_must_be_on_an_element_a_browser_renders() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<!-- <div data-snapshot></div> --><!-- marker -->")
            .requires_attributes(["data-snapshot"])
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
        assert!(error.to_string().contains("data-snapshot"), "{error}");

        let page = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<div data-snapshot></div><!-- marker -->")
            .requires_attributes(["data-snapshot"])
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap();
        assert!(page.contains("data-snapshot"));
    }

    /// A runtime binds every control it finds, so more than one carrier is a
    /// page that works — an approve button at the top and another at the
    /// bottom of a long page is not a contract violation.
    #[test]
    fn more_than_one_carrier_for_a_declaration_is_accepted() {
        let frontend = builtin_frontend();
        let page = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback(
                "<button data-decision=\"approve\">yes</button>\
                 <button data-decision=\"approve\">yes</button><!-- marker -->",
            )
            .requires_attribute_values("data-decision", ["approve"])
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap();
        assert!(page.contains("data-decision=\"approve\""));
    }

    /// One list: what the runtime binds is what the page must declare.
    #[test]
    fn a_decision_runtime_is_itself_the_page_contract() {
        let frontend = builtin_frontend();
        let runtime = crate::UiDecisionRuntime::new("data-decision")
            .unwrap()
            .with_decision("approve", "approve")
            .unwrap()
            .with_decision("refuse", "refuse")
            .unwrap();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<button data-decision=\"approve\">yes</button><!-- marker -->")
            .requires_decisions(&runtime)
            .runtime_marker("<!-- marker -->")
            .runtime(Some(format!("<script>{}</script>", runtime.source())))
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(error.to_string().contains("refuse"), "{error}");
    }

    /// A marker written twice would load a runtime twice — refused before
    /// `replace` ever gets the chance.
    #[test]
    fn a_marker_that_appears_twice_is_refused() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<!-- marker --><!-- marker -->")
            .runtime_marker("<!-- marker -->")
            .runtime(Some("<script></script>".to_owned()))
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }));
        assert!(error.to_string().contains("2 times"), "{error}");
    }

    /// A marker that never appears at all is the same kind of failure.
    #[test]
    fn a_missing_marker_is_refused() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<main></main>")
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }));
    }

    /// A comment inside `<template>` is parsed but inert: replacing its source
    /// would load the runtime only when unrelated page code cloned it later.
    #[test]
    fn a_marker_inside_an_inert_template_is_refused() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<template><!-- marker --></template>")
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
    }

    #[test]
    fn inert_marker_text_cannot_steal_the_runtime_splice() {
        let frontend = builtin_frontend();
        for fallback in [
            "<main data-marker=\"<!-- marker -->\"></main><!-- marker -->",
            "<template><!-- marker --></template><!-- marker -->",
        ] {
            let error = UiPage::builder(&frontend)
                .entry("page.html.j2")
                .fallback(fallback)
                .runtime_marker("<!-- marker -->")
                .runtime(Some("<script>run()</script>".to_owned()))
                .render(&EmptyDocument {})
                .unwrap_err();
            assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
            assert!(error.to_string().contains("2 times"), "{error}");
        }
    }

    #[test]
    fn a_runtime_marker_must_be_an_html_comment() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<main>marker</main>")
            .runtime_marker("marker")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
    }

    /// The `safe` filter is removed from the environment, so a template that
    /// reaches for it fails to render rather than silently turning escaping
    /// back off. This is the defense that still holds even for a Provider's
    /// own fallback, which the source-level guard below does not scan.
    #[test]
    fn a_template_reaching_for_the_removed_safe_filter_fails_to_render() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("{{ document|safe }}<!-- marker -->")
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageRender { .. }), "{error}");
        assert!(
            error.to_string().to_ascii_lowercase().contains("safe"),
            "{error}"
        );
    }

    /// A template references AFUI's own routes through AFUI, so moving one is
    /// a render that changes rather than six pages that quietly stop working.
    #[test]
    fn the_reserved_paths_reach_the_template_as_a_global() {
        let frontend = builtin_frontend();
        let page = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback(
                "<link rel=\"icon\" href=\"{{ afui.app_icon_path }}\">\
                 <link rel=\"stylesheet\" href=\"{{ afui.base_style_path }}\">\
                 <p data-end=\"{{ afui.end_path }}\"></p>\
                 <!-- marker -->",
            )
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap();
        assert!(page.contains("href=\"__afui/app-icon.svg\""), "{page}");
        assert!(page.contains("href=\"__afui/base.css\""), "{page}");
        assert!(page.contains("data-end=\"__afui/end\""), "{page}");
    }

    /// `afui` is reserved in the same way `document` is: an extension that
    /// could shadow it could point the page's icon or end route somewhere else,
    /// which is not a decision a replaceable layer gets to make.
    #[test]
    fn a_page_extension_may_not_shadow_the_reserved_afui_global() {
        let frontend = builtin_frontend();
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<!-- marker -->")
            .extend_environment(|environment| environment.add_global("afui", "mine".to_owned()))
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
        assert!(error.to_string().contains("`afui`"), "{error}");
    }

    #[test]
    fn a_page_extension_can_add_a_filter_but_not_restore_safe() {
        let frontend = builtin_frontend();
        let page = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback(
                "<p>{{ badge }} {{ document.name|initials }} {{ 'x'|normalized }}</p><!-- marker -->",
            )
            .extend_environment(|environment| {
                environment.add_filter("initials", |value: String| {
                    value.chars().next().unwrap_or_default().to_string()
                })?;
                environment.add_filter("normalized", |_value: String| {
                    UiPageSerializedValue::from_serialize(Value::from_safe_string(
                        "<normalized>".to_owned(),
                    ))
                })?;
                environment.add_global("badge", "<review>".to_owned())
            })
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&serde_json::json!({ "name": "Reviewer" }))
            .unwrap();
        assert!(
            page.contains("<p>&lt;review&gt; R &lt;normalized&gt;</p>"),
            "{page}"
        );

        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<!-- marker -->")
            .extend_environment(|environment| environment.add_filter("safe", |v: String| v))
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::PageIncomplete { .. }), "{error}");
    }

    /// A non-entry template participates in the same override/fallback
    /// resolution, and the entry can include it by the name it was
    /// registered under.
    #[test]
    fn an_included_template_is_registered_under_its_own_name() {
        let frontend = builtin_frontend();
        let page = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("{% include \"layout.html.j2\" %}")
            .template("layout.html.j2", "<main><!-- marker --></main>")
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap();
        assert_eq!(page, "<main></main>");
    }

    // ── Override behaviour needs a real frontend on disk ──────────────────

    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("afui-page-{name}-{stamp}"))
    }

    /// A trusted override at the global scope, with one file already written
    /// into it.
    fn overridden_frontend(root: &Path, file: &str, contents: &str) -> UiFrontend {
        let directory = root.join("frontends").join("reviewer").join("console");
        fs::create_dir_all(directory.join(Path::new(file).parent().unwrap_or(Path::new(""))))
            .unwrap();
        fs::write(
            directory.join("frontend.json"),
            r#"{"frontend_id":"custom","ui_api_version":"1"}"#,
        )
        .unwrap();
        fs::write(directory.join(file), contents).unwrap();
        let guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        // SAFETY: the lock makes this the only thread reading or writing the
        // variable while this frontend is resolved.
        unsafe { std::env::set_var(CONFIG_DIR_ENV, root) };
        let frontend = UiFrontend::resolve_with_safe_mode(
            Path::new("/nonexistent"),
            "reviewer",
            "console",
            "1",
            false,
        )
        .unwrap();
        drop(guard);
        frontend
    }

    /// A frontend may add partials the Provider never heard of and include
    /// them; what it may not do is shadow a template that is part of the
    /// contract, because that one keeps its own fallback.
    #[test]
    fn open_ended_partials_are_registered_and_contract_templates_are_left_alone() {
        let root = temp_root("partials");
        let frontend = overridden_frontend(
            &root,
            "templates/extra.html.j2",
            "<p id=\"status\">mine</p>",
        );
        fs::write(
            root.join("frontends/reviewer/console/templates/notes.txt"),
            "not a template",
        )
        .unwrap();
        let page = UiPage::builder(&frontend)
            .entry("templates/page.html.j2")
            .fallback("{% include \"templates/extra.html.j2\" %}<!-- marker -->")
            .template("templates/layout.html.j2", "<main></main>")
            .override_templates_under("templates", ".j2")
            .unwrap()
            .requires_elements(&["status"])
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap();
        assert!(page.contains("mine"), "{page}");
        let _ = fs::remove_dir_all(root);
    }

    /// A script hiding inside an override's entry template is refused before
    /// it ever reaches MiniJinja — the whole reason the source-level check
    /// exists rather than trusting the rendered-output guard alone.
    #[test]
    fn a_script_hidden_inside_an_override_is_refused() {
        let root = temp_root("script");
        let frontend = overridden_frontend(
            &root,
            "page.html.j2",
            "<main><script>alert(1)</script><!-- marker --></main>",
        );
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<main><!-- marker --></main>")
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::FrontendScript { .. }), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    /// The same refusal applies to a non-entry template an override supplies.
    #[test]
    fn a_script_hidden_inside_an_included_override_template_is_refused() {
        let root = temp_root("included-script");
        let frontend = overridden_frontend(
            &root,
            "layout.html.j2",
            "<main onclick=\"go()\"><!-- marker --></main>",
        );
        let error = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("{% include \"layout.html.j2\" %}")
            .template("layout.html.j2", "<main><!-- marker --></main>")
            .runtime_marker("<!-- marker -->")
            .runtime(None)
            .render(&EmptyDocument {})
            .unwrap_err();
        assert!(matches!(error, Error::FrontendScript { .. }), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    /// An override's entry replaces the fallback outright; the rest of the
    /// contract still applies to whatever it produced.
    #[test]
    fn an_override_entry_is_used_in_place_of_the_fallback() {
        let root = temp_root("entry");
        let frontend = overridden_frontend(
            &root,
            "page.html.j2",
            "<main id=\"status\">mine<!-- marker --></main>",
        );
        let page = UiPage::builder(&frontend)
            .entry("page.html.j2")
            .fallback("<main id=\"status\">builtin<!-- marker --></main>")
            .requires_elements(&["status"])
            .runtime_marker("<!-- marker -->")
            .runtime(Some("<script></script>".to_owned()))
            .render(&EmptyDocument {})
            .unwrap();
        assert!(page.contains("mine"));
        assert!(!page.contains("builtin"));
        let _ = fs::remove_dir_all(root);
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent_first_data::skill::{
    self, SkillAction, SkillAgentSelection, SkillAsset, SkillOptions, SkillScope, SkillSpec,
};
use agent_first_data::{
    ArgSpec, BoundOutcome, BuiltCliSpec, CliEmitter, CliSpec, CliSpecError, CliValue, Combination,
    CommandSpec, OutputFormat, OutputPlan, OutputSpec, OutputTo, ResolvedInvocation,
    build_afdata_cli, cli_error_event, cli_help_event, cli_version_event, render_cli_reference,
};
use agent_first_ui::{
    FrontendManifest, FrontendScope, SAFE_MODE_ENV, UiWindow, UiWindowConfig, frontend_root,
    global_afui_root, load_frontend, validate_frontend_identifier,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use walkdir::WalkDir;

#[path = "support/ask.rs"]
mod ask;

fn output() -> OutputSpec {
    OutputSpec::protocol_finite(
        ["json", "yaml", "plain"],
        ["split", "stdout", "stderr"],
        "json",
        "split",
    )
}

/// For the one verb that keeps running: it says where to go, then serves.
fn stream_output() -> OutputSpec {
    OutputSpec::protocol_stream(
        ["json", "yaml", "plain"],
        ["stdout", "stderr"],
        "json",
        "stdout",
    )
}

/// Loopback, because reaching this from another device is a decision.
const DEFAULT_LISTEN: &str = "127.0.0.1:8787";

/// The skill's task-specific references travel with the binary so an installed
/// skill can route to them without a checkout or network access. Exact flags
/// still come from the CLI registry's help.
const SKILL_SPEC: SkillSpec = SkillSpec {
    name: "agent-first-ui",
    source: include_str!("../../skills/agent-first-ui/SKILL.md"),
    title: "Agent-First UI",
    marker_slug: "afui",
    assets: &[
        SkillAsset {
            path: "references/ask.md",
            contents: include_str!("../../skills/agent-first-ui/references/ask.md"),
        },
        SkillAsset {
            path: "references/providers.md",
            contents: include_str!("../../skills/agent-first-ui/references/providers.md"),
        },
        SkillAsset {
            path: "references/sessions.md",
            contents: include_str!("../../skills/agent-first-ui/references/sessions.md"),
        },
        SkillAsset {
            path: "references/frontends.md",
            contents: include_str!("../../skills/agent-first-ui/references/frontends.md"),
        },
    ],
};

/// The named agents, and the fan-out value that is not one of them. Both skill
/// shapes partition `--agent` between these, so the enum and that partition are
/// built from one list.
const AGENTS: [&str; 4] = ["codex", "claude-code", "opencode", "hermes"];
const EVERY_AGENT: &str = "all";

fn cli_spec() -> Result<BuiltCliSpec, CliSpecError> {
    let mut spec = CliSpec::new("afui", env!("CARGO_PKG_VERSION"))
        .display_name(env!("DISPLAY_NAME"))
        .about("Ask a person questions, deliver UI sessions, and manage Provider-owned frontends.")
        .lifecycle_output(output())
        .command(CommandSpec::root())
        .command(
            CommandSpec::new(["frontend"])
                .about("Scaffold, check, preview, and trust Provider frontend replacements."),
        )
        .command(init_command())
        .command(ask::ask_command())
        .command(ask::wait_command());
    for (name, about) in [
        ("check", "Validate one frontend and report its trust state."),
        (
            "dev",
            "Prepare a validated frontend for Provider-owned development.",
        ),
        (
            "preview",
            "Prepare a validated frontend for Provider-owned preview.",
        ),
        (
            "enable",
            "Explicitly trust the current contents of one frontend.",
        ),
        ("disable", "Remove explicit trust for one frontend."),
        ("diff", "Compare workspace and global frontend files."),
    ] {
        spec = spec.command(frontend_command(name, about));
    }
    spec = spec
        .command(
            CommandSpec::new(["session"])
                .about("See and open the UI sessions Providers currently have waiting."),
        )
        .command(
            CommandSpec::new(["session", "list"])
                .about("List every UI session running on this machine.")
                .combination(
                    Combination::new("session-list")
                        .action("session_list")
                        .output(output()),
                ),
        )
        .command(
            CommandSpec::new(["session", "open"])
                .about("Open a window onto one running session.")
                .arg(
                    ArgSpec::positional("session_id", 0, "SESSION_ID")
                        .about("Session identifier from `afui session list`"),
                )
                .combination(
                    Combination::new("session-open")
                        .action("session_open")
                        .required(["session_id"])
                        .output(output()),
                ),
        )
        .command(
            CommandSpec::new(["session", "close"])
                .about(
                    "End a running UI session from outside the process that created it. \
                     Whoever created it gets `Outcome::Closed`, the same as if a person had \
                     closed its window.",
                )
                .arg(
                    ArgSpec::positional("session_id", 0, "SESSION_ID")
                        .about("Session identifier from `afui session list`"),
                )
                .combination(
                    Combination::new("session-close")
                        .action("session_close")
                        .required(["session_id"])
                        .output(output()),
                ),
        )
        .command(
            CommandSpec::new(["session", "serve"])
                .about(
                    "Serve every running session to a browser on another device. \
                     Binds a port and stops there: getting to that port is an SSH tunnel, \
                     WireGuard, Tailscale, or a TLS reverse proxy, none of which this is.",
                )
                .reference_note(
                    "When neither override is supplied, AFUI reads `attention.idle_timeout_s` \
                     (default `900`) and `attention.grace_period_s` (default `300`) from the \
                     global AFUI `config.json`: `~/Library/Application Support/afui/config.json` \
                     on macOS, `$XDG_CONFIG_HOME/afui/config.json` on Linux (or \
                     `~/.config/afui/config.json` when `XDG_CONFIG_HOME` is unset), or \
                     `%APPDATA%\\afui\\config.json` on Windows. `AFUI_CONFIG_DIR` replaces that \
                     file's parent directory. The same policy controls direct `link` delivery.\n\n\
                     The serving progress event reports the resolved values as `idle_timeout_s` \
                     and `grace_period_s`; `idle_timeout_s` is `null` when expiry is disabled.\n\n\
                     The shared entry URL stays valid until this process exits. Unattended \
                     revokes all mounts and streams while the current session list remains \
                     available. Heartbeat resumes attendance; lapsed cards need Reopen. \
                     Direct links retain terminal Expired delivery. AFUI-owned controls \
                     follow Accept-Language (English or Simplified Chinese, English fallback).",
                )
                .arg(
                    ArgSpec::option("--listen", "ADDRESS")
                        .default(DEFAULT_LISTEN)
                        .about(
                            "Address to bind. Loopback by default; widen it only behind a \
                             tunnel or VPN you trust",
                        ),
                )
                .arg(ArgSpec::option("--advertise-host", "HOST").about(
                    "Authority to put in the URL when the bound address is not one \
                         anybody can open",
                ))
                .arg(
                    ArgSpec::option_i64("--idle-minutes", "MINUTES")
                        .range(0, i64::MAX)
                        .about(
                            "Start the warning this long after nobody is attending the page: no \
                             visible tab, no traffic through a session. Overrides AFUI's global \
                             `attention.idle_timeout_s`; `0` disables attention expiry",
                        ),
                )
                .arg(
                    ArgSpec::option_i64("--grace-minutes", "MINUTES")
                        .range(0, i64::MAX)
                        .about(
                            "How long the page warns first, still fully working, with a button \
                             that renews it. Overrides AFUI's global `attention.grace_period_s`; \
                             `0` lapses without warning",
                        ),
                )
                .arg(ArgSpec::option("--session-origin-host", "HOST").about(
                    "Give every session an origin of its own, `s-<credential>.<HOST>`, so a \
                     framed UI keeps browser storage. Needs `*.<HOST>` to resolve here, and a \
                     wildcard certificate behind TLS; `localhost` needs neither, because \
                     browsers resolve `*.localhost` to this machine. Also the host the page \
                     itself is served on",
                ))
                .combination(
                    Combination::new("session-serve")
                        .action("session_serve")
                        .optional([
                            "listen",
                            "advertise_host",
                            "session_origin_host",
                            "idle_minutes",
                            "grace_minutes",
                        ])
                        .output(stream_output()),
                ),
        );
    spec =
        spec.command(CommandSpec::new(["skill"]).about(
            "Manage the Agent-First UI skill for Codex, Claude Code, opencode, and Hermes.",
        ))
        .command(skill_command(
            "status",
            "Show whether the Agent-First UI skill is installed, valid, and up to date.",
            false,
        ))
        .command(skill_command(
            "install",
            "Install the Agent-First UI skill.",
            true,
        ))
        .command(skill_command(
            "uninstall",
            "Remove an afui-managed Agent-First UI skill.",
            true,
        ));
    // Absent from a source tarball with no reachable .git, and the version
    // payload omits it rather than reporting the literal "unknown".
    if let Some(build) = Some(env!("GIT_SHA")).filter(|sha| *sha != "unknown") {
        spec = spec.build_id(build);
    }
    build_afdata_cli(spec)
}

fn base_frontend_command(path: &str, about: &str) -> CommandSpec {
    let scopes: &[&str] = if matches!(path, "enable" | "disable") {
        &["workspace"]
    } else {
        &["workspace", "global"]
    };
    CommandSpec::new(["frontend", path])
        .about(about)
        .arg(
            ArgSpec::positional("provider_id", 0, "PROVIDER_ID")
                .about("Stable Provider identifier"),
        )
        .arg(ArgSpec::positional("ui_kind", 1, "UI_KIND").about("Stable UI kind identifier"))
        .arg(
            ArgSpec::option_enum("--scope", scopes.iter().copied())
                .value_name("SCOPE")
                .default("workspace")
                .about("Frontend scope"),
        )
}

fn init_command() -> CommandSpec {
    base_frontend_command("init", "Create a frontend directory and manifest.")
        .arg(
            ArgSpec::option("--frontend-id", "FRONTEND_ID")
                .about("Stable identifier for this frontend implementation"),
        )
        .arg(
            ArgSpec::option("--ui-api-version", "VERSION")
                .default("1")
                .about("Provider-defined typed UI API version"),
        )
        .combination(
            Combination::new("frontend-init")
                .action("frontend_init")
                .required(["provider_id", "ui_kind", "frontend_id"])
                .optional(["scope", "ui_api_version"])
                .output(output()),
        )
}

fn frontend_command(name: &str, about: &str) -> CommandSpec {
    let mut command = base_frontend_command(name, about);
    let mut optional = vec!["scope"];
    if matches!(name, "check" | "dev" | "preview") {
        command = command.arg(
            ArgSpec::flag("--safe-mode")
                .about("Ignore user frontend code before reading its manifest"),
        );
        optional.push("safe_mode");
    }
    command.combination(
        Combination::new(format!("frontend-{name}"))
            .action(format!("frontend_{name}"))
            .required(["provider_id", "ui_kind"])
            .optional(optional)
            .output(output()),
    )
}

fn main() -> ExitCode {
    let cli = match cli_spec() {
        Ok(cli) => cli,
        Err(error) => return startup_error("cli_spec_invalid", &error.to_string()),
    };
    let app = match cli.bind_actions([
        (
            "frontend_init",
            run_init as fn(&ResolvedInvocation) -> ExitCode,
        ),
        ("session_list", run_session_list),
        ("session_open", run_session_open),
        ("session_close", run_session_close),
        ("session_serve", run_session_serve),
        ("ask", ask::run),
        ("ask_wait", ask::run_wait),
        ("frontend_check", run_check),
        ("frontend_dev", run_dev),
        ("frontend_preview", run_preview),
        ("frontend_enable", run_enable),
        ("frontend_disable", run_disable),
        ("frontend_diff", run_diff),
        ("skill_status", run_skill_status),
        ("skill_install", run_skill_install),
        ("skill_uninstall", run_skill_uninstall),
    ]) {
        Ok(app) => app,
        Err(error) => return startup_error("cli_actions_invalid", &error.to_string()),
    };
    let outcome = match app.resolve_from(std::env::args_os()) {
        Ok(outcome) => outcome,
        Err(error) => {
            return emit_event(
                cli_error_event(&error),
                OutputFormat::Json,
                OutputTo::Stderr,
                error.exit_code(),
            );
        }
    };
    match outcome {
        BoundOutcome::Run(invocation) => invocation.run(),
        BoundOutcome::Docs(docs) => write_text(
            &render_cli_reference(&cli),
            stream_of(docs.output_plan(), false),
        ),
        BoundOutcome::Help(help) => {
            let (format, output_to) = plan_output(help.output_plan());
            if format == OutputFormat::Plain {
                write_text(&help.plain(), stream_of(help.output_plan(), false))
            } else {
                emit_event(cli_help_event(&help), format, output_to, 0)
            }
        }
        BoundOutcome::Version(version) => {
            let (format, output_to) = plan_output(version.output_plan());
            emit_event(cli_version_event(&version), format, output_to, 0)
        }
    }
}

/// The name a Provider looks a frontend's stylesheet up under. One name across
/// the kit, because a person authoring a frontend should not have to learn a
/// different one per Provider.
const STYLESHEET_NAME: &str = "style.css";

/// What a new frontend starts as, rather than an empty directory whose first
/// render always fails.
///
/// It says the two things this directory cannot tell you by looking at it:
/// which parts of the page's appearance a replacement is taking on, and which
/// parts it cannot lose. There are no rules in it, because a rule here would
/// be AFUI having an opinion about a page it has never seen.
const STYLESHEET_STARTER: &str = "\
/* This file replaces the Provider's own stylesheet, in full, as soon as this
 * frontend is trusted (`afui frontend enable`). Anything the page looked like
 * because of a rule the Provider wrote is now yours to write.
 *
 * What a replacement never loses is the floor: AFUI serves `__afui/base.css`
 * itself, under the page's own session, and this file cannot remove it. The
 * palette, the reset, the focus ring, `[hidden]`, the in-flight look of a
 * disabled control and the inset that clears a phone's edges are already
 * there. Retune any of it by redeclaring the property:
 *
 *     :root {
 *       --afui-accent: #7a3cff;
 *       --afui-page-padding: 1.5rem;
 *     }
 *
 * Surfaces      --afui-page  --afui-panel  --afui-panel-strong
 * Text          --afui-ink  --afui-muted
 * Lines         --afui-line
 * Emphasis      --afui-accent  --afui-accent-ink  --afui-focus
 * Meaning       --afui-warn-ink/-bg  --afui-danger-ink/-bg  --afui-ok-ink/-bg
 * Shape         --afui-radius  --afui-shadow  --afui-font  --afui-mono
 * Layout        --afui-page-padding
 *
 * Every one of them has a light and a dark value already; redeclaring one
 * outside a `prefers-color-scheme` block sets it for both themes.
 *
 * The templates go in `templates/`, under the names the Provider looks them
 * up by. You do not have to guess which elements the page must carry: render
 * it once and the failure names every hook that is missing, all of them
 * together.
 */
";

fn run_init(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, "frontend_error", || {
        let context = context(invocation)?;
        let frontend_id = required(invocation, "frontend_id");
        validate_frontend_identifier("frontend_id", &frontend_id)
            .map_err(|error| error.to_string())?;
        let ui_api_version =
            optional(invocation, "ui_api_version").unwrap_or_else(|| "1".to_owned());
        if ui_api_version.trim().is_empty() {
            return Err("ui_api_version must not be empty".to_owned());
        }
        if context.root.exists() {
            return Err(format!(
                "frontend directory already exists: {}",
                context.root.display()
            ));
        }
        fs::create_dir_all(context.root.join("templates")).map_err(io_error)?;
        fs::create_dir_all(context.root.join("assets")).map_err(io_error)?;
        let manifest = FrontendManifest {
            frontend_id,
            ui_api_version,
        };
        write_json(&context.root.join("frontend.json"), &manifest)?;
        let stylesheet = context.root.join(STYLESHEET_NAME);
        fs::write(&stylesheet, STYLESHEET_STARTER).map_err(io_error)?;
        Ok(json!({
            "code": "frontend_init",
            "base_style_path": agent_first_ui::BASE_STYLE_PATH,
            "frontend_id": manifest.frontend_id,
            "frontend_root": context.root,
            "provider_id": context.provider_id,
            "scope": context.scope_name,
            "stylesheet": stylesheet,
            "ui_api_version": manifest.ui_api_version,
            "ui_kind": context.ui_kind,
        }))
    })
}

fn run_check(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, "frontend_error", || {
        inspect(invocation, "frontend_check")
    })
}

fn run_dev(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, "frontend_error", || {
        let mut value = inspect(invocation, "frontend_dev")?;
        value["provider_owned_runtime"] = Value::Bool(true);
        value["message"] = Value::String(
            "frontend is ready for the Provider's typed development server".to_owned(),
        );
        Ok(value)
    })
}

fn run_preview(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, "frontend_error", || {
        let mut value = inspect(invocation, "frontend_preview")?;
        value["provider_owned_runtime"] = Value::Bool(true);
        value["message"] =
            Value::String("frontend is ready for the Provider's typed preview server".to_owned());
        Ok(value)
    })
}

fn run_enable(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, "frontend_error", || {
        let context = context(invocation)?;
        let location = load_frontend(context.scope, context.root.clone())
            .map_err(|error| error.to_string())?;
        let fingerprint = fingerprint(&context.root)?;
        let mut trust = load_trust()?;
        trust.frontends.insert(
            trust_key(&context),
            TrustEntry {
                fingerprint: fingerprint.clone(),
            },
        );
        save_trust(&trust)?;
        Ok(json!({
            "code": "frontend_enable",
            "fingerprint": fingerprint,
            "frontend_id": location.manifest.frontend_id,
            "frontend_root": context.root,
            "provider_id": context.provider_id,
            "scope": context.scope_name,
            "trusted": true,
            "ui_kind": context.ui_kind,
        }))
    })
}

fn run_disable(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, "frontend_error", || {
        let context = context(invocation)?;
        let mut trust = load_trust()?;
        let disabled = trust.frontends.remove(&trust_key(&context)).is_some();
        save_trust(&trust)?;
        Ok(json!({
            "code": "frontend_disable",
            "disabled": disabled,
            "frontend_root": context.root,
            "provider_id": context.provider_id,
            "scope": context.scope_name,
            "trusted": false,
            "ui_kind": context.ui_kind,
        }))
    })
}

fn run_diff(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, "frontend_error", || {
        let provider_id = required(invocation, "provider_id");
        let ui_kind = required(invocation, "ui_kind");
        validate_frontend_identifier("provider_id", &provider_id)
            .map_err(|error| error.to_string())?;
        validate_frontend_identifier("ui_kind", &ui_kind).map_err(|error| error.to_string())?;
        let workspace = std::env::current_dir().map_err(io_error)?;
        let workspace_root =
            frontend_root(FrontendScope::Workspace, &workspace, &provider_id, &ui_kind)
                .map_err(|error| error.to_string())?;
        let global_root = frontend_root(FrontendScope::Global, &workspace, &provider_id, &ui_kind)
            .map_err(|error| error.to_string())?;
        let workspace_files = file_hashes(&workspace_root)?;
        let global_files = file_hashes(&global_root)?;
        let all_paths: BTreeSet<_> = workspace_files
            .keys()
            .chain(global_files.keys())
            .cloned()
            .collect();
        let mut changed = Vec::new();
        let mut only_global = Vec::new();
        let mut only_workspace = Vec::new();
        for path in all_paths {
            match (workspace_files.get(&path), global_files.get(&path)) {
                (Some(left), Some(right)) if left != right => changed.push(path),
                (Some(_), None) => only_workspace.push(path),
                (None, Some(_)) => only_global.push(path),
                _ => {}
            }
        }
        let different =
            !(only_workspace.is_empty() && only_global.is_empty() && changed.is_empty());
        Ok(json!({
            "changed_paths": changed,
            "code": "frontend_diff",
            "different": different,
            "global_root": global_root,
            "only_global_paths": only_global,
            "only_workspace_paths": only_workspace,
            "provider_id": provider_id,
            "ui_kind": ui_kind,
            "workspace_root": workspace_root,
        }))
    })
}

fn inspect(invocation: &ResolvedInvocation, code: &str) -> Result<Value, String> {
    let context = context(invocation)?;
    let safe_mode = flag(invocation, "safe_mode") || safe_mode_env();
    if safe_mode {
        return Ok(json!({
            "code": code,
            "frontend_root": context.root,
            "provider_id": context.provider_id,
            "safe_mode": true,
            "scope": context.scope_name,
            "status": "ignored",
            "trusted": false,
            "ui_kind": context.ui_kind,
        }));
    }
    let location =
        load_frontend(context.scope, context.root.clone()).map_err(|error| error.to_string())?;
    let current_fingerprint = fingerprint(&context.root)?;
    let trusted = match context.scope {
        FrontendScope::Global => true,
        FrontendScope::Workspace => load_trust()?
            .frontends
            .get(&trust_key(&context))
            .is_some_and(|entry| entry.fingerprint == current_fingerprint),
    };
    Ok(json!({
        "code": code,
        "fingerprint": current_fingerprint,
        "frontend_id": location.manifest.frontend_id,
        "frontend_root": context.root,
        "provider_id": context.provider_id,
        "safe_mode": false,
        "scope": context.scope_name,
        "status": if trusted { "ready" } else { "disabled" },
        "trusted": trusted,
        "ui_api_version": location.manifest.ui_api_version,
        "ui_kind": context.ui_kind,
    }))
}

struct Context {
    provider_id: String,
    ui_kind: String,
    scope: FrontendScope,
    scope_name: &'static str,
    root: PathBuf,
}

fn context(invocation: &ResolvedInvocation) -> Result<Context, String> {
    let provider_id = required(invocation, "provider_id");
    let ui_kind = required(invocation, "ui_kind");
    let (scope, scope_name) = match optional(invocation, "scope").as_deref() {
        Some("global") => (FrontendScope::Global, "global"),
        _ => (FrontendScope::Workspace, "workspace"),
    };
    let workspace = std::env::current_dir().map_err(io_error)?;
    let root = frontend_root(scope, &workspace, &provider_id, &ui_kind)
        .map_err(|error| error.to_string())?;
    Ok(Context {
        provider_id,
        ui_kind,
        scope,
        scope_name,
        root,
    })
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustStore {
    #[serde(default)]
    frontends: BTreeMap<String, TrustEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TrustEntry {
    fingerprint: String,
}

fn trust_path() -> Result<PathBuf, String> {
    global_afui_root()
        .map(|root| root.join("trust.json"))
        .map_err(|error| error.to_string())
}

fn load_trust() -> Result<TrustStore, String> {
    let path = trust_path()?;
    if !path.exists() {
        return Ok(TrustStore::default());
    }
    let bytes = fs::read(path).map_err(io_error)?;
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid trust.json: {error}"))
}

fn save_trust(trust: &TrustStore) -> Result<(), String> {
    let path = trust_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| "trust path has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let temporary = path.with_extension("json.tmp");
    write_json(&temporary, trust)?;
    fs::rename(temporary, path).map_err(io_error)
}

fn trust_key(context: &Context) -> String {
    format!(
        "{}:{}:{}",
        context.scope_name, context.provider_id, context.ui_kind
    )
}

fn fingerprint(root: &Path) -> Result<String, String> {
    let hashes = file_hashes(root)?;
    let mut hasher = blake3::Hasher::new();
    for (path, hash) in hashes {
        hasher.update(path.as_bytes());
        hasher.update(&[0]);
        hasher.update(hash.as_bytes());
        hasher.update(&[0]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn file_hashes(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut hashes = BTreeMap::new();
    if !root.exists() {
        return Ok(hashes);
    }
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| error.to_string())?;
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        // Refused as the library refuses it when it loads the frontend, or
        // `check` and `enable` would report ready a frontend no render can read.
        if entry.file_type().is_symlink() {
            return Err(format!(
                "a frontend may not supply `{relative}`: it is a symlink, and a trusted \
                 fingerprint covers files rather than the places they point at"
            ));
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let bytes = fs::read(entry.path()).map_err(io_error)?;
        hashes.insert(relative, blake3::hash(&bytes).to_hex().to_string());
    }
    Ok(hashes)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    fs::write(path, bytes).map_err(io_error)
}

fn safe_mode_env() -> bool {
    std::env::var_os(SAFE_MODE_ENV).is_some_and(|value| {
        let value = value.to_string_lossy();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

/// A value the matched shape declares as required or fixed.
///
/// Reading it cannot fail: the shape that matched supplies every id it
/// declares. Asking for an id it does not declare is a defect in this file, not
/// a value the caller omitted.
fn required(invocation: &ResolvedInvocation, id: &str) -> String {
    invocation
        .required(id)
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn optional(invocation: &ResolvedInvocation, id: &str) -> Option<String> {
    invocation
        .optional(id)
        .and_then(CliValue::as_str)
        .map(str::to_owned)
}

fn flag(invocation: &ResolvedInvocation, id: &str) -> bool {
    invocation
        .optional(id)
        .and_then(CliValue::as_bool)
        .unwrap_or(false)
}

/// How long the page outlives the person looking at it.
///
/// Both flags refuse a negative value at parse time. `0` means what the policy
/// makes of it: no idle expiry for `--idle-minutes`, no warning before the
/// lapse for `--grace-minutes`.
fn attention_policy_override(
    invocation: &ResolvedInvocation,
) -> Result<Option<agent_first_ui::UiAttentionPolicy>, String> {
    let minutes = |id: &str| -> Option<std::time::Duration> {
        invocation
            .optional(id)
            .and_then(CliValue::as_i64)
            .map(minutes_as_duration)
    };
    let idle = minutes("idle_minutes");
    let grace = minutes("grace_minutes");
    if idle.is_none() && grace.is_none() {
        return Ok(None);
    }
    let mut policy =
        agent_first_ui::UiAttentionPolicy::load_global().map_err(|error| error.to_string())?;
    if let Some(idle) = idle {
        policy = policy.with_idle_timeout(idle);
    }
    if let Some(grace) = grace {
        policy = policy.with_grace_period(grace);
    }
    Ok(Some(policy))
}

/// A minute count as a duration. Non-positive is zero; a count too large to
/// express in seconds saturates rather than wrapping to a short interval.
fn minutes_as_duration(minutes: i64) -> std::time::Duration {
    std::time::Duration::from_secs(
        u64::try_from(minutes.max(0))
            .unwrap_or(0)
            .saturating_mul(60),
    )
}

/// List the sessions Providers currently have open.
///
/// The credential never appears here. A session URL carries one, and this is
/// stdout — where an agent reads. `session open` is how a session is reached,
/// and it hands the URL straight to a window without printing it.
fn run_session_list(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, "session_list_failed", || {
        let sessions = agent_first_ui::list_sessions().map_err(|error| error.to_string())?;
        let listed: Vec<Value> = sessions
            .iter()
            .map(|entry| {
                let mut value = serde_json::json!({
                    "session_id": entry.session_id,
                    "provider_id": entry.provider_id,
                    "ui_kind": entry.ui_kind,
                    "owner_pid": entry.owner_pid,
                    // Whether a person is being waited on, not merely
                    // watched, and how long they have been waiting — the two
                    // facts a person or an agent needs to tell "a stopped
                    // agent" from "an open monitor" before assuming nothing
                    // is blocked. How to draw that is the caller's; this only
                    // reports what afui already knows.
                    "waits_for_decision": entry.waits_for_decision,
                    "announced_at_epoch_ms": entry.announced_at_epoch_ms,
                });
                if let Some(subject) = &entry.subject {
                    value["subject"] = Value::String(subject.clone());
                }
                value
            })
            .collect();
        Ok(serde_json::json!({
            "code": "session_list",
            "sessions": listed,
            "session_count": listed.len(),
        }))
    })
}

/// Open a window onto a session another process is running, and wait.
///
/// This does not take the session over. Whoever created it still decides what
/// completing it means; closing this window ends this view of it the same way
/// closing the Provider's own window would.
fn run_session_open(invocation: &ResolvedInvocation) -> ExitCode {
    run_coded(invocation, "session_open_failed", || {
        let session_id = required(invocation, "session_id");
        let entry = agent_first_ui::find_session(&session_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| session_not_found(&session_id))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        runtime.block_on(async {
            // The window URL carries the key that marks this window local;
            // a session with no runtime, or an upstream, has only its access URL.
            let url = entry
                .window_url_secret
                .as_deref()
                .unwrap_or(&entry.access_url_secret);
            let mut window = UiWindow::launch(url, &UiWindowConfig::default())
                .map_err(|error| error.to_string())?;
            window
                .wait_closed()
                .await
                .map_err(|error| error.to_string())
        })?;
        Ok(serde_json::json!({
            // Not `session_closed`: closing this window only ends this
            // *view*, exactly as closing the Provider's own window would.
            // The session itself may still be running — `session close` is
            // the verb for actually ending it, and its result carries that
            // name instead.
            "code": "session_view_closed",
            "session_id": entry.session_id,
            "provider_id": entry.provider_id,
            "ui_kind": entry.ui_kind,
        }))
    })
}

/// End a session from outside the process that created it.
///
/// Reaches the same reserved route the served shell's own "End" control and a
/// closed window both reach: `POST <access_url>` + [`agent_first_ui::SESSION_END_PATH`].
/// The credential bar is the session's own URL, exactly the one `session open`
/// already reads — whoever can list a session can end it, because that URL is
/// already the whole credential: the same threshold as seeing it, not a
/// higher one.
///
/// This reports only that the request was accepted, not that the owning
/// process has exited: the two are usually a heartbeat apart, but this command
/// has no way to wait for the second one.
fn run_session_close(invocation: &ResolvedInvocation) -> ExitCode {
    run_coded(invocation, "session_close_failed", || {
        let session_id = required(invocation, "session_id");
        let entry = agent_first_ui::find_session(&session_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| session_not_found(&session_id))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let status = runtime.block_on(post_session_end(&entry.access_url_secret))?;
        match status {
            204 => Ok(serde_json::json!({
                "code": "session_close_requested",
                "session_id": entry.session_id,
                "provider_id": entry.provider_id,
                "ui_kind": entry.ui_kind,
            })),
            404 => Err(Failure {
                code: Some("session_not_found"),
                message: format!(
                    "session `{session_id}` is already gone; there is nothing left to close"
                ),
            }),
            other => Err(format!(
                "closing session `{session_id}` failed: the session answered HTTP {other}"
            )
            .into()),
        }
    })
}

/// `POST <access_url><SESSION_END_PATH>`, by hand.
///
/// One request, no redirects, no body, no TLS — every property an HTTP client
/// dependency exists for is one this does not need, and every other verb in
/// this binary that talks to a session's own URL (`session serve`'s proxy
/// aside) already does the same by hand.
async fn post_session_end(access_url_secret: &str) -> Result<u16, String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        post_session_end_request(access_url_secret),
    )
    .await
    .map_err(|_| "session close request timed out after 5 seconds".to_owned())?
}

async fn post_session_end_request(access_url_secret: &str) -> Result<u16, String> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let rest = access_url_secret
        .strip_prefix("http://")
        .ok_or_else(|| "session close requires an http:// session URL".to_owned())?;
    // The same split the remote view's proxy makes of the same URL: the path
    // keeps its leading `/` and loses any trailing one, so a root path is the
    // empty prefix and contributes no separator of its own.
    let boundary = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, remainder) = rest.split_at(boundary);
    let (path, query) = remainder
        .split_once('?')
        .map_or((remainder, None), |(path, query)| (path, Some(query)));
    let mut target = format!(
        "{}/{}",
        path.trim_end_matches('/'),
        agent_first_ui::SESSION_END_PATH
    );
    if let Some(query) = query {
        target.push('?');
        target.push_str(query);
    }
    let mut stream = tokio::net::TcpStream::connect(authority)
        .await
        .map_err(|error| format!("could not reach the session at `{authority}`: {error}"))?;
    stream
        .write_all(
            format!(
                "POST {target} HTTP/1.1\r\nHost: {authority}\r\nContent-Length: 0\r\n\
                 Connection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .map_err(|error| error.to_string())?;
    // Only the status line matters. A peer can keep the body open, send binary
    // bytes, or echo the private request target; none belongs in diagnostics.
    const MAX_STATUS_LINE_BYTES: u64 = 8192;
    let mut response = Vec::new();
    BufReader::new(stream.take(MAX_STATUS_LINE_BYTES + 1))
        .read_until(b'\n', &mut response)
        .await
        .map_err(|error| error.to_string())?;
    let invalid = || "the session returned an invalid HTTP status line".to_owned();
    if response.len() as u64 > MAX_STATUS_LINE_BYTES || !response.ends_with(b"\r\n") {
        return Err(invalid());
    }
    let line = std::str::from_utf8(&response).map_err(|_| invalid())?;
    let mut fields = line.split(' ');
    if !matches!(fields.next(), Some("HTTP/1.0" | "HTTP/1.1")) {
        return Err(invalid());
    }
    let code = fields.next().ok_or_else(invalid)?;
    if code.len() != 3 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    code.parse::<u16>()
        .ok()
        .filter(|code| (100..=599).contains(code))
        .ok_or_else(invalid)
}

/// Serve the sessions running here to a browser that is not here.
///
/// This is the registry-backed form of AFUI's remote view, not a second
/// product: the page shows what `session list` shows, reverse-proxies each one
/// so a device that is not this machine can reach it, and stops when this call
/// does.
///
/// It is not a transport, and says so. The listener binds a port on this
/// machine; crossing anything to get to that port is WireGuard, Tailscale, an
/// SSH tunnel, or a TLS reverse proxy — mature things that already exist.
fn run_session_serve(invocation: &ResolvedInvocation) -> ExitCode {
    let (format, output_to) = plan_output(invocation.output_plan());
    let listen = optional(invocation, "listen").unwrap_or_else(|| DEFAULT_LISTEN.to_owned());
    let address = match listen.parse::<std::net::SocketAddr>() {
        Ok(address) => address,
        Err(error) => {
            return emit_error(
                "session_serve_listen_invalid",
                &format!("`--listen {listen}` is not an ADDRESS:PORT: {error}"),
                format,
                output_to,
                1,
            );
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return emit_error(
                "session_serve_failed",
                &format!("could not start the session server runtime: {error}"),
                format,
                output_to,
                1,
            );
        }
    };
    let mut emitter = CliEmitter::from_output_to(output_to, format).with_strict_protocol();
    let advertise_host = optional(invocation, "advertise_host");
    let session_origin_host = optional(invocation, "session_origin_host");
    let attention_policy = match attention_policy_override(invocation) {
        Ok(policy) => policy,
        Err(message) => {
            return emit_error(
                "session_serve_attention_invalid",
                &message,
                format,
                output_to,
                1,
            );
        }
    };
    let outcome = runtime.block_on(async {
        let mut server = agent_first_ui::SessionServer::new(address);
        if let Some(policy) = attention_policy {
            server = server.with_attention_policy(policy);
        }
        if let Some(host) = advertise_host {
            server = server.with_advertised_host(host);
        }
        if let Some(host) = session_origin_host {
            server = server.with_session_origin_host(host);
        }
        let served = server.bind().await.map_err(|error| error.to_string())?;
        let attention_policy = served.attention_policy();
        let bound = served.local_addr().to_string();
        // The URL carries the credential, and the person it is for is holding a
        // phone rather than sitting at this machine — there is no window to
        // hand it to instead. It is deliberately not `_secret`-suffixed: a
        // value someone has to read cannot be one the emitter redacts.
        emitter
            .emit(
                agent_first_data::json_progress(json!({
                    "phase": "serving",
                    "message": "Open this URL from your device. Reaching this port is a tunnel, a VPN, or a TLS reverse proxy you already trust — this command is none of them.",
                    "serve_url": served.access_url(),
                    "listen_address": bound,
                    // Absent when AFUI's own page is serving. An untrusted
                    // workspace frontend is skipped in silence, so this is how
                    // an agent tells "my override is running" from "my override
                    // is inert".
                    "ui_frontend_id": served.frontend_id(),
                    // Said at handoff, because it is the one thing about this
                    // URL a person cannot discover by looking at it: how long
                    // it lasts once they stop looking.
                    "idle_timeout_s": attention_policy
                        .idle_timeout()
                        .map(|idle| idle.as_secs()),
                    "grace_period_s": attention_policy.grace_period().as_secs(),
                }))
                .build(),
            )
            .map_err(|error| error.to_string())?;
        served
            .run(async {
                let _ignored = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok::<String, String>(bound)
    });
    let exit_code = if outcome.is_ok() { 0 } else { 1 };
    let emitted = match outcome {
        Ok(bound) => emitter.emit_result(json!({
            "code": "session_serve_stopped",
            "listen_address": bound,
        })),
        Err(message) => emitter.emit_error("session_serve_failed", &message),
    };
    match emitted {
        Ok(()) => ExitCode::from(exit_code),
        Err(_) => ExitCode::from(4),
    }
}

/// Why one verb failed. Most failures take the verb's own code; one that an
/// agent should branch on differently names a code of its own.
struct Failure {
    code: Option<&'static str>,
    message: String,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            code: None,
            message,
        }
    }
}

fn session_not_found(session_id: &str) -> Failure {
    Failure {
        code: Some("session_not_found"),
        message: format!(
            "no running session `{session_id}`; run `afui session list` to see what is open"
        ),
    }
}

/// Run one verb and report its failure under `error_code`, which names the
/// verb's own family so an agent branching on it can tell a broken frontend
/// from a session that is gone.
fn run_domain(
    invocation: &ResolvedInvocation,
    error_code: &'static str,
    action: impl FnOnce() -> Result<Value, String>,
) -> ExitCode {
    run_coded(invocation, error_code, || action().map_err(Failure::from))
}

/// [`run_domain`] for a verb with a failure of its own to name.
fn run_coded(
    invocation: &ResolvedInvocation,
    error_code: &'static str,
    action: impl FnOnce() -> Result<Value, Failure>,
) -> ExitCode {
    let (format, output_to) = plan_output(invocation.output_plan());
    match action() {
        Ok(value) => emit_result(value, format, output_to),
        Err(failure) => emit_error(
            failure.code.unwrap_or(error_code),
            &failure.message,
            format,
            output_to,
            1,
        ),
    }
}

fn io_error(error: io::Error) -> String {
    error.to_string()
}

/// One skill verb, in the two shapes that partition `--agent`.
///
/// `--skills-dir` names a single directory, so it is only legal once the call
/// has named one agent; the fan-out shape would have to write the same path for
/// all four.
fn skill_command(verb: &str, about: &str, force: bool) -> CommandSpec {
    let mut command = CommandSpec::new(["skill", verb])
        .about(about)
        .arg(
            ArgSpec::option_enum("--agent", std::iter::once(EVERY_AGENT).chain(AGENTS))
                .value_name("AGENT")
                .default(EVERY_AGENT)
                .about("Agent to manage"),
        )
        .arg(
            ArgSpec::option_enum("--scope", ["personal", "workspace"])
                .value_name("SCOPE")
                .default("personal")
                .about("Skill scope"),
        )
        .arg(ArgSpec::option("--skills-dir", "DIR").about("Directory that contains skill folders"));

    let mut every: Vec<&str> = vec!["scope"];
    let mut named: Vec<&str> = vec!["scope", "skills_dir"];
    if force {
        command = command.arg(
            ArgSpec::flag("--force")
                .about("Overwrite or remove an unmanaged Agent-First UI skill at the target path"),
        );
        every.push("force");
        named.push("force");
    }

    command
        .combination(
            Combination::new(format!("skill-{verb}-every-agent"))
                .action(format!("skill_{verb}"))
                .about("Target every agent that supports the scope")
                .fixed("agent", EVERY_AGENT)
                .optional(every)
                .output(output()),
        )
        .combination(
            Combination::new(format!("skill-{verb}-one-agent"))
                .action(format!("skill_{verb}"))
                .about("Target one named agent; only this shape accepts --skills-dir")
                .fixed_one_of("agent", AGENTS)
                .optional(named)
                .output(output()),
        )
}

fn run_skill_status(invocation: &ResolvedInvocation) -> ExitCode {
    run_skill(invocation, SkillAction::Status)
}

fn run_skill_install(invocation: &ResolvedInvocation) -> ExitCode {
    run_skill(invocation, SkillAction::Install)
}

fn run_skill_uninstall(invocation: &ResolvedInvocation) -> ExitCode {
    run_skill(invocation, SkillAction::Uninstall)
}

fn run_skill(invocation: &ResolvedInvocation, action: SkillAction) -> ExitCode {
    let (format, output_to) = plan_output(invocation.output_plan());
    let options = SkillOptions {
        agent: match optional(invocation, "agent").as_deref() {
            Some("codex") => SkillAgentSelection::Codex,
            Some("claude-code") => SkillAgentSelection::ClaudeCode,
            Some("opencode") => SkillAgentSelection::Opencode,
            Some("hermes") => SkillAgentSelection::Hermes,
            _ => SkillAgentSelection::All,
        },
        scope: match optional(invocation, "scope").as_deref() {
            Some("workspace") => SkillScope::Workspace,
            _ => SkillScope::Personal,
        },
        skills_dir: optional(invocation, "skills_dir"),
        force: invocation
            .optional("force")
            .and_then(CliValue::as_bool)
            .unwrap_or(false),
    };
    match skill::run_skill_admin(&SKILL_SPEC, action, &options) {
        Ok(report) => match serde_json::to_value(&report) {
            Ok(value) => emit_result(value, format, output_to),
            Err(error) => emit_error(
                "serialization_failed",
                &format!("failed to serialize skill report: {error}"),
                format,
                output_to,
                1,
            ),
        },
        Err(err) => emit_error("skill_error", &err.message, format, output_to, 1),
    }
}

fn plan_output(plan: &OutputPlan) -> (OutputFormat, OutputTo) {
    let format = plan.output_format().unwrap_or(OutputFormat::Json);
    let destination = plan.output_to().unwrap_or(OutputTo::Split);
    (format, destination)
}

fn stream_of(plan: &OutputPlan, is_error: bool) -> OutputTo {
    if is_error || plan.output_to() == Some(OutputTo::Stderr) {
        OutputTo::Stderr
    } else {
        OutputTo::Stdout
    }
}

fn emit_result(value: Value, format: OutputFormat, output_to: OutputTo) -> ExitCode {
    let mut emitter = CliEmitter::from_output_to(output_to, format).with_strict_protocol();
    match emitter.emit_result(value) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(4),
    }
}

fn emit_error(
    code: &str,
    message: &str,
    format: OutputFormat,
    output_to: OutputTo,
    exit_code: u8,
) -> ExitCode {
    let mut emitter = CliEmitter::from_output_to(output_to, format).with_strict_protocol();
    match emitter.emit_error(code, message) {
        Ok(()) => ExitCode::from(exit_code),
        Err(_) => ExitCode::from(4),
    }
}

fn emit_event(
    event: agent_first_data::Event,
    format: OutputFormat,
    output_to: OutputTo,
    exit_code: u8,
) -> ExitCode {
    let mut emitter = CliEmitter::from_output_to(output_to, format).with_strict_protocol();
    match emitter.emit(event) {
        Ok(()) => ExitCode::from(exit_code),
        Err(_) => ExitCode::from(4),
    }
}

fn startup_error(code: &str, message: &str) -> ExitCode {
    emit_error(code, message, OutputFormat::Json, OutputTo::Stderr, 1)
}

// `--docs` and plain help are raw documents AFDATA injects into every registry,
// so AFDATA owns writing them: stream routing and the rule that a closed reader
// is not a failure are its decisions, not this binary's.
fn write_text(text: &str, output_to: OutputTo) -> ExitCode {
    match agent_first_data::write_raw(&format!("{text}\n"), output_to) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(4),
    }
}

#[cfg(test)]
mod tests {
    use agent_first_data::CliOutcome;

    use super::*;

    #[test]
    fn every_combination_round_trips() {
        let cli = cli_spec().unwrap();
        for fixture in cli.synthetic_invocations() {
            assert!(matches!(
                cli.resolve_from(fixture.argv),
                Ok(CliOutcome::Run(_))
            ));
        }
    }

    #[test]
    fn minute_overrides_never_wrap_to_a_shorter_interval() {
        assert_eq!(minutes_as_duration(-5), std::time::Duration::ZERO);
        assert_eq!(minutes_as_duration(2), std::time::Duration::from_secs(120));
        assert_eq!(
            minutes_as_duration(i64::MAX),
            std::time::Duration::from_secs(u64::MAX)
        );
    }

    #[test]
    fn fingerprint_changes_with_frontend_contents() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("frontend.json"), b"one").unwrap();
        let first = fingerprint(directory.path()).unwrap();
        fs::write(directory.path().join("frontend.json"), b"two").unwrap();
        assert_ne!(first, fingerprint(directory.path()).unwrap());
    }

    /// `post_session_end` against a real running session — the exact request
    /// `session close` sends, without the registry lookup around it. Not
    /// gated on `AFUI_CONFIG_DIR`, so it needs no isolation from the other
    /// tests in this binary.
    #[tokio::test]
    async fn post_session_end_reaches_a_real_session_and_reports_its_status() {
        let host = agent_first_ui::UiHost::bind_local().await.unwrap();
        let session = agent_first_ui::UiSession::<()>::new("reviewer", "case_review").unwrap();
        let active = host
            .attach(
                session,
                axum::Router::new(),
                agent_first_ui::UiExpiry::Never,
            )
            .unwrap();
        let url = active.access_url().to_owned();

        assert_eq!(post_session_end(&url).await.unwrap(), 204);
        assert_eq!(
            active.wait().await.unwrap(),
            agent_first_ui::Outcome::Closed
        );
        // Gone now, the same as any credential that never existed.
        assert_eq!(post_session_end(&url).await.unwrap(), 404);
    }

    #[tokio::test]
    async fn post_session_end_preserves_queries_and_separates_paths() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        for (path, expected) in [
            ("/panel/sub", "/panel/sub/__afui/end"),
            (
                "/panel/sub?handoff_secret=abc",
                "/panel/sub/__afui/end?handoff_secret=abc",
            ),
            (
                "/panel/sub/?handoff_secret=abc",
                "/panel/sub/__afui/end?handoff_secret=abc",
            ),
            // A root path is the empty prefix: one `/`, never two.
            ("/", "/__afui/end"),
            ("/?q=1", "/__afui/end?q=1"),
            // No path at all. The registry never stores this shape — an
            // announced URL is given a `/` first — but the request line is the
            // same if one arrives.
            ("", "/__afui/end"),
            ("?q=1", "/__afui/end?q=1"),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}{path}", listener.local_addr().unwrap());
            let peer = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                assert_eq!(line, format!("POST {expected} HTTP/1.1\r\n"));
                stream.get_mut().write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            });
            assert_eq!(post_session_end(&url).await.unwrap(), 204);
            peer.await.unwrap();
        }
    }

    #[tokio::test]
    async fn post_session_end_has_a_deadline_when_the_peer_does_not_reply() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/panel", listener.local_addr().unwrap());
        let peer = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let started = std::time::Instant::now();
        let error = post_session_end(&url).await.unwrap_err();
        assert!(error.contains("timed out"));
        assert!(started.elapsed() < std::time::Duration::from_secs(7));
        peer.abort();
    }

    #[tokio::test]
    async fn post_session_end_reports_a_clear_error_for_a_url_it_cannot_use() {
        let error = post_session_end("https://example.com/private-canary/")
            .await
            .unwrap_err();
        assert!(!error.contains("private-canary"));
        assert!(post_session_end("http://127.0.0.1:1/").await.is_err());
    }

    #[tokio::test]
    async fn post_session_end_bounds_and_validates_the_status_without_echoing_the_peer() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        for reply in [
            b"private-canary 204 ignored\r\n".to_vec(),
            b"HTTP/1.1 2040 private-canary\r\n".to_vec(),
            b"HTTP/1.1 099 private-canary\r\n".to_vec(),
            b"HTTP/1.1 204 private-canary".to_vec(),
            b"private-canary".repeat(1024),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let peer = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                stream.read_line(&mut String::new()).await.unwrap();
                stream.get_mut().write_all(&reply).await.unwrap();
            });
            let error = post_session_end(&url).await.unwrap_err();
            assert_eq!(error, "the session returned an invalid HTTP status line");
            assert!(!error.contains("private-canary"));
            peer.await.unwrap();
        }
    }

    #[tokio::test]
    async fn post_session_end_does_not_wait_for_a_response_body_or_peer_close() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            stream.read_line(&mut String::new()).await.unwrap();
            stream
                .get_mut()
                .write_all(b"HTTP/1.1 204 No Content\r\n\r\n\xff")
                .await
                .unwrap();
            std::future::pending::<()>().await;
        });
        let status =
            tokio::time::timeout(std::time::Duration::from_secs(1), post_session_end(&url))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(status, 204);
        peer.abort();
    }
}

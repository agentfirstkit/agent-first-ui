use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent_first_data::skill::{
    self, SkillAction, SkillAgentSelection, SkillOptions, SkillScope, SkillSpec,
};
use agent_first_data::{
    ArgSpec, BoundOutcome, BuiltCliSpec, CliEmitter, CliSpec, CliSpecError, CliValue, Combination,
    CommandSpec, OutputFormat, OutputPlan, OutputSpec, OutputTo, ResolvedInvocation,
    build_afdata_cli, cli_error_event, cli_help_event, cli_parse_output, cli_version_event,
    render_cli_reference,
};
use agent_first_ui::{
    FrontendManifest, FrontendScope, SAFE_MODE_ENV, UiWindow, UiWindowConfig, frontend_root,
    global_afui_root, load_frontend, validate_frontend_identifier,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use walkdir::WalkDir;

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

/// The skill this binary installs. `assets` is empty because the skill is a
/// single file: it points agents at `afui frontend --help` rather than shipping
/// a reference tree that would have to be kept in step with the CLI.
const SKILL_SPEC: SkillSpec = SkillSpec {
    name: "agent-first-ui",
    source: include_str!("../../skills/agent-first-ui/SKILL.md"),
    title: "Agent-First UI",
    marker_slug: "afui",
    assets: &[],
};

/// The named agents, and the fan-out value that is not one of them. Both skill
/// shapes partition `--agent` between these, so the enum and that partition are
/// built from one list.
const AGENTS: [&str; 4] = ["codex", "claude-code", "opencode", "hermes"];
const EVERY_AGENT: &str = "all";

fn cli_spec() -> Result<BuiltCliSpec, CliSpecError> {
    let mut spec = CliSpec::new("afui", env!("CARGO_PKG_VERSION"))
        .display_name("Agent-First UI")
        .about("Create, validate, trust, inspect, and compare Provider-owned UI frontends.")
        .lifecycle_output(output())
        .command(CommandSpec::root())
        .command(init_command());
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
            CommandSpec::new(["session", "serve"])
                .about(
                    "Serve every running session to a browser on another device. \
                     Binds a port and stops there: getting to that port is an SSH tunnel, \
                     WireGuard, Tailscale, or a TLS reverse proxy, none of which this is.",
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
                        .optional(["listen", "advertise_host", "session_origin_host"])
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
    build_afdata_cli(spec)
}

fn base_frontend_command(path: &str, about: &str) -> CommandSpec {
    CommandSpec::new(["frontend", path])
        .about(about)
        .arg(
            ArgSpec::positional("provider_id", 0, "PROVIDER_ID")
                .about("Stable Provider identifier"),
        )
        .arg(ArgSpec::positional("ui_kind", 1, "UI_KIND").about("Stable UI kind identifier"))
        .arg(
            ArgSpec::option_enum("--scope", ["workspace", "global"])
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
        ("session_serve", run_session_serve),
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

fn run_init(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, || {
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
        Ok(json!({
            "code": "frontend_init",
            "frontend_id": manifest.frontend_id,
            "frontend_root": context.root,
            "provider_id": context.provider_id,
            "scope": context.scope_name,
            "ui_api_version": manifest.ui_api_version,
            "ui_kind": context.ui_kind,
        }))
    })
}

fn run_check(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, || inspect(invocation, "frontend_check"))
}

fn run_dev(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, || {
        let mut value = inspect(invocation, "frontend_dev")?;
        value["provider_owned_runtime"] = Value::Bool(true);
        value["message"] = Value::String(
            "frontend is ready for the Provider's typed development server".to_owned(),
        );
        Ok(value)
    })
}

fn run_preview(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, || {
        let mut value = inspect(invocation, "frontend_preview")?;
        value["provider_owned_runtime"] = Value::Bool(true);
        value["message"] =
            Value::String("frontend is ready for the Provider's typed preview server".to_owned());
        Ok(value)
    })
}

fn run_enable(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, || {
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
    run_domain(invocation, || {
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
    run_domain(invocation, || {
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
    let trust = load_trust()?;
    let trusted = trust
        .frontends
        .get(&trust_key(&context))
        .is_some_and(|entry| entry.fingerprint == current_fingerprint);
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
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
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

/// List the sessions Providers currently have open.
///
/// The credential never appears here. A session URL carries one, and this is
/// stdout — where an agent reads. `session open` is how a session is reached,
/// and it hands the URL straight to a window without printing it.
fn run_session_list(invocation: &ResolvedInvocation) -> ExitCode {
    run_domain(invocation, || {
        let sessions = agent_first_ui::list_sessions().map_err(|error| error.to_string())?;
        let listed: Vec<Value> = sessions
            .iter()
            .map(|entry| {
                let mut value = serde_json::json!({
                    "session_id": entry.session_id,
                    "provider_id": entry.provider_id,
                    "ui_kind": entry.ui_kind,
                    "owner_pid": entry.owner_pid,
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
    run_domain(invocation, || {
        let session_id = required(invocation, "session_id");
        let entry = agent_first_ui::find_session(&session_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!(
                    "no running session `{session_id}`; run `afui session list` to see what is open"
                )
            })?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        runtime.block_on(async {
            let mut window = UiWindow::launch(&entry.access_url_secret, &UiWindowConfig::default())
                .map_err(|error| error.to_string())?;
            window
                .wait_closed()
                .await
                .map_err(|error| error.to_string())
        })?;
        Ok(serde_json::json!({
            "code": "session_closed",
            "session_id": entry.session_id,
            "provider_id": entry.provider_id,
            "ui_kind": entry.ui_kind,
        }))
    })
}

/// Serve the sessions running here to a browser that is not here.
///
/// This is a third delivery of the same sessions, not a second product: the
/// page shows what `session list` shows, reverse-proxies each one so a device
/// that is not this machine can reach it, and stops when this call does.
///
/// It is not a transport, and says so. The listener binds a port on this
/// machine; crossing anything to get to that port is WireGuard, Tailscale, an
/// SSH tunnel, or a TLS reverse proxy — mature things that already exist.
#[cfg(feature = "serve")]
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
    let outcome = runtime.block_on(async {
        let mut server = agent_first_ui::SessionServer::new(address);
        if let Some(host) = advertise_host {
            server = server.with_advertised_host(host);
        }
        if let Some(host) = session_origin_host {
            server = server.with_session_origin_host(host);
        }
        let served = server.bind().await.map_err(|error| error.to_string())?;
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
    let emitted = match outcome {
        Ok(bound) => emitter.emit_result(json!({
            "code": "session_serve_stopped",
            "listen_address": bound,
        })),
        Err(message) => emitter.emit_error("session_serve_failed", &message),
    };
    match emitted {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(4),
    }
}

/// The same verb, in a build that left the shell and its proxy out.
///
/// Registered rather than absent: a person who typed the right command should
/// be told what to rebuild, not told the command does not exist.
#[cfg(not(feature = "serve"))]
fn run_session_serve(invocation: &ResolvedInvocation) -> ExitCode {
    let (format, output_to) = plan_output(invocation.output_plan());
    emit_error(
        "feature_unavailable",
        "`afui session serve` requires feature 'serve'; rebuild with: cargo install agent-first-ui --features serve",
        format,
        output_to,
        1,
    )
}

fn run_domain(
    invocation: &ResolvedInvocation,
    action: impl FnOnce() -> Result<Value, String>,
) -> ExitCode {
    let (format, output_to) = plan_output(invocation.output_plan());
    match action() {
        Ok(value) => emit_result(value, format, output_to),
        Err(message) => emit_error("frontend_error", &message, format, output_to, 1),
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
    let format = plan
        .format()
        .and_then(|value| cli_parse_output(value).ok())
        .unwrap_or(OutputFormat::Json);
    let destination = plan
        .destination()
        .and_then(|value| OutputTo::parse(value).ok())
        .unwrap_or(OutputTo::Split);
    (format, destination)
}

fn stream_of(plan: &OutputPlan, is_error: bool) -> OutputTo {
    if is_error || plan.destination() == Some("stderr") {
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
    fn fingerprint_changes_with_frontend_contents() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("frontend.json"), b"one").unwrap();
        let first = fingerprint(directory.path()).unwrap();
        fs::write(directory.path().join("frontend.json"), b"two").unwrap();
        assert_ne!(first, fingerprint(directory.path()).unwrap());
    }
}

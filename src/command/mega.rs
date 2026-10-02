//! Mega forge client: a small `gh`-style surface for Mega's HTTP API.

use std::{env, io::IsTerminal, net::IpAddr, time::Duration};

use clap::{Args, Parser, Subcommand};
use reqwest::{Client, Method, StatusCode, redirect::Policy};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use url::Url;

use crate::{
    internal::auth::{self, HostScope, Lookup},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
    },
};

const DEFAULT_HOST: &str = "https://git.gitmega.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub const MEGA_EXAMPLES: &str = "\
EXAMPLES:
    libra mega status
    printf '%s' \"$TOKEN\" | libra mega auth login --with-token
    libra mega auth status
    libra mega issue list --state open --limit 20
    libra --json mega issue view ISSUE_LINK
    libra mega issue create --title \"Bug report\" --body \"Steps to reproduce...\"
    libra mega issue close ISSUE_LINK
    libra mega cl list --state open --author alice
    libra mega cl view CL_LINK
    libra mega cl merge CL_LINK

ENVIRONMENT:
    MEGA_HOST   Mega API origin (default: https://git.gitmega.com)
    MEGA_TOKEN  Ephemeral bearer token for CI; takes precedence over stored auth

NOTES:
    Tokens passed to `auth login` are read from a hidden prompt or stdin and
    stored by Libra's encrypted host-scoped auth store. They are never printed.";

#[derive(Parser, Debug)]
#[command(after_help = MEGA_EXAMPLES)]
pub struct MegaArgs {
    /// Mega API origin. Overrides MEGA_HOST.
    #[arg(long, global = true, value_name = "URL")]
    pub host: Option<String>,

    #[command(subcommand)]
    pub command: MegaCommand,
}

#[derive(Subcommand, Debug)]
pub enum MegaCommand {
    /// Check whether the Mega API is reachable.
    #[command(after_help = MEGA_EXAMPLES)]
    Status,
    /// Manage the bearer token used for Mega API requests.
    #[command(subcommand, after_help = MEGA_EXAMPLES)]
    Auth(MegaAuthCommand),
    /// Work with Mega issues.
    #[command(subcommand, after_help = MEGA_EXAMPLES)]
    Issue(MegaIssueCommand),
    /// Work with Mega change lists (the Mega equivalent of pull requests).
    #[command(subcommand, after_help = MEGA_EXAMPLES)]
    Cl(MegaClCommand),
}

#[derive(Subcommand, Debug)]
pub enum MegaAuthCommand {
    /// Store a Mega access token in Libra's encrypted auth store.
    #[command(after_help = MEGA_EXAMPLES)]
    Login {
        /// Read the token from the first line of stdin instead of a hidden prompt.
        #[arg(long)]
        with_token: bool,
    },
    /// Verify the current token and show the associated Mega user.
    #[command(after_help = MEGA_EXAMPLES)]
    Status,
    /// Remove the stored token for this Mega host.
    #[command(after_help = MEGA_EXAMPLES)]
    Logout,
}

#[derive(Subcommand, Debug)]
pub enum MegaIssueCommand {
    /// List issues.
    #[command(after_help = MEGA_EXAMPLES)]
    List(ListArgs),
    /// Show one issue.
    #[command(after_help = MEGA_EXAMPLES)]
    View(LinkArgs),
    /// Create an issue.
    #[command(after_help = MEGA_EXAMPLES)]
    Create {
        /// Issue title.
        #[arg(long)]
        title: String,
        /// Issue description.
        #[arg(long, default_value = "")]
        body: String,
    },
    /// Close an issue.
    #[command(after_help = MEGA_EXAMPLES)]
    Close(LinkArgs),
    /// Reopen an issue.
    #[command(after_help = MEGA_EXAMPLES)]
    Reopen(LinkArgs),
}

#[derive(Subcommand, Debug)]
pub enum MegaClCommand {
    /// List change lists.
    #[command(after_help = MEGA_EXAMPLES)]
    List(ListArgs),
    /// Show one change list.
    #[command(after_help = MEGA_EXAMPLES)]
    View(LinkArgs),
    /// Close a change list without merging it.
    #[command(after_help = MEGA_EXAMPLES)]
    Close(LinkArgs),
    /// Reopen a closed change list.
    #[command(after_help = MEGA_EXAMPLES)]
    Reopen(LinkArgs),
    /// Merge a change list.
    #[command(after_help = MEGA_EXAMPLES)]
    Merge(LinkArgs),
}

#[derive(Args, Debug)]
pub struct LinkArgs {
    /// Mega issue or change-list link identifier.
    pub link: String,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Server-side state filter, such as open, closed, merged, draft, or all.
    #[arg(long, default_value = "open")]
    pub state: String,
    /// Filter by author username.
    #[arg(long)]
    pub author: Option<String>,
    /// Filter by assignee username; repeat for multiple assignees.
    #[arg(long, value_name = "USER")]
    pub assignee: Vec<String>,
    /// Filter by numeric label ID; repeat for multiple labels.
    #[arg(long, value_name = "ID")]
    pub label: Vec<i64>,
    /// Sort field understood by the Mega API.
    #[arg(long)]
    pub sort: Option<String>,
    /// Sort in ascending order instead of descending order.
    #[arg(long)]
    pub asc: bool,
    /// One-based result page.
    #[arg(long, default_value_t = 1)]
    pub page: u64,
    /// Number of results per page (1-100).
    #[arg(long, default_value_t = 30)]
    pub limit: u64,
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    req_result: bool,
    data: Option<T>,
    #[serde(default)]
    err_message: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApiPage<T> {
    total: u64,
    items: Vec<T>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Label {
    id: i64,
    name: String,
    color: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Item {
    id: i64,
    link: String,
    title: String,
    status: String,
    author: String,
    #[serde(default)]
    author_is_bot: bool,
    open_timestamp: i64,
    closed_at: Option<i64>,
    merge_timestamp: Option<i64>,
    updated_at: i64,
    #[serde(default)]
    labels: Vec<Label>,
    #[serde(default)]
    assignees: Vec<String>,
    #[serde(default)]
    comment_num: usize,
    #[serde(default)]
    build_status: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Detail {
    id: i64,
    link: String,
    title: String,
    status: Value,
    author: String,
    #[serde(default)]
    author_is_bot: bool,
    open_timestamp: i64,
    #[serde(default)]
    merge_timestamp: Option<i64>,
    #[serde(default)]
    conversations: Vec<Value>,
    #[serde(default)]
    labels: Vec<Label>,
    #[serde(default)]
    assignees: Vec<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct ListOutput<T> {
    page: u64,
    limit: u64,
    total: u64,
    items: Vec<T>,
}

struct MegaClient {
    base: Url,
    http: Client,
    token: Option<String>,
}

impl MegaClient {
    async fn new(base: Url, auth_required: bool) -> CliResult<Self> {
        let token = resolve_token(&base).await?;
        if auth_required && token.is_none() {
            return Err(CliError::fatal(format!(
                "no Mega access token is available for {}",
                base.origin().ascii_serialization()
            ))
            .with_stable_code(StableErrorCode::AuthMissingCredentials)
            .with_hint("run `libra mega auth login` or set MEGA_TOKEN for this invocation"));
        }

        let mut builder = Client::builder()
            .redirect(Policy::none())
            .timeout(REQUEST_TIMEOUT);
        if base.host_str().is_some_and(host_is_loopback) {
            builder = builder.no_proxy();
        }
        let http = builder.build().map_err(|error| {
            CliError::network(format!("failed to build the Mega HTTP client: {error}"))
        })?;
        Ok(Self { base, http, token })
    }

    fn endpoint(&self, segments: &[&str]) -> CliResult<Url> {
        let mut url = self.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| CliError::internal("failed to construct a Mega API endpoint URL"))?;
            path.clear();
            for segment in segments {
                path.push(segment);
            }
        }
        Ok(url)
    }

    async fn request_value(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&Value>,
    ) -> CliResult<Value> {
        let url = self.endpoint(segments)?;
        let mut request = self.http.request(method, url.clone());
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|error| {
            CliError::network(format!(
                "failed to contact Mega API at {}: {error}",
                url.origin().ascii_serialization()
            ))
            .with_hint("check --host/MEGA_HOST and verify that the Mega service is reachable")
        })?;
        let status = response.status();
        let text = response.text().await.map_err(|error| {
            CliError::network(format!("failed to read the Mega API response: {error}"))
        })?;
        if !status.is_success() {
            return Err(response_error(status, &text));
        }
        serde_json::from_str(&text).map_err(|error| {
            CliError::fatal(format!("Mega returned invalid JSON: {error}"))
                .with_stable_code(StableErrorCode::NetworkProtocol)
                .with_hint("verify that --host points to the Mega API origin, not the web UI")
        })
    }

    async fn envelope<T: DeserializeOwned>(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&Value>,
    ) -> CliResult<Option<T>> {
        let value = self.request_value(method, segments, body).await?;
        let envelope: ApiEnvelope<T> = serde_json::from_value(value).map_err(|error| {
            CliError::fatal(format!(
                "Mega returned an incompatible API response: {error}"
            ))
            .with_stable_code(StableErrorCode::NetworkProtocol)
        })?;
        if !envelope.req_result {
            let message = if envelope.err_message.trim().is_empty() {
                "Mega rejected the request".to_string()
            } else {
                envelope.err_message
            };
            return Err(CliError::fatal(message).with_stable_code(StableErrorCode::NetworkProtocol));
        }
        Ok(envelope.data)
    }

    async fn required<T: DeserializeOwned>(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&Value>,
    ) -> CliResult<T> {
        self.envelope(method, segments, body).await?.ok_or_else(|| {
            CliError::fatal("Mega returned success without the expected response data")
                .with_stable_code(StableErrorCode::NetworkProtocol)
        })
    }
}

fn host_is_loopback(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn resolve_base_url(host: Option<&str>) -> CliResult<Url> {
    let configured = host
        .map(str::to_owned)
        .or_else(|| env::var("MEGA_HOST").ok())
        .unwrap_or_else(|| DEFAULT_HOST.to_string());
    let configured = configured.trim();
    let with_scheme = if configured.contains("://") {
        configured.to_string()
    } else {
        format!("https://{configured}")
    };
    let parsed = Url::parse(&with_scheme).map_err(|error| {
        CliError::command_usage(format!("invalid Mega --host: {error}"))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
    })?;
    HostScope::parse(parsed.as_str()).map_err(|error| {
        CliError::command_usage(format!("invalid Mega --host: {error}"))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
    })?;
    Url::parse(&parsed.origin().ascii_serialization()).map_err(|error| {
        CliError::internal(format!("failed to normalize the Mega API origin: {error}"))
    })
}

fn scope_for(base: &Url) -> CliResult<HostScope> {
    HostScope::parse(base.as_str()).map_err(|error| {
        CliError::command_usage(format!("invalid Mega host scope: {error}"))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
    })
}

async fn resolve_token(base: &Url) -> CliResult<Option<String>> {
    if let Ok(token) = env::var("MEGA_TOKEN") {
        validate_token(&token)?;
        return Ok(Some(token));
    }
    let scope = scope_for(base)?;
    match auth::lookup(&scope).await {
        Lookup::Valid { token, .. } => Ok(Some(token)),
        Lookup::Miss => Ok(None),
        Lookup::Expired { .. } => Err(CliError::fatal(format!(
            "the stored Mega token for {} has expired",
            scope.display()
        ))
        .with_stable_code(StableErrorCode::AuthMissingCredentials)
        .with_hint("run `libra mega auth login` to replace it")),
        Lookup::Undecryptable => Err(CliError::fatal(format!(
            "the stored Mega token for {} cannot be decrypted",
            scope.display()
        ))
        .with_stable_code(StableErrorCode::AuthMissingCredentials)
        .with_hint("run `libra mega auth login` to replace it")),
    }
}

fn validate_token(token: &str) -> CliResult<()> {
    if token.trim().is_empty() {
        return Err(CliError::command_usage("Mega token must not be empty")
            .with_stable_code(StableErrorCode::CliInvalidArguments));
    }
    if token.len() > 8192 || token.chars().any(char::is_control) {
        return Err(CliError::command_usage(
            "Mega token is too long or contains control characters",
        )
        .with_stable_code(StableErrorCode::CliInvalidArguments));
    }
    Ok(())
}

fn read_token(with_token: bool) -> CliResult<String> {
    let token = if with_token {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|error| {
            CliError::fatal(format!("failed to read the Mega token from stdin: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        line.trim().to_string()
    } else if std::io::stdin().is_terminal() {
        rpassword::prompt_password("Mega token (input hidden): ").map_err(|error| {
            CliError::fatal(format!("failed to read the Mega token: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?
    } else {
        return Err(CliError::command_usage(
            "stdin is not a TTY; pass --with-token and pipe the Mega token in",
        )
        .with_stable_code(StableErrorCode::CliInvalidArguments)
        .with_hint("printf '%s' \"$TOKEN\" | libra mega auth login --with-token"));
    };
    validate_token(&token)?;
    Ok(token)
}

fn response_error(status: StatusCode, body: &str) -> CliError {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("err_message")
                .or_else(|| value.get("message"))
                .or_else(|| value.get("error"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| format!("server returned HTTP {status}"));
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return CliError::fatal(format!("Mega authentication failed: {detail}"))
            .with_stable_code(StableErrorCode::AuthPermissionDenied)
            .with_hint(
                "generate a Mega access token in the web UI, then run `libra mega auth login`",
            );
    }
    CliError::fatal(format!("Mega API request failed: {detail}"))
        .with_stable_code(StableErrorCode::NetworkProtocol)
}

fn validate_list(args: &ListArgs) -> CliResult<()> {
    if args.page == 0 {
        return Err(CliError::command_usage("--page must be at least 1")
            .with_stable_code(StableErrorCode::CliInvalidArguments));
    }
    if !(1..=100).contains(&args.limit) {
        return Err(CliError::command_usage("--limit must be between 1 and 100")
            .with_stable_code(StableErrorCode::CliInvalidArguments));
    }
    if args.state.trim().is_empty() {
        return Err(CliError::command_usage("--state must not be empty")
            .with_stable_code(StableErrorCode::CliInvalidArguments));
    }
    Ok(())
}

fn list_body(args: &ListArgs) -> Value {
    json!({
        "pagination": { "page": args.page, "per_page": args.limit },
        "additional": {
            "status": args.state,
            "author": args.author,
            "labels": (!args.label.is_empty()).then_some(&args.label),
            "assignees": (!args.assignee.is_empty()).then_some(&args.assignee),
            "sort_by": args.sort,
            "asc": args.asc
        }
    })
}

fn print_list(kind: &str, output: &ListOutput<Item>, config: &OutputConfig) -> CliResult<()> {
    if config.is_json() {
        return emit_json_data(&format!("mega {kind} list"), output, config);
    }
    if config.quiet {
        return Ok(());
    }
    if output.items.is_empty() {
        println!("No {kind}s found.");
        return Ok(());
    }
    for item in &output.items {
        let labels = item
            .labels
            .iter()
            .map(|label| label.name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let suffix = if labels.is_empty() {
            String::new()
        } else {
            format!(" [{labels}]")
        };
        println!(
            "{}\t{}\t{}\t{}{}",
            item.link, item.status, item.author, item.title, suffix
        );
    }
    Ok(())
}

fn status_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string().trim_matches('"').to_string())
}

fn print_detail(kind: &str, detail: &Detail, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data(&format!("mega {kind} view"), detail, output);
    }
    if output.quiet {
        return Ok(());
    }
    println!("{}  {}", detail.link, detail.title);
    println!("state:  {}", status_text(&detail.status));
    println!("author: {}", detail.author);
    if let Some(path) = detail.path.as_deref().filter(|path| !path.is_empty()) {
        println!("path:   {path}");
    }
    if !detail.assignees.is_empty() {
        println!("assignees: {}", detail.assignees.join(", "));
    }
    if !detail.labels.is_empty() {
        println!(
            "labels: {}",
            detail
                .labels
                .iter()
                .map(|label| label.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!("comments: {}", detail.conversations.len());
    Ok(())
}

fn print_action(resource: &str, action: &str, link: &str, output: &OutputConfig) -> CliResult<()> {
    let report = json!({ "resource": resource, "action": action, "link": link });
    if output.is_json() {
        return emit_json_data(&format!("mega {resource} {action}"), &report, output);
    }
    if !output.quiet {
        println!("{action} {resource} {link}");
    }
    Ok(())
}

async fn list_resource(
    client: &MegaClient,
    kind: &str,
    args: ListArgs,
    output: &OutputConfig,
) -> CliResult<()> {
    validate_list(&args)?;
    let body = list_body(&args);
    let page: ApiPage<Item> = client
        .required(Method::POST, &["api", "v1", kind, "list"], Some(&body))
        .await?;
    let report = ListOutput {
        page: args.page,
        limit: args.limit,
        total: page.total,
        items: page.items,
    };
    print_list(kind, &report, output)
}

async fn view_resource(
    client: &MegaClient,
    kind: &str,
    link: &str,
    output: &OutputConfig,
) -> CliResult<()> {
    let detail: Detail = client
        .required(Method::GET, &["api", "v1", kind, link, "detail"], None)
        .await?;
    print_detail(kind, &detail, output)
}

async fn mutate_resource(
    client: &MegaClient,
    kind: &str,
    action: &str,
    link: &str,
    output: &OutputConfig,
) -> CliResult<()> {
    client
        .envelope::<Value>(Method::POST, &["api", "v1", kind, link, action], None)
        .await?;
    print_action(kind, action, link, output)
}

/// Execute a Mega forge command.
///
/// # Side Effects
/// Performs bounded HTTP requests. `auth login/logout` also update the global,
/// encrypted host-token store. No repository objects or worktree files change.
///
/// # Errors
/// Rejects unsafe host URLs, missing/invalid credentials, incompatible API
/// responses, and failed server operations with stable CLI error categories.
pub async fn execute_safe(args: MegaArgs, output: &OutputConfig) -> CliResult<()> {
    let base = resolve_base_url(args.host.as_deref())?;
    match args.command {
        MegaCommand::Status => {
            let client = MegaClient::new(base, false).await?;
            let status = client
                .request_value(Method::GET, &["api", "v1", "status"], None)
                .await?;
            if output.is_json() {
                emit_json_data(
                    "mega status",
                    &json!({ "host": client.base.as_str(), "status": status }),
                    output,
                )
            } else {
                if !output.quiet {
                    println!("Mega API {}: {}", client.base, status_text(&status));
                }
                Ok(())
            }
        }
        MegaCommand::Auth(MegaAuthCommand::Login { with_token }) => {
            let token = read_token(with_token)?;
            let scope = scope_for(&base)?;
            auth::store_token(&scope, "oauth2", &token, None)
                .await
                .map_err(|error| {
                    CliError::fatal(format!("failed to store the Mega token: {error}"))
                        .with_stable_code(StableErrorCode::IoWriteFailed)
                })?;
            let report = json!({ "host": base.as_str(), "stored": true });
            if output.is_json() {
                emit_json_data("mega auth login", &report, output)
            } else {
                if !output.quiet {
                    println!("Stored Mega token for {}", scope.display());
                }
                Ok(())
            }
        }
        MegaCommand::Auth(MegaAuthCommand::Status) => {
            let client = MegaClient::new(base, true).await?;
            let user: Value = client
                .required(Method::GET, &["api", "v1", "user", ""], None)
                .await?;
            if output.is_json() {
                emit_json_data("mega auth status", &user, output)
            } else {
                if !output.quiet {
                    let identity = user
                        .get("github_login")
                        .and_then(Value::as_str)
                        .or_else(|| user.get("campsite_user_id").and_then(Value::as_str))
                        .unwrap_or("authenticated user");
                    println!("Logged in to {} as {identity}", client.base);
                }
                Ok(())
            }
        }
        MegaCommand::Auth(MegaAuthCommand::Logout) => {
            let scope = scope_for(&base)?;
            let removed = auth::remove(&scope).await.map_err(|error| {
                CliError::fatal(format!("failed to remove the Mega token: {error}"))
                    .with_stable_code(StableErrorCode::IoWriteFailed)
            })?;
            let report = json!({ "host": base.as_str(), "removed": removed });
            if output.is_json() {
                emit_json_data("mega auth logout", &report, output)
            } else {
                if !output.quiet {
                    if removed {
                        println!("Removed Mega token for {}", scope.display());
                    } else {
                        println!("No stored Mega token for {}", scope.display());
                    }
                }
                Ok(())
            }
        }
        MegaCommand::Issue(MegaIssueCommand::List(args)) => {
            let client = MegaClient::new(base, false).await?;
            list_resource(&client, "issue", args, output).await
        }
        MegaCommand::Issue(MegaIssueCommand::View(args)) => {
            let client = MegaClient::new(base, true).await?;
            view_resource(&client, "issue", &args.link, output).await
        }
        MegaCommand::Issue(MegaIssueCommand::Create { title, body }) => {
            if title.trim().is_empty() {
                return Err(CliError::command_usage("--title must not be empty")
                    .with_stable_code(StableErrorCode::CliInvalidArguments));
            }
            let client = MegaClient::new(base, true).await?;
            client
                .envelope::<Value>(
                    Method::POST,
                    &["api", "v1", "issue", "new"],
                    Some(&json!({ "title": title, "description": body })),
                )
                .await?;
            let report = json!({ "resource": "issue", "action": "create", "title": title });
            if output.is_json() {
                emit_json_data("mega issue create", &report, output)
            } else {
                if !output.quiet {
                    println!("Created issue: {title}");
                }
                Ok(())
            }
        }
        MegaCommand::Issue(MegaIssueCommand::Close(args)) => {
            let client = MegaClient::new(base, true).await?;
            mutate_resource(&client, "issue", "close", &args.link, output).await
        }
        MegaCommand::Issue(MegaIssueCommand::Reopen(args)) => {
            let client = MegaClient::new(base, true).await?;
            mutate_resource(&client, "issue", "reopen", &args.link, output).await
        }
        MegaCommand::Cl(MegaClCommand::List(args)) => {
            let client = MegaClient::new(base, false).await?;
            list_resource(&client, "cl", args, output).await
        }
        MegaCommand::Cl(MegaClCommand::View(args)) => {
            let client = MegaClient::new(base, true).await?;
            view_resource(&client, "cl", &args.link, output).await
        }
        MegaCommand::Cl(MegaClCommand::Close(args)) => {
            let client = MegaClient::new(base, true).await?;
            mutate_resource(&client, "cl", "close", &args.link, output).await
        }
        MegaCommand::Cl(MegaClCommand::Reopen(args)) => {
            let client = MegaClient::new(base, true).await?;
            mutate_resource(&client, "cl", "reopen", &args.link, output).await
        }
        MegaCommand::Cl(MegaClCommand::Merge(args)) => {
            let client = MegaClient::new(base, true).await?;
            mutate_resource(&client, "cl", "merge", &args.link, output).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_validation_rejects_paths_and_non_tls_remotes() {
        assert!(resolve_base_url(Some("https://git.example.com")).is_ok());
        assert!(resolve_base_url(Some("http://127.0.0.1:8080")).is_ok());
        assert!(resolve_base_url(Some("http://git.example.com")).is_err());
        assert!(resolve_base_url(Some("https://git.example.com/api")).is_err());
        assert!(resolve_base_url(Some("https://user:pass@git.example.com")).is_err());
    }

    #[test]
    fn list_validation_bounds_pagination() {
        let mut args = ListArgs {
            state: "open".to_string(),
            author: None,
            assignee: Vec::new(),
            label: Vec::new(),
            sort: None,
            asc: false,
            page: 1,
            limit: 100,
        };
        assert!(validate_list(&args).is_ok());
        args.limit = 101;
        assert!(validate_list(&args).is_err());
        args.limit = 1;
        args.page = 0;
        assert!(validate_list(&args).is_err());
    }
}

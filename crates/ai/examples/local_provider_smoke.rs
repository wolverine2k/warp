//! End-to-end smoke binary for the Custom Local LLM Provider (specs/GH9303/).
//!
//! Connects directly to a user-configured OpenAI-compatible endpoint and prints
//! the streamed response events. Useful for:
//!
//! - **Pre-launch verification**: confirm a chosen endpoint speaks the expected
//!   wire format before bringing up the full Warp client.
//! - **Network audit**: pair with mitmproxy to confirm zero `*.warp.dev`
//!   traffic on the LLM call.
//! - **Tool-call sanity**: pass `--with-tools` and ask the model to read a file
//!   to check whether it emits a parseable tool call against our schemas.
//!
//! Examples:
//! ```bash
//! # Ollama on localhost
//! cargo run --example local_provider_smoke -p ai -- \
//!     --base-url http://localhost:11434/v1 \
//!     --model llama3.1 \
//!     --query "Say hi in one word"
//!
//! # NVIDIA NIM with bearer auth
//! cargo run --example local_provider_smoke -p ai -- \
//!     --base-url https://integrate.api.nvidia.com/v1 \
//!     --model meta/llama-3.1-70b-instruct \
//!     --api-key "$NVIDIA_API_KEY" \
//!     --query "What is 2+2?"
//!
//! # Tool-call exercise — ask the model to read a file
//! cargo run --example local_provider_smoke -p ai -- \
//!     --base-url http://localhost:11434/v1 \
//!     --model qwen2.5-coder:7b \
//!     --with-tools \
//!     --query "Use the read_files tool to read Cargo.toml"
//! ```

use ai::local_provider::{
    config::LocalProviderConfig,
    request::LocalProviderInput,
    run::{run_chat_turn, LocalRunError},
};
use clap::Parser;
use futures::stream::StreamExt;
use std::process::ExitCode;
use warp_multi_agent_api as api;

#[derive(Parser, Debug)]
#[command(
    name = "local_provider_smoke",
    about = "End-to-end smoke test for the Warp Custom Local LLM Provider"
)]
struct Args {
    /// Base URL of the OpenAI-compatible endpoint (e.g. http://localhost:11434/v1).
    #[arg(long)]
    base_url: String,

    /// Model id the endpoint expects (e.g. llama3.1, qwen2.5-coder:7b).
    #[arg(long)]
    model: String,

    /// Optional bearer token, sent as `Authorization: Bearer <key>`.
    #[arg(long)]
    api_key: Option<String>,

    /// User query to send. Defaults to a benign greeting.
    #[arg(long, default_value = "Say hi in one word.")]
    query: String,

    /// Display name for log output.
    #[arg(long, default_value = "Local")]
    display_name: String,

    /// Send the v1 tool schemas in the `tools` field of the request.
    /// Use this to test whether the model can produce parseable tool calls.
    #[arg(long, default_value_t = false)]
    with_tools: bool,

    /// Optional context-window hint surfaced in the system prompt.
    #[arg(long)]
    context_window: Option<u32>,

    /// Cancel after this many seconds (0 = no timeout).
    #[arg(long, default_value_t = 60)]
    timeout_secs: u64,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Init the rustls provider (the Warp app does this in lib.rs::init).
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("install rustls crypto provider");

    let args = Args::parse();
    println!(
        "Smoke against {display}: {url} model={model} tools={tools} key={has_key}",
        display = args.display_name,
        url = args.base_url,
        model = args.model,
        tools = args.with_tools,
        has_key = args.api_key.is_some(),
    );

    let cfg = LocalProviderConfig {
        display_name: args.display_name.clone(),
        base_url: args.base_url.clone(),
        model_id: args.model.clone(),
        api_key: args.api_key.clone(),
        supports_tools: args.with_tools,
        context_window: args.context_window,
    };
    if let Err(e) = cfg.validate() {
        eprintln!("Config validation failed: {e}");
        return ExitCode::from(2);
    }

    let supported_tools = if args.with_tools {
        // Same v1 set the production picker advertises.
        vec![
            api::ToolType::ReadFiles,
            api::ToolType::ApplyFileDiffs,
            api::ToolType::RunShellCommand,
            api::ToolType::Grep,
            api::ToolType::FileGlobV2,
        ]
    } else {
        vec![]
    };

    let input = LocalProviderInput {
        user_query: Some(args.query.clone()),
        tasks: vec![],
        supported_tools,
    };

    let (cancel_tx, cancel_rx) = futures::channel::oneshot::channel();
    if args.timeout_secs > 0 {
        let secs = args.timeout_secs;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            let _ = cancel_tx.send(());
        });
    }

    let http = reqwest::Client::new();
    let started = std::time::Instant::now();
    let mut stream = match run_chat_turn(input, cfg, cancel_rx, http).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("run_chat_turn returned an error before any events: {e}");
            return match e {
                LocalRunError::InvalidConfig(_) => ExitCode::from(2),
                LocalRunError::Transport(_) => ExitCode::from(3),
                LocalRunError::EncodeRequest(_) => ExitCode::from(4),
            };
        }
    };

    let mut text_buf = String::new();
    let mut reasoning_buf = String::new();
    let mut tool_calls = Vec::<String>::new();
    let mut transactions: i32 = 0;
    let mut events: usize = 0;
    let mut had_finished = false;
    let mut finish_label = "<no finish>".to_string();

    while let Some(ev) = stream.next().await {
        events += 1;
        match &ev.r#type {
            Some(api::response_event::Type::Init(init)) => {
                println!(
                    "[init] conversation_id={} request_id={} run_id={}",
                    init.conversation_id, init.request_id, init.run_id
                );
            }
            Some(api::response_event::Type::ClientActions(ca)) => {
                for action in &ca.actions {
                    handle_action(
                        action,
                        &mut text_buf,
                        &mut reasoning_buf,
                        &mut tool_calls,
                        &mut transactions,
                    );
                }
            }
            Some(api::response_event::Type::Finished(f)) => {
                had_finished = true;
                finish_label = describe_finish(&f.reason);
            }
            None => {}
        }
    }

    let elapsed = started.elapsed();
    println!();
    println!("---- summary ----");
    println!("elapsed:      {:?}", elapsed);
    println!("events:       {events}");
    println!("transactions: {transactions} (begin/commit/rollback toggles)");
    println!("had Finished: {had_finished} (reason: {finish_label})");
    if !text_buf.is_empty() {
        println!();
        println!("---- assistant text ----");
        println!("{}", text_buf);
    }
    if !reasoning_buf.is_empty() {
        println!();
        println!("---- reasoning ----");
        println!("{}", reasoning_buf);
    }
    if !tool_calls.is_empty() {
        println!();
        println!("---- tool calls ----");
        for tc in &tool_calls {
            println!("{tc}");
        }
    }

    if !had_finished {
        eprintln!("FAIL: stream did not produce a Finished event");
        return ExitCode::from(5);
    }
    if matches!(
        finish_label.as_str(),
        "Done"
    ) {
        ExitCode::SUCCESS
    } else {
        eprintln!("FAIL: stream finished with non-Done reason: {finish_label}");
        ExitCode::from(6)
    }
}

fn handle_action(
    action: &api::ClientAction,
    text_buf: &mut String,
    reasoning_buf: &mut String,
    tool_calls: &mut Vec<String>,
    transactions: &mut i32,
) {
    let Some(action) = &action.action else { return };
    match action {
        api::client_action::Action::BeginTransaction(_) => {
            *transactions += 1;
            println!("[begin]");
        }
        api::client_action::Action::CommitTransaction(_) => {
            *transactions -= 1;
            println!("[commit]");
        }
        api::client_action::Action::RollbackTransaction(_) => {
            *transactions -= 1;
            println!("[rollback]");
        }
        api::client_action::Action::AddMessagesToTask(amt) => {
            for msg in &amt.messages {
                handle_message(&msg.message, text_buf, reasoning_buf, tool_calls, "open");
            }
        }
        api::client_action::Action::AppendToMessageContent(append) => {
            if let Some(msg) = &append.message {
                handle_message(&msg.message, text_buf, reasoning_buf, tool_calls, "append");
            }
        }
        _ => {}
    }
}

fn handle_message(
    inner: &Option<api::message::Message>,
    text_buf: &mut String,
    reasoning_buf: &mut String,
    tool_calls: &mut Vec<String>,
    op: &str,
) {
    match inner {
        Some(api::message::Message::AgentOutput(a)) => {
            text_buf.push_str(&a.text);
            print!("{}", a.text);
            use std::io::Write;
            let _ = std::io::stdout().flush();
            if op == "open" {
                println!();
            }
        }
        Some(api::message::Message::AgentReasoning(r)) => {
            reasoning_buf.push_str(&r.reasoning);
            eprintln!("[reasoning] {}", r.reasoning);
        }
        Some(api::message::Message::ToolCall(tc)) => {
            let tool_name = match tc.tool.as_ref() {
                Some(api::message::tool_call::Tool::ReadFiles(_)) => "read_files",
                Some(api::message::tool_call::Tool::ApplyFileDiffs(_)) => "apply_file_diffs",
                Some(api::message::tool_call::Tool::RunShellCommand(_)) => "run_shell_command",
                Some(api::message::tool_call::Tool::Grep(_)) => "grep",
                Some(api::message::tool_call::Tool::FileGlobV2(_)) => "file_glob_v2",
                Some(_) => "<other>",
                None => "<unparsed>",
            };
            tool_calls.push(format!("{}: id={} typed={}", tool_name, tc.tool_call_id, tc.tool.is_some()));
            println!();
            println!("[tool_call name={tool_name} id={}]", tc.tool_call_id);
        }
        _ => {}
    }
}

fn describe_finish(reason: &Option<api::response_event::stream_finished::Reason>) -> String {
    use api::response_event::stream_finished::Reason;
    match reason {
        Some(Reason::Done(_)) => "Done".into(),
        Some(Reason::Other(_)) => "Other".into(),
        Some(Reason::MaxTokenLimit(_)) => "MaxTokenLimit".into(),
        Some(Reason::QuotaLimit(_)) => "QuotaLimit".into(),
        Some(Reason::ContextWindowExceeded(_)) => "ContextWindowExceeded".into(),
        Some(Reason::LlmUnavailable(_)) => "LlmUnavailable".into(),
        Some(Reason::InvalidApiKey(_)) => "InvalidApiKey".into(),
        Some(Reason::InternalError(e)) => format!("InternalError({})", e.message),
        None => "<no reason>".into(),
    }
}

use std::fs;
use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Router as AxumRouter;
use axum::routing::any;
use clap::{Parser, Subcommand};
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use tokio::net::TcpListener;
use tracing::info;

use croxy::attach;
use croxy::cli_config;
use croxy::config::Config;
use croxy::metrics::MetricsStore;
use croxy::metrics_log::MetricsLogger;
use croxy::proxy::{AppState, handle_request};
use croxy::router::Router;
use croxy::tui::ExitMode;

#[derive(Parser)]
#[command(name = "croxy", about = "Model-routing proxy for the Anthropic API")]
struct Cli {
    /// Path to config file
    #[arg(short, long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Enable debug logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Metrics retention window in minutes
    #[arg(long, global = true, default_value = "60", value_name = "MINUTES")]
    retention: u64,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start proxy in background
    Start,
    /// Stop a detached instance
    Stop,
    /// Print shell environment variables (for eval)
    Shellenv,
    /// Create default config file
    Init,
    /// Read or modify configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Set a configuration value (dot-separated key)
    Set { key: String, value: String },
    /// Get a configuration value (dot-separated key)
    Get { key: String },
    /// Print the config file path
    Path,
}

fn config_dir() -> PathBuf {
    dirs::home_dir()
        .expect("could not determine home directory")
        .join(".config/croxy")
}

fn default_config_path() -> PathBuf {
    config_dir().join("config.toml")
}

fn pid_path() -> PathBuf {
    config_dir().join("croxy.pid")
}

fn log_path() -> PathBuf {
    config_dir().join("croxy.log")
}

fn load_config(path: &PathBuf) -> Config {
    Figment::new()
        .merge(Toml::file(path))
        .merge(Env::prefixed("CROXY_").split("_"))
        .extract()
        .unwrap_or_else(|e| {
            eprintln!("failed to load config: {e}");
            std::process::exit(1);
        })
}

fn read_pid() -> Option<i32> {
    fs::read_to_string(pid_path())
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

fn pid_is_alive(pid: i32) -> bool {
    kill(Pid::from_raw(pid), None).is_ok()
}

fn remove_pid_file() {
    let _ = fs::remove_file(pid_path());
}

fn write_pid_file() {
    let pid = std::process::id();
    fs::write(pid_path(), pid.to_string()).unwrap_or_else(|e| {
        eprintln!("failed to write pid file: {e}");
    });
}

fn cmd_stop() {
    match read_pid() {
        Some(pid) if pid_is_alive(pid) => {
            kill(Pid::from_raw(pid), Signal::SIGTERM).unwrap_or_else(|e| {
                eprintln!("failed to send SIGTERM to {pid}: {e}");
                std::process::exit(1);
            });
            remove_pid_file();
            eprintln!("stopped croxy (pid {pid})");
        }
        Some(_) => {
            remove_pid_file();
            eprintln!("croxy is not running (stale pid file removed)");
        }
        None => {
            eprintln!("croxy is not running (no pid file)");
        }
    }
}

fn cmd_init() {
    let dir = config_dir();
    let path = dir.join("config.toml");

    if path.exists() {
        eprintln!("config already exists: {}", path.display());
        return;
    }

    fs::create_dir_all(&dir).unwrap_or_else(|e| {
        eprintln!("failed to create {}: {e}", dir.display());
        std::process::exit(1);
    });

    let default_config = r#"[server]
host = "127.0.0.1"
port = 3100
# max_body_size = 10485760  # 10 MiB

[provider.anthropic]
url = "https://api.anthropic.com"

[provider.ollama]
url = "http://localhost:11434"
strip_auth = true
api_key = "ollama"
stub_count_tokens = true

[[routes]]
pattern = "opus"
provider = "anthropic"

[[routes]]
pattern = "sonnet|haiku"
provider = "ollama"
model = "qwen2.5-coder:32b"

[default]
provider = "anthropic"

# [logging.metrics]
# enabled = true
# path = "~/.config/croxy/logs/metrics.jsonl"
# max_size_mb = 50
# max_files = 5
"#;

    fs::write(&path, default_config).unwrap_or_else(|e| {
        eprintln!("failed to write {}: {e}", path.display());
        std::process::exit(1);
    });

    eprintln!("created {}", path.display());
}

fn cmd_shellenv(config_path: &PathBuf) {
    let config = load_config(config_path);
    let host = match config.server.host.as_str() {
        "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        other => other,
    };
    let addr = format!("{host}:{}", config.server.port);

    if TcpStream::connect(&addr).is_ok() {
        println!("export ANTHROPIC_BASE_URL=http://{addr}");
    }
}

fn detach(config_path: &PathBuf, verbose: bool, retention: u64) {
    if let Some(pid) = read_pid() {
        if pid_is_alive(pid) {
            eprintln!("croxy is already running (pid {pid})");
            std::process::exit(1);
        }
        remove_pid_file();
    }

    let config = load_config(config_path);
    let host = match config.server.host.as_str() {
        "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        other => other,
    };
    let probe_addr = format!("{host}:{}", config.server.port);

    let dir = config_dir();
    fs::create_dir_all(&dir).unwrap_or_else(|e| {
        eprintln!("failed to create {}: {e}", dir.display());
        std::process::exit(1);
    });

    let log = fs::File::create(log_path()).unwrap_or_else(|e| {
        eprintln!("failed to create log file: {e}");
        std::process::exit(1);
    });
    let log_err = log.try_clone().unwrap();

    let exe = std::env::current_exe().unwrap_or_else(|e| {
        eprintln!("failed to determine executable path: {e}");
        std::process::exit(1);
    });

    let devnull = fs::File::open("/dev/null").unwrap_or_else(|e| {
        eprintln!("failed to open /dev/null: {e}");
        std::process::exit(1);
    });

    let mut cmd = Command::new(exe);
    cmd.arg("--config").arg(config_path);
    cmd.arg("--retention").arg(retention.to_string());
    if verbose {
        cmd.arg("--verbose");
    }
    cmd.stdin(devnull);

    // Create new session so child survives terminal close
    // SAFETY: setsid is async-signal-safe per POSIX
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setsid().map_err(std::io::Error::other)?;
            Ok(())
        });
    }

    let mut child = cmd.stdout(log).stderr(log_err).spawn().unwrap_or_else(|e| {
        eprintln!("failed to spawn detached process: {e}");
        std::process::exit(1);
    });

    let child_pid = child.id();

    // Detach: we don't want to wait on the child (it's the daemon).
    // Reap it so we don't leave a zombie during the brief startup check.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    fs::write(pid_path(), child_pid.to_string()).unwrap_or_else(|e| {
        eprintln!("failed to write pid file: {e}");
        std::process::exit(1);
    });

    // Poll until the daemon is accepting connections or the process dies
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if !pid_is_alive(i32::try_from(child_pid).expect("invalid pid")) {
            remove_pid_file();
            eprintln!("croxy failed to start, check {}", log_path().display());
            std::process::exit(1);
        }
        if TcpStream::connect(&probe_addr).is_ok() {
            eprintln!(
                "croxy started (pid {child_pid}), log: {}",
                log_path().display()
            );
            return;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!(
                "croxy started (pid {child_pid}) but not yet accepting connections, log: {}",
                log_path().display()
            );
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn run_attached(config_path: &PathBuf, retention_minutes: u64) {
    let config = load_config(config_path);

    if !config.logging.metrics.enabled {
        eprintln!("cannot attach: [logging.metrics] enabled = true required in config");
        std::process::exit(1);
    }

    let retention = std::time::Duration::from_secs(retention_minutes * 60);
    let metrics = Arc::new(MetricsStore::new(retention));

    attach::load_history(&config.logging.metrics, &metrics, retention);

    let log_path = PathBuf::from(&config.logging.metrics.path);
    let stop = Arc::new(AtomicBool::new(false));

    let tail_store = metrics.clone();
    let tail_stop = stop.clone();
    let _tail_handle = std::thread::spawn(move || {
        attach::tail_log(&log_path, tail_store, tail_stop);
    });

    let evict_metrics = metrics.clone();
    let evict_stop = stop.clone();
    let _evict_handle = std::thread::spawn(move || {
        while !evict_stop.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_secs(60));
            evict_metrics.evict_expired();
        }
    });

    croxy::tui::run(metrics, true).unwrap_or_else(|e| {
        eprintln!("TUI error: {e}");
        std::process::exit(1);
    });

    stop.store(true, Ordering::Relaxed);
    // Don't join -- the evict thread sleeps 60s and we don't want to block exit.
    // The process is exiting anyway; these threads will be cleaned up.
}

fn init_tracing(use_tui: bool, verbose: bool) {
    let default_filter = if verbose { "croxy=debug" } else { "croxy=info" };
    let env_filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| default_filter.parse().unwrap())
    };

    if use_tui {
        let log_dir = config_dir();
        let _ = fs::create_dir_all(&log_dir);
        let log_file = fs::File::create(log_dir.join("croxy.log")).unwrap_or_else(|e| {
            eprintln!("failed to create log file: {e}");
            std::process::exit(1);
        });
        tracing_subscriber::fmt()
            .with_env_filter(env_filter())
            .with_writer(std::sync::Mutex::new(log_file))
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter())
            .init();
    }
}

fn create_metrics(config: &Config, retention: std::time::Duration) -> Arc<MetricsStore> {
    Arc::new(if config.logging.metrics.enabled {
        match MetricsLogger::new(&config.logging.metrics) {
            Ok(logger) => {
                info!(path = %config.logging.metrics.path, "metrics logging enabled");
                MetricsStore::with_logger(retention, logger)
            }
            Err(e) => {
                tracing::warn!("failed to initialize metrics logger: {e}");
                MetricsStore::new(retention)
            }
        }
    } else {
        MetricsStore::new(retention)
    })
}

fn spawn_eviction_task(metrics: &Arc<MetricsStore>) {
    let evict_metrics = metrics.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            evict_metrics.evict_expired();
        }
    });
}

async fn run_tui(metrics: Arc<MetricsStore>) -> ExitMode {
    tokio::task::spawn_blocking(move || croxy::tui::run(metrics, false))
        .await
        .unwrap()
        .unwrap_or_else(|e| {
            eprintln!("TUI error: {e}");
            std::process::exit(1);
        })
}

async fn await_shutdown_signal() {
    // Use explicit unix signals because crossterm's signal-hook
    // handler can interfere with tokio::signal::ctrl_c().
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("failed to register SIGINT handler");
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register SIGTERM handler");

    tokio::select! {
        _ = sigint.recv() => {}
        _ = sigterm.recv() => {}
    }
}

async fn run_foreground(listener: TcpListener, app: AxumRouter, metrics: Arc<MetricsStore>) {
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap_or_else(|e| tracing::error!("server error: {e}"));
    });

    spawn_eviction_task(&metrics);

    match run_tui(metrics).await {
        ExitMode::Quit => {
            let _ = shutdown_tx.send(());
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        ExitMode::Detach => {
            write_pid_file();
            eprintln!("detached (pid {})", std::process::id());
            await_shutdown_signal().await;
            let _ = shutdown_tx.send(());
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            remove_pid_file();
        }
    }
}

async fn run_headless(listener: TcpListener, app: AxumRouter) {
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            info!("shutting down");
        })
        .await
        .unwrap();
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(default_config_path);

    match cli.command {
        Some(Commands::Start) => return detach(&config_path, cli.verbose, cli.retention),
        Some(Commands::Stop) => return cmd_stop(),
        Some(Commands::Init) => return cmd_init(),
        Some(Commands::Shellenv) => return cmd_shellenv(&config_path),
        Some(Commands::Config { action }) => {
            return match action {
                ConfigAction::Set { key, value } => {
                    cli_config::config_set(&config_path, &key, &value)
                }
                ConfigAction::Get { key } => cli_config::config_get(&config_path, &key),
                ConfigAction::Path => println!("{}", config_path.display()),
            };
        }
        None => {}
    }

    let use_tui = std::io::IsTerminal::is_terminal(&std::io::stdin());

    // Auto-attach: if a daemon is already running and we have a TUI, attach to it
    if use_tui
        && let Some(pid) = read_pid()
        && pid_is_alive(pid)
    {
        return run_attached(&config_path, cli.retention);
    }

    init_tracing(use_tui, cli.verbose);

    let config = load_config(&config_path);
    let router = Router::from_config(&config).unwrap_or_else(|e| {
        eprintln!("failed to build router: {e}");
        std::process::exit(1);
    });

    let retention = std::time::Duration::from_secs(cli.retention * 60);
    let metrics = create_metrics(&config, retention);

    let state = Arc::new(AppState {
        router,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("failed to build HTTP client"),
        metrics: metrics.clone(),
        max_body_size: config.server.max_body_size,
    });

    let app = AxumRouter::new()
        .fallback(any(handle_request))
        .with_state(state);

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = TcpListener::bind(&addr).await.unwrap_or_else(|e| {
        eprintln!("failed to bind {addr}: {e}");
        std::process::exit(1);
    });

    info!(addr = %addr, "croxy listening");

    if use_tui {
        run_foreground(listener, app, metrics).await;
    } else {
        run_headless(listener, app).await;
    }
}

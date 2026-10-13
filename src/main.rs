use android_use::{
    backend::{self, AndroidBackend},
    http::{self, ApiState, Session},
    mcp::{AndroidHandler, DaemonClient},
};
use clap::{Parser, Subcommand};
use rmcp::{transport::stdio, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs::OpenOptions, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

#[derive(Parser)]
#[command(
    name = "android-use",
    version,
    about = "Android control for agents, sharing the iphone-use control core"
)]
struct Cli {
    #[arg(long, global = true)]
    url: Option<String>,
    #[arg(long, global = true, default_value = "cli")]
    owner: String,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// Download and verify the pinned UIAutomator runner (no Python required).
    Setup,
    /// Serve one explicitly selected, authorized Android over loopback HTTP.
    Serve {
        #[arg(long)]
        serial: String,
        #[arg(long, default_value_t = 44322)]
        port: u16,
        #[arg(long)]
        runner_jar: Option<PathBuf>,
    },
    /// Start a background daemon for the sole attached phone or an explicit serial.
    Connect {
        #[arg(long)]
        serial: Option<String>,
        #[arg(long, default_value_t = 44322)]
        port: u16,
    },
    /// Inspect ADB authorization and attached devices without touching the screen.
    Doctor,
    /// Install the latest independently published GitHub Release, or only check.
    Upgrade {
        #[arg(long)]
        check: bool,
    },
    /// Stop this daemon and release its runner and ADB forward.
    Disconnect,
    Status,
    Capabilities,
    Elements {
        #[arg(long)]
        since: Option<String>,
    },
    /// Execute one action JSON or a batch JSON containing steps.
    Act {
        json: String,
    },
    Screenshot {
        output: PathBuf,
    },
    Release,
    /// MCP stdio server; stdout is reserved for protocol messages.
    Mcp,
}
#[derive(Serialize, Deserialize)]
struct Connection {
    url: String,
    token: String,
}
fn config_path() -> anyhow::Result<PathBuf> {
    Ok(backend::data_dir()?.join("connection.json"))
}
fn save_connection(connection: &Connection) -> anyhow::Result<()> {
    let path = config_path()?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(&temp)?;
    file.write_all(&serde_json::to_vec(connection)?)?;
    file.sync_all()?;
    std::fs::rename(&temp, path)?;
    Ok(())
}
fn connection(url: Option<String>) -> anyhow::Result<Connection> {
    let saved = std::fs::read(config_path()?)
        .ok()
        .and_then(|b| serde_json::from_slice::<Connection>(&b).ok());
    let token = std::env::var("ANDROID_USE_TOKEN").ok();
    match (url, token, saved) {
        (Some(url), Some(token), _) => Ok(Connection { url, token }),
        (Some(url), None, Some(saved)) if url == saved.url => Ok(saved),
        (None, token, Some(mut saved)) => {
            if let Some(token) = token {
                saved.token = token;
            }
            Ok(saved)
        }
        _ => anyhow::bail!("Start android-use serve, or specify --url and ANDROID_USE_TOKEN"),
    }
}
async fn attached() -> anyhow::Result<Vec<Value>> {
    let mut command = tokio::process::Command::new("adb");
    if cfg!(target_os = "macos") && std::env::var_os("ADB_LIBUSB").is_none() {
        command.env("ADB_LIBUSB", "1");
    }
    let output = command
        .args(["devices", "-l"])
        .output()
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "ADB is missing; install Android platform-tools and rerun android-use doctor"
            )
        })?;
    if !output.status.success() {
        anyhow::bail!("ADB device discovery failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields.next()?;
            let state = fields.next()?;
            if serial == "List" || !matches!(state, "device" | "unauthorized" | "offline") {
                return None;
            }
            Some(json!({"serial":serial,"state":state}))
        })
        .collect())
}
fn print_result(value: Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(&value)?);
    if value.get("ok") == Some(&json!(false)) {
        std::process::exit(1);
    }
    Ok(())
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Upgrade { check } => {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .user_agent("android-use")
                .build()?;
            let release: Value = client
                .get("https://api.github.com/repos/leeguooooo/android-use/releases/latest")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let tag = release["tag_name"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Release tag missing"))?;
            if !tag.starts_with('v')
                || !tag[1..]
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            {
                anyhow::bail!("Invalid release tag");
            }
            let update = tag != concat!("v", env!("CARGO_PKG_VERSION"));
            if check || !update {
                print_result(
                    json!({"ok":true,"version":env!("CARGO_PKG_VERSION"),"latest":tag,"update_available":update}),
                )?;
                return Ok(());
            }
            let exe = std::env::current_exe()?;
            let destination = exe
                .parent()
                .ok_or_else(|| anyhow::anyhow!("Install directory unavailable"))?;
            #[cfg(unix)]
            {
                use tokio::io::AsyncWriteExt;
                let mut child = tokio::process::Command::new("sh")
                    .env("ANDROID_USE_TAG", tag)
                    .env("ANDROID_USE_INSTALL_DIR", destination)
                    .stdin(std::process::Stdio::piped())
                    .spawn()?;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(include_bytes!("../install.sh"))
                    .await?;
                if !child.wait().await?.success() {
                    anyhow::bail!("Release installer failed; existing binary was preserved");
                }
            }
            #[cfg(windows)]
            {
                let directory = backend::data_dir()?;
                std::fs::create_dir_all(&directory)?;
                let script = directory.join(format!("upgrade-{}.ps1", std::process::id()));
                std::fs::write(&script, include_bytes!("../install.ps1"))?;
                let status = tokio::process::Command::new("powershell.exe")
                    .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                    .arg(&script)
                    .arg("-Tag")
                    .arg(tag)
                    .arg("-InstallDir")
                    .arg(destination)
                    .status()
                    .await;
                let _ = std::fs::remove_file(script);
                if !status?.success() {
                    anyhow::bail!("Release installer failed");
                }
            }
            println!(
                "{}",
                json!({"ok":true,"installed":tag,"hint":"Restart a foreground daemon to use the new version; family upgrades refresh the skill"})
            );
        }
        Commands::Doctor => {
            let devices = attached().await?;
            println!(
                "{}",
                json!({"ok":true,"devices":devices,"hint":"Enable USB debugging and authorize this computer on the phone"})
            );
        }
        Commands::Connect { serial, port } => {
            let devices = attached().await?;
            let authorized: Vec<_> = devices.iter().filter(|d| d["state"] == "device").collect();
            let serial = match serial {
                Some(serial) if authorized.iter().any(|d| d["serial"] == serial) => serial,
                Some(_) => anyhow::bail!(
                    "Selected device is not attached and authorized; run android-use doctor"
                ),
                None if authorized.len() == 1 => authorized[0]["serial"].as_str().unwrap().into(),
                _ => anyhow::bail!(
                    "Select one authorized device using --serial; run android-use doctor"
                ),
            };
            if let Ok(saved) = connection(None) {
                let client = DaemonClient::new(saved.url.clone(), saved.token, cli.owner.clone())?;
                let status = tokio::time::timeout(
                    std::time::Duration::from_millis(750),
                    client.request("/agent/status", None),
                )
                .await
                .unwrap_or(json!({"drivable":false}));
                if status["drivable"] == true {
                    if status["device"] != serial {
                        anyhow::bail!("Existing daemon serves another device");
                    }
                    print_result(status)?;
                    return Ok(());
                }
                let response = client.request("/agent/disconnect", Some(json!({}))).await;
                if response["error"] == "device_busy" {
                    anyhow::bail!("Another session owns the existing daemon");
                }
            }
            let dir = backend::data_dir()?;
            std::fs::create_dir_all(&dir)?;
            // A just-disconnected daemon may still be removing its runner.
            // Wait for its lock, then leave the actual acquisition to serve.
            let lock_path = dir
                .join("locks")
                .join(format!("{:x}.lock", Sha256::digest(serial.as_bytes())));
            if lock_path.exists() {
                let lock = OpenOptions::new().read(true).write(true).open(lock_path)?;
                let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while fs2::FileExt::try_lock_exclusive(&lock).is_err() {
                    if std::time::Instant::now() >= until {
                        anyhow::bail!("Existing daemon has not released its device yet");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                fs2::FileExt::unlock(&lock)?;
            }
            let mut options = OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let log = options.open(dir.join("daemon.log"))?;
            let mut daemon = tokio::process::Command::new(std::env::current_exe()?);
            // The daemon must survive the invoking CLI's terminal/process group.
            #[cfg(unix)]
            daemon.process_group(0);
            #[cfg(windows)]
            daemon.creation_flags(0x00000008 | 0x00000200);
            let mut child = daemon
                .args(["serve", "--serial", &serial, "--port", &port.to_string()])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(log)
                .spawn()?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
            loop {
                if child.try_wait()?.is_some() {
                    anyhow::bail!(
                        "Daemon startup failed; inspect private ~/.android-use/daemon.log"
                    );
                }
                if let Ok(saved) = connection(None) {
                    if let Ok(client) = DaemonClient::new(saved.url, saved.token, cli.owner.clone())
                    {
                        let status = tokio::time::timeout(
                            std::time::Duration::from_millis(750),
                            client.request("/agent/status", None),
                        )
                        .await
                        .unwrap_or(json!({"drivable":false}));
                        if status["drivable"] == true && status["device"] == serial {
                            print_result(status)?;
                            break;
                        }
                    }
                }
                if std::time::Instant::now() >= deadline {
                    child.kill().await?;
                    anyhow::bail!("Daemon startup timed out");
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
        Commands::Setup => {
            let path = backend::setup_runner().await?;
            println!(
                "{}",
                json!({"ok":true,"runner":path,"sha256":backend::RUNNER_SHA256,"requires":"adb and an Android with authorized USB debugging"})
            );
        }
        Commands::Serve {
            serial,
            port,
            runner_jar,
        } => {
            if serial.is_empty() || serial.len() > 255 || serial.chars().any(char::is_control) {
                anyhow::bail!("Invalid Android serial");
            }
            let dir = backend::data_dir()?;
            std::fs::create_dir_all(dir.join("locks"))?;
            let lock_path = dir
                .join("locks")
                .join(format!("{:x}.lock", Sha256::digest(serial.as_bytes())));
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)?;
            fs2::FileExt::try_lock_exclusive(&lock)
                .map_err(|_| anyhow::anyhow!("Another android-use daemon owns this device"))?;
            let listener =
                tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
            let url = format!("http://{}", listener.local_addr()?);
            let jar = match runner_jar {
                Some(path) => path,
                None => backend::setup_runner().await?,
            };
            let device = AndroidBackend::connect(serial, &jar)
                .await
                .map_err(|e| anyhow::anyhow!("{}: {}", e.error, e.message))?;
            let mut random = [0u8; 32];
            getrandom::fill(&mut random)
                .map_err(|_| anyhow::anyhow!("Random source unavailable"))?;
            use base64::Engine as _;
            let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
            save_connection(&Connection {
                url: url.clone(),
                token: token.clone(),
            })?;
            let session = Arc::new(Mutex::new(Session::new(device)));
            let stopping = session.lock().await.control.stopping.clone();
            eprintln!("android-use listening at {url}; connection credentials saved privately");
            let api = http::router(ApiState::new(session.clone(), token));
            let served = axum::serve(listener, api)
                .with_graceful_shutdown(async move {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {},
                        _ = async { while !stopping.load(std::sync::atomic::Ordering::Acquire) { tokio::time::sleep(std::time::Duration::from_millis(50)).await; } } => {},
                    }
                    stopping.store(true, std::sync::atomic::Ordering::Release);
                })
                .await;
            session.lock().await.control.backend.close().await;
            served?;
        }
        command => {
            let connection = connection(cli.url)?;
            let owner = if matches!(command, Commands::Mcp) {
                format!("mcp-{}", std::process::id())
            } else {
                cli.owner
            };
            let client = DaemonClient::new(connection.url, connection.token, owner)?;
            match command {
                Commands::Disconnect => {
                    print_result(client.request("/agent/disconnect", Some(json!({}))).await)?
                }
                Commands::Status => print_result(client.request("/agent/status", None).await)?,
                Commands::Capabilities => {
                    print_result(client.request("/agent/capabilities", None).await)?
                }
                Commands::Elements { since } => {
                    let path = if let Some(id) = since {
                        if !id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                        {
                            anyhow::bail!("Invalid snapshot token");
                        }
                        format!("/agent/elements?since={id}")
                    } else {
                        "/agent/elements".into()
                    };
                    print_result(client.request(&path, None).await)?;
                }
                Commands::Act { json: text } => {
                    let value: Value = serde_json::from_str(&text)?;
                    let path = if value.get("steps").is_some() {
                        "/agent/actions"
                    } else {
                        "/agent/input"
                    };
                    print_result(client.request(path, Some(value)).await)?;
                }
                Commands::Screenshot { output } => {
                    let bytes = client.screenshot().await.map_err(|e| anyhow::anyhow!(e))?;
                    tokio::fs::write(&output, bytes).await?;
                    println!("{}", json!({"ok":true,"path":output}));
                }
                Commands::Release => {
                    print_result(client.request("/agent/release", Some(json!({}))).await)?
                }
                Commands::Mcp => {
                    let service = AndroidHandler::new(client.clone()).serve(stdio()).await?;
                    let waited = service.waiting().await;
                    let _ = client.request("/agent/release", Some(json!({}))).await;
                    waited?;
                }
                Commands::Setup
                | Commands::Serve { .. }
                | Commands::Doctor
                | Commands::Upgrade { .. }
                | Commands::Connect { .. } => unreachable!(),
            }
        }
    }
    Ok(())
}

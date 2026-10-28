//! `hj`: a low-level tool for talking to a Makera machine. Used to explore and verify the protocol
//! against real hardware; not a user-facing controller.
//!
//! Commands typed in `hj console` are sent to the machine verbatim, including motion commands.

use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use hj_makera::command::{self, Realtime};
use hj_makera::connection::{Connection, Event};
use hj_makera::discovery::{self, DEFAULT_TCP_PORT};
use hj_makera::frame::{Frame, FrameType};
use hj_makera::message::Message;
use hj_makera::nusb::MaybeFuture;
use hj_makera::status::Status;
use hj_makera::transfer;
use hj_makera::transport::{self, Link, usb};
use tokio::io::{AsyncBufReadExt, BufReader};

mod probe;
mod session;

#[derive(Parser)]
#[command(name = "hj", about = "Low-level Makera machine protocol tool")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Listen for WiFi discovery broadcasts.
    Discover {
        /// Stop after this many seconds (0 = run until interrupted).
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
    /// List attached Z1 USB devices and their descriptors.
    Usb,
    /// Connect, ask the machine to identify itself, print everything it says, and exit.
    Query {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        log: LogArgs,
        /// How long to wait for replies, in seconds.
        #[arg(long, default_value_t = 3)]
        wait: u64,
    },
    /// Upload a file to the machine's SD card.
    Upload {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        log: LogArgs,
        /// Local file to upload.
        file: PathBuf,
        /// Destination path on the machine (default: /sd/gcodes/<file name>).
        remote: Option<String>,
    },
    /// Download a file from the machine's SD card.
    Download {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        log: LogArgs,
        /// Path on the machine, e.g. /sd/gcodes/part.nc.
        remote: String,
        /// Where to write it (default: the file name, in the current directory).
        out: Option<PathBuf>,
        /// Skip the transfer if the machine's copy matches this local file.
        #[arg(long)]
        unless_same_as: Option<PathBuf>,
    },
    /// Probe a cylinder lying along X (e.g. the tailstock) from both Y sides and report its centre.
    /// Start with the probe in the spindle, idle, somewhere above the part. Read-only: no work
    /// offsets are changed. Ctrl-C halts the machine.
    ProbeCylinderY {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        log: LogArgs,
        #[command(flatten)]
        params: probe::CylinderYParams,
    },
    /// Interactive console: lines from stdin are sent as commands. Type /help for extras.
    Console {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        log: LogArgs,
        /// Poll status every N milliseconds (0 = don't poll).
        #[arg(long, default_value_t = 500)]
        poll_ms: u64,
        /// Which status reports to print.
        #[arg(long, value_enum, default_value_t = StatusDisplay::Changes)]
        status: StatusDisplay,
    },
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct Target {
    /// Connect over WiFi: HOST or HOST:PORT (default port 2222).
    #[arg(long, value_name = "HOST[:PORT]")]
    tcp: Option<String>,
    /// Connect over USB.
    #[arg(long)]
    usb: bool,
    /// Connect to the first machine found by WiFi discovery.
    #[arg(long)]
    discover: bool,
}

#[derive(Args)]
struct LogArgs {
    /// Append every frame sent and received to this file as JSON lines.
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StatusDisplay {
    All,
    Changes,
    None,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().command {
        Cmd::Discover { seconds } => discover(seconds).await,
        Cmd::Usb => list_usb().await,
        Cmd::Query { target, log, wait } => query(target, log, wait).await,
        Cmd::Upload {
            target,
            log,
            file,
            remote,
        } => upload(target, log, file, remote).await,
        Cmd::Download {
            target,
            log,
            remote,
            out,
            unless_same_as,
        } => download(target, log, remote, out, unless_same_as).await,
        Cmd::ProbeCylinderY {
            target,
            log,
            params,
        } => {
            let log = FrameLog::open(&log)?;
            let conn = Connection::start(open(&target).await?);
            eprintln!("connected: {}", conn.description);
            let mut s = session::Session::new(conn, log);
            let halt = s.sender();
            tokio::select! {
                result = probe::cylinder_y_center(&mut s, &params) => result,
                _ = tokio::signal::ctrl_c() => {
                    let _ = halt.send(Realtime::Halt.frame()).await;
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    bail!("interrupted: machine halted (clear with $X)");
                }
            }
        }
        Cmd::Console {
            target,
            log,
            poll_ms,
            status,
        } => console(target, log, poll_ms, status).await,
    }
}

async fn discover(seconds: u64) -> Result<()> {
    let listener = discovery::listen().context("binding UDP 3333")?;
    eprintln!(
        "listening for discovery broadcasts on UDP {}",
        discovery::DISCOVERY_PORT
    );
    let run = async {
        loop {
            let (announcement, raw, sender) = listener.recv().await?;
            match announcement {
                Some(a) => println!(
                    "{:<24} {:<22} busy={} state={} (advertised ip {})",
                    a.name,
                    a.addr,
                    a.busy,
                    a.state.as_deref().unwrap_or("-"),
                    a.advertised_ip
                ),
                None => println!("{sender}: unrecognized {:?}", String::from_utf8_lossy(&raw)),
            }
        }
    };
    if seconds == 0 {
        run.await
    } else {
        match tokio::time::timeout(Duration::from_secs(seconds), run).await {
            Ok(r) => r,
            Err(_) => Ok(()),
        }
    }
}

async fn list_usb() -> Result<()> {
    let devices = usb::list().await?;
    if devices.is_empty() {
        println!(
            "no {:04x}:{:04x} devices found",
            usb::VENDOR_ID,
            usb::PRODUCT_ID
        );
    }
    for info in devices {
        println!(
            "{:04x}:{:04x} bus {} addr {} speed {:?}",
            info.vendor_id(),
            info.product_id(),
            info.bus_id(),
            info.device_address(),
            info.speed()
        );
        println!(
            "  manufacturer {:?} product {:?} serial {:?}",
            info.manufacturer_string(),
            info.product_string(),
            info.serial_number()
        );
        println!(
            "  class {:#04x}/{:#04x}/{:#04x}, usb {:#06x}, device version {:#06x}",
            info.class(),
            info.subclass(),
            info.protocol(),
            info.usb_version(),
            info.device_version()
        );
        let device = match info.open().wait() {
            Ok(d) => d,
            Err(e) => {
                println!("  (could not open to read descriptors: {e})");
                continue;
            }
        };
        for config in device.configurations() {
            println!("  configuration {}", config.configuration_value());
            for alt in config.interface_alt_settings() {
                println!(
                    "    interface {} alt {} class {:#04x}/{:#04x}/{:#04x}",
                    alt.interface_number(),
                    alt.alternate_setting(),
                    alt.class(),
                    alt.subclass(),
                    alt.protocol()
                );
                for ep in alt.endpoints() {
                    println!(
                        "      endpoint {:#04x} {:?} {:?} max packet {}",
                        ep.address(),
                        ep.direction(),
                        ep.transfer_type(),
                        ep.max_packet_size()
                    );
                }
            }
        }
    }
    Ok(())
}

async fn open(target: &Target) -> Result<Link> {
    if target.usb {
        return usb::open(None).await.context("opening USB device");
    }
    let addr = if let Some(host) = &target.tcp {
        resolve(host)?
    } else {
        eprintln!("waiting for a discovery broadcast...");
        let listener = discovery::listen().context("binding UDP 3333")?;
        let a = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let (Some(a), ..) = listener.recv().await? {
                    return anyhow::Ok(a);
                }
            }
        })
        .await
        .context("no machine found within 10s")??;
        eprintln!(
            "found {} at {}{}",
            a.name,
            a.addr,
            if a.busy { " (busy)" } else { "" }
        );
        a.addr
    };
    transport::connect_tcp(addr)
        .await
        .with_context(|| format!("connecting to {addr}"))
}

fn resolve(host: &str) -> Result<SocketAddr> {
    let with_port = if host.contains(':') {
        host.to_owned()
    } else {
        format!("{host}:{DEFAULT_TCP_PORT}")
    };
    with_port
        .to_socket_addrs()?
        .next()
        .with_context(|| format!("could not resolve {host}"))
}

/// JSON-lines log of every frame in both directions.
struct FrameLog {
    out: Option<BufWriter<File>>,
    start: Instant,
}

impl FrameLog {
    fn open(args: &LogArgs) -> Result<Self> {
        let out = match &args.log {
            Some(path) => Some(BufWriter::new(
                File::options()
                    .create(true)
                    .append(true)
                    .open(path)
                    .with_context(|| format!("opening {}", path.display()))?,
            )),
            None => None,
        };
        Ok(Self {
            out,
            start: Instant::now(),
        })
    }

    fn record(&mut self, dir: &str, frame: &Frame) {
        let Some(out) = &mut self.out else { return };
        let mut entry = serde_json::json!({
            "t": self.start.elapsed().as_secs_f64(),
            "dir": dir,
            "type": format!("{:#04x}", frame.kind.0),
            "name": frame.kind.name(),
        });
        match std::str::from_utf8(&frame.payload) {
            Ok(text)
                if !text
                    .chars()
                    .any(|c| c.is_control() && !"\r\n\t".contains(c)) =>
            {
                entry["text"] = text.into();
            }
            _ => entry["hex"] = hex(&frame.payload).into(),
        }
        let _ = writeln!(out, "{entry}");
        let _ = out.flush();
    }

    fn note(&mut self, what: &str) {
        let Some(out) = &mut self.out else { return };
        let entry = serde_json::json!({ "t": self.start.elapsed().as_secs_f64(), "note": what });
        let _ = writeln!(out, "{entry}");
        let _ = out.flush();
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Sends a frame, logging it.
async fn send(conn: &Connection, log: &mut FrameLog, frame: Frame) -> Result<()> {
    log.record("tx", &frame);
    conn.sender.send(frame).await?;
    Ok(())
}

async fn query(target: Target, log: LogArgs, wait: u64) -> Result<()> {
    let mut log = FrameLog::open(&log)?;
    let mut conn = Connection::start(open(&target).await?);
    eprintln!("connected: {}", conn.description);
    log.note(&format!("connected: {}", conn.description));

    send(&conn, &mut log, Realtime::Status.frame()).await?;
    for cmd in ["version", "model", "ftype", "diagnose"] {
        send(&conn, &mut log, command::command(cmd)).await?;
    }

    let deadline = tokio::time::sleep(Duration::from_secs(wait));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            event = conn.events.recv() => {
                let Some(event) = event else { break };
                if !print_event(&event, &mut log, StatusDisplay::All, &mut None) {
                    break;
                }
            }
        }
    }
    Ok(())
}

async fn upload(target: Target, log: LogArgs, file: PathBuf, remote: Option<String>) -> Result<()> {
    let data = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
    let remote = match remote {
        Some(r) => r,
        None => format!(
            "/sd/gcodes/{}",
            file.file_name().context("no file name")?.to_string_lossy()
        ),
    };
    let block_size = if target.usb {
        transfer::USB_BLOCK_SIZE
    } else {
        transfer::TCP_BLOCK_SIZE
    };

    let mut log = FrameLog::open(&log)?;
    let mut conn = Connection::start(open(&target).await?);
    eprintln!(
        "connected: {}; uploading {} bytes to {remote} (md5 {})",
        conn.description,
        data.len(),
        transfer::md5_hex(&data)
    );
    let started = Instant::now();
    // Unrelated events (console output, status) arriving mid-transfer are printed afterwards.
    // Transfer frames themselves are not logged.
    let mut seen = Vec::new();
    let result = transfer::upload(
        &mut conn,
        &remote,
        &data,
        block_size,
        |sent, total| eprint!("\r{sent}/{total} blocks"),
        |event| seen.push(event),
    )
    .await;
    eprintln!();
    for event in &seen {
        print_event(event, &mut log, StatusDisplay::None, &mut None);
    }
    result?;
    eprintln!(
        "done in {:.2}s ({:.1} KiB/s)",
        started.elapsed().as_secs_f64(),
        data.len() as f64 / 1024.0 / started.elapsed().as_secs_f64()
    );
    Ok(())
}

async fn download(
    target: Target,
    log: LogArgs,
    remote: String,
    out: Option<PathBuf>,
    unless_same_as: Option<PathBuf>,
) -> Result<()> {
    let out = match out {
        Some(o) => o,
        None => PathBuf::from(remote.rsplit('/').next().context("no file name")?),
    };
    let have_md5 = match &unless_same_as {
        Some(p) => Some(transfer::md5_hex(
            &std::fs::read(p).with_context(|| format!("reading {}", p.display()))?,
        )),
        None => None,
    };
    let mut log = FrameLog::open(&log)?;
    let mut conn = Connection::start(open(&target).await?);
    eprintln!("connected: {}; downloading {remote}", conn.description);
    log.note(&format!("connected: {}", conn.description));
    let started = Instant::now();
    let mut d = transfer::Download::new(&remote, have_md5);
    send(&conn, &mut log, d.start()).await?;
    let result = loop {
        let event = tokio::time::timeout(Duration::from_secs(12), conn.events.recv())
            .await
            .context("no response from the machine")?
            .context("connection closed")?;
        let Event::Frame(frame) = &event else {
            print_event(&event, &mut log, StatusDisplay::None, &mut None);
            continue;
        };
        log.record("rx", frame);
        match d.receive(frame)? {
            transfer::DownloadStep::Send(frames) => {
                for f in frames {
                    send(&conn, &mut log, f).await?;
                }
                if let Some(total) = d.blocks() {
                    eprint!("\r{}/{total} blocks", d.received_blocks);
                }
            }
            transfer::DownloadStep::Done(result) => break result,
            transfer::DownloadStep::Ignored => {}
        }
    };
    eprintln!();
    match result {
        transfer::Downloaded::Unchanged => eprintln!("unchanged; nothing downloaded"),
        transfer::Downloaded::Data(data) => {
            std::fs::write(&out, &data).with_context(|| format!("writing {}", out.display()))?;
            eprintln!(
                "{} bytes to {} in {:.2}s",
                data.len(),
                out.display(),
                started.elapsed().as_secs_f64()
            );
        }
    }
    // Let trailing replies arrive so they're logged.
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(event) = conn.events.try_recv() {
        print_event(&event, &mut log, StatusDisplay::None, &mut None);
    }
    Ok(())
}

async fn console(target: Target, log: LogArgs, poll_ms: u64, display: StatusDisplay) -> Result<()> {
    let mut log = FrameLog::open(&log)?;
    let mut conn = Connection::start(open(&target).await?);
    eprintln!("connected: {} (type /help for help)", conn.description);
    log.note(&format!("connected: {}", conn.description));

    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    let mut poll = tokio::time::interval(Duration::from_millis(poll_ms.max(1)));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_status = None;

    loop {
        tokio::select! {
            _ = poll.tick(), if poll_ms > 0 => {
                send(&conn, &mut log, Realtime::Status.frame()).await?;
            }
            line = stdin.next_line() => {
                let Some(line) = line? else { break };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let frame = match line {
                    "/quit" | "/q" => break,
                    "/help" | "/h" => {
                        eprintln!("{HELP}");
                        continue;
                    }
                    "/?" => Realtime::Status.frame(),
                    "/hold" => Realtime::FeedHold.frame(),
                    "/resume" => Realtime::Resume.frame(),
                    "/halt" => Realtime::Halt.frame(),
                    _ if line.starts_with("/raw ") => match parse_raw(&line[5..]) {
                        Ok(f) => f,
                        Err(e) => {
                            eprintln!("{e}");
                            continue;
                        }
                    },
                    _ if line.starts_with('/') => {
                        eprintln!("unknown command {line}; /help for help");
                        continue;
                    }
                    _ => command::command(line),
                };
                send(&conn, &mut log, frame).await?;
            }
            event = conn.events.recv() => {
                let Some(event) = event else { break };
                if !print_event(&event, &mut log, display, &mut last_status) {
                    break;
                }
            }
        }
    }
    Ok(())
}

const HELP: &str = "\
Lines are sent to the machine as commands (G-code or console commands like `version`).
  /?          request one status report
  /hold       feed hold (!)
  /resume     resume (~)
  /halt       halt (Ctrl-X); clear with $X or M999
  /raw T HEX  send a frame of type T (hex byte) with a hex payload, e.g. `/raw a1 3f`
  /quit       exit";

fn parse_raw(args: &str) -> Result<Frame> {
    let mut parts = args.split_whitespace();
    let kind = parts.next().context("usage: /raw TYPE [HEXPAYLOAD]")?;
    let kind = u8::from_str_radix(kind.trim_start_matches("0x"), 16).context("bad type byte")?;
    let payload = parts.collect::<String>();
    if payload.len() % 2 != 0 {
        bail!("hex payload must have an even number of digits");
    }
    let payload = (0..payload.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&payload[i..i + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .context("bad hex payload")?;
    Ok(Frame::new(FrameType(kind), payload))
}

/// Print an event. Returns false when the connection is gone.
fn print_event(
    event: &Event,
    log: &mut FrameLog,
    display: StatusDisplay,
    last_status: &mut Option<String>,
) -> bool {
    match event {
        Event::Frame(frame) => log.record("rx", frame),
        Event::Message(Message::Status(status)) => {
            let line = format_status(status);
            let show = match display {
                StatusDisplay::All => true,
                StatusDisplay::Changes => last_status.as_ref() != Some(&line),
                StatusDisplay::None => false,
            };
            if show {
                println!("{line}");
            }
            *last_status = Some(line);
        }
        Event::Message(Message::Diagnose(d)) => println!("diagnose: {:?}", d.0),
        Event::Message(Message::Malformed { line, error }) => println!("?? {error}: {line}"),
        Event::Message(Message::Line(line)) => println!("< {line}"),
        Event::Message(Message::Eot) => println!("< EOT"),
        Event::Message(Message::LoadInfo(p)) => {
            println!("load info: {}", String::from_utf8_lossy(p))
        }
        Event::Message(Message::LoadFinish) => println!("load finish"),
        Event::Message(Message::LoadError(p)) => {
            println!("load error: {}", String::from_utf8_lossy(p))
        }
        Event::Message(Message::Other(frame)) => {
            println!(
                "frame {} ({} bytes): {}",
                frame.kind,
                frame.payload.len(),
                hex(&frame.payload)
            )
        }
        Event::DecodeError(e) => {
            println!("!! {e}");
            if let hj_makera::frame::DecodeError::Unframed(bytes) = e {
                println!("   as text: {:?}", String::from_utf8_lossy(bytes));
            }
        }
        Event::Closed(err) => {
            match err {
                Some(e) => eprintln!("connection lost: {e}"),
                None => eprintln!("connection closed by machine"),
            }
            log.note("closed");
            return false;
        }
    }
    true
}

fn format_status(s: &Status) -> String {
    let p = |a: &hj_makera::status::Axes| {
        format!("{:9.3} {:9.3} {:9.3} {:8.3}", a.x(), a.y(), a.z(), a.a())
    };
    let mut out = format!(
        "[{:?}] M {} | W {}",
        s.state,
        p(&s.machine_pos),
        p(&s.work_pos)
    );
    if let Some(f) = &s.feed {
        out += &format!(
            " | F {:.0}/{:.0} {:.0}%",
            f.current, f.requested, f.override_pct
        );
    }
    if let Some(sp) = &s.spindle {
        out += &format!(" | S {:.0}/{:.0}", sp.current_rpm, sp.target_rpm);
        if let Some(t) = sp.spindle_temp {
            out += &format!(" {t:.0}°C");
        }
    }
    if let Some(t) = &s.tool {
        out += &format!(" | T{} {:.3}", t.number, t.offset);
        if let Some(target) = t.target.filter(|&target| target >= 0 && target != t.number) {
            out += &format!(" -> T{target}");
        }
    }
    if let Some(v) = s.probe_voltage {
        out += &format!(" | probe {v:.2}V");
    }
    if let Some(p) = &s.progress {
        out += &format!(" | job {}% line {} {}s", p.percent, p.lines, p.elapsed_secs);
    }
    if let Some(h) = s.halt_reason {
        out += &format!(" | HALT {h:?} ({:?})", h.recovery());
    }
    if let Some(m) = &s.machine {
        out += &format!(" | {}", m.model_name());
    }
    for (k, v) in &s.extra {
        out += &format!(" | {k}:{v}");
    }
    out
}

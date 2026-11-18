use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use mp305::client::default_cell_voltage;
use mp305::commands::SystemSetCommand;
use mp305::transport::HidTransport;
use mp305::types::{ChargeInfo, ChargeState, HardwareInfo, State};
use mp305::{BatteryType, ChargeSettings, Mode, Mp305, OutputSettings};

#[derive(Parser)]
#[command(
    name = "mp305",
    version,
    about = "Control an ISDT MP305 bench power supply over USB"
)]
struct Cli {
    /// HID device path (see `mp305 list`); defaults to the first MP305 found.
    #[arg(long, global = true)]
    device: Option<String>,
    /// Reply timeout in milliseconds.
    #[arg(long, global = true, default_value_t = 1500)]
    timeout: u64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List connected MP305 HID interfaces.
    List,
    /// Device name, id, hardware/boot/firmware versions.
    Info {
        #[arg(long)]
        json: bool,
    },
    /// One-shot readings and setpoints.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Set voltage / current limit / output (unspecified values are kept).
    Set(SetArgs),
    /// Turn the output on.
    On,
    /// Turn the output off.
    Off,
    /// Hand control back to the front panel.
    Release,
    /// Switch operating mode.
    Mode { mode: ModeArg },
    /// Live readings; optionally log to CSV or emit JSON lines.
    Watch(WatchArgs),
    /// Show or change system settings.
    Settings {
        #[command(subcommand)]
        cmd: Option<SettingsCmd>,
    },
    /// Battery charging.
    Charge {
        #[command(subcommand)]
        cmd: ChargeCmd,
    },
}

#[derive(Args)]
struct SetArgs {
    /// Output voltage in volts (0–30).
    #[arg(short, long)]
    voltage: Option<f64>,
    /// Current limit in amps (0–5).
    #[arg(short = 'i', long)]
    current: Option<f64>,
    /// Turn the output on.
    #[arg(long, conflicts_with = "off")]
    on: bool,
    /// Turn the output off.
    #[arg(long)]
    off: bool,
    /// Over-current trips the output off (OCP) instead of limiting.
    #[arg(long, conflicts_with = "cc")]
    ocp: bool,
    /// Over-current is limited (constant-current mode).
    #[arg(long)]
    cc: bool,
    /// Cycle the output after applying so a lowered current limit takes effect immediately.
    #[arg(long)]
    reapply: bool,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ModeArg {
    Dc,
    Charge,
}

#[derive(Args)]
struct WatchArgs {
    /// Sample interval in milliseconds.
    #[arg(long, default_value_t = 500)]
    interval: u64,
    /// Stop after this many samples.
    #[arg(long)]
    count: Option<u64>,
    /// Append samples to this CSV file.
    #[arg(long)]
    csv: Option<PathBuf>,
    /// Print one JSON object per sample instead of the live line.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum SettingsCmd {
    /// Show system settings (default).
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Change system settings (read-modify-write; unspecified fields are kept).
    Set {
        #[arg(long)]
        power_limit: Option<u8>,
        #[arg(long)]
        volume: Option<u8>,
        #[arg(long)]
        screen_off: Option<u8>,
        #[arg(long)]
        shutdown: Option<u8>,
        #[arg(long)]
        screen_direction: Option<u8>,
    },
}

#[derive(Subcommand)]
enum ChargeCmd {
    /// Charge progress and stored charge settings.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Switch to charge mode and start charging (unspecified values come from the device).
    Start {
        #[arg(long = "type", value_enum)]
        battery_type: Option<BatteryType>,
        #[arg(long)]
        cells: Option<u8>,
        /// Charge current in amps.
        #[arg(long)]
        current: Option<f64>,
        /// Full-charge voltage per cell (defaults to the chemistry's standard value).
        #[arg(long)]
        cell_voltage: Option<f64>,
        /// Skip the confirmation prompt.
        #[arg(short, long)]
        yes: bool,
    },
    /// Stop charging.
    Stop,
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn open(cli: &Cli) -> Result<Mp305<HidTransport>> {
    let mut psu = Mp305::open_usb(cli.device.as_deref())?;
    psu.set_timeout(Duration::from_millis(cli.timeout));
    Ok(psu)
}

fn run(cli: Cli) -> Result<()> {
    match &cli.cmd {
        Cmd::List => {
            let list = HidTransport::list()?;
            if list.is_empty() {
                println!("no MP305 found");
            }
            for d in list {
                println!(
                    "{}  {} (pid 0x{:04X}, usage {:#04x}/{:#04x})",
                    d.path,
                    d.product.as_deref().unwrap_or("?"),
                    d.product_id,
                    d.usage_page,
                    d.usage
                );
            }
        }
        Cmd::Info { json } => {
            let info = open(&cli)?.info()?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                print_info(&info);
            }
        }
        Cmd::Status { json } => {
            let mut psu = open(&cli)?;
            let info = psu.info()?;
            let st = psu.state()?;
            if *json {
                let v = serde_json::json!({ "device": info, "state": st });
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!("{} — firmware {}", info.device_name, info.firmware_version);
                print_state(&st);
            }
        }
        Cmd::Set(a) => {
            if a.voltage.is_none() && a.current.is_none() && !a.on && !a.off && !a.ocp && !a.cc {
                anyhow::bail!(
                    "nothing to set — pass --voltage, --current, --on/--off or --ocp/--cc"
                );
            }
            let o = OutputSettings {
                voltage: a.voltage,
                current: a.current,
                on: if a.on {
                    Some(true)
                } else if a.off {
                    Some(false)
                } else {
                    None
                },
                ocp: if a.ocp {
                    Some(true)
                } else if a.cc {
                    Some(false)
                } else {
                    None
                },
                reapply: a.reapply,
            };
            let st = open(&cli)?.set_output(&o)?;
            print_state(&st);
        }
        Cmd::On | Cmd::Off => {
            let on = matches!(cli.cmd, Cmd::On);
            let st = open(&cli)?.set_output(&OutputSettings {
                on: Some(on),
                ..Default::default()
            })?;
            print_state(&st);
        }
        Cmd::Release => {
            open(&cli)?.release()?;
            println!("control returned to the front panel");
        }
        Cmd::Mode { mode } => {
            let target = match mode {
                ModeArg::Dc => Mode::Dc,
                ModeArg::Charge => Mode::Charge,
            };
            open(&cli)?.set_mode(target)?;
            println!("mode: {target} (remote control held; `mp305 release` returns to the panel)");
        }
        Cmd::Watch(a) => watch(&cli, a)?,
        Cmd::Settings { cmd } => settings(&cli, cmd.as_ref())?,
        Cmd::Charge { cmd } => charge(&cli, cmd)?,
    }
    Ok(())
}

fn print_info(i: &HardwareInfo) {
    println!("device:    {}", i.device_name);
    println!("device id: {}", i.device_id);
    println!("firmware:  {}", i.firmware_version);
    println!("hardware:  {}", i.hardware_version);
    println!("boot:      {}", i.boot_version);
}

fn hms(s: u32) -> String {
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn print_state(s: &State) {
    println!("mode:      {}", s.mode);
    println!("output:    {}", if s.output { "ON" } else { "off" });
    println!(
        "measured:  {:6.2} V  {:6.3} A  {:7.2} W",
        s.voltage, s.current, s.power
    );
    println!(
        "setpoint:  {:6.2} V  {:6.3} A  ({})",
        s.set_voltage,
        s.set_current,
        if s.current_over == 1 {
            "OCP trip"
        } else {
            "CC limit"
        }
    );
    println!(
        "energy:    {:.3} Wh over {}",
        s.energy_wh,
        hms(s.working_time_s)
    );
    println!("temp:      {} °C", s.temperature_c);
    if !s.errors.is_empty() {
        println!("errors:    {}", s.errors.join(", "));
    }
}

fn watch(cli: &Cli, a: &WatchArgs) -> Result<()> {
    let mut psu = open(cli)?;
    let mut csv = match &a.csv {
        Some(p) => {
            let new = !p.exists() || std::fs::metadata(p)?.len() == 0;
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .with_context(|| format!("opening {}", p.display()))?;
            if new {
                writeln!(
                    f,
                    "timestamp,elapsed_s,voltage_v,current_a,power_w,energy_wh,set_voltage_v,set_current_a,output,mode,temperature_c,errors"
                )?;
            }
            Some(f)
        }
        None => None,
    };
    let tty = std::io::stdout().is_terminal() && !a.json;
    let start = Instant::now();
    let interval = Duration::from_millis(a.interval.max(50));
    let mut n = 0u64;
    loop {
        let tick = Instant::now();
        let st = psu.state()?;
        let now = chrono::Local::now();
        let elapsed = start.elapsed().as_secs_f64();
        if let Some(f) = csv.as_mut() {
            writeln!(
                f,
                "{},{:.3},{:.2},{:.3},{:.2},{:.3},{:.2},{:.3},{},{},{},\"{}\"",
                now.to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
                elapsed,
                st.voltage,
                st.current,
                st.power,
                st.energy_wh,
                st.set_voltage,
                st.set_current,
                st.output as u8,
                st.mode,
                st.temperature_c,
                st.errors.join("; ")
            )?;
            f.flush()?;
        }
        if a.json {
            let v = serde_json::json!({ "timestamp": now.to_rfc3339(), "elapsed_s": elapsed, "state": st });
            println!("{v}");
        } else {
            let line = format!(
                "{}  {:6.2} V  {:6.3} A  {:7.2} W  {:8.3} Wh  {:>3} °C  out {}  set {:.2} V / {:.3} A{}",
                now.format("%H:%M:%S"),
                st.voltage,
                st.current,
                st.power,
                st.energy_wh,
                st.temperature_c,
                if st.output { "ON " } else { "off" },
                st.set_voltage,
                st.set_current,
                if st.errors.is_empty() {
                    String::new()
                } else {
                    format!("  ! {}", st.errors.join(", "))
                }
            );
            if tty {
                print!("\r\x1b[2K{line}");
                std::io::stdout().flush()?;
            } else {
                println!("{line}");
            }
        }
        n += 1;
        if a.count.is_some_and(|c| n >= c) {
            break;
        }
        std::thread::sleep(interval.saturating_sub(tick.elapsed()));
    }
    if tty {
        println!();
    }
    Ok(())
}

fn settings(cli: &Cli, cmd: Option<&SettingsCmd>) -> Result<()> {
    let mut psu = open(cli)?;
    let cur = psu.system_settings()?;
    match cmd {
        None | Some(SettingsCmd::Show { json: false }) => {
            println!("power limit:      {}", cur.power_limit);
            println!("volume:           {}", cur.volume);
            println!("screen off:       {}", cur.screen_off);
            println!("auto shutdown:    {}", cur.shutdown);
            println!("screen direction: {}", cur.screen_direction);
            println!("slope steps:      {}", cur.slope_steps);
            println!("current over:     {}", cur.current_over);
            if let Some(u) = cur.usb_line {
                println!("usb line:         {u}");
            }
        }
        Some(SettingsCmd::Show { json: true }) => {
            println!("{}", serde_json::to_string_pretty(&cur)?)
        }
        Some(SettingsCmd::Set {
            power_limit,
            volume,
            screen_off,
            shutdown,
            screen_direction,
        }) => {
            let mut s = SystemSetCommand::from(&cur);
            if let Some(v) = power_limit {
                s.power_limit = *v;
            }
            if let Some(v) = volume {
                s.volume = *v;
            }
            if let Some(v) = screen_off {
                s.screen_off = *v;
            }
            if let Some(v) = shutdown {
                s.shutdown = *v;
            }
            if let Some(v) = screen_direction {
                s.screen_direction = *v;
            }
            psu.set_system_settings(&s)?;
            let after = psu.system_settings()?;
            println!("{}", serde_json::to_string_pretty(&after)?);
        }
    }
    Ok(())
}

fn print_charge(cs: &ChargeState, ci: &ChargeInfo) {
    let ty = |t: Option<BatteryType>| t.map(|t| t.label()).unwrap_or("?");
    if cs.mode == Mode::Charge {
        println!("output:    {}", if cs.output { "CHARGING" } else { "off" });
        println!(
            "battery:   {} {}S  {}%{}",
            ty(cs.battery_type),
            cs.cells,
            cs.percentage,
            if cs.charge_full { " (full)" } else { "" }
        );
        println!(
            "measured:  {:6.2} V  {:6.3} A  {:7.2} W",
            cs.voltage, cs.current, cs.power
        );
        println!(
            "charged:   {} mAh  {:.3} Wh over {}",
            cs.capacity_mah,
            cs.energy_wh,
            hms(cs.working_time_s)
        );
        println!("temp:      {} °C", cs.temperature_c);
        if !cs.errors.is_empty() {
            println!("errors:    {}", cs.errors.join(", "));
        }
    } else {
        println!("not in charge mode (currently {})", cs.mode);
    }
    if ci.battery_type == Some(BatteryType::Nimh) {
        println!(
            "settings:  NiMH/Cd  target raw {}  {:.3} A",
            ci.capacity_voltage_raw, ci.current
        );
    } else {
        println!(
            "settings:  {} {}S  {:.3} V/cell  {:.3} A",
            ty(ci.battery_type),
            ci.cells,
            ci.cell_voltage(),
            ci.current
        );
    }
}

fn charge(cli: &Cli, cmd: &ChargeCmd) -> Result<()> {
    let mut psu = open(cli)?;
    match cmd {
        ChargeCmd::Status { json } => {
            let cs = psu.charge_state()?;
            let ci = psu.charge_settings()?;
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({ "state": cs, "settings": ci })
                    )?
                );
            } else {
                print_charge(&cs, &ci);
            }
        }
        ChargeCmd::Start {
            battery_type,
            cells,
            current,
            cell_voltage,
            yes,
        } => {
            let stored = psu.charge_settings()?;
            let ty = battery_type
                .or(stored.battery_type)
                .unwrap_or(BatteryType::Lipo);
            if ty == BatteryType::Nimh {
                anyhow::bail!(
                    "NiMH/Cd charging isn't supported yet (its encoding is unverified); pass --type for a lithium/LiFe/Pb pack"
                );
            }
            let cv = cell_voltage.unwrap_or(
                if Some(ty) == stored.battery_type && stored.cell_voltage() > 1.0 {
                    stored.cell_voltage()
                } else {
                    default_cell_voltage(ty)
                },
            );
            let n = cells.unwrap_or(stored.cells).max(1);
            if !(1.0..=4.5).contains(&cv) {
                anyhow::bail!("per-cell voltage {cv} V looks wrong; pass --cell-voltage");
            }
            let i = current.unwrap_or(stored.current);
            eprintln!(
                "About to charge: {} {}S to {:.3} V/cell ({:.2} V pack) at {:.3} A.",
                ty.label(),
                n,
                cv,
                cv * n as f64,
                i
            );
            eprintln!(
                "Note: charge control is not hardware-verified upstream — watch the PSU's screen."
            );
            if !yes {
                eprint!("Proceed? [y/N] ");
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if !matches!(line.trim(), "y" | "Y" | "yes") {
                    anyhow::bail!("aborted");
                }
            }
            psu.start_charge(&ChargeSettings {
                battery_type: Some(ty),
                cells: Some(n),
                current: Some(i),
                cell_voltage: Some(cv),
            })?;
            std::thread::sleep(Duration::from_millis(300));
            print_charge(&psu.charge_state()?, &psu.charge_settings()?);
        }
        ChargeCmd::Stop => {
            psu.stop_charge()?;
            println!(
                "charging stopped (still in charge mode; `mp305 mode dc` or `mp305 release` to leave)"
            );
        }
    }
    Ok(())
}

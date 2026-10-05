// SPDX-License-Identifier: GPL-3.0-only
// Play every preset a firmware reaches with the PRESETS encoder:
//   preset_sweep FIRMWARE OUT_DIR [MAX_CLICKS]
// Boots, then per preset (one clockwise click apart, until the LCD footer
// shows the first preset's name again or MAX_CLICKS, default 250): holds a
// four-note chord for 1.5 s of guest time, releases, and prints the footer
// hash and the audio level. A guest fault is written to OUT_DIR/fault-K.txt
// with the last instructions; the sweep then reboots, clicks back to preset
// K (checking its footer) and goes on. OUT_DIR/footers.png stacks the footer
// of every preset, one band per preset, for reading the names.
use fm1_emu::{
    lcd,
    player::{Level, Player},
    png, ui_knobs,
};
use std::{env, fs, path::Path};

const CHORD: [usize; 4] = [18, 20, 22, 23];
const BOOT_SECONDS: f64 = 4.0;
const SETTLE_SECONDS: f64 = 0.3;
const HOLD_SECONDS: f64 = 1.5;
const RELEASE_SECONDS: f64 = 0.3;
/// The preset name row at the bottom of the screen.
const FOOTER_TOP: usize = 218;
const FOOTER_ROWS: usize = lcd::HEIGHT - FOOTER_TOP;
/// Peak below which a held chord counts as silent.
const SILENT: f64 = 0.001;
const TRACE: usize = 200;

struct Preset {
    footer: Vec<u32>,
    hash: u64,
    level: Option<Level>,
    fault: Option<String>,
}

fn footer(player: &Player) -> Vec<u32> {
    player.cpu.bus.lcd.pixels[FOOTER_TOP * lcd::WIDTH..].to_vec()
}

fn hash(pixels: &[u32]) -> u64 {
    pixels.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &p| {
        (h ^ p as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// One click clockwise, then let the screen follow.
fn click(player: &mut Player) -> Result<(), String> {
    player.encoders.turn(ui_knobs::knob::PRESETS, 1);
    player.settle_encoders(1.0)?;
    player.run_seconds(SETTLE_SECONDS)
}

fn play(player: &mut Player) -> Result<Level, String> {
    for id in CHORD {
        player.held[id] = true;
    }
    let level = player.level(HOLD_SECONDS)?;
    player.held = [false; 41];
    player.run_seconds(RELEASE_SECONDS)?;
    Ok(level)
}

/// A fresh boot clicked forward to preset `k`.
fn reach(firmware: &Path, k: usize) -> Result<Player, String> {
    let mut player = Player::boot(firmware, TRACE)?;
    player.run_seconds(BOOT_SECONDS)?;
    for _ in 0..k {
        click(&mut player)?;
    }
    Ok(player)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if !(2..=3).contains(&args.len()) {
        return Err("usage: preset_sweep FIRMWARE OUT_DIR [MAX_CLICKS]".into());
    }
    let firmware = Path::new(&args[0]);
    let out = Path::new(&args[1]);
    let max: usize = args
        .get(2)
        .map_or(Ok(250), |v| v.parse())
        .map_err(|_| "bad MAX_CLICKS")?;
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;

    let mut presets: Vec<Preset> = Vec::new();
    let mut player = reach(firmware, 0)?;
    let mut wrapped = false;
    println!("preset  footer            frames  rms     peak    result");
    for k in 0..=max {
        if k > 0 {
            if let Err(report) = click(&mut player) {
                return Err(format!("fault while selecting preset {k}:\n{report}"));
            }
        }
        let pixels = footer(&player);
        let h = hash(&pixels);
        if k > 0 && h == presets[0].hash {
            wrapped = true;
            println!("preset {k}: the footer of preset 0 again, stopping");
            break;
        }
        if k == max {
            break;
        }
        let mut preset = Preset {
            footer: pixels,
            hash: h,
            level: None,
            fault: None,
        };
        match play(&mut player) {
            Ok(level) => preset.level = Some(level),
            Err(report) => {
                let fault = report.lines().last().unwrap_or("").to_string();
                let path = out.join(format!("fault-{k}.txt"));
                fs::write(&path, &report).map_err(|e| format!("{}: {e}", path.display()))?;
                preset.fault = Some(fault);
                player = reach(firmware, k)?;
                if hash(&footer(&player)) != h {
                    println!(
                        "preset {k}: after the reboot the footer differs; continuing from there"
                    );
                }
            }
        }
        let result = match (&preset.level, &preset.fault) {
            (_, Some(fault)) => format!("FAULT {fault}"),
            (Some(level), _) if level.peak < SILENT => "silent".to_string(),
            _ => "ok".to_string(),
        };
        let level = preset.level.unwrap_or_default();
        println!(
            "{k:>6}  {:016x}  {:>6}  {:.4}  {:.4}  {result}",
            preset.hash,
            level.frames,
            level.rms(),
            level.peak
        );
        presets.push(preset);
    }

    let sheet: Vec<u32> = presets
        .iter()
        .flat_map(|p| p.footer.iter().copied())
        .collect();
    let bytes = png::encode_rgb(lcd::WIDTH, presets.len() * FOOTER_ROWS, &sheet)?;
    let path = out.join("footers.png");
    fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let faults = presets.iter().filter(|p| p.fault.is_some()).count();
    let silent = presets
        .iter()
        .filter(|p| p.level.is_some_and(|l| l.peak < SILENT))
        .count();
    println!(
        "presets played: {} ({}), faults: {faults}, silent: {silent}",
        presets.len(),
        if wrapped {
            "wrapped to preset 0"
        } else {
            "click limit"
        }
    );
    Ok(())
}

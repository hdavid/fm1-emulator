// SPDX-License-Identifier: GPL-3.0-only
// Write the decoded application image of a package/ELF/raw file for offline
// disassembly. The output starts at the application execution address.
use fm1_emu::firmware::Firmware;
use std::{env, fs, path::Path, process::ExitCode};

fn main() -> ExitCode {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: extract FIRMWARE.{{fwsc,elf,bin}} OUTPUT.bin");
        return ExitCode::FAILURE;
    }
    match Firmware::load(Path::new(&args[0])).and_then(|firmware| {
        fs::write(&args[1], &firmware.image).map_err(|error| error.to_string())?;
        Ok(firmware)
    }) {
        Ok(firmware) => {
            eprintln!(
                "wrote {} bytes; entry 0x{:08x}",
                firmware.image.len(),
                firmware.entry
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("extract: {error}");
            ExitCode::FAILURE
        }
    }
}

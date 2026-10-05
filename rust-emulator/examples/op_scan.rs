// SPDX-License-Identifier: GPL-3.0-only
// Static scan for pi32v2 instructions the interpreter does not decode.
//
//   op_scan FIRMWARE LISTING [--base HEX] [--tsv FILE] [--min-run N]
//
// LISTING is the JieLi objdump -d output for FIRMWARE (scripts/op-scan.sh
// makes it): of the ELF itself (addresses as listed, --base 0), or of the
// extracted application image wrapped in an object (offsets, --base
// 0x02000120). Every instruction objdump decodes is described by the
// interpreter's decoder (`fm1_emu::describe`) and executed once on a scratch
// CPU; the report lists, grouped by encoding, the ones it decodes as
// Unsupported, rejects when executing, gives another length than objdump, or
// that a conditional block would skip with another length.
//
// Code or data: for an ELF, an instruction is code when it lies inside an
// STT_FUNC symbol's range. Without symbols (.fwsc, raw images) it is
// "reachable" when recursive descent reaches it (from the entry, branch and
// call targets, tbb/tbh tables, and pointers to function prologues found in
// the image or in 32-bit immediates), "clean" when it is not reachable but
// lies in a run of at least --min-run instructions objdump decodes with no
// <unknown>, and "data" otherwise.
use fm1_emu::{bus::Bus, cpu::Cpu, cpu::Fault, describe, firmware::Firmware, RAM, XIP};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::{env, fmt::Write as _, fs, path::Path, process::ExitCode};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Class {
    Function,
    Reachable,
    Clean,
    Outside,
    Data,
}

impl Class {
    fn name(self) -> &'static str {
        match self {
            Class::Function => "code (in STT_FUNC)",
            Class::Reachable => "code (reachable)",
            Class::Clean => "likely code (clean run, unreached)",
            Class::Outside => "outside any STT_FUNC (data in .text?)",
            Class::Data => "likely data",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Instruction,
    Unknown,
    Data,
}

struct Line {
    address: u32,
    bytes: Vec<u8>,
    text: String,
    kind: Kind,
    label: usize,
    /// Inside a conditional block (`if (...) {`), where skip lengths matter.
    conditional: bool,
    /// Inside any block (conditional or repeat): no terminator ends the flow.
    nested: bool,
}

struct Listing {
    lines: Vec<Line>,
    labels: Vec<String>,
}

/// Block depth of an instruction's text: objdump indents block contents by
/// one tab per level; a bundle's following slot adds a tab and a space.
fn depth(text_part: &str) -> usize {
    let tabs = text_part.chars().take_while(|&c| c == '\t').count();
    let slot = text_part[tabs..].starts_with(' ');
    tabs.saturating_sub(usize::from(slot))
}

fn parse_listing(text: &str, base: u32) -> Listing {
    let mut lines = Vec::new();
    let mut labels = vec![String::from("?")];
    // conditional[d]: whether the block opened at depth d is an if-block.
    // Depth comes from the indentation, so blocks that objdump never closes
    // (data decoded as an opener) cannot leak into later lines.
    let mut conditional_at: Vec<bool> = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        let Some((head, rest)) = raw.split_once(':') else {
            continue;
        };
        let head_trim = head.trim();
        let is_address = !head_trim.is_empty()
            && head.starts_with(' ')
            && head_trim.bytes().all(|b| b.is_ascii_hexdigit());
        if !is_address {
            if !raw.starts_with(' ')
                && !raw.starts_with('\t')
                && trimmed.ends_with(':')
                && !trimmed.starts_with("Disassembly of section")
            {
                labels.push(trimmed.trim_end_matches(':').to_string());
            }
            continue;
        }
        let address = u32::from_str_radix(head_trim, 16)
            .unwrap_or(0)
            .wrapping_add(base);
        let label = labels.len() - 1;
        if rest.starts_with('\t') {
            // Section data rows: bytes then ASCII.
            conditional_at.clear();
            lines.push(Line {
                address,
                bytes: Vec::new(),
                text: rest.trim().into(),
                kind: Kind::Data,
                label,
                conditional: false,
                nested: false,
            });
            continue;
        }
        let (bytes_part, text_part) = match rest.split_once('\t') {
            Some((b, t)) => (b, t),
            None => {
                // "<unknown instruction>" and "< 5 : 0x5 >" rows have no tab.
                let at = rest.find('<').unwrap_or(rest.len());
                (&rest[..at], &rest[at..])
            }
        };
        let bytes: Vec<u8> = bytes_part
            .split_whitespace()
            .map_while(|t| u8::from_str_radix(t, 16).ok())
            .collect();
        let level = if rest.contains('\t') {
            depth(text_part)
        } else {
            0
        };
        conditional_at.truncate(level);
        let conditional = conditional_at.iter().any(|&c| c);
        let nested = level > 0;
        let text = text_part.trim().to_string();
        let kind = if text.contains("<unknown") {
            Kind::Unknown
        } else if text.starts_with('<') || bytes.len() % 2 == 1 || bytes.is_empty() {
            Kind::Data
        } else {
            Kind::Instruction
        };
        if kind == Kind::Instruction && text.trim_end_matches('#').trim_end().ends_with('{') {
            conditional_at.resize(level, false);
            conditional_at.push(text.starts_with("if"));
        }
        lines.push(Line {
            address,
            bytes,
            text,
            kind,
            label,
            conditional,
            nested,
        });
    }
    Listing { lines, labels }
}

/// STT_FUNC ranges (start, end, name) of an ELF32 file.
fn elf_functions(data: &[u8]) -> Vec<(u32, u32, String)> {
    let u16_at = |o: usize| {
        data.get(o..o + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
    };
    let u32_at = |o: usize| {
        data.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut out = Vec::new();
    if !data.starts_with(b"\x7fELF") {
        return out;
    }
    let (Some(shoff), Some(shentsize), Some(shnum)) = (u32_at(32), u16_at(46), u16_at(48)) else {
        return out;
    };
    for i in 0..shnum {
        let sh = shoff as usize + i * shentsize;
        if u32_at(sh + 4) != Some(2) {
            continue; // SHT_SYMTAB
        }
        let (Some(off), Some(size), Some(link)) =
            (u32_at(sh + 16), u32_at(sh + 20), u32_at(sh + 24))
        else {
            continue;
        };
        let strtab = shoff as usize + link as usize * shentsize;
        let Some(stroff) = u32_at(strtab + 16) else {
            continue;
        };
        for entry in (off as usize..(off + size) as usize).step_by(16) {
            let (Some(name), Some(value), Some(length)) =
                (u32_at(entry), u32_at(entry + 4), u32_at(entry + 8))
            else {
                break;
            };
            let info = data.get(entry + 12).copied().unwrap_or(0);
            if info & 15 != 2 || length == 0 {
                continue;
            }
            let start = stroff as usize + name as usize;
            let end = data[start..]
                .iter()
                .position(|&b| b == 0)
                .map_or(start, |n| start + n);
            out.push((
                value,
                value + length,
                String::from_utf8_lossy(&data[start..end]).into(),
            ));
        }
    }
    out.sort();
    out
}

/// The absolute target in objdump's "<sym+0x.. : ADDR >" annotation.
fn annotated_target(text: &str, base: u32) -> Option<u32> {
    let end = text.rfind(" >")?;
    let start = text[..end].rfind(": ")? + 2;
    u32::from_str_radix(text[start..end].trim(), 16)
        .ok()
        .map(|t| t.wrapping_add(base))
}

fn ends_flow(text: &str) -> bool {
    let t = text.trim_end_matches('#').trim();
    t.starts_with("goto ")
        || t == "rts"
        || t == "rti"
        || t.starts_with("{pc")
        || t.starts_with("pc =")
        || t.starts_with("tbb ")
        || t.starts_with("tbh ")
}

fn is_prologue(text: &str) -> bool {
    text.starts_with("[--sp] = {rets")
}

/// Instruction lines by address.
fn instruction_index(listing: &Listing) -> HashMap<u32, usize> {
    listing
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind == Kind::Instruction)
        .map(|(i, l)| (l.address, i))
        .collect()
}

/// Root candidates: the entry and prologues pointed to by aligned image words
/// or 32-bit immediates are strong; code right after the end of another flow
/// (a leaf function has no prologue) is weak and must pass `closure`'s check.
fn roots(
    listing: &Listing,
    index: &HashMap<u32, usize>,
    image: &[u8],
    entry: u32,
) -> (Vec<u32>, Vec<u32>) {
    let (mut strong, mut weak) = (vec![entry], Vec::new());
    let mut pointer = |value: u32| {
        let Some(&i) = index.get(&value) else { return };
        let after_end = i > 0 && {
            let previous = &listing.lines[i - 1];
            previous.kind == Kind::Instruction
                && !previous.nested
                && ends_flow(&previous.text)
                && previous.address + previous.bytes.len() as u32 == value
        };
        if is_prologue(&listing.lines[i].text) {
            strong.push(value);
        } else if after_end {
            weak.push(value);
        }
    };
    for chunk in image.as_chunks::<4>().0 {
        pointer(u32::from_le_bytes(*chunk));
    }
    for line in &listing.lines {
        if line.kind == Kind::Instruction && line.bytes.len() == 6 {
            pointer(u32::from_le_bytes([
                line.bytes[2],
                line.bytes[3],
                line.bytes[4],
                line.bytes[5],
            ]));
        }
    }
    (strong, weak)
}

/// Targets of a tbb/tbh table after `line`: forward halfword offsets from the
/// table, which ends where the first target begins.
fn table_targets(line: &Line, image: &[u8]) -> Vec<u32> {
    let byte = |address: u32| {
        address
            .checked_sub(XIP)
            .and_then(|o| image.get(o as usize))
            .copied()
    };
    let size = if line.text.starts_with("tbb") { 1 } else { 2 };
    let table = line.address + line.bytes.len() as u32;
    let (mut end, mut k, mut out) = (u32::MAX, 0, Vec::new());
    while table + k * size < end && k < 1024 {
        let at = table + k * size;
        let entry = if size == 1 {
            byte(at).map(u32::from)
        } else {
            byte(at)
                .zip(byte(at + 1))
                .map(|(a, b)| u32::from(a) | u32::from(b) << 8)
        };
        let Some(entry) = entry else { break };
        let target = table + entry * 2;
        if target <= at {
            break;
        }
        end = end.min(target);
        out.push(target);
        k += 1;
    }
    out
}

/// Lines reached from `starts` that `seen` does not hold yet, and whether the
/// flow looked like data: it ran into bytes objdump does not decode, into four
/// zero words (nop) in a row, or into a pfetch (a flash pointer's high half).
fn closure(
    listing: &Listing,
    index: &HashMap<u32, usize>,
    image: &[u8],
    base: u32,
    starts: &[u32],
    seen: &BTreeSet<usize>,
) -> (BTreeSet<usize>, bool) {
    let mut reached = BTreeSet::new();
    let mut suspicious = false;
    let mut work: VecDeque<u32> = starts.iter().copied().collect();
    while let Some(start) = work.pop_front() {
        let Some(&first) = index.get(&start) else {
            suspicious = true;
            continue;
        };
        let (mut i, mut zeros) = (first, 0);
        loop {
            if seen.contains(&i) || !reached.insert(i) {
                break;
            }
            let line = &listing.lines[i];
            let text = line.text.as_str();
            zeros = if line.bytes.iter().all(|&b| b == 0) {
                zeros + 1
            } else {
                0
            };
            // "pfetch [rN]" is 0x020N: the high half of a little-endian
            // pointer into flash (0x020N_xxxx), the mark of a pointer table.
            if zeros >= 4 || text.starts_with("pfetch") {
                suspicious = true;
            }
            if text.contains("goto") || text.starts_with("call") {
                if let Some(t) = annotated_target(text, base) {
                    work.push_back(t);
                }
            }
            if text.starts_with("tbb ") || text.starts_with("tbh ") {
                work.extend(table_targets(line, image));
            }
            if !line.nested && ends_flow(text) {
                break;
            }
            let next = line.address + line.bytes.len() as u32;
            match index.get(&next) {
                Some(&n) => i = n,
                None => {
                    suspicious = true;
                    break;
                }
            }
        }
    }
    (reached, suspicious)
}

/// Recursive descent: everything reached from the strong roots, plus each
/// weak root's own closure when that does not look like data.
fn reachable(listing: &Listing, image: &[u8], base: u32, entry: u32) -> BTreeSet<usize> {
    let index = instruction_index(listing);
    let (strong, weak) = roots(listing, &index, image, entry);
    let (mut seen, _) = closure(listing, &index, image, base, &strong, &BTreeSet::new());
    for root in weak {
        let (reached, suspicious) = closure(listing, &index, image, base, &[root], &seen);
        if !suspicious {
            seen.extend(reached);
        }
    }
    seen
}

/// Lines inside runs of at least `min_run` consecutive decoded instructions.
fn clean_runs(listing: &Listing, min_run: usize) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    let mut run: Vec<usize> = Vec::new();
    let flush = |run: &mut Vec<usize>, out: &mut BTreeSet<usize>| {
        if run.len() >= min_run {
            out.extend(run.iter().copied());
        }
        run.clear();
    };
    for (i, line) in listing.lines.iter().enumerate() {
        match line.kind {
            Kind::Instruction => run.push(i),
            _ => flush(&mut run, &mut out),
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Executes single instructions on a scratch CPU; reports execute-time
/// Unsupported faults.
struct Prober {
    cpu: Cpu,
    cache: HashMap<Vec<u16>, bool>,
}

const CODE: u32 = RAM + 0x7_0000;

impl Prober {
    fn new() -> Self {
        Self {
            cpu: Self::fresh(),
            cache: HashMap::new(),
        }
    }

    fn fresh() -> Cpu {
        Cpu::new(Bus::new(vec![0; 64]).unwrap(), XIP)
    }

    /// Whether executing `words` (the slot as the interpreter runs it)
    /// raises Unsupported with either of two register states.
    fn rejects(&mut self, words: &[u16]) -> bool {
        if let Some(&known) = self.cache.get(words) {
            return known;
        }
        let mut rejected = false;
        for small in [false, true] {
            let cpu = &mut self.cpu;
            for (i, w) in words.iter().chain([0u16; 4].iter()).enumerate() {
                cpu.bus.write(CODE + 2 * i as u32, *w as u32, 2).unwrap();
            }
            for i in 0..16 {
                cpu.r[i] = if small {
                    4 + 4 * i as u32
                } else {
                    RAM + 0x2_0000 + 0x100 * i as u32
                };
            }
            cpu.sr[14] = RAM + 0x4_0000;
            cpu.sr[13] = RAM + 0x4_8000;
            cpu.sr[3] = CODE + 0x40;
            cpu.pc = CODE;
            let result = cpu.step();
            let reset = match &result {
                Ok(name) => name.starts_with("repeat") || name.starts_with("lock"),
                Err(_) => true,
            };
            if matches!(result, Err(Fault::Unsupported { .. })) {
                rejected = true;
            }
            if reset {
                self.cpu = Self::fresh();
            }
        }
        self.cache.insert(words.to_vec(), rejected);
        rejected
    }
}

/// Reliability of the symbol-less classes: how many instructions of each
/// class lie inside the STT_FUNC ranges of the ELF the image was built from.
fn check_against(
    elf: &str,
    listing: &Listing,
    class_of: &dyn Fn(usize, u32) -> Class,
) -> Result<(), String> {
    let functions = elf_functions(&fs::read(elf).map_err(|e| e.to_string())?);
    if functions.is_empty() {
        return Err("no STT_FUNC symbols".into());
    }
    let mut table: BTreeMap<Class, (usize, usize)> = BTreeMap::new();
    let mut in_functions = 0;
    let mut stray: Vec<(u32, u32, usize)> = Vec::new();
    for (i, line) in listing.lines.iter().enumerate() {
        if line.kind != Kind::Instruction {
            continue;
        }
        let k = functions.partition_point(|f| f.0 <= line.address);
        let inside = k > 0 && line.address < functions[k - 1].1;
        let class = class_of(i, line.address);
        if class == Class::Reachable && !inside {
            match stray.last_mut() {
                Some((_, end, n)) if *end == line.address => {
                    *end += line.bytes.len() as u32;
                    *n += 1;
                }
                _ => stray.push((line.address, line.address + line.bytes.len() as u32, 1)),
            }
        }
        let entry = table.entry(class).or_default();
        if inside {
            entry.0 += 1;
            in_functions += 1;
        } else {
            entry.1 += 1;
        }
    }
    let code_bytes: u32 = functions
        .iter()
        .filter(|f| f.0 >= XIP)
        .map(|f| f.1 - f.0)
        .sum();
    println!(
        "
## check against {elf}
"
    );
    println!(
        "flash STT_FUNC bytes {code_bytes}; decoded instructions inside them {in_functions}
"
    );
    println!("| class | inside STT_FUNC | outside |");
    println!("|---|---|---|");
    for (class, (inside, outside)) in &table {
        println!("| {} | {inside} | {outside} |", class.name());
    }
    stray.sort_by_key(|r| std::cmp::Reverse(r.2));
    println!(
        "
largest reachable runs outside STT_FUNC:"
    );
    for (start, end, n) in stray.iter().take(8) {
        println!("- 0x{start:08x}-0x{end:08x}: {n} instructions");
    }
    Ok(())
}

#[derive(Default)]
struct Group {
    count: usize,
    words: BTreeSet<u16>,
    examples: Vec<String>,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut positional = Vec::new();
    let (mut base, mut tsv, mut min_run, mut check) = (0u32, None, 32usize, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--base" => {
                base = it
                    .next()
                    .and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                    .unwrap_or(0)
            }
            "--tsv" => tsv = it.next().cloned(),
            "--min-run" => min_run = it.next().and_then(|v| v.parse().ok()).unwrap_or(32),
            "--check-against" => check = it.next().cloned(),
            _ => positional.push(a.clone()),
        }
    }
    if positional.len() != 2 {
        eprintln!("usage: op_scan FIRMWARE LISTING [--base HEX] [--tsv FILE] [--min-run N] [--check-against ELF]");
        return ExitCode::FAILURE;
    }
    let firmware_path = Path::new(&positional[0]);
    let raw = match fs::read(firmware_path) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("op_scan: {}: {e}", firmware_path.display());
            return ExitCode::FAILURE;
        }
    };
    let firmware = match Firmware::load(firmware_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("op_scan: {e}");
            return ExitCode::FAILURE;
        }
    };
    let listing = match fs::read_to_string(&positional[1]) {
        Ok(text) => parse_listing(&text, base),
        Err(e) => {
            eprintln!("op_scan: {}: {e}", positional[1]);
            return ExitCode::FAILURE;
        }
    };
    let functions = elf_functions(&raw);
    let by_symbols = !functions.is_empty();
    let reach = if by_symbols {
        BTreeSet::new()
    } else {
        reachable(&listing, &firmware.image, base, firmware.entry)
    };
    let clean = if by_symbols {
        BTreeSet::new()
    } else {
        clean_runs(&listing, min_run)
    };
    let class_of = |i: usize, address: u32| -> Class {
        if by_symbols {
            let k = functions.partition_point(|f| f.0 <= address);
            if k > 0 && address < functions[k - 1].1 {
                Class::Function
            } else {
                Class::Outside
            }
        } else if reach.contains(&i) {
            Class::Reachable
        } else if clean.contains(&i) {
            Class::Clean
        } else {
            Class::Data
        }
    };
    let function_name = |address: u32, label: usize| -> String {
        let k = functions.partition_point(|f| f.0 <= address);
        if k > 0 && address < functions[k - 1].1 {
            functions[k - 1].2.clone()
        } else {
            listing.labels[label].clone()
        }
    };

    let mut prober = Prober::new();
    let mut totals: BTreeMap<Class, usize> = BTreeMap::new();
    let mut groups: BTreeMap<(Class, &'static str, String), Group> = BTreeMap::new();
    let mut detail = String::new();
    for (i, line) in listing.lines.iter().enumerate() {
        if line.kind != Kind::Instruction {
            continue;
        }
        let class = class_of(i, line.address);
        *totals.entry(class).or_default() += 1;
        let words: Vec<u16> = line
            .bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        let d = describe(&words);
        let length = line.bytes.len() as u32;
        let mut problems: Vec<&'static str> = Vec::new();
        if d.length.is_none() {
            problems.push("unsupported");
        } else {
            if d.length != Some(length) {
                problems.push("length");
            }
            let mut slot = words.clone();
            slot[0] = d.word;
            if d.form != "Rti" && prober.rejects(&slot) {
                problems.push("rejected");
            }
        }
        if line.conditional && d.skip_length != length {
            problems.push("skip-length");
        }
        let h = words[0] as u32;
        let x = words.get(1).copied().unwrap_or(0) as u32;
        for problem in problems {
            let wide = (d.word as u32) >> 13 == 7;
            let key = if wide {
                format!("{:04x}/x&f={:x}", h & 0xfff0, x & 15)
            } else {
                format!("{:04x}", h & 0xff00)
            };
            let group = groups.entry((class, problem, key)).or_default();
            group.count += 1;
            group.words.insert(words[0]);
            let hex: Vec<String> = words.iter().map(|w| format!("{w:04x}")).collect();
            let name = function_name(line.address, line.label);
            if group.examples.len() < 3 {
                group.examples.push(format!(
                    "0x{:08x} {} `{}` ({}; emu {} len {:?}/{})",
                    line.address,
                    hex.join(" "),
                    line.text.trim_end_matches('#').trim(),
                    name,
                    d.form,
                    d.length,
                    length
                ));
            }
            let _ = writeln!(
                detail,
                "{}\t{:08x}\t{}\t{}\t{}\t{}\t{:?}\t{}\t{}",
                class.name(),
                line.address,
                problem,
                hex.join(" "),
                line.text.trim_end_matches('#').trim(),
                d.form,
                d.length,
                length,
                name
            );
        }
    }
    println!("# {}", firmware_path.display());
    println!(
        "mode: {}",
        if by_symbols {
            "ELF, STT_FUNC ranges"
        } else {
            "image, recursive descent + clean runs"
        }
    );
    for (class, n) in &totals {
        println!("decoded instructions, {}: {n}", class.name());
    }
    if groups.is_empty() {
        println!("no gaps");
    }
    let mut current = None;
    for ((class, problem, key), group) in &groups {
        if current != Some(*class) {
            println!("\n## {}\n", class.name());
            println!("| problem | pattern | count | words | example |");
            println!("|---|---|---|---|---|");
            current = Some(*class);
        }
        println!(
            "| {problem} | {key} | {} | {} | {} |",
            group.count,
            group.words.len(),
            group.examples.join("<br>")
        );
    }
    if let Some(elf) = check {
        if let Err(e) = check_against(&elf, &listing, &class_of) {
            eprintln!("op_scan: {elf}: {e}");
            return ExitCode::FAILURE;
        }
    }
    if let Some(path) = tsv {
        if let Err(e) = fs::write(&path, detail) {
            eprintln!("op_scan: {path}: {e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

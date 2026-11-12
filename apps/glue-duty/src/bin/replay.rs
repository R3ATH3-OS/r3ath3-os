//! Native host for the same Glue Duty source the kernel runs as wasm.
//! `replay` prints the `wasm-glue` proof. `replay --title` prints the title
//! screen as 25 rows of 80 VGA cells (`AA GG`, attribute 0x0A).

use glue_duty::logic::{fnv64, Game};

fn main() {
    let mut args = std::env::args();
    let _bin = args.next();
    if args.next().as_deref() == Some("--title") {
        let mut game = Game::fresh();
        game.init(1);
        let grid = game.grid();
        for row in 0..25 {
            for col in 0..80 {
                if col > 0 {
                    print!(" ");
                }
                let glyph = grid[row * 80 + col];
                print!("{:04X}", 0x0A00 | u16::from(glyph));
            }
            println!();
        }
        return;
    }
    let mut game = Game::fresh();
    game.init(1);
    print!("R3ATH3_STAGE rust_main\n");
    print!("R3ATH3_WASM_GLUE start seed=1\n");
    let mut prev = game.state();
    for n in 1u32..=2000 {
        let mut keys = [0u8; 4];
        let len = proof_keys(n, &mut keys);
        for i in 0..len {
            game.key(keys[i]);
        }
        let state = game.step();
        if state != prev || n % 100 == 0 {
            let frame = fnv64(game.grid());
            let summary = summary_text(game.summary());
            print!("R3ATH3_WASM_GLUE step={n} state={state} frame={frame:016x} {summary}\n");
            prev = state;
        }
    }
    print!("R3ATH3_WASM_GLUE ok steps=2000\n");
}

/// Same rule as `kernel/src/apps.rs` `printable`: `0x00` is a space, bytes
/// outside `0x20..=0x7E` are `?`.
fn summary_text(raw: &[u8]) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let len = raw[0] as usize;
    let end = (1 + len).min(raw.len());
    let mut out = String::new();
    for &byte in &raw[1..end] {
        let glyph = if byte == 0x00 {
            b' '
        } else if (0x20..=0x7E).contains(&byte) {
            byte
        } else {
            b'?'
        };
        out.push(glyph as char);
    }
    out
}

/// One packed key code per step. The same file drives `gluetest.rs`.
const PROOF_KEYS: &[u8] = include_bytes!("../../proof-keys.bin");

/// Keys applied before step `n`. Codes: 0 none, 1 space, 2 a, 3 d,
/// 4 w+a, 5 w+d, 6 s+a, 7 s+d, 8 w, 9 s.
fn proof_keys(n: u32, out: &mut [u8; 4]) -> usize {
    assert_eq!(PROOF_KEYS.len(), 2000);
    let keys: &[u8] = match PROOF_KEYS[(n - 1) as usize] {
        0 => b"",
        1 => b" ",
        2 => b"a",
        3 => b"d",
        4 => b"wa",
        5 => b"wd",
        6 => b"sa",
        7 => b"sd",
        8 => b"w",
        9 => b"s",
        other => panic!("key code {other}"),
    };
    out[..keys.len()].copy_from_slice(keys);
    keys.len()
}

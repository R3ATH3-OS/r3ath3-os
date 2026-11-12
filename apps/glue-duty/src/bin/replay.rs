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

fn summary_text(raw: &[u8]) -> &str {
    if raw.is_empty() {
        return "";
    }
    let len = raw[0] as usize;
    let end = (1 + len).min(raw.len());
    core::str::from_utf8(&raw[1..end]).unwrap_or("")
}

/// Keys applied before step `n`. Keep this in step with `gluetest.rs`.
fn proof_keys(n: u32, out: &mut [u8; 4]) -> usize {
    let mut len = 0;
    if n == 1 || n == 1001 {
        out[len] = b' ';
        len += 1;
    }
    if (2..=740).contains(&n) || (1002..=1400).contains(&n) {
        if n % 60 == 0 {
            out[len] = if (n / 60) % 2 == 0 { b'w' } else { b's' };
            len += 1;
        }
        if n % 150 == 75 {
            out[len] = b' ';
            len += 1;
        }
        if n == 300 || n == 310 {
            out[len] = b'p';
            len += 1;
        }
        out[len] = if n % 2 == 0 { b'a' } else { b'd' };
        len += 1;
    }
    len
}

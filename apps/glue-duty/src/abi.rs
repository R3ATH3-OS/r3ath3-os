//! Wasm exports for the r3app ABI. One state cell. Two privileged lines.
//! This file is guest code: it compiles to wasm32 and runs inside the
//! sandbox. The panic path calls `core::arch::wasm32::unreachable()`, a
//! safe function on rustc 1.83.0.

use core::cell::UnsafeCell;

use super::logic::Game;
use super::tone;

struct Slot(UnsafeCell<Game>);

// SAFETY: One wasm32 thread. The six exports do not call each other, so
// `with_game` is not re-entered. `GAME` is the only alias of this cell.
unsafe impl Sync for Slot {}

static GAME: Slot = Slot(UnsafeCell::new(Game::blank()));

fn with_game<R>(f: impl FnOnce(&mut Game) -> R) -> R {
    // SAFETY: `GAME` is that cell. Each export borrows it as `&mut Game`
    // for one call and does not hold the borrow across another export.
    // No other thread exists on wasm32, so the borrow is exclusive.
    f(unsafe { &mut *GAME.0.get() })
}

#[no_mangle]
pub extern "C" fn r3_abi() -> i32 {
    1
}

#[no_mangle]
pub extern "C" fn r3_init(seed: i32) {
    with_game(|game| game.init(seed as u32));
}

#[no_mangle]
pub extern "C" fn r3_key(key: i32) {
    let byte = key as u8;
    with_game(|game| game.key(byte));
}

#[no_mangle]
pub extern "C" fn r3_step() -> i32 {
    with_game(|game| i32::from(game.step()))
}

#[no_mangle]
pub extern "C" fn r3_text() -> i32 {
    with_game(|game| game.grid.as_ptr() as usize as i32)
}

#[no_mangle]
pub extern "C" fn r3_summary() -> i32 {
    with_game(|game| game.summary.as_ptr() as usize as i32)
}

/// Offset of the 64,000-byte 320×200×8 buffer. Optional. The host blits it.
#[no_mangle]
pub extern "C" fn r3_pixels() -> i32 {
    with_game(|game| game.pixels.as_ptr() as usize as i32)
}

/// Frequency word for the PC speaker. Zero is silence. Optional.
#[no_mangle]
pub extern "C" fn r3_tone() -> i32 {
    with_game(|game| tone::hz(game) as i32)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

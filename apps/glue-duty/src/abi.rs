//! Wasm exports for the r3app ABI. One state cell. Two privileged lines.
//! The panic path calls `unreachable`. If that function is not safe on
//! rustc 1.83, the third line is the block around the call.

use core::cell::UnsafeCell;

use super::logic::Game;

struct Slot(UnsafeCell<Game>);

// One wasm thread. The exports do not re-enter.
unsafe impl Sync for Slot {}

static GAME: Slot = Slot(UnsafeCell::new(Game::fresh()));

fn with_game<R>(f: impl FnOnce(&mut Game) -> R) -> R {
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

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

//! Frequency word for the picture. Integers only. The host plays it on
//! PIT channel 2. This module does not touch a port.
#![forbid(unsafe_code)]

use super::logic::{Game, GAME_OVER, PAUSED, PLAYING, SHIFT_DONE, TITLE};

/// PIT clock used by the speaker driver. `divisor(hz) = PIT_HZ / hz`.
pub const PIT_HZ: u32 = 1_193_182;

pub const HZ_TITLE: u32 = 250;
pub const HZ_PLAY: u32 = 400;
pub const HZ_SCRUB: u32 = 500;
pub const HZ_SOLVENT: u32 = 640;
pub const HZ_ESCAPE: u32 = 180;
pub const HZ_SHIFT: u32 = 2000;
pub const HZ_GUM: u32 = 80;

/// The tone for the game as it stands. An event line wins over the state
/// drone. Zero is silence. Every non-zero value divides `PIT_HZ` to an even
/// divisor, so the channel-2 latch stays even.
pub fn hz(game: &Game) -> u32 {
    if log_is(game, b"Scrubbed a blob.") {
        return HZ_SCRUB;
    }
    if log_is(game, b"Solvent blast.") {
        return HZ_SOLVENT;
    }
    if log_is(game, b"A blob reached the discharge.") {
        return HZ_ESCAPE;
    }
    if log_is(game, b"Shift complete.") {
        return HZ_SHIFT;
    }
    if log_is(game, b"The belt gummed up.") {
        return HZ_GUM;
    }
    match game.state {
        TITLE => HZ_TITLE,
        PLAYING => HZ_PLAY,
        PAUSED => 0,
        SHIFT_DONE => HZ_SHIFT,
        GAME_OVER => HZ_GUM,
        _ => 0,
    }
}

/// Same integer division the kernel speaker uses. Zero stays zero.
pub fn divisor(hz: u32) -> u16 {
    if hz == 0 {
        0
    } else {
        (PIT_HZ / hz) as u16
    }
}

fn log_is(game: &Game, text: &[u8]) -> bool {
    let n = game.log_len as usize;
    n == text.len() && &game.log[..n] == text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::Game;

    #[test]
    fn divisors_are_even_and_match_the_pit() {
        let tones = [
            HZ_TITLE, HZ_PLAY, HZ_SCRUB, HZ_SOLVENT, HZ_ESCAPE, HZ_SHIFT, HZ_GUM,
        ];
        for hz in tones {
            let div = divisor(hz);
            assert!(div > 20, "{hz}");
            assert_eq!(div % 2, 0, "{hz} -> {div}");
            assert_eq!(div as u32, PIT_HZ / hz);
        }
        assert_eq!(divisor(0), 0);
        assert_eq!(divisor(HZ_TITLE), 4772);
        assert_eq!(divisor(HZ_PLAY), 2982);
        assert_eq!(divisor(HZ_SCRUB), 2386);
        assert_eq!(divisor(HZ_SOLVENT), 1864);
        assert_eq!(divisor(HZ_ESCAPE), 6628);
        assert_eq!(divisor(HZ_SHIFT), 596);
        assert_eq!(divisor(HZ_GUM), 14914);
    }

    #[test]
    fn title_is_the_low_drone_and_pause_is_silent() {
        let mut game = Game::fresh();
        game.init(1);
        assert_eq!(hz(&game), HZ_TITLE);
        game.key(b' ');
        assert_eq!(game.state(), PLAYING);
        assert_eq!(hz(&game), HZ_PLAY);
        game.key(b'p');
        assert_eq!(hz(&game), 0);
        game.key(b'p');
        assert_eq!(hz(&game), HZ_PLAY);
    }

    /// First scrub on the committed key script. The kernel proof expects this step.
    #[test]
    fn first_scrub_on_the_proof_script() {
        const KEYS: &[u8] = include_bytes!("../proof-keys.bin");
        let mut game = Game::fresh();
        game.init(1);
        let mut found = None;
        for n in 1u32..=KEYS.len() as u32 {
            for &key in proof_keys(KEYS[(n - 1) as usize]) {
                game.key(key);
            }
            let _ = game.step();
            if hz(&game) == HZ_SCRUB {
                found = Some(n);
                break;
            }
        }
        assert_eq!(found, Some(SCRUB_STEP));
    }

    const SCRUB_STEP: u32 = 34;

    fn proof_keys(code: u8) -> &'static [u8] {
        match code {
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
            _ => panic!("code {code}"),
        }
    }
}

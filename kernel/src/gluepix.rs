//! `glue-pixels` proof. Glue Duty's 320×200 buffer is blitted, then text
//! mode 3 comes back through `finish`. Safe code only.

use core::fmt::Write;

use crate::apps::{self, GLUE_DUTY, HELLO};
use crate::fb::{self, Format};
use crate::frames;
use crate::heap;
use crate::qemu_exit;
use crate::serial;
use crate::wasm::{self, LoadError, Trap};

const TITLE_SUM: u32 = 660_216;
const TITLE_FRAME: u64 = 0xfbc1_f073_536d_b373;
const PLAY_SUM: u32 = 574_839;
const PLAY_FRAME: u64 = 0xf14e_49b4_7018_956c;

struct View {
    sum: u32,
    frame: u64,
    px0: u8,
    px1: u8,
    last: u8,
}

pub fn run() -> ! {
    let heap_before = heap::used();
    let free_before = frames::stats().1;
    let _ = write!(serial::Writer, "R3ATH3_GLUE_PIXELS start\n");

    let font_before = fb::font_fnv();
    let dac07 = fb::dac(0x07);
    let dac3a = fb::dac(0x3A);
    let dac3f = fb::dac(0x3F);

    let ran = wasm::with_instance(GLUE_DUTY, |loaded| {
        let mut inst = match loaded {
            Ok(inst) => inst,
            Err(error) => fail(0, error.word()),
        };
        if !apps::check_abi(&mut inst) {
            fail(0, "abi");
        }
        match inst.call("r3_init", &[1]) {
            Ok(_) => {}
            Err(trap) => fail(0, trap.word()),
        }
        let title = match guest_view(&mut inst) {
            Ok(view) => view,
            Err(detail) => fail(2, detail),
        };
        if title.sum != TITLE_SUM
            || title.frame != TITLE_FRAME
            || title.px0 != 0
            || title.px1 != 28
            || title.last != 0
        {
            fail(2, "title");
        }
        match apps::blit_exported(&mut inst) {
            Ok(true) => {}
            Ok(false) => fail(1, "no-export"),
            Err(trap) => fail(1, trap.word()),
        }
        let view = fb::info();
        if view.width != 320
            || view.height != 200
            || view.stride != 320
            || view.format != Format::Indexed8
            || !fb::in_graphics()
        {
            fail(1, "info");
        }
        let gc6 = fb::gc(0x06);
        let crtc9 = fb::crtc(0x09);
        if gc6 != 0x05 || crtc9 != 0x41 {
            fail(1, "regs");
        }
        let _ = write!(
            serial::Writer,
            "R3ATH3_GLUE_PIXELS step=1 mode=13h width=320 height=200 stride=320 format=indexed8 gc6=05 crtc9=41\n"
        );
        if !hw_matches(&title) {
            fail(2, "hw");
        }
        let _ = write!(
            serial::Writer,
            "R3ATH3_GLUE_PIXELS step=2 title sum=660216 frame=fbc1f073536db373 px0=0 px1=28 px63999=0 hw=match\n"
        );

        match inst.call("r3_key", &[i64::from(b' ')]) {
            Ok(_) => {}
            Err(trap) => fail(3, trap.word()),
        }
        match inst.call("r3_step", &[]) {
            Ok(Some(1)) => {}
            Ok(Some(_)) => fail(3, "state"),
            Ok(None) => fail(3, "step-none"),
            Err(trap) => fail(3, trap.word()),
        }
        let play = match guest_view(&mut inst) {
            Ok(view) => view,
            Err(detail) => fail(3, detail),
        };
        if play.sum != PLAY_SUM || play.frame != PLAY_FRAME || play.frame == title.frame {
            fail(3, "play");
        }
        match apps::blit_exported(&mut inst) {
            Ok(true) => {}
            Ok(false) => fail(3, "no-export"),
            Err(trap) => fail(3, trap.word()),
        }
        if !hw_matches(&play) {
            fail(3, "hw");
        }
        let _ = write!(
            serial::Writer,
            "R3ATH3_GLUE_PIXELS step=3 play sum=574839 frame=f14e49b47018956c changed=ok hw=match\n"
        );
        play.sum
    });
    let Some(play_sum) = ran else {
        fail(0, "busy");
    };

    match load_hello() {
        Ok(false) => {}
        Ok(true) => fail(4, "hello-export"),
        Err(detail) => fail(4, detail),
    }
    let same = fb::with_bytes(|buf| sum_bytes(buf) == play_sum);
    if same != Some(true) || fb::hardware_sum() != play_sum {
        fail(4, "changed");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_GLUE_PIXELS step=4 absent=ok sum=same\n"
    );

    fb::enter_text();
    if !text_restored(font_before, dac07, dac3a, dac3f) {
        fail(5, "restore");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_GLUE_PIXELS step=5 text gc6=0e crtc9=4f cell=0a41 font={font_before:08x} dac07={},{},{} dac3a={},{},{} dac3f={},{},{}\n",
        dac07.0, dac07.1, dac07.2, dac3a.0, dac3a.1, dac3a.2, dac3f.0, dac3f.1, dac3f.2
    );

    fb::enter_graphics();
    if !fb::blit() {
        fail(6, "blit");
    }
    if fb::hardware_sum() != play_sum {
        fail(6, "hw");
    }
    apps::finish_for_proof("glue-duty");
    if fb::in_graphics()
        || fb::gc(0x06) != 0x0E
        || fb::crtc(0x09) != 0x4F
        || fb::font_fnv() != font_before
        || fb::dac(0x07) != dac07
        || fb::dac(0x3A) != dac3a
        || fb::dac(0x3F) != dac3f
    {
        fail(6, "finish");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_GLUE_PIXELS step=6 finish gc6=0e crtc9=4f font={font_before:08x} dac07={},{},{} dac3a={},{},{} dac3f={},{},{}\n",
        dac07.0, dac07.1, dac07.2, dac3a.0, dac3a.1, dac3a.2, dac3f.0, dac3f.1, dac3f.2
    );

    if heap::used() != heap_before || frames::stats().1 != free_before {
        fail(7, "changed");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_GLUE_PIXELS step=7 heap=same frames=same\n"
    );
    let _ = write!(serial::Writer, "R3ATH3_GLUE_PIXELS ok\n");
    qemu_exit::glue_pixels_ok();
}

fn guest_view(inst: &mut wasm::Instance<'_, '_>) -> Result<View, &'static str> {
    let offset = match inst.call("r3_pixels", &[]) {
        Ok(Some(value)) => value,
        Ok(None) => return Err("pixels-none"),
        Err(trap) => return Err(trap.word()),
    };
    let memory = inst.memory();
    let Some(start) = apps::offset_of(offset, memory.len(), fb::LEN) else {
        return Err("pixels-range");
    };
    let buf = &memory[start..start + fb::LEN];
    Ok(View {
        sum: sum_bytes(buf),
        frame: fnv64(buf),
        px0: buf[0],
        px1: buf[1],
        last: buf[fb::LEN - 1],
    })
}

fn hw_matches(view: &View) -> bool {
    fb::hardware_sum() == view.sum
        && fb::hw_byte(0) == Some(view.px0)
        && fb::hw_byte(1) == Some(view.px1)
        && fb::hw_byte(fb::LEN - 1) == Some(view.last)
}

fn load_hello() -> Result<bool, &'static str> {
    let outcome = wasm::with_instance(HELLO, |loaded| match loaded {
        Ok(mut inst) => apps::blit_exported(&mut inst).map_err(trap_word),
        Err(error) => Err(error.word()),
    });
    match outcome {
        Some(result) => result,
        None => Err(LoadError::Busy.word()),
    }
}

fn trap_word(trap: Trap) -> &'static str {
    trap.word()
}

fn sum_bytes(buf: &[u8]) -> u32 {
    let mut sum = 0u32;
    for &byte in buf {
        sum = sum.wrapping_add(u32::from(byte));
    }
    sum
}

fn fnv64(data: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in data {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn text_restored(
    font: u32,
    dac07: (u8, u8, u8),
    dac3a: (u8, u8, u8),
    dac3f: (u8, u8, u8),
) -> bool {
    !fb::in_graphics()
        && fb::gc(0x06) == 0x0E
        && fb::crtc(0x09) == 0x4F
        && fb::text_cell_roundtrip(0x0A41) == 0x0A41
        && fb::font_fnv() == font
        && fb::dac(0x07) == dac07
        && fb::dac(0x3A) == dac3a
        && fb::dac(0x3F) == dac3f
}

fn fail(step: u8, detail: &str) -> ! {
    let _ = write!(
        serial::Writer,
        "R3ATH3_GLUE_PIXELS fail step={step} detail={detail}\n"
    );
    qemu_exit::fail();
}

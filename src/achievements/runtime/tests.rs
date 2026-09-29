use super::*;
use std::cell::RefCell;

/// Memory the tests change between frames.
struct Ram(RefCell<Vec<u8>>);

impl Ram {
    fn new(bytes: &[u8]) -> Self {
        Self(RefCell::new(bytes.to_vec()))
    }

    fn set(&self, address: usize, value: u8) {
        self.0.borrow_mut()[address] = value;
    }
}

impl Peek for Ram {
    fn peek(&self, address: u32, bytes: usize) -> u32 {
        let ram = self.0.borrow();
        (0..bytes).fold(0, |v, i| {
            v | (ram.get(address as usize + i).copied().unwrap_or(0) as u32) << (8 * i)
        })
    }
}

fn frames(runtime: &mut Runtime, ram: &Ram, n: usize) -> Vec<Event> {
    (0..n).flat_map(|_| runtime.do_frame(ram, true)).collect()
}

#[test]
fn a_trigger_waits_for_its_conditions_to_be_false_first() {
    let ram = Ram::new(&[0x00, 0x12, 0x34, 0xAB, 0x56]);
    let mut runtime = Runtime::new();
    runtime.add_achievement(1, "0xH0001=18").unwrap();
    // True from the start: it doesn't trigger.
    assert!(frames(&mut runtime, &ram, 3).is_empty());
    ram.set(1, 0);
    assert!(frames(&mut runtime, &ram, 1).is_empty());
    ram.set(1, 18);
    assert_eq!(frames(&mut runtime, &ram, 1), [Event::Triggered(1)]);
    // Once.
    assert!(frames(&mut runtime, &ram, 2).is_empty());
    assert!(!runtime.is_active(1));
}

#[test]
fn hit_counts_resets_and_pauses() {
    let ram = Ram::new(&[0, 0, 0, 0]);
    let mut runtime = Runtime::new();
    // Byte 0 is 1 for three frames, unless byte 1 is 1 (reset) or byte 2
    // is 1 (pause).
    runtime
        .add_achievement(7, "0xH0000=1.3._R:0xH0001=1_P:0xH0002=1")
        .unwrap();
    frames(&mut runtime, &ram, 1);
    ram.set(0, 1);
    assert!(frames(&mut runtime, &ram, 2).is_empty());
    ram.set(1, 1);
    frames(&mut runtime, &ram, 1);
    ram.set(1, 0);
    assert!(
        frames(&mut runtime, &ram, 2).is_empty(),
        "the reset took the hits"
    );
    ram.set(2, 1);
    assert!(frames(&mut runtime, &ram, 5).is_empty(), "paused");
    ram.set(2, 0);
    assert_eq!(frames(&mut runtime, &ram, 1), [Event::Triggered(7)]);
}

#[test]
fn delta_addsource_and_addaddress() {
    let ram = Ram::new(&[0x02, 0x10, 0x20, 0x05, 0x06]);
    let mut runtime = Runtime::new();
    // The byte at [0]+2 went up.
    runtime
        .add_achievement(1, "I:0xH0000_0xH0002>d0xH0002")
        .unwrap();
    // Bytes 1 and 2 add up to 0x40.
    runtime.add_achievement(2, "A:0xH0001_0xH0002=64").unwrap();
    frames(&mut runtime, &ram, 2);
    ram.set(2, 0x21);
    assert!(
        frames(&mut runtime, &ram, 1).is_empty(),
        "byte 2 isn't where the pointer points"
    );
    ram.set(4, 7);
    assert_eq!(frames(&mut runtime, &ram, 1), [Event::Triggered(1)]);
    ram.set(2, 0x30);
    assert_eq!(frames(&mut runtime, &ram, 1), [Event::Triggered(2)]);
}

#[test]
fn measured_progress_is_reported() {
    let ram = Ram::new(&[0, 0]);
    let mut runtime = Runtime::new();
    runtime.add_achievement(3, "M:0xH0000>=10").unwrap();
    frames(&mut runtime, &ram, 1);
    ram.set(0, 4);
    assert_eq!(
        frames(&mut runtime, &ram, 1),
        [Event::Progress {
            id: 3,
            value: 4,
            target: 10,
            percent: false
        }]
    );
    assert_eq!(runtime.measured(3), Some((4, 10)));
    ram.set(0, 10);
    assert!(frames(&mut runtime, &ram, 1).contains(&Event::Triggered(3)));
}

#[test]
fn leaderboards_start_update_and_submit() {
    let ram = Ram::new(&[0, 0, 0]);
    let mut runtime = Runtime::new();
    runtime
        .add_leaderboard(
            9,
            "STA:0xH0000=1::CAN:0xH0000=2::SUB:0xH0000=3::VAL:0xH0001*2",
        )
        .unwrap();
    frames(&mut runtime, &ram, 1);
    ram.set(0, 1);
    ram.set(1, 5);
    assert_eq!(
        frames(&mut runtime, &ram, 1),
        [Event::LeaderboardStarted(9)]
    );
    ram.set(1, 6);
    assert_eq!(
        frames(&mut runtime, &ram, 1),
        [Event::LeaderboardUpdated { id: 9, value: 12 }]
    );
    ram.set(0, 3);
    assert_eq!(
        frames(&mut runtime, &ram, 1),
        [Event::LeaderboardSubmitted { id: 9, value: 12 }]
    );
    // Not in softcore.
    let mut runtime = Runtime::new();
    runtime
        .add_leaderboard(
            9,
            "STA:0xH0000=1::CAN:0xH0000=2::SUB:0xH0000=3::VAL:0xH0001",
        )
        .unwrap();
    ram.set(0, 0);
    runtime.do_frame(&ram, false);
    ram.set(0, 1);
    assert!(runtime.do_frame(&ram, false).is_empty());
}

#[test]
fn rich_presence_lookups_formats_and_conditions() {
    let ram = Ram::new(&[2, 0x34, 0x12, 1]);
    let mut runtime = Runtime::new();
    let script = "Lookup:Stage\r\n0=Start\r\n1-2=Forest\r\n*=Unknown\r\n\r\nFormat:Points\r\nFormatType=SCORE\r\n\r\n\
        Display:\r\n// a comment\r\n?0xH0003=0?Title screen\r\n@Stage(0xH0000), @Points(0x 0001) points, \\@home\r\n";
    runtime.set_richpresence(script).unwrap();
    runtime.do_frame(&ram, false);
    assert_eq!(
        runtime.richpresence().unwrap(),
        "Forest, 004660 points, @home"
    );
    ram.set(3, 0);
    runtime.do_frame(&ram, false);
    assert_eq!(runtime.richpresence().unwrap(), "Title screen");
    // A formula makes a helper value.
    let mut runtime = Runtime::new();
    runtime
        .set_richpresence("Display:\n@Number(0xH0000*10_0xH0003) and @Unknown(1)")
        .unwrap();
    runtime.do_frame(&ram, false);
    assert_eq!(
        runtime.richpresence().unwrap(),
        "20 and [Unknown macro]Unknown(1)"
    );
}

#[test]
fn definitions_that_do_not_parse_say_why() {
    let mut memrefs = memref::Memrefs::default();
    assert_eq!(
        parse_trigger("0xH1234=", &mut memrefs).unwrap_err(),
        Error::InvalidMemoryOperand
    );
    assert_eq!(
        parse_trigger("0xH1234", &mut memrefs).unwrap_err(),
        Error::InvalidOperator
    );
    assert_eq!(
        parse_trigger("X:0xH1234=1", &mut memrefs).unwrap_err(),
        Error::InvalidConditionType
    );
    assert_eq!(
        parse_trigger("M:0xH1234>1_M:0xH1235>1", &mut memrefs).unwrap_err(),
        Error::MultipleMeasured
    );
    assert_eq!(
        parse_lboard("STA:0xH1=1::CAN:0xH1=2::SUB:0xH1=3", &mut memrefs).unwrap_err(),
        Error::MissingValue
    );
}

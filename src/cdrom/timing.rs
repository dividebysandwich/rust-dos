//! How long a CD-ROM drive takes to read: where the sectors are on the
//! disc and how far the head and the disc have to move to reach them.
//!
//! A pressed CD is one spiral track read at constant linear velocity
//! (CLV): every sector is as long as every other, so a turn of the
//! spiral holds about 9 sectors at the inside and 22 at the outside. A
//! sector's place on the spiral, in turns from address 00:00:00, follows
//! from the linear velocity `v`, the track pitch `p` and the radius `r0`
//! the spiral starts at:
//!
//! ```text
//!   r(a)     = sqrt(r0^2 + p * L * a / pi)     L = v / 75, a in sectors
//!   turns(a) = (r(a) - r0) / p
//! ```
//!
//! The drive spins the disc at a constant rate (CAV), as the drives of
//! 1997 on do for data, so the disc's angle at emulated time `t` is
//! `rps * t` turns. A sector passes under the head when its angle,
//! `turns` modulo 1, is the disc's.
//!
//! After a read the drive goes on reading the sectors that follow into
//! its buffer as the disc turns, up to `READ_AHEAD` of them. A read of
//! sectors the buffer holds, or will hold by then, waits only for those
//! still to come and then sends them. Any other read moves the head to
//! its first sector's radius (`seek_ms`), waits for the sector to come
//! round, and reads its sectors as they pass.
//!
//! Real drives don't take exactly the same time twice: the servo settles
//! a little differently each time. Each read's time varies by up to
//! `JITTER` either way, by an amount worked out from the sector and the
//! emulated time of the read, so a run with `--deterministic` takes the
//! same times every time.
//!
//! An image with DPM data (`dpm::Dpm`, BlindWrite's or Alcohol 120%'s)
//! says where the sectors of the disc it was made from are: the angles a
//! drive measured. Where it has them, they take the place of `turns`
//! (#591). SecuROM 4.8 and StarForce compare read times with those of the
//! pressed disc, whose sectors are where its mastering put them, not where
//! a uniform spiral has them.

use super::dpm::Dpm;

/// Linear velocity of the spiral, in m/s: the Red Book allows 1.2 to 1.4.
const VELOCITY: f64 = 1.3;
/// Track pitch, in m: the Red Book allows 1.5 to 1.7 um.
const PITCH: f64 = 1.6e-6;
/// The radius of address 00:00:00, in m: where the program area starts.
const START_RADIUS: f64 = 0.025;
/// The outer edge of the program area, in m.
const OUTER_RADIUS: f64 = 0.058;
/// The drive's speed at the outer edge, as a multiple of 1x (75 sectors
/// a second), a 40x drive of 2000: it spins at about 8600 rpm.
const SPEED: f64 = 40.0;
/// Sectors the drive reads ahead into its buffer after a read: 128 KB.
const READ_AHEAD: i64 = 64;
/// Sending a sector from the buffer to the host, in ms: 2 KB at the
/// 16.6 MB/s of ATA PIO mode 4.
const SEND_MS: f64 = 0.125;
/// A seek's time, in ms: settling on the track, and the move across the
/// whole program area, which grows with the root of the distance.
const SEEK_SETTLE_MS: f64 = 1.0;
const FULL_STROKE_MS: f64 = 120.0;
/// How much a read's time varies, either way, as a fraction of it.
const JITTER: f64 = 0.02;

/// Sectors per second at 1x.
const FRAMES: f64 = super::FRAMES_PER_SECOND as f64;

/// Where the sectors of a disc are.
#[derive(Clone, Copy, Debug)]
pub struct Geometry<'a> {
    velocity: f64,
    pitch: f64,
    start_radius: f64,
    dpm: Option<&'a Dpm>,
}

impl<'a> Geometry<'a> {
    /// A disc mastered at the nominal velocity and pitch, with the DPM
    /// data of its image if it has some.
    pub fn new(dpm: Option<&'a Dpm>) -> Self {
        Self {
            velocity: VELOCITY,
            pitch: PITCH,
            start_radius: START_RADIUS,
            dpm,
        }
    }

    /// The radius of sector `lba`, in m.
    pub fn radius(&self, lba: i64) -> f64 {
        let a = (lba + super::LBA_OFFSET as i64).max(0) as f64;
        let l = self.velocity / FRAMES;
        (self.start_radius * self.start_radius + self.pitch * l * a / std::f64::consts::PI).sqrt()
    }

    /// Turns of the uniform spiral from address 00:00:00 to sector `lba`.
    fn spiral_turns(&self, lba: i64) -> f64 {
        (self.radius(lba) - self.start_radius) / self.pitch
    }

    /// Turns from address 00:00:00 to sector `lba`: by the DPM data where
    /// it has the sector, and the uniform spiral before and after it,
    /// carried on from the data's ends.
    pub fn turns(&self, lba: i64) -> f64 {
        let Some(dpm) = self.dpm else {
            return self.spiral_turns(lba);
        };
        let start = dpm.start as i64;
        let res = dpm.resolution as i64;
        let base = self.spiral_turns(start);
        // The measured angle of sector `start + i * res`, in turns from
        // `start`: entry i - 1 of the data.
        let at = |i: usize| {
            if i == 0 {
                0.0
            } else {
                dpm.angles[i - 1] as f64 / 256.0
            }
        };
        let last = dpm.angles.len();
        if lba <= start {
            return self.spiral_turns(lba);
        }
        let end = start + res * last as i64;
        if lba >= end {
            return base + at(last) + self.spiral_turns(lba) - self.spiral_turns(end);
        }
        let i = ((lba - start) / res) as usize;
        let frac = ((lba - start) % res) as f64 / res as f64;
        base + at(i) + (at(i + 1) - at(i)) * frac
    }

    /// Sectors a turn holds at sector `lba`.
    pub fn sectors_per_turn(&self, lba: i64) -> f64 {
        let l = self.velocity / FRAMES;
        2.0 * std::f64::consts::PI * self.radius(lba) / l
    }
}

/// Turns a second the drive spins the disc at.
pub fn turns_per_second() -> f64 {
    SPEED * VELOCITY / (2.0 * std::f64::consts::PI * OUTER_RADIUS)
}

/// How long a seek from radius `from` to radius `to` (m) takes, in ms.
pub fn seek_ms(from: f64, to: f64) -> f64 {
    let stroke = OUTER_RADIUS - START_RADIUS;
    SEEK_SETTLE_MS + FULL_STROKE_MS * ((to - from).abs() / stroke).min(1.0).sqrt()
}

/// The fraction of `x` in [0, 1).
fn wrap(x: f64) -> f64 {
    x - x.floor()
}

/// A number in [-1, 1) from the read's sector and time: the same for the
/// same read, different from one read to the next.
fn jitter(lba: i64, now: f64) -> f64 {
    let mut h = (lba as u64) ^ now.to_bits().rotate_left(29) ^ 0x9E37_79B9_7F4A_7C15;
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    (h >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// What the drive last read: the sectors in its buffer, from `start` up
/// to `next`, which it had read at `since` (seconds of emulated time) and
/// read on from as the disc turned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Head {
    pub start: i64,
    pub next: i64,
    pub since: f64,
}

crate::state_fields!(Head { start, next, since });

impl Default for Head {
    /// The head at the start of the disc, with nothing in the buffer.
    fn default() -> Self {
        Self {
            start: i64::MAX,
            next: 0,
            since: 0.0,
        }
    }
}

impl Head {
    /// Sectors the drive has read on from `next` by `now`, at most
    /// `READ_AHEAD`: how far reading ahead has taken the head.
    fn ahead(&self, disc: &Geometry, now: f64) -> i64 {
        let turns = (now - self.since).max(0.0) * turns_per_second();
        ((turns * disc.sectors_per_turn(self.next)) as i64).min(READ_AHEAD)
    }

    /// How long, in ms, a SEEK to sector `lba` that starts at `now` takes:
    /// the head moves to the sector's track. The buffer is emptied, and
    /// the drive reads on from the sector once it comes round.
    pub fn seek(&mut self, disc: &Geometry, now: f64, lba: i64) -> f64 {
        let rps = turns_per_second();
        let head = self.next + self.ahead(disc, now);
        let seek = seek_ms(disc.radius(head), disc.radius(lba)) / 1000.0;
        let wait = wrap(disc.turns(lba) - rps * (now + seek)) / rps;
        *self = Self {
            start: lba,
            next: lba,
            since: now + seek + wait,
        };
        seek * 1000.0
    }

    /// How long, in ms, a read of `count` sectors from `lba` takes when it
    /// starts at `now` (seconds of emulated time) on the disc `disc`. The
    /// head and the buffer move on to its sectors.
    pub fn read(&mut self, disc: &Geometry, now: f64, lba: i64, count: u32) -> f64 {
        let rps = turns_per_second();
        let end = lba + count as i64;
        let send = count as f64 * SEND_MS / 1000.0;
        // The buffer holds the last READ_AHEAD sectors read: reading ahead
        // pushes the oldest out.
        let ahead = self.ahead(disc, now);
        let held = self.start.max(self.next + ahead - READ_AHEAD);
        let (ready, seconds) = if lba >= held && end <= self.next + READ_AHEAD {
            // The buffer has the sectors, or will have them as the disc
            // turns on from `next`: wait for the last one, then send them.
            let ready = self.since + (disc.turns(end) - disc.turns(self.next)).max(0.0) / rps;
            (ready, (ready - now).max(0.0) + send)
        } else {
            // Move the head from where reading ahead took it, wait for
            // the sector to come round, and read on.
            let head = self.next + ahead;
            let seek = seek_ms(disc.radius(head), disc.radius(lba)) / 1000.0;
            let target = disc.turns(lba);
            let wait = wrap(target - rps * (now + seek)) / rps;
            let reading = (disc.turns(end) - target).max(0.0) / rps;
            self.start = lba;
            self.next = lba;
            let ready = now + seek + wait + reading;
            (ready, ready - now + send)
        };
        if end > self.next {
            self.next = end;
            self.since = ready;
        }
        self.start = self.start.max(self.next - READ_AHEAD);
        seconds * 1000.0 * (1.0 + JITTER * jitter(lba, now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spiral_holds_9_to_22_sectors_a_turn() {
        let disc = Geometry::new(None);
        let inner = disc.sectors_per_turn(-150);
        assert!((inner - 9.06).abs() < 0.01, "{inner}");
        // 74 minutes of spiral at 1.3 m/s and 1.6 um reach 59.7 mm.
        let end = 74 * 60 * 75 - 150;
        let outer = disc.radius(end);
        assert!((outer - 0.0597).abs() < 0.0001, "{outer}");
        let spt = disc.sectors_per_turn(end);
        assert!((spt - 21.64).abs() < 0.01, "{spt}");
    }

    #[test]
    fn a_turn_of_sectors_is_a_turn_of_the_spiral() {
        let disc = Geometry::new(None);
        for lba in [0, 50_000, 200_000] {
            let spt = disc.sectors_per_turn(lba);
            let turned = disc.turns(lba + spt.round() as i64) - disc.turns(lba);
            assert!((turned - 1.0).abs() < 0.06, "{lba}: {turned}");
        }
    }

    #[test]
    fn a_40x_drive_spins_at_about_8600_rpm() {
        let rpm = turns_per_second() * 60.0;
        assert!((rpm - 8562.0).abs() < 10.0, "{rpm}");
    }

    #[test]
    fn seeks_take_longer_the_further_they_go() {
        let short = seek_ms(0.030, 0.0301);
        let half = seek_ms(0.030, 0.0465);
        let full = seek_ms(0.025, 0.058);
        assert!(short < 8.0, "{short}");
        assert!(short < half && half < full, "{short} {half} {full}");
        assert!((full - 121.0).abs() < 0.01, "{full}");
    }

    /// The time of a read without the jitter.
    fn exact(head: &mut Head, disc: &Geometry, now: f64, lba: i64, count: u32) -> f64 {
        let ms = head.read(disc, now, lba, count);
        ms / (1.0 + JITTER * jitter(lba, now))
    }

    #[test]
    fn the_sectors_after_a_read_come_from_the_buffer() {
        let disc = Geometry::new(None);
        let rps = turns_per_second();
        let mut head = Head::default();
        let first = exact(&mut head, &disc, 0.5, 1000, 1);
        // Straight after: the next 16 come as the disc turns.
        let then = 0.5 + first / 1000.0;
        let next = exact(&mut head, &disc, then, 1001, 16);
        let turning = (disc.turns(1017) - disc.turns(1001)) / rps * 1000.0;
        let send = 16.0 * SEND_MS;
        let early = (then - head_ready(first, 0.5)) * 1000.0;
        assert!(
            (next - (turning - early + send)).abs() < 1e-6,
            "{next} {turning} {send}"
        );
        // A second later the drive has read them all ahead: 16 more are
        // only sent.
        let later = exact(&mut head, &disc, then + 1.0, 1017, 16);
        assert!((later - send).abs() < 1e-9, "{later}");
    }

    #[test]
    fn reading_ahead_pushes_the_oldest_sectors_out_of_the_buffer() {
        let disc = Geometry::new(None);
        let mut head = Head::default();
        exact(&mut head, &disc, 0.5, 1000, 16);
        // A second later the drive has read 64 sectors on from 1016, which
        // is all the buffer holds: 1016 is still there, 1015 is gone.
        let kept = exact(&mut head, &disc, 1.5, 1016, 1);
        assert!((kept - SEND_MS).abs() < 1e-9, "{kept}");
        let mut head = Head::default();
        exact(&mut head, &disc, 0.5, 1000, 16);
        let gone = exact(&mut head, &disc, 1.5, 1015, 1);
        assert!(gone > SEEK_SETTLE_MS + SEND_MS, "{gone}");
    }

    #[test]
    fn a_seek_moves_the_head_and_the_read_after_it_waits_for_its_sector() {
        let disc = Geometry::new(None);
        let rps = turns_per_second();
        let mut head = Head::default();
        exact(&mut head, &disc, 1.0, 1000, 1);
        let near = { head }.seek(&disc, 1.01, 1100);
        let far = head.seek(&disc, 1.01, 300_000);
        assert!(near < far && far > 100.0, "{near} {far}");
        // The read waits for the sector to come round after the seek.
        let now = 1.01 + far / 1000.0;
        let ms = exact(&mut head, &disc, now, 300_000, 1);
        let wait = wrap(disc.turns(300_000) - rps * now) / rps * 1000.0;
        let read = (disc.turns(300_001) - disc.turns(300_000)) / rps * 1000.0;
        assert!((ms - (wait + read + SEND_MS)).abs() < 1e-6, "{ms} {wait}");
    }

    /// When the first read of a test had its sector: it ended at `ms` less
    /// the time to send it.
    fn head_ready(ms: f64, now: f64) -> f64 {
        now + (ms - SEND_MS) / 1000.0
    }

    #[test]
    fn a_skip_past_the_buffer_waits_for_its_sector_to_come_round() {
        let disc = Geometry::new(None);
        let rps = turns_per_second();
        let mut head = Head::default();
        let first = exact(&mut head, &disc, 1.0, 10_000, 1);
        let now = 1.0 + first / 1000.0;
        let ms = exact(&mut head, &disc, now, 10_200, 1);
        // The head read ahead a little while the first sector was sent.
        let ahead = (now - head_ready(first, 1.0)) * rps * disc.sectors_per_turn(10_001);
        let from = disc.radius(10_001 + ahead as i64);
        let seek = seek_ms(from, disc.radius(10_200));
        let wait = wrap(disc.turns(10_200) - rps * (now + seek / 1000.0)) / rps * 1000.0;
        let read = (disc.turns(10_201) - disc.turns(10_200)) / rps * 1000.0;
        assert!((ms - (seek + wait + read + SEND_MS)).abs() < 1e-6, "{ms}");
        assert!(ms > seek && ms < seek + 1000.0 / rps + read + SEND_MS);
    }

    #[test]
    fn where_a_skip_lands_on_the_disc_sets_its_time() {
        // Reads of sector n + 100 straight after sector n: the time each
        // takes follows where its sector is round the disc from where the
        // head lands, so it varies along the disc by up to a turn.
        let disc = Geometry::new(None);
        let turn = 1000.0 / turns_per_second();
        let times: Vec<f64> = (0..40)
            .map(|i| {
                let mut head = Head::default();
                let n = 20_000 + i * 337;
                let first = exact(&mut head, &disc, 3.0, n, 1);
                exact(&mut head, &disc, 3.0 + first / 1000.0, n + 100, 1)
            })
            .collect();
        let (lo, hi) = times
            .iter()
            .fold((f64::MAX, 0.0f64), |(lo, hi), &t| (lo.min(t), hi.max(t)));
        assert!(hi - lo > turn / 2.0 && hi - lo < turn, "{lo} {hi} {turn}");
    }

    #[test]
    fn a_read_takes_the_same_time_for_the_same_sector_and_time() {
        let disc = Geometry::new(None);
        let mut a = Head::default();
        let mut b = Head::default();
        assert_eq!(a.read(&disc, 2.0, 5000, 1), b.read(&disc, 2.0, 5000, 1));
        // And varies by at most JITTER from one read to the next.
        let ratios: Vec<f64> = (0..50)
            .map(|i| {
                let now = 2.0 + i as f64 / 1000.0;
                let ms = Head::default().read(&disc, now, 5000, 1);
                ms / exact(&mut Head::default(), &disc, now, 5000, 1)
            })
            .collect();
        assert!(ratios.iter().all(|r| (r - 1.0).abs() <= JITTER + 1e-9));
        assert!(ratios.iter().any(|r| (r - 1.0).abs() > JITTER / 4.0));
    }

    #[test]
    fn dpm_data_says_where_its_sectors_are() {
        // Every 100th sector from 1000, a turn and a half apart each.
        let dpm = Dpm {
            start: 1000,
            resolution: 100,
            angles: vec![384, 768, 1152],
        };
        let disc = Geometry::new(Some(&dpm));
        let plain = Geometry::new(None);
        let base = plain.turns(1000);
        assert_eq!(disc.turns(500), plain.turns(500));
        assert!((disc.turns(1100) - (base + 1.5)).abs() < 1e-9);
        assert!((disc.turns(1150) - (base + 2.25)).abs() < 1e-9);
        assert!((disc.turns(1300) - (base + 4.5)).abs() < 1e-9);
        // After the data the spiral goes on from its last sector.
        let after = disc.turns(1400) - disc.turns(1300);
        assert!((after - (plain.turns(1400) - plain.turns(1300))).abs() < 1e-9);
    }
}

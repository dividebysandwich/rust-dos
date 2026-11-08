//! Data position measurement (DPM): where a pressed disc's sectors are,
//! as a drive measured them. Copy protections such as SecuROM 4.8 and
//! StarForce time reads to check that sectors are where the pressed disc
//! has them, which a dump of the sectors alone can't show. BlindWrite
//! (`b6t`) and Alcohol 120% (`mds`) store the measurement in the same
//! form, which this holds; the drive's timing model (`timing::Geometry`)
//! places sectors by it whichever image it came from.

/// The angle of every `resolution`th sector from `start` on, as the
/// drive that made the image measured them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dpm {
    pub start: u32,
    pub resolution: u32,
    /// Angles of sectors `start + resolution`, `start + 2 * resolution`
    /// and on, in 256ths of a rotation from sector `start`.
    pub angles: Vec<u32>,
}

impl Dpm {
    /// The measurement of `angles` every `resolution` sectors from
    /// `start`, if it can be one: a resolution above 0, at least one
    /// angle, and angles that don't go back. Aaru checks the same
    /// before it keeps a block (`DataPositionMeasurement.Validate`, by
    /// Natalia Portillo).
    pub fn new(start: u32, resolution: u32, angles: Vec<u32>) -> Option<Self> {
        let grows = angles.windows(2).all(|w| w[0] <= w[1]);
        (resolution > 0 && !angles.is_empty() && grows).then_some(Dpm {
            start,
            resolution,
            angles,
        })
    }
}

/// The measurement at byte `at` of `data`, as both formats store it:
/// little-endian u32s of the start sector, the resolution, the count of
/// angles, and the angles. None if `data` ends first or the block isn't
/// one `Dpm::new` takes.
pub fn parse_block(data: &[u8], at: usize) -> Option<Dpm> {
    let word = |i: usize| {
        let from = at.checked_add(i.checked_mul(4)?)?;
        data.get(from..from.checked_add(4)?)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
    };
    let (start, resolution, count) = (word(0)?, word(1)?, word(2)?);
    // More angles than `data` has room for can't be there; checking
    // first keeps a count of billions from allocating.
    if count as usize > data.len() / 4 {
        return None;
    }
    let angles = (0..count as usize)
        .map(|i| word(3 + i))
        .collect::<Option<Vec<_>>>()?;
    Dpm::new(start, resolution, angles)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn blocks() {
        let data = words(&[7, 0, 256, 3, 700, 1390, 2070]);
        let dpm = parse_block(&data, 4).unwrap();
        assert_eq!(
            dpm,
            Dpm {
                start: 0,
                resolution: 256,
                angles: vec![700, 1390, 2070]
            }
        );
        assert_eq!(parse_block(&data[..data.len() - 1], 4), None, "cut short");
        assert_eq!(parse_block(&data, 40), None, "past the end");
        assert_eq!(parse_block(&words(&[0, 0, 1, 5]), 0), None, "resolution 0");
        assert_eq!(parse_block(&words(&[0, 10, 0]), 0), None, "no angles");
        assert_eq!(
            parse_block(&words(&[0, 10, 2, 9, 8]), 0),
            None,
            "going back"
        );
        assert!(parse_block(&words(&[0, 10, 2, 9, 9]), 0).is_some(), "level");
        let huge = words(&[0, 10, u32::MAX, 1]);
        assert_eq!(parse_block(&huge, 0), None, "a count past the data");
        assert_eq!(parse_block(&huge, usize::MAX - 2), None);
    }
}

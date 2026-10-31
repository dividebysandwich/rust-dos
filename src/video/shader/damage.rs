//! Bounded source rectangles shared by preparation and cached GPU passes.

/// Conservative x/y/width/height rectangles at one source revision.
#[derive(Clone, Default)]
pub struct XbrzDamage {
    // Cap bookkeeping at 256 bytes; merge excess fragments conservatively.
    rects: Vec<[u32; 4]>,
}

impl XbrzDamage {
    pub fn rects(&self) -> &[[u32; 4]] {
        &self.rects
    }

    pub fn clear(&mut self) {
        self.rects.clear();
    }

    pub fn add(&mut self, mut rect: [u32; 4]) {
        if rect[2] == 0 || rect[3] == 0 {
            return;
        }
        let mut i = 0;
        while i < self.rects.len() {
            let old = self.rects[i];
            let adjacent = rect[0] <= old[0].saturating_add(old[2])
                && old[0] <= rect[0].saturating_add(rect[2])
                && rect[1] <= old[1].saturating_add(old[3])
                && old[1] <= rect[1].saturating_add(rect[3]);
            if adjacent || self.rects.len() == 16 {
                let right = rect[0]
                    .saturating_add(rect[2])
                    .max(old[0].saturating_add(old[2]));
                let bottom = rect[1]
                    .saturating_add(rect[3])
                    .max(old[1].saturating_add(old[3]));
                rect[0] = rect[0].min(old[0]);
                rect[1] = rect[1].min(old[1]);
                rect[2] = right - rect[0];
                rect[3] = bottom - rect[1];
                self.rects.swap_remove(i);
                i = 0;
            } else {
                i += 1;
            }
        }
        self.rects.push(rect);
    }

    /// Resolve kernels depend on source pixels up to two places away.
    pub fn expanded(&self, size: (u32, u32)) -> impl Iterator<Item = [u32; 4]> + '_ {
        self.rects
            .iter()
            .map(move |&[x, y, w, h]| {
                let left = x.saturating_sub(2).min(size.0);
                let top = y.saturating_sub(2).min(size.1);
                let right = x.saturating_add(w).saturating_add(2).min(size.0);
                let bottom = y.saturating_add(h).saturating_add(2).min(size.1);
                [left, top, right - left, bottom - top]
            })
            .filter(|rect| rect[2] != 0 && rect[3] != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalescing_and_fragmentation_keep_every_dirty_pixel_bounded() {
        let mut damage = XbrzDamage::default();
        for y in 0..8 {
            damage.add([3, y, 4, 1]);
        }
        assert_eq!(damage.rects(), &[[3, 0, 4, 8]]);
        damage.clear();
        for i in 0..80 {
            damage.add([i * 5, i * 7, 1, 1]);
            assert!(damage.rects().len() <= 16);
        }
        for i in 0..80 {
            assert!(damage.rects().iter().any(|&[x, y, w, h]| {
                (x..x + w).contains(&(i * 5)) && (y..y + h).contains(&(i * 7))
            }));
        }
        damage.clear();
        damage.add([0, 0, 0, 5]);
        assert!(damage.rects().is_empty());
        damage.add([0, 0, 1, 1]);
        damage.add([9, 7, 1, 1]);
        assert_eq!(
            damage.expanded((10, 8)).collect::<Vec<_>>(),
            &[[0, 0, 3, 3], [7, 5, 3, 3]]
        );
        damage.clear();
        damage.add([u32::MAX, u32::MAX, 1, 1]);
        assert!(damage.expanded((10, 8)).next().is_none());
    }
}

//! The hands' fingertips as the headset's cameras see them
//! (XR_EXT_hand_tracking): where the fingers type finds the keyboard.
//! Hands a runtime makes up from the controllers held don't count: with
//! XR_EXT_hand_tracking_data_source, only the cameras' are asked for.
//! Through the raw calls: the crate's tracker takes no data source.

use openxr as xr;
use xr::sys::Handle as _;

/// The fingertips found, by their joints' numbers (index, middle, ring and
/// little: XR_HAND_JOINT_*_TIP_EXT); not the thumb's, which rests on the
/// space bar.
const TIPS: [usize; 4] = [10, 15, 20, 25];

pub struct Hands {
    fp: xr::raw::HandTrackingEXT,
    trackers: [xr::sys::HandTrackerEXT; 2],
}

impl Hands {
    /// Trackers of both hands, with the extension enabled; asking for the
    /// cameras' hands only with `sourced` (its data source extension).
    pub fn new(session: &xr::Session<xr::AnyGraphics>, sourced: bool) -> Result<Hands, xr::sys::Result> {
        let fp = *session.instance().exts().ext_hand_tracking.as_ref().ok_or(xr::sys::Result::ERROR_EXTENSION_NOT_PRESENT)?;
        let mut sources = [xr::sys::HandTrackingDataSourceEXT::UNOBSTRUCTED];
        let source = xr::sys::HandTrackingDataSourceInfoEXT {
            ty: xr::sys::HandTrackingDataSourceInfoEXT::TYPE,
            next: std::ptr::null(),
            requested_data_source_count: 1,
            requested_data_sources: sources.as_mut_ptr(),
        };
        let mut hands = Hands { fp, trackers: [xr::sys::HandTrackerEXT::NULL; 2] };
        for (tracker, hand) in hands.trackers.iter_mut().zip([xr::Hand::LEFT, xr::Hand::RIGHT]) {
            let info = xr::sys::HandTrackerCreateInfoEXT {
                ty: xr::sys::HandTrackerCreateInfoEXT::TYPE,
                next: if sourced { &source as *const _ as *const _ } else { std::ptr::null() },
                hand,
                hand_joint_set: xr::sys::HandJointSetEXT::DEFAULT,
            };
            // SAFETY: the session is alive with the extension enabled; the
            // info (and the source it points to) outlive the call.
            let made = unsafe { (fp.create_hand_tracker)(session.as_raw(), &info, tracker) };
            if made.into_raw() < 0 {
                // (Those made so far go with `hands`.)
                return Err(made);
            }
        }
        Ok(hands)
    }

    /// The fingertips of each hand tracked at `time`, in `space`.
    pub fn tips(&self, space: &xr::Space, time: xr::Time) -> [Option<[glam::Vec3; 4]>; 2] {
        let valid = xr::SpaceLocationFlags::POSITION_VALID;
        let mut found = [None; 2];
        for (tips, &tracker) in found.iter_mut().zip(&self.trackers) {
            let mut joints = [xr::sys::HandJointLocationEXT::default(); xr::HAND_JOINT_COUNT];
            let info = xr::sys::HandJointsLocateInfoEXT {
                ty: xr::sys::HandJointsLocateInfoEXT::TYPE,
                next: std::ptr::null(),
                base_space: space.as_raw(),
                time,
            };
            let mut locations = xr::sys::HandJointLocationsEXT {
                ty: xr::sys::HandJointLocationsEXT::TYPE,
                next: std::ptr::null_mut(),
                is_active: false.into(),
                joint_count: joints.len() as u32,
                joint_locations: joints.as_mut_ptr(),
            };
            // SAFETY: the tracker and space are the session's, alive; the
            // joints array is as long as said.
            let located = unsafe { (self.fp.locate_hand_joints)(tracker, &info, &mut locations) };
            if located.into_raw() < 0 || !bool::from(locations.is_active) {
                continue;
            }
            if TIPS.iter().any(|&i| !joints[i].location_flags.contains(valid)) {
                continue;
            }
            *tips = Some(TIPS.map(|i| {
                let p = joints[i].pose.position;
                glam::Vec3::new(p.x, p.y, p.z)
            }));
        }
        found
    }
}

impl Drop for Hands {
    fn drop(&mut self) {
        for &tracker in &self.trackers {
            if tracker != xr::sys::HandTrackerEXT::NULL {
                // SAFETY: made by `new`, the session alive (`Xr` drops this
                // before it).
                unsafe { (self.fp.destroy_hand_tracker)(tracker) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tips_are_the_fingers_ends() {
        use xr::sys::HandJointEXT as J;
        let ends = [J::INDEX_TIP, J::MIDDLE_TIP, J::RING_TIP, J::LITTLE_TIP].map(|j| j.into_raw() as usize);
        assert_eq!(TIPS, ends);
    }
}

//! The controllers OpenXR runtimes know, and what their inputs are bound
//! to.

/// A profile's inputs: the control, the hand ("left", "right" or both)
/// and the input's path under /user/hand/<hand>/input/.
pub type Inputs = &'static [(Control, &'static str, &'static str)];

/// What a controller's input is bound to.
#[derive(Clone, Copy)]
pub enum Control {
    Aim,
    Select,
    Squeeze,
    Stick,
    Primary,
    Secondary,
    Menu,
}

/// The suggested bindings of the controllers OpenXR runtimes know, by
/// their interaction profile: the control, the hand ("left", "right" or
/// both) and the input's path under /user/hand/<hand>/input/. SteamVR lets
/// the player change them.
pub const PROFILES: &[(&str, Inputs)] = &[
    (
        "/interaction_profiles/khr/simple_controller",
        &[(Control::Aim, "", "aim/pose"), (Control::Select, "", "select/click"), (Control::Menu, "", "menu/click")],
    ),
    (
        "/interaction_profiles/valve/index_controller",
        &[
            (Control::Aim, "", "aim/pose"),
            (Control::Select, "", "trigger/click"),
            (Control::Squeeze, "", "squeeze/value"),
            (Control::Stick, "", "thumbstick"),
            (Control::Primary, "", "a/click"),
            (Control::Secondary, "", "b/click"),
            (Control::Menu, "left", "thumbstick/click"),
        ],
    ),
    (
        "/interaction_profiles/oculus/touch_controller",
        &[
            (Control::Aim, "", "aim/pose"),
            (Control::Select, "", "trigger/value"),
            (Control::Squeeze, "", "squeeze/value"),
            (Control::Stick, "", "thumbstick"),
            (Control::Primary, "left", "x/click"),
            (Control::Secondary, "left", "y/click"),
            (Control::Primary, "right", "a/click"),
            (Control::Secondary, "right", "b/click"),
            (Control::Menu, "left", "menu/click"),
        ],
    ),
    (
        "/interaction_profiles/htc/vive_controller",
        &[
            (Control::Aim, "", "aim/pose"),
            (Control::Select, "", "trigger/click"),
            (Control::Squeeze, "", "squeeze/click"),
            (Control::Stick, "", "trackpad"),
            (Control::Menu, "", "menu/click"),
        ],
    ),
    (
        "/interaction_profiles/microsoft/motion_controller",
        &[
            (Control::Aim, "", "aim/pose"),
            (Control::Select, "", "trigger/value"),
            (Control::Squeeze, "", "squeeze/click"),
            (Control::Stick, "", "thumbstick"),
            (Control::Menu, "", "menu/click"),
        ],
    ),
];

/// Hands tracked without controllers (XR_EXT_hand_interaction): a hand
/// points, pinching clicks, grabbing is the second button.
pub const HAND_PROFILE: (&str, Inputs) = (
    "/interaction_profiles/ext/hand_interaction_ext",
    &[(Control::Aim, "", "aim/pose"), (Control::Select, "", "aim_activate_ext/value"), (Control::Squeeze, "", "grasp_ext/value")],
);

/// The profiles to suggest bindings for: the hands' too if the runtime's
/// `hand_interaction` extension is enabled.
pub fn profiles(hand_interaction: bool) -> impl Iterator<Item = (&'static str, Inputs)> {
    PROFILES.iter().copied().chain(hand_interaction.then_some(HAND_PROFILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_well_formed() {
        let named = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        assert_eq!(profiles(false).count(), PROFILES.len());
        assert_eq!(profiles(true).last().map(|p| p.0), Some(HAND_PROFILE.0));
        for (profile, inputs) in profiles(true) {
            let parts: Vec<&str> = profile.split('/').collect();
            assert!(parts.len() == 4 && parts[..2] == ["", "interaction_profiles"] && named(parts[2]) && named(parts[3]), "{}", profile);
            for &(_, hand, input) in inputs {
                assert!(["", "left", "right"].contains(&hand), "{}: {}", profile, hand);
                assert!(input.split('/').all(named), "{}: {}", profile, input);
            }
            // Each hand points and clicks.
            for side in ["left", "right"] {
                for wanted in [Control::Aim as u8, Control::Select as u8] {
                    assert!(
                        inputs.iter().any(|&(c, h, _)| c as u8 == wanted && (h.is_empty() || h == side)),
                        "{} {}",
                        profile,
                        side
                    );
                }
            }
            // No control bound twice on a hand.
            for (i, &(c, h, _)) in inputs.iter().enumerate() {
                for &(c2, h2, _) in &inputs[i + 1..] {
                    let same_hand = h.is_empty() || h2.is_empty() || h == h2;
                    assert!(!(c as u8 == c2 as u8 && same_hand), "{}", profile);
                }
            }
        }
    }
}

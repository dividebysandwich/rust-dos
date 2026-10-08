//! The controllers OpenXR runtimes know, and what their inputs are bound
//! to.

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
pub const PROFILES: &[(&str, &[(Control, &str, &str)])] = &[
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

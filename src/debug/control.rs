//! Transport-neutral observation boundary. Adapters own wire formats.
#![deny(clippy::all)]

use std::future::Future;
use std::time::Duration;

use crate::keyboard::PcKey;

/// An observation cannot be completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlError {
    Unavailable,
    Closed,
    Timeout,
    Rejected(u16),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("observation unavailable"),
            Self::Closed => f.write_str("the emulator has shut down"),
            Self::Timeout => f.write_str("timed out waiting for the emulator"),
            Self::Rejected(code) => write!(f, "the emulator rejected the observation ({code})"),
        }
    }
}

impl std::error::Error for ControlError {}

/// Correlation and debugger metadata, without an atomic image-epoch promise.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationContext {
    pub instance: String,
    pub request: u64,
    pub state_seq: u64,
    pub journal_seq: u64,
    pub stop_seq: Option<u64>,
    pub mouse_picture: Option<MousePicture>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PictureSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MouseState {
    pub installed: bool,
    pub buttons: u8,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusObservation {
    pub context: ObservationContext,
    pub paused: bool,
    pub program: String,
    pub instructions: u64,
    pub picture: PictureSize,
    pub mouse: MouseState,
}

#[derive(Debug, Eq, PartialEq)]
pub struct CaptureObservation {
    pub context: ObservationContext,
    pub picture: PictureSize,
    pub rgb: Vec<u8>,
}

/// Only read-only observations belong here; input contracts are separate.
pub trait ObservationService {
    type Status;
    type Capture;

    fn status(&self) -> impl Future<Output = Result<Self::Status, ControlError>> + Send;
    fn capture(&self) -> impl Future<Output = Result<Self::Capture, ControlError>> + Send;
}

/// An explicit unavailable backend, without fabricated guest observations.
pub struct UnavailableObservations;

impl ObservationService for UnavailableObservations {
    type Status = ();
    type Capture = ();

    async fn status(&self) -> Result<(), ControlError> {
        Err(ControlError::Unavailable)
    }

    async fn capture(&self) -> Result<(), ControlError> {
        Err(ControlError::Unavailable)
    }
}

/// An input operation uses the actor's authoritative request identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationId {
    pub instance: String,
    pub request: u64,
}

/// A retained input claim; generation changes even on a same-instance restore.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoldHandle {
    pub operation: OperationId,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HoldState {
    Reserved,
    Delivered,
    Released,
    Expired,
    Reset,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoldObservation {
    pub handle: HoldHandle,
    pub state: HoldState,
    /// Remaining monotonic lease at observation, not a wall-clock timestamp.
    pub remaining: Duration,
}

/// Canonical keys come from the existing keyboard table, not another mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyChord {
    pub key: PcKey,
    pub modifiers: Vec<PcKey>,
}

/// The captured picture basis for mouse input, not an atomic rendering epoch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MousePicture {
    pub instance: String,
    pub mode_epoch: u64,
    pub size: PictureSize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Absolute units are captured picture pixels; relative units are guest deltas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseMotion {
    Absolute { x: i32, y: i32 },
    Relative { dx: i32, dy: i32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MouseDragStep {
    pub motion: MouseMotion,
    /// Time held at this point before the following motion or final release.
    pub delay: Duration,
}

/// Mouse ownership and delivery share the existing guest-input boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MouseInput {
    Move {
        picture: MousePicture,
        motion: MouseMotion,
        handle: Option<HoldHandle>,
    },
    Click {
        picture: MousePicture,
        motion: Option<MouseMotion>,
        button: MouseButton,
        duration: Duration,
    },
    Down {
        picture: MousePicture,
        motion: Option<MouseMotion>,
        button: MouseButton,
        lease: Duration,
    },
    Up {
        handle: HoldHandle,
    },
    Renew {
        handle: HoldHandle,
        lease: Duration,
    },
    Drag {
        picture: MousePicture,
        button: MouseButton,
        path: Vec<MouseDragStep>,
    },
}

/// Domain input; wire adapters validate syntax and the actor validates its guest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GuestInput {
    Press { chord: KeyChord, duration: Duration },
    Down { chord: KeyChord, lease: Duration },
    Up { handle: HoldHandle },
    Renew { handle: HoldHandle, lease: Duration },
    Type { text: String, delay: Duration },
    Mouse(MouseInput),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputConflict {
    KeyReserved,
    ModifierInterference,
    LegacyInput,
    MouseReserved,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputError {
    Backend(ControlError),
    Invalid(String),
    Conflict(InputConflict),
    Capacity,
    StaleInstance,
    StalePicture,
    UnknownHold,
    ReleasedHold,
    ExpiredHold,
    ResetHold,
    ExpiredBeforeAcceptance,
}

/// Caller wait completion is independent of accepted input delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputWait {
    Pending,
    Delivered,
    TimedOut,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputOutcome {
    Accepted {
        operation: OperationId,
        context: ObservationContext,
        wait: InputWait,
        held: bool,
        hold: Option<HoldObservation>,
    },
    /// No reply proved acceptance; inspect this ID rather than replay input.
    Unknown { operation: OperationId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputDelivery {
    Pending,
    Delivered,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputOperation {
    pub operation: OperationId,
    pub context: ObservationContext,
    pub delivery: InputDelivery,
    pub hold: Option<HoldObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationError {
    Backend(ControlError),
    StaleInstance,
    Unknown,
    Evicted,
}

pub trait GuestInputService {
    fn input(
        &self,
        input: GuestInput,
    ) -> impl Future<Output = Result<InputOutcome, InputError>> + Send;
}

pub trait OperationQuery {
    fn operation(
        &self,
        id: OperationId,
    ) -> impl Future<Output = Result<InputOperation, OperationError>> + Send;
}

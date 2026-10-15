//! Shared actor submission. Wire adapters translate replies and failures.
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

use super::{Answer, Cmd, Request, Shared};
#[cfg(feature = "mcp")]
mod metrics;
#[cfg(feature = "mcp")]
pub(super) use metrics::Metrics;
use rust_dos::control::{
    CaptureObservation, ControlError, MouseState, ObservationContext, ObservationService,
    PictureSize, StatusObservation,
};

#[derive(Clone, Debug)]
pub(super) struct RequestContext {
    pub(super) req: u64,
    pub(super) tag: Option<Arc<str>>,
    pub(super) if_state: Option<u64>,
    pub(super) deadline: Option<Instant>,
    pub(super) capture: Option<CapturePolicy>,
}

#[derive(Clone, Debug)]
pub(super) struct CapturePolicy {
    pub(super) deadline: Instant,
    pub(super) _retention: Arc<dyn std::fmt::Debug + Send + Sync>,
}

#[derive(Clone)]
pub(super) struct ControlClient {
    pub(super) tx: mpsc::Sender<Request>,
    pub(super) shared: Arc<Shared>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SubmissionError {
    Closed,
    Dropped,
    Timeout,
    Busy,
}

impl ControlClient {
    pub(super) fn observations(&self) -> Observations<'_> {
        let mut context = self.context(None, None);
        context.deadline = Some(Instant::now() + Duration::from_millis(250));
        Observations {
            client: self,
            context,
        }
    }
    pub(super) fn context(&self, tag: Option<Arc<str>>, if_state: Option<u64>) -> RequestContext {
        RequestContext {
            req: self.shared.next_req.fetch_add(1, Ordering::Relaxed),
            tag,
            if_state,
            deadline: None,
            capture: None,
        }
    }

    pub(super) async fn submit(
        &self,
        context: &RequestContext,
        cmd: Cmd,
        timeout: Duration,
    ) -> Result<Answer, SubmissionError> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(SubmissionError::Closed);
        }
        let native_admission = if context.deadline.is_some() {
            Some(
                self.shared
                    .native_commands
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| SubmissionError::Busy)?,
            )
        } else {
            None
        };
        let (reply, receive) = oneshot::channel();
        self.tx
            .send(Request {
                cmd,
                reply,
                req: context.req,
                tag: context.tag.clone(),
                if_state: context.if_state,
                deadline: context.deadline,
                #[cfg(feature = "mcp")]
                queued_at: context.deadline.map(|_| Instant::now()),
                capture: context.capture.clone(),
                _native_admission: native_admission,
            })
            .map_err(|_| SubmissionError::Closed)?;
        match tokio::time::timeout(timeout, receive).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(_)) if self.shared.closed.load(Ordering::Acquire) => {
                Err(SubmissionError::Closed)
            }
            Ok(Err(_)) => Err(SubmissionError::Dropped),
            Err(_) => Err(SubmissionError::Timeout),
        }
    }
}

pub(super) struct Observations<'a> {
    client: &'a ControlClient,
    context: RequestContext,
}

impl Observations<'_> {
    pub(super) fn request_id(&self) -> u64 {
        self.context.req
    }
    pub(super) fn with_deadline(mut self, deadline: Instant) -> Self {
        self.context.deadline = Some(
            self.context
                .deadline
                .map_or(deadline, |actor| actor.min(deadline)),
        );
        self
    }
    pub(super) fn with_capture(mut self, policy: CapturePolicy) -> Self {
        self.context.capture = Some(policy);
        self
    }
    fn metadata(&self, meta: super::Meta) -> ObservationContext {
        ObservationContext {
            instance: self.client.shared.instance_hex(),
            request: self.context.req,
            state_seq: meta.state_seq,
            journal_seq: meta.last,
            stop_seq: meta.stop_seq,
            mouse_picture: {
                #[cfg(feature = "mcp")]
                {
                    meta.mouse_picture.map(|(mode_epoch, width, height)| {
                        rust_dos::control::MousePicture {
                            instance: self.client.shared.instance_hex(),
                            mode_epoch,
                            size: PictureSize { width, height },
                        }
                    })
                }
                #[cfg(not(feature = "mcp"))]
                {
                    None
                }
            },
        }
    }

    async fn answer(&self, command: Cmd) -> Result<Answer, ControlError> {
        let answer = self
            .client
            .submit(&self.context, command, Duration::from_secs(1))
            .await
            .map_err(|error| match error {
                SubmissionError::Closed => ControlError::Closed,
                SubmissionError::Dropped => ControlError::Unavailable,
                SubmissionError::Timeout => ControlError::Timeout,
                SubmissionError::Busy => ControlError::Rejected(503),
            })?;
        match &answer.reply {
            super::Reply::Error(code, _) | super::Reply::ErrorJson(code, _) => {
                Err(ControlError::Rejected(*code))
            }
            _ => Ok(answer),
        }
    }
}

impl ObservationService for Observations<'_> {
    type Status = StatusObservation;
    type Capture = CaptureObservation;

    async fn status(&self) -> Result<StatusObservation, ControlError> {
        let Answer {
            reply: super::Reply::Json(value),
            meta,
        } = self.answer(Cmd::Status).await?
        else {
            return Err(ControlError::Unavailable);
        };
        let invalid = || ControlError::Unavailable;
        let frame = &value["video"]["frame"];
        let mouse = &value["mouse"];
        Ok(StatusObservation {
            context: self.metadata(meta),
            paused: value["paused"].as_bool().ok_or_else(invalid)?,
            program: value["program"].as_str().ok_or_else(invalid)?.to_owned(),
            instructions: value["icount"].as_u64().ok_or_else(invalid)?,
            picture: PictureSize {
                width: frame["width"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(invalid)?,
                height: frame["height"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(invalid)?,
            },
            mouse: MouseState {
                installed: mouse["installed"].as_bool().ok_or_else(invalid)?,
                buttons: mouse["buttons"]
                    .as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(invalid)?,
                x: mouse["x"]
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(invalid)?,
                y: mouse["y"]
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(invalid)?,
            },
        })
    }

    async fn capture(&self) -> Result<CaptureObservation, ControlError> {
        let Answer {
            reply: super::Reply::Frame(frame),
            meta,
        } = self.answer(Cmd::Screenshot).await?
        else {
            return Err(ControlError::Unavailable);
        };
        Ok(CaptureObservation {
            context: self.metadata(meta),
            picture: PictureSize {
                width: frame.width,
                height: frame.height,
            },
            rgb: frame.rgb,
        })
    }
}

/// An in-flight request or stream, included in the existing shutdown drain.
pub(super) struct Busy(Arc<Shared>);

impl Busy {
    #[cfg(feature = "mcp")]
    pub(super) fn shared(&self) -> &Shared {
        &self.0
    }
    pub(super) fn new(shared: &Arc<Shared>) -> Self {
        shared.busy.fetch_add(1, Ordering::AcqRel);
        Self(shared.clone())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.busy.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> (ControlClient, mpsc::Receiver<Request>) {
        let (tx, receive) = mpsc::channel();
        (
            ControlClient {
                tx,
                shared: Arc::new(Shared::new(Instant::now())),
            },
            receive,
        )
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn native_observations_reject_misordered_actual_actor_answers() {
        let picture = crate::debug::server::http_tests::native_guard_answer(Cmd::Screenshot).await;
        let status = crate::debug::server::http_tests::native_guard_answer(Cmd::Status).await;
        let (client, receive) = client();
        let actor = std::thread::spawn(move || {
            let request = receive.recv().unwrap();
            assert!(matches!(request.cmd, Cmd::Status));
            request
                .reply
                .send(picture)
                .unwrap_or_else(|_| panic!("status consumer exists"));
            let request = receive.recv().unwrap();
            assert!(matches!(request.cmd, Cmd::Screenshot));
            request
                .reply
                .send(status)
                .unwrap_or_else(|_| panic!("capture consumer exists"));
        });
        assert!(matches!(
            client.observations().status().await,
            Err(ControlError::Unavailable)
        ));
        assert!(matches!(
            client.observations().capture().await,
            Err(ControlError::Unavailable)
        ));
        actor.join().unwrap();
    }

    #[test]
    fn native_deadline_keeps_the_stricter_actor_bound() {
        let (client, _receive) = client();
        let expired = Instant::now() - Duration::from_millis(1);
        assert_eq!(
            client
                .observations()
                .with_deadline(expired)
                .context
                .deadline,
            Some(expired)
        );
        let observations = client.observations();
        let actor = observations.context.deadline;
        assert_eq!(
            observations
                .with_deadline(Instant::now() + Duration::from_secs(2))
                .context
                .deadline,
            actor
        );
    }

    #[tokio::test]
    async fn contexts_survive_submission_and_reply_retirement() {
        let (client, receive) = client();
        let mut context = client.context(Some(Arc::from("native-observation")), Some(12));
        context.deadline = Some(Instant::now() + Duration::from_secs(5));
        let expected = context.clone();
        let shared = client.shared.clone();
        let actor = std::thread::spawn(move || {
            let request = receive.recv().unwrap();
            assert_eq!(request.req, expected.req);
            assert_eq!(request.tag, expected.tag);
            assert_eq!(request.if_state, expected.if_state);
            assert_eq!(request.deadline, expected.deadline);
            drop(request);
        });
        assert!(matches!(
            client
                .submit(&context, Cmd::Status, Duration::from_secs(1))
                .await,
            Err(SubmissionError::Dropped)
        ));
        actor.join().unwrap();
        let next = client.context(None, None);
        assert!(next.req > context.req);
        assert!(next.deadline.is_none());
        assert_eq!(shared.busy.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn closed_channels_shutdown_and_wait_timeouts_are_distinct() {
        let (client, receive) = client();
        let context = client.context(None, None);
        client.shared.closed.store(true, Ordering::Release);
        assert!(matches!(
            client
                .submit(&context, Cmd::Status, Duration::from_secs(1))
                .await,
            Err(SubmissionError::Closed)
        ));
        assert!(matches!(receive.try_recv(), Err(mpsc::TryRecvError::Empty)));
        client.shared.closed.store(false, Ordering::Release);
        drop(receive);
        assert!(matches!(
            client
                .submit(&context, Cmd::Status, Duration::from_secs(1))
                .await,
            Err(SubmissionError::Closed)
        ));

        let (client, receive) = self::client();
        let context = client.context(None, None);
        assert!(matches!(
            client
                .submit(&context, Cmd::Status, Duration::from_millis(1))
                .await,
            Err(SubmissionError::Timeout)
        ));
        let request = receive.try_recv().unwrap();
        assert_eq!(
            request.req, context.req,
            "timeout does not remove queued work"
        );
        drop(request);

        let shared = client.shared.clone();
        let actor = std::thread::spawn(move || {
            let request = receive.recv().unwrap();
            shared.closed.store(true, Ordering::Release);
            drop(request);
        });
        assert!(matches!(
            client
                .submit(&context, Cmd::Status, Duration::from_secs(1))
                .await,
            Err(SubmissionError::Closed)
        ));
        actor.join().unwrap();
    }

    #[test]
    fn busy_guards_count_until_their_own_drop() {
        let shared = Arc::new(Shared::new(Instant::now()));
        let first = Busy::new(&shared);
        let second = Busy::new(&shared);
        assert_eq!(shared.busy.load(Ordering::Acquire), 2);
        drop(first);
        assert_eq!(shared.busy.load(Ordering::Acquire), 1);
        drop(second);
        assert_eq!(shared.busy.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn native_admission_is_retained_by_queued_requests_after_timeout() {
        let (client, receive) = client();
        for _ in 0..8 {
            let mut context = client.context(None, None);
            context.deadline = Some(Instant::now() + Duration::from_secs(5));
            assert!(matches!(
                client.submit(&context, Cmd::Status, Duration::ZERO).await,
                Err(SubmissionError::Timeout)
            ));
        }
        let mut context = client.context(None, None);
        context.deadline = Some(Instant::now() + Duration::from_secs(5));
        assert!(
            matches!(
                client.submit(&context, Cmd::Status, Duration::ZERO).await,
                Err(SubmissionError::Busy)
            ),
            "ninth native request is refused before enqueue"
        );
        let queued = receive.try_recv().unwrap();
        drop(queued);
        assert!(
            matches!(
                client.submit(&context, Cmd::Status, Duration::ZERO).await,
                Err(SubmissionError::Timeout)
            ),
            "retiring one queued request restores one slot"
        );
    }
}

#[cfg(feature = "mcp")]
pub(super) struct Inputs<'a> {
    client: &'a ControlClient,
    context: RequestContext,
}

#[cfg(feature = "mcp")]
impl ControlClient {
    pub(super) fn inputs(&self, deadline: Instant) -> Inputs<'_> {
        let mut context = self.context(None, None);
        context.deadline = Some(deadline.min(Instant::now() + Duration::from_millis(250)));
        Inputs {
            client: self,
            context,
        }
    }
}

#[cfg(feature = "mcp")]
impl rust_dos::control::GuestInputService for Inputs<'_> {
    async fn input(
        &self,
        input: rust_dos::control::GuestInput,
    ) -> Result<rust_dos::control::InputOutcome, rust_dos::control::InputError> {
        use rust_dos::control::{
            HoldObservation, HoldState, InputConflict, InputError, InputOutcome, InputWait,
            OperationId,
        };
        let operation = OperationId {
            instance: self.client.shared.instance_hex(),
            request: self.context.req,
        };
        let job = if matches!(
            &input,
            rust_dos::control::GuestInput::Up { .. }
                | rust_dos::control::GuestInput::Renew { .. }
                | rust_dos::control::GuestInput::Mouse(
                    rust_dos::control::MouseInput::Up { .. }
                        | rust_dos::control::MouseInput::Renew { .. }
                )
        ) {
            None
        } else {
            Some(Arc::new(super::owned_input::Job {
                _permit: self
                    .client
                    .shared
                    .native_jobs
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| InputError::Capacity)?,
                _busy: Busy::new(&self.client.shared),
                state: std::sync::atomic::AtomicU8::new(0),
            }))
        };
        let answer = match self
            .client
            .submit(
                &self.context,
                Cmd::NativeInput { input, job },
                Duration::from_secs(1),
            )
            .await
        {
            Ok(answer) => answer,
            Err(SubmissionError::Timeout | SubmissionError::Dropped) => {
                return Ok(InputOutcome::Unknown { operation });
            }
            Err(SubmissionError::Closed) => return Err(InputError::Backend(ControlError::Closed)),
            Err(SubmissionError::Busy) => return Err(InputError::Capacity),
        };
        let value = match answer.reply {
            super::Reply::Json(value) => value,
            super::Reply::Error(400, message) => return Err(InputError::Invalid(message)),
            super::Reply::Error(429, _) => return Err(InputError::Capacity),
            super::Reply::Error(408, _) => return Err(InputError::ExpiredBeforeAcceptance),
            super::Reply::Error(409, message) => {
                return Err(match message.as_str() {
                    "stale input instance" => InputError::StaleInstance,
                    "stale mouse picture; capture again" => InputError::StalePicture,
                    "hold reset" => InputError::ResetHold,
                    "hold expired" => InputError::ExpiredHold,
                    "hold released" => InputError::ReleasedHold,
                    "effective modifier conflict" => {
                        InputError::Conflict(InputConflict::ModifierInterference)
                    }
                    "key reserved" | "key or modifier reserved" => {
                        InputError::Conflict(InputConflict::KeyReserved)
                    }
                    "legacy mouse button reserved" => {
                        InputError::Conflict(InputConflict::LegacyInput)
                    }
                    "mouse pointer or button reserved" => {
                        InputError::Conflict(InputConflict::MouseReserved)
                    }
                    "unknown hold" => InputError::UnknownHold,
                    "unknown mouse hold" => InputError::UnknownHold,
                    _ => InputError::Backend(ControlError::Rejected(409)),
                });
            }
            super::Reply::Error(code, _) | super::Reply::ErrorJson(code, _) => {
                return Err(InputError::Backend(ControlError::Rejected(code)));
            }
            _ => return Err(InputError::Backend(ControlError::Unavailable)),
        };
        let hold = value
            .get("hold")
            .map(|value| {
                let handle =
                    super::owned_input::parse_token(value["handle"].as_str().unwrap_or_default())
                        .ok_or(InputError::Backend(ControlError::Unavailable))?;
                let state = match value["state"].as_str() {
                    Some("reserved") => HoldState::Reserved,
                    Some("delivered") => HoldState::Delivered,
                    Some("released") => HoldState::Released,
                    Some("expired") => HoldState::Expired,
                    Some("reset") => HoldState::Reset,
                    _ => return Err(InputError::Backend(ControlError::Unavailable)),
                };
                Ok(HoldObservation {
                    handle,
                    state,
                    remaining: Duration::from_millis(
                        value["remaining_ms"]
                            .as_u64()
                            .ok_or(InputError::Backend(ControlError::Unavailable))?,
                    ),
                })
            })
            .transpose()?;
        Ok(InputOutcome::Accepted {
            operation,
            context: ObservationContext {
                instance: self.client.shared.instance_hex(),
                request: self.context.req,
                state_seq: answer.meta.state_seq,
                journal_seq: answer.meta.last,
                stop_seq: answer.meta.stop_seq,
                mouse_picture: None,
            },
            wait: InputWait::Pending,
            held: value["held"].as_bool().unwrap_or(false),
            hold,
        })
    }
}

#[cfg(feature = "mcp")]
impl rust_dos::control::OperationQuery for Inputs<'_> {
    async fn operation(
        &self,
        id: rust_dos::control::OperationId,
    ) -> Result<rust_dos::control::InputOperation, rust_dos::control::OperationError> {
        use rust_dos::control::{
            HoldObservation, HoldState, InputDelivery, InputOperation, OperationError,
        };
        let answer = self
            .client
            .submit(
                &self.context,
                Cmd::NativeOperation(id.clone()),
                Duration::from_secs(1),
            )
            .await
            .map_err(|error| {
                OperationError::Backend(match error {
                    SubmissionError::Closed => ControlError::Closed,
                    SubmissionError::Timeout => ControlError::Timeout,
                    SubmissionError::Busy => ControlError::Rejected(503),
                    SubmissionError::Dropped => ControlError::Unavailable,
                })
            })?;
        let value = match answer.reply {
            super::Reply::Json(value) => value,
            super::Reply::Error(404, _) => return Err(OperationError::Unknown),
            super::Reply::Error(410, _) => return Err(OperationError::Evicted),
            super::Reply::Error(409, _) => return Err(OperationError::StaleInstance),
            _ => return Err(OperationError::Backend(ControlError::Unavailable)),
        };
        let delivery = match value["delivery"].as_str() {
            Some("pending") => InputDelivery::Pending,
            Some("delivered") => InputDelivery::Delivered,
            Some("cancelled") => InputDelivery::Cancelled,
            Some("failed") => InputDelivery::Failed,
            _ => return Err(OperationError::Backend(ControlError::Unavailable)),
        };
        let hold = value
            .get("hold")
            .map(|value| {
                let handle =
                    super::owned_input::parse_token(value["handle"].as_str().unwrap_or_default())
                        .ok_or(OperationError::Backend(ControlError::Unavailable))?;
                let state = match value["state"].as_str() {
                    Some("reserved") => HoldState::Reserved,
                    Some("delivered") => HoldState::Delivered,
                    Some("released") => HoldState::Released,
                    Some("expired") => HoldState::Expired,
                    Some("reset") => HoldState::Reset,
                    _ => return Err(OperationError::Backend(ControlError::Unavailable)),
                };
                Ok(HoldObservation {
                    handle,
                    state,
                    remaining: Duration::from_millis(
                        value["remaining_ms"]
                            .as_u64()
                            .ok_or(OperationError::Backend(ControlError::Unavailable))?,
                    ),
                })
            })
            .transpose()?;
        Ok(InputOperation {
            operation: id,
            context: ObservationContext {
                instance: self.client.shared.instance_hex(),
                request: self.context.req,
                state_seq: answer.meta.state_seq,
                journal_seq: answer.meta.last,
                stop_seq: answer.meta.stop_seq,
                mouse_picture: None,
            },
            delivery,
            hold,
        })
    }
}

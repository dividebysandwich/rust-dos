//! Admission and owned SDK work, including retained HTTP response bytes.
use super::{NativeObservations, sessions::Sessions};
use crate::debug::control::ControlClient;
use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use http_body::{Body as HttpBody, Frame, SizeHint};
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use tower_service::Service;

pub(super) struct Limits {
    pub(super) requests: Arc<Semaphore>,
    pub(super) captures: Arc<Semaphore>,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            requests: Arc::new(Semaphore::new(16)),
            captures: Arc::new(Semaphore::new(1)),
        }
    }
}

#[derive(Clone)]
pub(super) struct AdmissionLease {
    _permit: Arc<OwnedSemaphorePermit>,
}
impl AdmissionLease {
    pub(super) fn new(permit: Arc<OwnedSemaphorePermit>) -> Self {
        Self { _permit: permit }
    }
}
#[derive(Clone, Copy)]
pub(super) struct Deadline(pub(super) Instant);

#[derive(Default)]
struct CaptureState {
    permit: Option<Arc<OwnedSemaphorePermit>>,
    closed: bool,
}
#[derive(Clone, Default)]
pub(super) struct CaptureHolder(Arc<Mutex<CaptureState>>);
impl CaptureHolder {
    #[cfg(test)]
    pub(super) fn close_for_test(&self) {
        self.close();
    }
    pub(super) fn retain(&self, permit: Arc<OwnedSemaphorePermit>) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || state.permit.is_some() {
            return false;
        }
        state.permit = Some(permit);
        true
    }
    fn close(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        state.permit.take();
    }
    fn permit(&self) -> Option<Arc<OwnedSemaphorePermit>> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .permit
            .clone()
    }
}
struct CaptureOwner(CaptureHolder);
impl Drop for CaptureOwner {
    fn drop(&mut self) {
        self.0.close();
    }
}

struct PermitBody {
    inner: Body,
    admission: Arc<OwnedSemaphorePermit>,
    capture: CaptureOwner,
    emitted: usize,
}
type BodyFrame = Option<Result<Frame<Bytes>, axum::Error>>;

struct FrameOwner {
    bytes: Bytes,
    _admission: Arc<OwnedSemaphorePermit>,
    _capture: Option<Arc<OwnedSemaphorePermit>>,
}
impl AsRef<[u8]> for FrameOwner {
    fn as_ref(&self) -> &[u8] {
        self.bytes.as_ref()
    }
}

struct WorkerBody {
    inner: Option<PermitBody>,
    pending: Option<tokio::task::JoinHandle<(PermitBody, BodyFrame)>>,
    cancellation: CancellationToken,
    deadline: tokio::time::Instant,
}
impl Drop for WorkerBody {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl HttpBody for WorkerBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<BodyFrame> {
        let this = self.get_mut();
        if this.pending.is_none() {
            let Some(mut inner) = this.inner.take() else {
                return Poll::Ready(None);
            };
            let runtime = tokio::runtime::Handle::current();
            let cancellation = this.cancellation.clone();
            let deadline = this.deadline;
            this.pending = Some(tokio::task::spawn_blocking(move || {
                let frame = if tokio::time::Instant::now() >= deadline
                    || cancellation.is_cancelled()
                {
                    None
                } else {
                    runtime.block_on(async {
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => None,
                        _ = tokio::time::sleep_until(deadline) => None,
                        frame = std::future::poll_fn(|cx| Pin::new(&mut inner.inner).poll_frame(cx)) => frame,
                    }
                    })
                };
                let frame = frame.map(|result| {
                    result.and_then(|frame| match frame.into_data() {
                        Ok(bytes) => {
                            inner.emitted =
                                inner.emitted.checked_add(bytes.len()).ok_or_else(|| {
                                    axum::Error::new(std::io::Error::other(
                                        "native response size overflow",
                                    ))
                                })?;
                            if inner.emitted > 24 * 1024 * 1024 {
                                return Err(axum::Error::new(std::io::Error::other(
                                    "native response limit",
                                )));
                            }
                            let capture = inner.capture.0.permit();
                            Ok(Frame::data(Bytes::from_owner(FrameOwner {
                                bytes,
                                _admission: inner.admission.clone(),
                                _capture: capture,
                            })))
                        }
                        Err(frame) => Ok(frame),
                    })
                });
                (inner, frame)
            }));
        }
        let completed = std::task::ready!(std::future::Future::poll(
            Pin::new(this.pending.as_mut().expect("body worker exists")),
            cx
        ));
        this.pending.take();
        match completed {
            Ok((inner, frame)) => {
                if frame.as_ref().is_some_and(Result::is_ok) {
                    this.inner = Some(inner);
                }
                Poll::Ready(frame)
            }
            Err(error) => Poll::Ready(Some(Err(axum::Error::new(error)))),
        }
    }
    fn is_end_stream(&self) -> bool {
        self.pending.is_none()
            && self
                .inner
                .as_ref()
                .is_none_or(|inner| inner.inner.is_end_stream())
    }
    fn size_hint(&self) -> SizeHint {
        self.inner
            .as_ref()
            .map_or_else(SizeHint::default, |inner| inner.inner.size_hint())
    }
}

pub(super) struct NativeState {
    pub(super) control: ControlClient,
    pub(super) limits: Arc<Limits>,
    pub(super) sessions: Arc<Sessions>,
    pub(super) config: StreamableHttpServerConfig,
}

pub(super) async fn handle(State(state): State<Arc<NativeState>>, request: Request) -> Response {
    let deadline = Instant::now() + Duration::from_secs(2);
    let Ok(admission) = state.limits.requests.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if request
        .headers()
        .get("content-encoding")
        .is_some_and(|value| value != "identity")
    {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let header_bytes = request
        .headers()
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len() + 4)
        .sum::<usize>();
    if request.headers().len() > 32 || header_bytes > 8192 {
        return StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE.into_response();
    }
    let (mut parts, body) = request.into_parts();
    let bytes =
        match tokio::time::timeout_at(deadline.into(), axum::body::to_bytes(body, 64 * 1024)).await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(_)) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
            Err(_) => return StatusCode::REQUEST_TIMEOUT.into_response(),
        };
    let capture = CaptureOwner(CaptureHolder::default());
    let admission = Arc::new(admission);
    parts
        .extensions
        .insert(AdmissionLease::new(admission.clone()));
    parts.extensions.insert(capture.0.clone());
    parts.extensions.insert(Deadline(deadline));
    let request = Request::from_parts(parts, Body::from(bytes));
    let runtime = tokio::runtime::Handle::current();
    let (response, admission) = match tokio::task::spawn_blocking(move || {
        let response = if Instant::now() >= deadline {
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        } else {
            let control = state.control.clone();
            let limits = state.limits.clone();
            let mut service = StreamableHttpService::new(
                move || {
                    Ok(NativeObservations {
                        control: control.clone(),
                        limits: limits.clone(),
                    })
                },
                state.sessions.clone(),
                state.config.clone(),
            );
            match runtime.block_on(service.call(request)) {
                Ok(response) => response.map(Body::new),
                Err(infallible) => match infallible {},
            }
        };
        (response, admission)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    response.map(|inner| {
        Body::new(WorkerBody {
            inner: Some(PermitBody {
                inner,
                admission,
                capture,
                emitted: 0,
            }),
            pending: None,
            cancellation: CancellationToken::new(),
            deadline: deadline.into(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned_body(limits: &Limits, bytes: Bytes) -> WorkerBody {
        let holder = CaptureHolder::default();
        assert!(holder.retain(Arc::new(
            limits.captures.clone().try_acquire_owned().unwrap()
        )));
        WorkerBody {
            inner: Some(PermitBody {
                inner: Body::from(bytes),
                admission: Arc::new(limits.requests.clone().try_acquire_owned().unwrap()),
                capture: CaptureOwner(holder),
                emitted: 0,
            }),
            pending: None,
            cancellation: CancellationToken::new(),
            deadline: tokio::time::Instant::now() + Duration::from_secs(2),
        }
    }

    fn actual_picture() -> Bytes {
        Bytes::from(crate::debug::server::http_tests::native_guard_picture())
    }

    #[test]
    fn native_queued_sdk_job_keeps_admission_and_rejects_expired_start() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let limits = Arc::new(Limits::default());
            let (tx, receive) = std::sync::mpsc::channel();
            let state = Arc::new(NativeState {
                control: ControlClient {
                    tx,
                    shared: Arc::new(crate::debug::Shared::new(Instant::now())),
                },
                limits: limits.clone(),
                sessions: Arc::new(Sessions::default()),
                config: StreamableHttpServerConfig::default(),
            });
            let (started, ready) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                wait.recv().unwrap();
            });
            ready.await.unwrap();
            let request = tokio::spawn(handle(State(state), Request::new(Body::empty())));
            let deadline = Instant::now() + Duration::from_secs(1);
            while limits.requests.available_permits() == 16 {
                assert!(
                    Instant::now() < deadline,
                    "native request acquires admission"
                );
                tokio::task::yield_now().await;
            }
            tokio::time::sleep(Duration::from_millis(2100)).await;
            assert_eq!(
                limits.requests.available_permits(),
                15,
                "queued work retains admission after start deadline"
            );
            release.send(()).unwrap();
            blocker.await.unwrap();
            let response = request.await.unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert!(
                receive.try_recv().is_err(),
                "expired SDK work never reaches actor"
            );
            drop(response);
            assert_eq!(
                limits.requests.available_permits(),
                16,
                "retired SDK job and response release capacity"
            );
        });
    }

    #[test]
    fn native_capture_holder_rejects_duplicate_and_retired_owners() {
        let limits = Limits::default();
        let permit = Arc::new(limits.captures.clone().try_acquire_owned().unwrap());
        let holder = CaptureHolder::default();
        assert!(holder.retain(permit.clone()));
        assert!(!holder.retain(permit.clone()));
        holder.close();
        assert!(!holder.retain(permit.clone()));
        drop(permit);
        assert_eq!(limits.captures.available_permits(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_cancelled_body_retires_capacity_without_emitting_bytes() {
        let limits = Limits::default();
        let mut body = owned_body(&limits, actual_picture());
        body.cancellation.cancel();
        assert!(
            futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
                .await
                .is_none()
        );
        assert!(body.is_end_stream());
        assert_eq!(limits.captures.available_permits(), 1);
        assert_eq!(limits.requests.available_permits(), 16);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_pending_body_is_not_an_end_stream_and_keeps_capacity() {
        let limits = Limits::default();
        let mut body = owned_body(&limits, actual_picture());
        let picture = actual_picture();
        let (send, receive) = tokio::sync::oneshot::channel::<()>();
        body.inner.as_mut().unwrap().inner =
            Body::from_stream(futures_util::stream::once(async move {
                receive.await.unwrap();
                Ok::<_, std::io::Error>(picture)
            }));
        std::future::poll_fn(|cx| {
            assert!(Pin::new(&mut body).poll_frame(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(!body.is_end_stream());
        assert_eq!(limits.captures.available_permits(), 0);
        assert_eq!(limits.requests.available_permits(), 15);
        send.send(()).unwrap();
        let frame = futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
            .await
            .unwrap()
            .unwrap();
        assert!(frame.is_data());
        drop(frame);
        drop(body);
        assert_eq!(limits.captures.available_permits(), 1);
        assert_eq!(limits.requests.available_permits(), 16);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_expired_body_retires_capacity_without_emitting_bytes() {
        let limits = Limits::default();
        let picture = actual_picture();
        let size = picture.len() as u64;
        let mut body = owned_body(&limits, picture);
        assert!(!body.is_end_stream());
        assert_eq!(body.size_hint().exact(), Some(size));
        body.deadline = tokio::time::Instant::now();
        assert!(
            futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
                .await
                .is_none()
        );
        assert!(body.is_end_stream());
        assert_eq!(body.size_hint().lower(), 0);
        assert!(
            futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
                .await
                .is_none()
        );
        assert_eq!(limits.captures.available_permits(), 1);
        assert_eq!(limits.requests.available_permits(), 16);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_response_budget_rejects_growth_and_retires_capacity() {
        let limits = Limits::default();
        let mut body = owned_body(&limits, actual_picture());
        body.inner.as_mut().unwrap().emitted = 24 * 1024 * 1024;
        let result = futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
            .await
            .unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("native response limit")
        );
        assert!(body.is_end_stream());
        assert_eq!(limits.captures.available_permits(), 1);
        assert_eq!(limits.requests.available_permits(), 16);
    }

    #[tokio::test]
    async fn native_request_admission_rejects_before_polling_upload() {
        let limits = Arc::new(Limits::default());
        let mut owners = Vec::new();
        for _ in 0..16 {
            owners.push(limits.requests.clone().try_acquire_owned().unwrap());
        }
        let (tx, receive) = std::sync::mpsc::channel();
        let state = Arc::new(NativeState {
            control: ControlClient {
                tx,
                shared: Arc::new(crate::debug::Shared::new(Instant::now())),
            },
            limits: limits.clone(),
            sessions: Arc::new(Sessions::default()),
            config: StreamableHttpServerConfig::default(),
        });
        let polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = polled.clone();
        let body = Body::from_stream(futures_util::stream::once(async move {
            observed.store(true, std::sync::atomic::Ordering::Release);
            Err::<Bytes, _>(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "injected upload failure",
            ))
        }));
        let response = handle(State(state), Request::new(body)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(!polled.load(std::sync::atomic::Ordering::Acquire));
        assert!(receive.try_recv().is_err(), "rejection never reaches actor");
        drop(owners);
        assert_eq!(limits.requests.available_permits(), 16);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_emitted_byte_clones_retain_capture_and_request_admission() {
        let limits = Limits::default();
        let holder = CaptureHolder::default();
        assert!(holder.retain(Arc::new(
            limits.captures.clone().try_acquire_owned().unwrap()
        )));
        let mut body = WorkerBody {
            inner: Some(PermitBody {
                inner: Body::from(actual_picture()),
                admission: Arc::new(limits.requests.clone().try_acquire_owned().unwrap()),
                capture: CaptureOwner(holder),
                emitted: 0,
            }),
            pending: None,
            cancellation: CancellationToken::new(),
            deadline: tokio::time::Instant::now() + Duration::from_secs(2),
        };
        let data = futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
            .await
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        let last_clone = data.clone();
        drop(body);
        drop(data);
        assert_eq!(
            limits.captures.available_permits(),
            0,
            "emitted byte clone owns capture admission"
        );
        assert_eq!(
            limits.requests.available_permits(),
            15,
            "emitted byte clone owns request admission"
        );
        drop(last_clone);
        assert_eq!(
            limits.captures.available_permits(),
            1,
            "last byte owner retires capture admission"
        );
        assert_eq!(
            limits.requests.available_permits(),
            16,
            "last byte owner retires request admission"
        );
    }
}

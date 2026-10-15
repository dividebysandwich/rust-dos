//! Bounded legacy sessions and retirement of terminal SDK delivery channels.
use super::transport::AdmissionLease;
use futures_util::{Stream, stream};
use rmcp::{
    RoleServer,
    model::{ClientJsonRpcMessage, GetExtensions, RequestId, ServerJsonRpcMessage},
    transport::{
        Transport,
        streamable_http_server::session::{
            ServerSseMessage, SessionId, SessionManager,
            local::{LocalSessionHandle, LocalSessionManager},
        },
    },
};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::OwnedSemaphorePermit;

#[derive(Clone)]
pub(super) struct DeliveryTicket {
    _admission: AdmissionLease,
    pub(super) capture: Arc<Mutex<Option<Arc<OwnedSemaphorePermit>>>>,
    channel: Arc<Mutex<Option<u64>>>,
}
type TicketBook = Arc<Mutex<HashMap<RequestId, DeliveryTicket>>>;

pub(super) struct RetiringTransport {
    inner: <LocalSessionManager as SessionManager>::Transport,
    handle: LocalSessionHandle,
    book: TicketBook,
}

fn error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

impl Transport<RoleServer> for RetiringTransport {
    type Error = io::Error;

    fn send(
        &mut self,
        message: ServerJsonRpcMessage,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        let id = match &message {
            ServerJsonRpcMessage::Response(response) => Some(response.id.clone()),
            ServerJsonRpcMessage::Error(error) => error.id.clone(),
            _ => None,
        };
        let ticket = id.and_then(|id| {
            self.book
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&id)
                .cloned()
                .map(|ticket| (id, ticket))
        });
        let send = self.inner.send(message);
        let handle = self.handle.clone();
        let book = self.book.clone();
        async move {
            let sent = send.await.map_err(error);
            if let Some((id, ticket)) = ticket {
                let channel = *ticket.channel.lock().unwrap_or_else(|e| e.into_inner());
                let retired = match channel {
                    Some(channel) => handle
                        .close_request_wise_channel(channel)
                        .await
                        .map_err(error),
                    None => Err(error("native delivery channel missing")),
                };
                book.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                drop(ticket);
                sent.and(retired)
            } else {
                sent
            }
        }
    }

    async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
        self.inner.receive().await
    }
    async fn close(&mut self) -> Result<(), Self::Error> {
        let result = self.inner.close().await.map_err(error);
        self.book.lock().unwrap_or_else(|e| e.into_inner()).clear();
        result
    }
}

pub(super) struct Sessions {
    inner: LocalSessionManager,
    creating: tokio::sync::Mutex<()>,
    seen: Mutex<HashMap<SessionId, Instant>>,
    books: Mutex<HashMap<SessionId, TicketBook>>,
}

impl Default for Sessions {
    fn default() -> Self {
        let mut inner = LocalSessionManager::default();
        inner.session_config.channel_capacity = 1;
        inner.session_config.completed_cache_ttl = Duration::ZERO;
        inner.session_config.keep_alive = Some(Duration::from_secs(300));
        Self {
            inner,
            creating: tokio::sync::Mutex::new(()),
            seen: Mutex::new(HashMap::new()),
            books: Mutex::new(HashMap::new()),
        }
    }
}

impl Sessions {
    fn touch(&self, id: &SessionId) {
        if let Some(seen) = self
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(id)
        {
            *seen = Instant::now();
        }
    }

    pub(super) async fn reclaim_expired(&self) -> io::Result<()> {
        let expired: Vec<_> = self
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, time)| time.elapsed() >= Duration::from_secs(300))
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.close_session(&id).await?;
        }
        Ok(())
    }

    pub(super) async fn shutdown(&self) {
        let ids: Vec<_> = self
            .seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        for id in ids {
            let _ = self.close_session(&id).await;
        }
    }
}

impl SessionManager for Sessions {
    type Error = io::Error;
    type Transport = RetiringTransport;

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        let _creating = self
            .creating
            .try_lock()
            .map_err(|_| error("session creation busy"))?;
        self.reclaim_expired().await?;
        if self.inner.sessions.read().await.len() >= 8 {
            return Err(error("native session limit"));
        }
        let (id, inner) = self.inner.create_session().await.map_err(error)?;
        let handle = self
            .inner
            .sessions
            .read()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(|| error("created session missing"))?;
        let book = Arc::new(Mutex::new(HashMap::new()));
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), Instant::now());
        self.books
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), book.clone());
        Ok((
            id,
            RetiringTransport {
                inner,
                handle,
                book,
            },
        ))
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        self.inner
            .initialize_session(id, message)
            .await
            .map_err(error)
    }
    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        self.touch(id);
        self.inner.has_session(id).await.map_err(error)
    }
    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        self.books
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        self.inner.close_session(id).await.map_err(error)
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        mut message: ClientJsonRpcMessage,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        let handle = self
            .inner
            .sessions
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| error("session missing"))?;
        let book = self
            .books
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| error("session delivery state missing"))?;
        let ClientJsonRpcMessage::Request(request) = &mut message else {
            return Err(error("stream requires request"));
        };
        let request_id = request.id.clone();
        let admission = request
            .request
            .extensions_mut()
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<AdmissionLease>())
            .cloned()
            .ok_or_else(|| error("native admission missing"))?;
        let ticket = DeliveryTicket {
            _admission: admission,
            capture: Arc::new(Mutex::new(None)),
            channel: Arc::new(Mutex::new(None)),
        };
        {
            let mut book = book.lock().unwrap_or_else(|e| e.into_inner());
            if book.len() >= 16 || book.contains_key(&request_id) {
                return Err(error("native request already active or full"));
            }
            book.insert(request_id.clone(), ticket.clone());
        }
        let receiver = match handle.establish_request_wise_channel().await {
            Ok(receiver) => receiver,
            Err(failure) => {
                book.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&request_id);
                return Err(error(failure));
            }
        };
        let Some(channel) = receiver.http_request_id else {
            book.lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&request_id);
            return Err(error("native request channel missing"));
        };
        ticket
            .channel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(channel);
        request.request.extensions_mut().insert(ticket);
        if let Err(failure) = handle.push_message(message, Some(channel)).await {
            book.lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&request_id);
            let _ = handle.close_request_wise_channel(channel).await;
            return Err(error(failure));
        }
        let mut receiver = receiver.inner;
        Ok(stream::poll_fn(move |cx| receiver.poll_recv(cx)))
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        self.inner.accept_message(id, message).await.map_err(error)
    }
    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.inner.create_standalone_stream(id).await.map_err(error)
    }
    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.inner.resume(id, last_event_id).await.map_err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_terminal_send_keeps_request_id_reserved_until_retirement() {
        use super::super::transport::{AdmissionLease, Limits};
        use futures_util::StreamExt;
        use std::{future::Future, task::Poll};
        let (mut actual, initialize, initialize_reply, terminal_reply) =
            crate::debug::server::http_tests::native_guard_sdk_capture_post().await;
        let limits = Limits::default();
        let (mut parts, _) = axum::http::Request::builder()
            .uri(actual.uri.as_ref())
            .body(())
            .unwrap()
            .into_parts();
        parts.headers = actual.headers.into_iter().collect();
        parts.extensions.insert(AdmissionLease::new(Arc::new(
            limits.requests.clone().try_acquire_owned().unwrap(),
        )));
        let ClientJsonRpcMessage::Request(request) = &mut actual.message else {
            panic!("actual SDK producer emitted a request");
        };
        let request_id = request.id.clone();
        request.request.extensions_mut().insert(parts);
        let sessions = Sessions::default();
        let (id, mut transport) = sessions.create_session().await.unwrap();
        let initialization = sessions.initialize_session(&id, initialize.message);
        let initialize_send = async {
            assert!(matches!(
                transport.receive().await,
                Some(ClientJsonRpcMessage::Request(_))
            ));
            transport.send(initialize_reply).await.unwrap();
        };
        let (initialized, ()) = tokio::join!(initialization, initialize_send);
        initialized.unwrap();
        let duplicate = actual.message.clone();
        let mut original = Box::pin(sessions.create_stream(&id, actual.message).await.unwrap());
        assert!(matches!(
            transport.receive().await,
            Some(ClientJsonRpcMessage::Request(_))
        ));
        let expected = serde_json::to_value(&terminal_reply).unwrap();
        let mut terminal_send = Box::pin(transport.send(terminal_reply));
        // Deliberately hold the real send future before its first poll.
        assert!(
            transport.book.lock().unwrap().contains_key(&request_id),
            "terminal send retains the request ID until channel retirement"
        );
        std::future::poll_fn(|cx| {
            assert!(
                terminal_send.as_mut().poll(cx).is_pending(),
                "actual queued send awaits SDK channel retirement"
            );
            Poll::Ready(())
        })
        .await;
        assert!(
            transport.book.lock().unwrap().contains_key(&request_id),
            "request ID remains reserved after send starts and before retirement finishes"
        );
        let rejected = match sessions.create_stream(&id, duplicate).await {
            Ok(_) => panic!("same request ID must not replace an active terminal delivery"),
            Err(error) => error,
        };
        assert!(rejected.to_string().contains("already active"));
        let receive_original = async {
            let mut replies = 0;
            while let Some(event) = original.next().await {
                if let Some(message) = event.message {
                    assert!(
                        serde_json::to_value(message.as_ref()).unwrap() == expected,
                        "actual terminal response reaches its original stream"
                    );
                    replies += 1;
                }
            }
            assert_eq!(replies, 1);
        };
        let (sent, ()) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(terminal_send, receive_original)
        })
        .await
        .expect("actual send and retirement complete within their bound");
        sent.unwrap();
        assert!(transport.book.lock().unwrap().is_empty());
        assert_eq!(limits.requests.available_permits(), 16);
        sessions.shutdown().await;
    }

    #[tokio::test]
    async fn native_session_closure_between_establishment_and_push_retires_ticket() {
        use super::super::transport::{AdmissionLease, Limits};
        use std::{future::Future, task::Poll};
        let (mut actual, initialize, initialize_reply, _) =
            crate::debug::server::http_tests::native_guard_sdk_capture_post().await;
        let limits = Limits::default();
        let (mut parts, _) = axum::http::Request::builder()
            .uri(actual.uri.as_ref())
            .body(())
            .unwrap()
            .into_parts();
        parts.headers = actual.headers.into_iter().collect();
        parts.extensions.insert(AdmissionLease::new(Arc::new(
            limits.requests.clone().try_acquire_owned().unwrap(),
        )));
        let ClientJsonRpcMessage::Request(request) = &mut actual.message else {
            panic!("actual SDK producer emitted a request");
        };
        request.request.extensions_mut().insert(parts);
        let sessions = Sessions::default();
        let (id, mut transport) = sessions.create_session().await.unwrap();
        let initialization = sessions.initialize_session(&id, initialize.message);
        let reply = async {
            assert!(
                matches!(
                    transport.receive().await,
                    Some(ClientJsonRpcMessage::Request(_))
                ),
                "actual SDK initialize reaches the transport"
            );
            transport.send(initialize_reply).await.unwrap();
        };
        let (initialized, ()) = tokio::join!(initialization, reply);
        assert!(
            initialized.is_ok(),
            "replayed actual native handshake initializes SDK session"
        );
        let handle = sessions
            .inner
            .sessions
            .read()
            .await
            .get(&id)
            .unwrap()
            .clone();
        let mut creation = Box::pin(sessions.create_stream(&id, actual.message));
        std::future::poll_fn(|cx| {
            assert!(
                creation.as_mut().poll(cx).is_pending(),
                "pause while SDK establishes request channel"
            );
            Poll::Ready(())
        })
        .await;
        assert_eq!(transport.book.lock().unwrap().len(), 1);
        // This control event is queued after establishment. Stop polling the
        // request until the actual SDK worker processes both events and exits.
        handle.close().await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while handle.close().await.is_ok() {
            assert!(Instant::now() < deadline, "actual SDK worker terminates");
            tokio::task::yield_now().await;
        }
        let failure = match creation.await {
            Ok(_) => panic!("closed worker cannot accept request"),
            Err(error) => error,
        };
        assert!(failure.to_string().contains("terminated"), "{failure}");
        assert!(
            transport.book.lock().unwrap().is_empty(),
            "failed push removes its ticket"
        );
        assert_eq!(
            limits.requests.available_permits(),
            16,
            "failed request releases admission"
        );
        assert!(
            transport.receive().await.is_none(),
            "request never reaches SDK actor delivery"
        );
        sessions.shutdown().await;
        assert!(sessions.books.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn native_legacy_idle_expiry_retires_owned_sessions_and_reopens_capacity() {
        let sessions = Sessions::default();
        let mut transports = Vec::new();
        let mut ids = Vec::new();
        for _ in 0..8 {
            let (id, transport) = sessions.create_session().await.unwrap();
            ids.push(id);
            transports.push(transport);
        }
        assert!(sessions.create_session().await.is_err());
        *sessions.seen.lock().unwrap().get_mut(&ids[0]).unwrap() =
            Instant::now() - Duration::from_secs(301);
        sessions.reclaim_expired().await.unwrap();
        assert!(!sessions.has_session(&ids[0]).await.unwrap());
        assert!(!sessions.books.lock().unwrap().contains_key(&ids[0]));
        assert!(
            sessions.has_session(&ids[1]).await.unwrap(),
            "expiry preserves live siblings"
        );
        let (replacement, transport) = sessions.create_session().await.unwrap();
        assert!(
            !ids.contains(&replacement),
            "replacement session identity is fresh"
        );
        transports.push(transport);
        sessions.shutdown().await;
        assert!(sessions.seen.lock().unwrap().is_empty());
        assert!(sessions.books.lock().unwrap().is_empty());
        assert!(sessions.inner.sessions.read().await.is_empty());
        for mut transport in transports {
            assert!(
                tokio::time::timeout(Duration::from_secs(1), transport.receive())
                    .await
                    .unwrap()
                    .is_none(),
                "SDK transports observe retirement"
            );
        }
    }

    #[tokio::test]
    async fn native_legacy_session_creation_rejects_waiters() {
        let sessions = Sessions::default();
        let guard = sessions.creating.lock().await;
        assert!(sessions.create_session().await.is_err());
        assert!(
            sessions.inner.sessions.read().await.is_empty(),
            "busy creation allocates no SDK session"
        );
        drop(guard);
        let (_, mut transport) = sessions.create_session().await.unwrap();
        sessions.shutdown().await;
        assert!(transport.receive().await.is_none());
    }
}
